use std::path::PathBuf;

use super::ComputerBrowser;
use crate::{RuntimeError, RuntimeResult};

pub(super) fn find_browser(browser: ComputerBrowser) -> RuntimeResult<PathBuf> {
    browser_candidates(browser)
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            RuntimeError::new(
                "computer_browser_not_found",
                format!("{} executable was not found", browser_name(browser)),
            )
        })
}

fn browser_name(browser: ComputerBrowser) -> &'static str {
    match browser {
        ComputerBrowser::Chrome => "Chrome",
        ComputerBrowser::Edge => "Edge",
        ComputerBrowser::Brave => "Brave",
    }
}

#[cfg(target_os = "windows")]
fn browser_candidates(browser: ComputerBrowser) -> Vec<PathBuf> {
    let (vendor, executable) = match browser {
        ComputerBrowser::Chrome => ("Google\\Chrome\\Application", "chrome.exe"),
        ComputerBrowser::Edge => ("Microsoft\\Edge\\Application", "msedge.exe"),
        ComputerBrowser::Brave => ("BraveSoftware\\Brave-Browser\\Application", "brave.exe"),
    };
    [
        std::env::var_os("PROGRAMFILES"),
        std::env::var_os("PROGRAMFILES(X86)"),
        std::env::var_os("LOCALAPPDATA"),
    ]
    .into_iter()
    .flatten()
    .map(PathBuf::from)
    .map(|base| base.join(vendor).join(executable))
    .collect()
}

#[cfg(target_os = "macos")]
fn browser_candidates(browser: ComputerBrowser) -> Vec<PathBuf> {
    let path = match browser {
        ComputerBrowser::Chrome => "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        ComputerBrowser::Edge => "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        ComputerBrowser::Brave => "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
    };
    vec![PathBuf::from(path)]
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn browser_candidates(browser: ComputerBrowser) -> Vec<PathBuf> {
    let names: &[&str] = match browser {
        ComputerBrowser::Chrome => &[
            "/usr/bin/google-chrome",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
        ],
        ComputerBrowser::Edge => &["/usr/bin/microsoft-edge", "/usr/bin/microsoft-edge-stable"],
        ComputerBrowser::Brave => &["/usr/bin/brave-browser", "/usr/bin/brave-browser-stable"],
    };
    names.iter().map(PathBuf::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brave_candidates_are_not_chrome_or_edge() {
        let paths = browser_candidates(ComputerBrowser::Brave);
        assert!(!paths.is_empty());
        assert!(paths.iter().all(|path| {
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            name.to_ascii_lowercase().contains("brave")
        }));
    }
}
