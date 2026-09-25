use super::{backend_error, hwnd};
use crate::{RuntimeError, RuntimeResult};
use std::{
    io::Cursor,
    sync::{Arc, Condvar, Mutex, MutexGuard},
    time::{Duration, Instant},
};
use windows_capture::{
    capture::{CaptureControl, Context, GraphicsCaptureApiHandler},
    frame::Frame,
    graphics_capture_api::InternalCaptureControl,
    settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    },
    window::Window,
};

pub(super) const CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_PIXELS: u64 = 16_777_216;
const BYTES_PER_PIXEL: usize = 4;

#[derive(Clone, Debug)]
struct RgbaFrame {
    sequence: u64,
    width: u32,
    height: u32,
    pixels: Arc<Vec<u8>>,
}

#[derive(Default)]
struct CacheState {
    latest: Option<RgbaFrame>,
    next_sequence: u64,
    terminal_error: Option<String>,
}

#[derive(Default)]
struct FrameCache {
    state: Mutex<CacheState>,
    changed: Condvar,
}

impl FrameCache {
    fn publish(&self, width: u32, height: u32, pixels: Vec<u8>) -> Result<Option<Vec<u8>>, String> {
        validate_rgba(width, height, pixels.len())?;
        let mut state = self.lock().map_err(|error| error.to_string())?;
        let sequence = state
            .next_sequence
            .checked_add(1)
            .ok_or_else(|| "capture frame sequence exhausted".to_owned())?;
        state.next_sequence = sequence;
        let previous = state.latest.replace(RgbaFrame {
            sequence,
            width,
            height,
            pixels: Arc::new(pixels),
        });
        drop(state);
        self.changed.notify_all();
        Ok(previous.and_then(|frame| Arc::try_unwrap(frame.pixels).ok()))
    }

    fn fail(&self, message: impl Into<String>) {
        if let Ok(mut state) = self.lock() {
            state.terminal_error = Some(message.into());
            drop(state);
            self.changed.notify_all();
        }
    }

    fn latest(&self) -> RuntimeResult<Option<RgbaFrame>> {
        let state = self.lock_runtime()?;
        if let Some(message) = &state.terminal_error {
            return Err(RuntimeError::new("desktop_capture_closed", message.clone()));
        }
        Ok(state.latest.clone())
    }

    fn current_sequence(&self) -> RuntimeResult<u64> {
        let state = self.lock_runtime()?;
        if let Some(message) = &state.terminal_error {
            return Err(RuntimeError::new("desktop_capture_closed", message.clone()));
        }
        Ok(state.latest.as_ref().map_or(0, |frame| frame.sequence))
    }

    fn wait_after(&self, sequence: u64, timeout: Duration) -> RuntimeResult<RgbaFrame> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(Instant::now);
        let mut state = self.lock_runtime()?;
        loop {
            if let Some(message) = &state.terminal_error {
                return Err(RuntimeError::new("desktop_capture_closed", message.clone()));
            }
            if let Some(frame) = state
                .latest
                .as_ref()
                .filter(|frame| frame.sequence > sequence)
            {
                return Ok(frame.clone());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(capture_timeout());
            }
            let (next, result) = self
                .changed
                .wait_timeout(state, remaining)
                .map_err(|error| backend_error(error.to_string()))?;
            state = next;
            if result.timed_out()
                && state
                    .latest
                    .as_ref()
                    .is_none_or(|frame| frame.sequence <= sequence)
            {
                return Err(capture_timeout());
            }
        }
    }

    fn lock(
        &self,
    ) -> Result<MutexGuard<'_, CacheState>, std::sync::PoisonError<MutexGuard<'_, CacheState>>>
    {
        self.state.lock()
    }

    fn lock_runtime(&self) -> RuntimeResult<MutexGuard<'_, CacheState>> {
        self.lock()
            .map_err(|error| backend_error(error.to_string()))
    }
}

struct PersistentCapture {
    cache: Arc<FrameCache>,
    scratch: Vec<u8>,
}

impl GraphicsCaptureApiHandler for PersistentCapture {
    type Flags = Arc<FrameCache>;
    type Error = String;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self {
            cache: ctx.flags,
            scratch: Vec::new(),
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        if let Err(error) = cache_frame(frame, &self.cache, &mut self.scratch) {
            self.cache.fail(error.clone());
            control.stop();
            return Err(error);
        }
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        self.cache.fail("target window closed during capture");
        Ok(())
    }
}

