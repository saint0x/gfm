use crate::{normalize_text, ContentDocument};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RichKind {
    Html,
    Rtf,
    Email,
}

pub(crate) fn extract_rich(bytes: &[u8], kind: RichKind) -> Option<ContentDocument> {
    let raw = String::from_utf8_lossy(bytes);
    let text = match kind {
        RichKind::Html => html_text(&raw),
        RichKind::Rtf => rtf_text(&raw),
        RichKind::Email => email_text(&raw),
    };
    let text = normalize_text(text.trim());
    (!text.is_empty()).then_some(ContentDocument {
        bytes_read: bytes.len(),
        text,
    })
}

fn html_text(input: &str) -> String {
    let mut output = String::new();
    let mut tag = String::new();
    let mut in_tag = false;
    let mut skipping: Option<&'static str> = None;
    let mut entity = String::new();
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '>' if in_tag => {
                in_tag = false;
                let normalized = tag
                    .trim()
                    .trim_start_matches('/')
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                match normalized.as_str() {
                    "script" | "style" if !tag.trim_start().starts_with('/') => {
                        skipping = Some(if normalized == "script" {
                            "script"
                        } else {
                            "style"
                        });
                    }
                    "script" if tag.trim_start().starts_with('/') && skipping == Some("script") => {
                        skipping = None;
                    }
                    "style" if tag.trim_start().starts_with('/') && skipping == Some("style") => {
                        skipping = None;
                    }
                    "br" | "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3" if skipping.is_none() => {
                        output.push(' ');
                    }
                    _ => {}
                }
            }
            _ if in_tag => tag.push(ch),
            '&' if skipping.is_none() => {
                entity.clear();
                while let Some(next) = chars.peek().copied() {
                    chars.next();
                    if next == ';' || entity.len() >= 16 {
                        break;
                    }
                    entity.push(next);
                }
                output.push_str(decode_entity(&entity));
            }
            _ if skipping.is_none() => output.push(ch),
            _ => {}
        }
    }
    collapse_whitespace(&output)
}

fn rtf_text(input: &str) -> String {
    let mut output = String::new();
    let mut chars = input.chars().peekable();
    let mut depth = 0usize;
    while let Some(ch) = chars.next() {
        match ch {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            '\\' => match chars.peek().copied() {
                Some('\\' | '{' | '}') => {
                    output.push(chars.next().unwrap());
                }
                Some('\'') => {
                    chars.next();
                    let hi = chars.next().and_then(|ch| ch.to_digit(16));
                    let lo = chars.next().and_then(|ch| ch.to_digit(16));
                    if let (Some(hi), Some(lo)) = (hi, lo) {
                        output.push(char::from_u32(hi * 16 + lo).unwrap_or(' '));
                    }
                }
                Some(_) => {
                    let control = take_control_word(&mut chars);
                    match control.as_str() {
                        "par" | "line" | "tab" => output.push(' '),
                        _ => {}
                    }
                }
                None => {}
            },
            '\n' | '\r' => output.push(' '),
            _ if depth > 0 => output.push(ch),
            _ => output.push(ch),
        }
    }
    collapse_whitespace(&output)
}

fn take_control_word(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut word = String::new();
    while let Some(ch) = chars.peek().copied() {
        if ch.is_ascii_alphabetic() {
            word.push(ch);
            chars.next();
        } else if ch.is_ascii_digit() || ch == '-' {
            chars.next();
        } else {
            if ch == ' ' {
                chars.next();
            }
            break;
        }
    }
    word
}

fn email_text(input: &str) -> String {
    let normalized = input.replace("\r\n", "\n");
    let (headers, body) = normalized
        .split_once("\n\n")
        .unwrap_or((normalized.as_str(), ""));
    let mut output = String::new();
    for line in unfolded_header_lines(headers) {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("subject:") || lower.starts_with("from:") || lower.starts_with("to:") {
            output.push_str(
                line.split_once(':')
                    .map(|(_, value)| value)
                    .unwrap_or(line)
                    .trim(),
            );
            output.push(' ');
        }
    }
    output.push_str(&decode_quoted_printable(body));
    collapse_whitespace(&output)
}

fn unfolded_header_lines(headers: &str) -> Vec<&str> {
    headers
        .lines()
        .filter(|line| !line.starts_with(char::is_whitespace))
        .collect()
}

fn decode_quoted_printable(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'=' {
            if matches!(bytes.get(index + 1), Some(b'\r' | b'\n')) {
                index += 1;
                while matches!(bytes.get(index), Some(b'\r' | b'\n')) {
                    index += 1;
                }
                continue;
            }
            if let (Some(hi), Some(lo)) = (
                bytes.get(index + 1).and_then(|byte| hex_value(*byte)),
                bytes.get(index + 2).and_then(|byte| hex_value(*byte)),
            ) {
                output.push((hi << 4) | lo);
                index += 3;
                continue;
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn decode_entity(entity: &str) -> &str {
    match entity {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        "nbsp" => " ",
        _ => " ",
    }
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn collapse_whitespace(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_html_visible_text() {
        let doc = extract_rich(
            b"<html><style>.x{}</style><body><h1>Hello &amp; welcome</h1><script>hidden()</script><p>needle text</p></body></html>",
            RichKind::Html,
        )
        .unwrap();

        assert_eq!(doc.text, "Hello & welcome needle text");
    }

    #[test]
    fn extracts_rtf_visible_text() {
        let doc = extract_rich(br"{\rtf1\ansi Hello\par rich \'74ext}", RichKind::Rtf).unwrap();

        assert_eq!(doc.text, "Hello rich text");
    }

    #[test]
    fn extracts_email_headers_and_body() {
        let doc = extract_rich(
            b"From: Ada <ada@example.com>\r\nTo: Team\r\nSubject: Launch\r\n\r\nBody has mailneedle=20text",
            RichKind::Email,
        )
        .unwrap();

        assert_eq!(
            doc.text,
            "Ada <ada@example.com> Team Launch Body has mailneedle text"
        );
    }
}
