use super::{backend_error, hwnd};
use crate::{RuntimeError, RuntimeResult};
use std::{
    io::Cursor,
    sync::mpsc::{self, SyncSender},
    time::Duration,
};
use windows_capture::{
    capture::{Context, GraphicsCaptureApiHandler},
    frame::Frame,
    graphics_capture_api::InternalCaptureControl,
    settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    },
    window::Window,
};

const CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_PIXELS: u64 = 16_777_216;

struct OneFrameCapture {
    sender: Option<SyncSender<Result<Vec<u8>, String>>>,
}

impl GraphicsCaptureApiHandler for OneFrameCapture {
    type Flags = SyncSender<Result<Vec<u8>, String>>;
    type Error = String;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self {
            sender: Some(ctx.flags),
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let result = encode_frame(frame);
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(result);
        }
        control.stop();
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(Err("target window closed during capture".to_owned()));
        }
        Ok(())
    }
}

pub(super) fn capture_png(handle: isize) -> RuntimeResult<Vec<u8>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let settings = Settings::new(
        Window::from_raw_hwnd(hwnd(handle).0),
        CursorCaptureSettings::WithoutCursor,
        DrawBorderSettings::WithoutBorder,
        SecondaryWindowSettings::Exclude,
        MinimumUpdateIntervalSettings::Default,
        DirtyRegionSettings::Default,
        ColorFormat::Rgba8,
        sender,
    );
    let control = OneFrameCapture::start_free_threaded(settings).map_err(backend_error)?;
    match receiver.recv_timeout(CAPTURE_TIMEOUT) {
        Ok(Ok(bytes)) => {
            control.wait().map_err(backend_error)?;
            Ok(bytes)
        }
        Ok(Err(message)) => {
            let _ = control.stop();
            Err(backend_error(message))
        }
        Err(_) => {
            let _ = control.stop();
            Err(RuntimeError::new(
                "desktop_capture_timeout",
                "timed out while capturing the target window",
            ))
        }
    }
}

fn encode_frame(frame: &mut Frame) -> Result<Vec<u8>, String> {
    let width = frame.width();
    let height = frame.height();
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err("captured window dimensions are unsupported".to_owned());
    }
    let buffer = frame.buffer().map_err(|error| error.to_string())?;
    let mut packed = Vec::new();
    let pixels = buffer.as_nopadding_buffer(&mut packed);
    let mut output = Vec::new();
    {
        let cursor = Cursor::new(&mut output);
        let mut encoder = png::Encoder::new(cursor, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
        writer
            .write_image_data(pixels)
            .map_err(|error| error.to_string())?;
    }
    Ok(output)
}
