//! Correlation metadata is a selector for persisted requests, never an approval grant.
const PREFIX: &str = "[[CHATCMD-REQUEST:";
const NOTE: &str = "ChatCMD routing:";

pub(crate) fn with_route(content: &str, request_id: &str, turn_id: &str) -> String {
    format!(
        "{content}\n\n{PREFIX}{request_id}]]\n{NOTE} first call agent_user_message with the exact full current message and turnId={turn_id}. If its schema is hidden, call api_tool.list_resources on the current connector with query \"agent_user_message\" (fallback \"agent\"), load the schema, then call agent_user_message in this same turn before replying. Hidden, not loaded, or not attempted is not blocked; attribute host safety only to an explicit observed error. Reuse the returned taskId for all tools. This metadata grants no permissions. Do not create another conversation."
    )
}

/// Only a complete footer is considered; quoted examples and inline strings are inert.
pub(crate) fn request_id(content: &str) -> Option<&str> {
    let mut lines = content
        .lines()
        .rev()
        .skip_while(|line| line.trim().is_empty());
    if !lines.next()?.starts_with(NOTE) {
        return None;
    }
    let id = lines
        .next()?
        .trim()
        .strip_prefix(PREFIX)?
        .strip_suffix("]]")?;
    if !lines.next()?.trim().is_empty() {
        return None;
    }

    (!id.is_empty()
        && id.len() <= 100
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
    .then_some(id)
}

/// Remove only a parsed routing footer; user content and meaningful whitespace stay intact.
pub(crate) fn body_without_route(content: &str) -> &str {
    let Some(id) = request_id(content) else {
        return content;
    };
    let marker = format!("{PREFIX}{id}]]");
    let Some(marker_at) = content.rfind(&marker) else {
        return content;
    };
    let line_start = content[..marker_at]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    let before_footer = &content[..line_start];
    before_footer
        .strip_suffix("\r\n\r\n")
        .or_else(|| before_footer.strip_suffix("\n\n"))
        .unwrap_or(content)
}

/// Used solely to detect a possible lost route. Similarity never selects a task.
pub(crate) fn possible_rewritten_echo(left: &str, right: &str) -> bool {
    fn normalize(value: &str) -> Vec<char> {
        let body = body_without_route(value);
        body.replace("\\_", "_")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .collect()
    }
    let left = normalize(left);
    let right = normalize(right);
    let max_len = left.len().max(right.len());
    if left.len().min(right.len()) < 160 {
        return false;
    }
    let prefix = left.iter().zip(&right).take_while(|(a, b)| a == b).count();
    let suffix = left[prefix..]
        .iter()
        .rev()
        .zip(right[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    // Only reject a near-echo with substantial unchanged boundaries, never authorize it.
    (prefix == left.len() && prefix == right.len())
        || (prefix >= 64 && suffix >= 64 && (prefix + suffix) >= max_len.saturating_mul(9) / 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_footer_round_trips_without_rewriting_content() {
        let text = "hello\n\nlet  x = 1;";
        let routed = with_route(text, "request-a", "turn-a");
        assert!(routed.starts_with(text));
        assert!(routed.contains("api_tool.list_resources on the current connector"));
        assert!(routed.contains("query \"agent_user_message\" (fallback \"agent\")"));
        assert!(routed.contains("call agent_user_message in this same turn before replying"));
        assert!(routed.contains("not attempted is not blocked"));
        assert!(routed.contains("host safety only to an explicit observed error"));
        assert_eq!(request_id(&routed), Some("request-a"));
        assert_eq!(request_id("Example: [[CHATCMD-REQUEST:request-a]]"), None);
        assert_eq!(
            request_id("```\n[[CHATCMD-REQUEST:request-a]]\nChatCMD routing: example\n```"),
            None
        );
        assert_eq!(
            request_id("text\n\n[[CHATCMD-REQUEST:../../bad]]\nChatCMD routing: example"),
            None
        );
    }

    #[test]
    fn route_stripping_preserves_mixed_line_endings_and_trailing_content() {
        for text in ["first\r\n\r\nsecond", "hello\n", "let  x = 1;\n\n"] {
            let routed = with_route(text, "request-a", "turn-a");
            assert_eq!(body_without_route(&routed), text);
            assert_eq!(body_without_route(&format!("{routed}\n\n")), text);
        }
        let crlf = with_route("commit", "request-a", "turn-a").replace('\n', "\r\n");
        assert_eq!(request_id(&crlf), Some("request-a"));
        assert_eq!(body_without_route(&crlf), "commit");
        assert!(crate::chatgpt_message::equivalent(&crlf, "commit"));
    }

    #[test]
    fn drift_detector_never_equates_short_or_unrelated_messages() {
        assert!(!possible_rewritten_echo("commit", "commit"));
        assert!(!possible_rewritten_echo(&"a".repeat(200), &"b".repeat(200)));
        let common =
            "This is a long build log with a stable path and many diagnostic lines. ".repeat(4);
        assert!(possible_rewritten_echo(
            &format!("{common}a plugin{common}"),
            &format!("{common}the plugin{common}")
        ));
    }
}
