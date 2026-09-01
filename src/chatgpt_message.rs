pub(crate) fn equivalent(left: &str, right: &str) -> bool {
    left == right || canonical(left) == canonical(right)
}

fn canonical(value: &str) -> String {
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
    unescape_chatgpt_markdown(&collapse_echoed_links(&normalized))
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
    fn accepts_chatgpt_agent_escape_and_echoed_url_link() {
        let submitted =
            "Sử dụng plugin @test_rust để xử lý http://localhost:8080/api/local/payment/create";
        let from_chatgpt = "Sử dụng plugin @test\\_rust để xử lý [http://localhost:8080/api/local/payment/create](http://localhost:8080/api/local/payment/create)";

        assert!(equivalent(submitted, from_chatgpt));
    }
}