/// A live Windows Graphics Capture pipeline for one target window.
///
/// The capture thread retains its D3D/WGC resources and only the newest RGBA
/// frame is cached. PNG encoding is deferred until [`Self::latest_png`] or
/// [`Self::png_after`] is called.
pub(super) struct CaptureSession {
    cache: Arc<FrameCache>,
    control: Option<CaptureControl<PersistentCapture, String>>,
}

impl CaptureSession {
    /// Starts a persistent capture pipeline for `handle`.
    pub(super) fn start(handle: isize) -> RuntimeResult<Self> {
        let cache = Arc::new(FrameCache::default());
        let settings = Settings::new(
            Window::from_raw_hwnd(hwnd(handle).0),
            CursorCaptureSettings::WithoutCursor,
            DrawBorderSettings::WithoutBorder,
            SecondaryWindowSettings::Exclude,
            MinimumUpdateIntervalSettings::Default,
            DirtyRegionSettings::Default,
            ColorFormat::Rgba8,
            cache.clone(),
        );
        let control = PersistentCapture::start_free_threaded(settings).map_err(backend_error)?;
        Ok(Self {
            cache,
            control: Some(control),
        })
    }

    /// Encodes the newest cached frame, waiting only when the first frame has not arrived.
    pub(super) fn latest_png(&self, timeout: Duration) -> RuntimeResult<(u64, Vec<u8>)> {
        match self.cache.latest()? {
            Some(frame) => Ok((frame.sequence, encode_frame(&frame)?)),
            None => self.png_after(0, timeout),
        }
    }

    /// Returns the latest frame sequence, or zero while waiting for the first frame.
    pub(super) fn current_sequence(&self) -> RuntimeResult<u64> {
        self.cache.current_sequence()
    }

    /// Returns a usable pre-action sequence, waiting for the initial frame without encoding it.
    pub(super) fn wait_for_sequence(&self, timeout: Duration) -> RuntimeResult<u64> {
        let current = self.current_sequence()?;
        if current == 0 {
            Ok(self.cache.wait_after(0, timeout)?.sequence)
        } else {
            Ok(current)
        }
    }

    /// Waits for a frame newer than `sequence` and encodes it as PNG.
    pub(super) fn png_after(
        &self,
        sequence: u64,
        timeout: Duration,
    ) -> RuntimeResult<(u64, Vec<u8>)> {
        let frame = self.cache.wait_after(sequence, timeout)?;
        Ok((frame.sequence, encode_frame(&frame)?))
    }

    /// Stops the live capture thread. Calling this more than once is harmless.
    pub(super) fn stop(&mut self) -> RuntimeResult<()> {
        self.cache.fail("capture session stopped");
        if let Some(control) = self.control.take() {
            control.stop().map_err(backend_error)?;
        }
        Ok(())
    }
}

