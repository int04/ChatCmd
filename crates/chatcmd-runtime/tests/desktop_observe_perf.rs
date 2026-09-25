//! Manual, read-only latency smoke test for a window chosen by the developer.
//!
//! Example (PowerShell, with a visible Notepad window):
//! `$env:CHATCMD_DESKTOP_BENCH_APP = "notepad.exe"`
//! `cargo test -p chatcmd-runtime --test desktop_observe_perf -- --ignored --nocapture`
//!
//! Set `RUST_LOG=chatcmd_runtime::desktop::timing=debug` in the running app
//! to see the capture, UIA and encoding phase timings during real MCP calls.

#![cfg(target_os = "windows")]

use chatcmd_runtime::{DesktopControlService, DesktopObserveRequest, OperationContext};
use std::time::{Duration, Instant};

const SAMPLES: usize = 15;

#[tokio::test]
#[ignore = "manual benchmark; set CHATCMD_DESKTOP_BENCH_APP to a visible, safe app executable"]
async fn selected_window_observation_latency() {
    let application = std::env::var("CHATCMD_DESKTOP_BENCH_APP")
        .expect("set CHATCMD_DESKTOP_BENCH_APP to the executable of a visible safe window");
    assert!(!application.trim().is_empty());

    let service = DesktopControlService::new();
    let context =
        OperationContext::new("desktop-perf", "desktop-perf-agent", "desktop_window_list");
    let listed = service
        .list_windows(&context)
        .await
        .expect("window enumeration should succeed");
    let window = listed
        .windows
        .iter()
        .find(|window| !window.minimized && window.application.eq_ignore_ascii_case(&application))
        .expect("the selected application must have a visible allowed window");

    for (label, screenshot, elements) in [
        ("metadata", false, false),
        ("screenshot", true, false),
        ("uia", false, true),
        ("combined", true, true),
    ] {
        observe(&service, &context, &window.window_id, screenshot, elements).await;
        let mut samples = Vec::with_capacity(SAMPLES);
        for _ in 0..SAMPLES {
            let started = Instant::now();
            observe(&service, &context, &window.window_id, screenshot, elements).await;
            samples.push(started.elapsed());
        }
        samples.sort_unstable();
        eprintln!(
            "{label}: p50={:.1}ms p95={:.1}ms max={:.1}ms (n={SAMPLES})",
            millis(samples[SAMPLES / 2]),
            millis(samples[(SAMPLES - 1) * 95 / 100]),
            millis(samples[SAMPLES - 1]),
        );
    }
}

async fn observe(
    service: &DesktopControlService,
    context: &OperationContext,
    window_id: &str,
    include_screenshot: bool,
    include_elements: bool,
) {
    service
        .observe(
            context,
            DesktopObserveRequest {
                window_id: window_id.to_owned(),
                include_screenshot,
                include_elements,
            },
        )
        .await
        .expect("selected window should remain observable");
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}
