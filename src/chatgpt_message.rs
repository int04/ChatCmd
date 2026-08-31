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
    normalized
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
}
