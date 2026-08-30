use std::{fs::OpenOptions, io::Write as _, path::PathBuf};

use time::{OffsetDateTime, UtcOffset};

const DEFAULT_LOG_PATH: &str = "logs/chatcmd.log";

/// Append one AI-readable diagnostic line to the ChatCMD log file.
///
/// Call sites should pass `file!()` and `line!()` so the source location is
/// captured where the problem occurs.
pub(crate) fn log_issue(file: &str, line: u32, message: &str) {
    let now = local_now();
    let entry = format_log_line(now, file, line, message);
    let path = log_path();

    if let Some(parent) = path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            tracing::error!(%error, %entry, "failed to create ChatCMD log directory");
            return;
        }
    }

    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(mut file) => {
            if let Err(error) = writeln!(file, "{entry}") {
                tracing::error!(%error, %entry, "failed to append ChatCMD log entry");
            }
        }
        Err(error) => tracing::error!(%error, %entry, "failed to open ChatCMD log file"),
    }
}

fn local_now() -> OffsetDateTime {
    let now = OffsetDateTime::now_utc();
    UtcOffset::current_local_offset()
        .map(|offset| now.to_offset(offset))
        .unwrap_or(now)
}

fn log_path() -> PathBuf {
    std::env::var_os("CHATCMD_LOG_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_LOG_PATH))
}

fn format_log_line(now: OffsetDateTime, file: &str, line: u32, message: &str) -> String {
    let source = normalize_source_path(file);
    format!(
        "{}:{:02} {:02}/{:02}/{:04} [{} - {}]: {}",
        now.hour(),
        now.minute(),
        now.day(),
        u8::from(now.month()),
        now.year(),
        source,
        line,
        message.trim()
    )
}

fn normalize_source_path(file: &str) -> String {
    let normalized = file.replace('/', "\\");
    if normalized.starts_with('\\') {
        normalized
    } else {
        format!("\\{normalized}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::{Date, Month, Time};

    #[test]
    fn formats_ai_readable_log_line() {
        let now = Date::from_calendar_date(2024, Month::June, 26)
            .unwrap()
            .with_time(Time::from_hms(8, 40, 0).unwrap())
            .assume_utc();

        assert_eq!(
            format_log_line(now, "src/main.rs", 100, "Show lỗi ra ở đây"),
            "8:40 26/06/2024 [\\src\\main.rs - 100]: Show lỗi ra ở đây"
        );
    }
}
