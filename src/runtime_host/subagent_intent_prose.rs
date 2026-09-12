//! Extract command prose without promoting quoted examples or routing metadata to intent.

pub(super) fn command_prose(content: &str) -> String {
    let mut output = String::with_capacity(content.len());
    let mut fence: Option<(char, usize)> = None;
    for line in content.lines() {
        let trimmed = line.trim_start();
        if let Some((delimiter, width)) = fence {
            let closing = trimmed.chars().take_while(|c| *c == delimiter).count();
            if closing >= width && trimmed[closing..].trim().is_empty() {
                fence = None;
            }
            continue;
        }
        if trimmed.starts_with("[[CHATCMD-REQUEST:") {
            break; // The appended routing envelope is data, not another user instruction.
        }
        if trimmed.starts_with("ChatCMD routing:")
            || trimmed.starts_with('>')
            || line.starts_with("    ")
            || line.starts_with('\t')
        {
            continue;
        }
        if let Some(delimiter @ ('`' | '~')) = trimmed.chars().next() {
            let width = trimmed.chars().take_while(|c| *c == delimiter).count();
            if width >= 3 {
                fence = Some((delimiter, width));
                continue;
            }
        }
        let mut chars = line.chars().peekable();
        let mut previous: Option<char> = None;
        while let Some(character) = chars.next() {
            let apostrophe = matches!(character, '\'' | '’')
                && previous.is_some_and(char::is_alphanumeric)
                && chars.peek().is_some_and(|c| c.is_alphanumeric());
            let closing = match character {
                '`' | '"' => Some(character),
                '\'' if !apostrophe => Some('\''),
                '“' => Some('”'),
                '‘' => Some('’'),
                _ => None,
            };
            if let Some(closing) = closing {
                // Keep a barrier, not an empty string: `Chia "example"agent` is not a command.
                output.push_str(" quoteddata ");
                let mut width = 1;
                if character == '`' {
                    while chars.peek() == Some(&'`') {
                        chars.next();
                        width += 1;
                    }
                }
                let mut seen = 0;
                for next in chars.by_ref() {
                    seen = if next == closing { seen + 1 } else { 0 };
                    if seen == width {
                        break;
                    }
                }
            } else if character != '*' {
                output.push(if apostrophe { '\'' } else { character });
            }
            previous = Some(character);
        }
        output.push('\n');
    }
    output
}

pub(super) fn normalize(content: &str) -> String {
    // Preserve the distinction between Vietnamese "đừng" (never) and "dùng" (use).
    let lower = content
        .to_lowercase()
        .replace("đừng", "never")
        .replace("đư\u{0300}ng", "never")
        .replace("đu\u{031b}\u{0300}ng", "never");
    let folded: String = lower
        .chars()
        .filter_map(|c| {
            Some(match c {
                'à' | 'á' | 'ả' | 'ã' | 'ạ' | 'ă' | 'ằ' | 'ắ' | 'ẳ' | 'ẵ' | 'ặ' | 'â' | 'ầ'
                | 'ấ' | 'ẩ' | 'ẫ' | 'ậ' => 'a',
                'è' | 'é' | 'ẻ' | 'ẽ' | 'ẹ' | 'ê' | 'ề' | 'ế' | 'ể' | 'ễ' | 'ệ' => {
                    'e'
                }
                'ì' | 'í' | 'ỉ' | 'ĩ' | 'ị' => 'i',
                'ò' | 'ó' | 'ỏ' | 'õ' | 'ọ' | 'ô' | 'ồ' | 'ố' | 'ổ' | 'ỗ' | 'ộ' | 'ơ' | 'ờ'
                | 'ớ' | 'ở' | 'ỡ' | 'ợ' => 'o',
                'ù' | 'ú' | 'ủ' | 'ũ' | 'ụ' | 'ư' | 'ừ' | 'ứ' | 'ử' | 'ữ' | 'ự' => {
                    'u'
                }
                'ỳ' | 'ý' | 'ỷ' | 'ỹ' | 'ỵ' => 'y',
                'đ' => 'd',
                '\u{0300}'..='\u{036f}' => return None,
                ':' | '@' | '\'' => c,
                _ if c.is_alphanumeric() => c,
                _ => ' ',
            })
        })
        .collect();
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}
