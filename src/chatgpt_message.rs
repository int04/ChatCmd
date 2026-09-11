pub(crate) fn equivalent(left: &str, right: &str) -> bool {
    left == right || canonical(left) == canonical(right)
}

fn canonical(value: &str) -> String {
    let value = crate::chatgpt_routing::body_without_route(value);
    let mut normalized = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                normalized.push('\n');
            }
            '\u{00a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}' => normalized.push(' '),
            _ => normalized.push(ch),
        }
    }
    let normalized = unescape_chatgpt_markdown(&collapse_echoed_links(&normalized));
    collapse_excess_blank_lines(strip_boundary_image_markers(&normalized))
}

/// ChatGPT may describe an image outside the submitted text when echoing a turn.
/// Strip only whole presentation-marker lines at the edges, never inline text,
/// code fences, arbitrary attachment descriptions, or an image-only message.
fn strip_boundary_image_markers(value: &str) -> &str {
    let is_marker = |line: &str| {
        matches!(
            line.trim(),
            "<uploaded image>" | "<<ImageDisplayed>>" | "[ImageDisplayed]"
        )
    };
    let mut rest = value;
    loop {
        let candidate = rest.trim_start_matches('\n');
        let Some((line, remaining)) = candidate.split_once('\n') else {
            break;
        };
        if !is_marker(line) {
            break;
        }
        rest = remaining.trim_start_matches('\n');
    }
    loop {
        let candidate = rest.trim_end_matches('\n');
        let Some((remaining, line)) = candidate.rsplit_once('\n') else {
            break;
        };
        if !is_marker(line) {
            break;
        }
        rest = remaining.trim_end_matches('\n');
    }
    // Image-only placeholders do not identify the image or an existing task.
    if rest.trim().is_empty() || is_marker(rest) {
        value
    } else {
        rest
    }
}

fn collapse_excess_blank_lines(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut consecutive_newlines = 0_u8;
    for ch in value.chars() {
        if ch == '\n' {
            consecutive_newlines = consecutive_newlines.saturating_add(1);
            if consecutive_newlines <= 2 {
                output.push(ch);
            }
        } else {
            consecutive_newlines = 0;
            output.push(ch);
        }
    }
    output
}

fn collapse_echoed_links(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(open) = rest.find('[') {
        output.push_str(&rest[..open]);
        let candidate = &rest[open + 1..];
        let Some(middle) = candidate.find("](") else {
            output.push_str(&rest[open..]);
            return output;
        };
        let label = &candidate[..middle];
        let destination = &candidate[middle + 2..];
        let Some(close) = destination.find(')') else {
            output.push_str(&rest[open..]);
            return output;
        };
        let url = &destination[..close];
        if label == url && (url.starts_with("http://") || url.starts_with("https://")) {
            output.push_str(url);
            rest = &destination[close + 1..];
        } else {
            output.push('[');
            rest = candidate;
        }
    }
    output.push_str(rest);
    output
}

fn unescape_chatgpt_markdown(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' && chars.peek() == Some(&'_') {
            chars.next();
            output.push('_');
        } else {
            output.push(ch);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::equivalent;

    #[test]
    fn accepts_boundary_image_markers_without_rewriting_prompt_text() {
        let prompt = "Sử dụng plugin @rust_test\n\nĐọc docs/huongdan.md";
        for echoed in [
            format!("<uploaded image>\n\n{prompt}"),
            format!("{prompt}\n\n<<ImageDisplayed>>"),
            format!("<<ImageDisplayed>>\n<uploaded image>\n\n{prompt}\n[ImageDisplayed]"),
            format!("<uploaded image>\r\n\r\n{}", prompt.replace('\n', "\r\n")),
        ] {
            assert!(equivalent(prompt, &echoed), "marker echo: {echoed:?}");
            assert!(equivalent(&echoed, prompt), "matching must be symmetric");
        }
    }

    #[test]
    fn does_not_strip_inline_quoted_fenced_or_arbitrary_attachment_text() {
        for content in [
            "Please explain <uploaded image>",
            "`<uploaded image>`\n\nPrompt",
            "> <uploaded image>\n\nPrompt",
            "```text\n<uploaded image>\n```\n\nPrompt",
            "<uploaded file>\n\nPrompt",
            "[a screenshot containing private instructions]\n\nPrompt",
            "Prompt\n<uploaded image>\nDifferent request",
        ] {
            assert!(!equivalent("Prompt", content), "must preserve {content:?}");
        }
        assert!(!equivalent("let x = 1;", "<uploaded image>\n\nlet  x = 1;"));
    }

    #[test]
    fn image_only_markers_do_not_match_empty_or_different_images() {
        assert!(!equivalent("", "<uploaded image>\n\n"));
        assert!(!equivalent("<uploaded image>", "<<ImageDisplayed>>"));
        assert!(!equivalent(
            "<uploaded image>",
            "<uploaded image>\n<uploaded image>"
        ));
    }

    #[test]
    fn accepts_dom_unicode_spaces_and_line_endings() {
        let submitted = "D:\\DEV\\ChatCMD\\ChatCMD (ChatCMD.Tunnel) \r\n\r\nVí dụ abcd ";
        let from_chatgpt =
            "D:\\DEV\\ChatCMD\\ChatCMD (ChatCMD.Tunnel)\u{00a0}\n\nVí dụ abcd\u{202f}";

        assert!(equivalent(submitted, from_chatgpt));
    }

    #[test]
    fn keeps_meaningful_whitespace_distinct() {
        assert!(!equivalent("let x = 1;", "let  x = 1;"));
        assert!(!equivalent("line one\nline two", "line one line two"));
    }

    #[test]
    fn accepts_chatgpt_blank_line_jitter_between_paragraphs() {
        assert!(equivalent(
            "Sử dụng plugin @rust_test\n\nPhần một\n\n\nPhần hai",
            "Sử dụng plugin @rust\\_test\n\nPhần một\n\n\n\nPhần hai"
        ));
    }

    #[test]
    fn accepts_chatgpt_agent_escape_and_echoed_url_link() {
        let submitted =
            "Sử dụng plugin @test_rust để xử lý http://localhost:8080/api/local/overview";
        let from_chatgpt = "Sử dụng plugin @test\\_rust để xử lý [http://localhost:8080/api/local/overview](http://localhost:8080/api/local/overview)";

        assert!(equivalent(submitted, from_chatgpt));
    }
}