impl Drop for CaptureSession {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn cache_frame(frame: &mut Frame, cache: &FrameCache, scratch: &mut Vec<u8>) -> Result<(), String> {
    let width = frame.width();
    let height = frame.height();
    validate_dimensions(width, height)?;
    let mut buffer = frame.buffer().map_err(|error| error.to_string())?;
    let expected = rgba_len(width, height)?;
    scratch.clear();
    if scratch.capacity() < expected {
        scratch.reserve(expected);
    }
    if buffer.has_padding() {
        let _ = buffer.as_nopadding_buffer(scratch);
    } else {
        scratch.extend_from_slice(buffer.as_raw_buffer());
    }
    let pixels = std::mem::take(scratch);
    if let Some(reusable) = cache.publish(width, height, pixels)? {
        *scratch = reusable;
    }
    Ok(())
}

fn encode_frame(frame: &RgbaFrame) -> RuntimeResult<Vec<u8>> {
    let mut output = Vec::new();
    {
        let cursor = Cursor::new(&mut output);
        let mut encoder = png::Encoder::new(cursor, frame.width, frame.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(backend_error)?;
        writer
            .write_image_data(frame.pixels.as_slice())
            .map_err(backend_error)?;
    }
    Ok(output)
}

fn validate_dimensions(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err("captured window dimensions are unsupported".to_owned());
    }
    Ok(())
}

fn validate_rgba(width: u32, height: u32, actual: usize) -> Result<(), String> {
    let expected = rgba_len(width, height)?;
    if actual != expected {
        return Err("captured frame buffer length is invalid".to_owned());
    }
    Ok(())
}

fn rgba_len(width: u32, height: u32) -> Result<usize, String> {
    validate_dimensions(width, height)?;
    let pixels = usize::try_from(u64::from(width) * u64::from(height))
        .map_err(|_| "captured window dimensions are unsupported".to_owned())?;
    pixels
        .checked_mul(BYTES_PER_PIXEL)
        .ok_or_else(|| "captured window dimensions are unsupported".to_owned())
}

fn capture_timeout() -> RuntimeError {
    let mut error = RuntimeError::new(
        "desktop_capture_timeout",
        "timed out waiting for a target-window frame newer than the requested sequence; retry observation",
    );
    error.retryable = true;
    error
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn frame_cache_keeps_only_latest_sequence() {
        let cache = FrameCache::default();
        let _ = cache.publish(1, 1, vec![1, 2, 3, 4]).expect("first frame");
        let _ = cache.publish(1, 1, vec![5, 6, 7, 8]).expect("second frame");

        let latest = cache.latest().expect("latest frame").expect("a frame");
        assert_eq!(latest.sequence, 2);
        assert_eq!(latest.pixels.as_slice(), &[5, 6, 7, 8]);
    }

    #[test]
    fn wait_after_observes_a_later_frame() {
        let cache = Arc::new(FrameCache::default());
        let _ = cache.publish(1, 1, vec![0; 4]).expect("initial frame");
        let publisher = cache.clone();
        let publisher_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            let _ = publisher.publish(1, 1, vec![9; 4]).expect("new frame");
        });

        let frame = cache
            .wait_after(1, Duration::from_secs(1))
            .expect("later frame");
        publisher_thread.join().expect("publisher thread");
        assert_eq!(frame.sequence, 2);
        assert_eq!(frame.pixels.as_slice(), &[9; 4]);
    }

    #[test]
    fn invalid_or_oversized_frames_are_rejected() {
        assert!(validate_rgba(1, 1, 3).is_err());
        assert!(validate_dimensions(0, 1).is_err());
        assert!(validate_dimensions(4097, 4096).is_err());
    }

    #[test]
    fn terminal_error_wakes_waiters_without_returning_a_stale_frame() {
        let cache = Arc::new(FrameCache::default());
        let _ = cache.publish(1, 1, vec![0; 4]).expect("initial frame");
        let failed = cache.clone();
        let failure_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            failed.fail("window closed");
        });

        let error = cache
            .wait_after(1, Duration::from_secs(1))
            .expect_err("terminal failure");
        failure_thread.join().expect("failure thread");
        assert_eq!(error.code, "desktop_capture_closed");
        assert!(cache.latest().is_err());
    }

    #[test]
    fn wait_after_has_a_bounded_timeout() {
        let cache = FrameCache::default();
        let error = cache
            .wait_after(0, Duration::ZERO)
            .expect_err("timeout without a frame");
        assert_eq!(error.code, "desktop_capture_timeout");
        assert!(error.retryable);
    }

    #[test]
    fn wait_after_never_returns_the_baseline_frame_on_timeout() {
        let cache = FrameCache::default();
        let _ = cache.publish(1, 1, vec![0; 4]).expect("baseline frame");

        let error = cache
            .wait_after(1, Duration::ZERO)
            .expect_err("baseline is not a newer frame");
        assert_eq!(error.code, "desktop_capture_timeout");
        assert!(error.retryable);
    }

    #[test]
    fn current_sequence_is_zero_then_tracks_published_frames() {
        let cache = FrameCache::default();
        assert_eq!(cache.current_sequence().expect("empty sequence"), 0);
        let _ = cache.publish(1, 1, vec![0; 4]).expect("first frame");
        assert_eq!(cache.current_sequence().expect("first sequence"), 1);
    }

    #[test]
    fn capture_session_can_be_shared_with_blocking_workers() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<CaptureSession>();
    }

    #[test]
    fn rgba_frame_encodes_as_png_on_demand() {
        let frame = RgbaFrame {
            sequence: 1,
            width: 1,
            height: 1,
            pixels: Arc::new(vec![255, 0, 0, 255]),
        };
        let encoded = encode_frame(&frame).expect("PNG encoding");
        assert_eq!(&encoded[..8], b"\x89PNG\r\n\x1a\n");
    }
}
