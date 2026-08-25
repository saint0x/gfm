use crate::{normalize_text, ContentDocument};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RichKind {
    Html,
    Rtf,
    Email,
}

pub(crate) fn extract_rich(
    bytes: &[u8],
    kind: RichKind,
    max_text_bytes: usize,
) -> Option<ContentDocument> {
    let raw = String::from_utf8_lossy(bytes);
    let text = match kind {
        RichKind::Html => html_text(&raw),
        RichKind::Rtf => rtf_text(&raw),
        RichKind::Email => email_text(&raw),
    };
    let text = truncate_text(&normalize_text(text.trim()), max_text_bytes);
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
                    .unwrap_or(&line)
                    .trim(),
            );
            output.push(' ');
        }
    }
    let mut budget = MimeTraversalBudget {
        remaining_parts: 128,
        remaining_depth: 8,
    };
    output.push_str(&extract_mime_text(headers, body, &mut budget));
    collapse_whitespace(&output)
}

#[derive(Debug, Clone, Copy)]
struct MimeTraversalBudget {
    remaining_parts: usize,
    remaining_depth: usize,
}

fn extract_mime_text(headers: &str, body: &str, budget: &mut MimeTraversalBudget) -> String {
    if budget.remaining_parts == 0 || budget.remaining_depth == 0 {
        return String::new();
    }
    budget.remaining_parts -= 1;
    let header_lines = unfolded_header_lines(headers);
    if header_value(&header_lines, "content-disposition")
        .is_some_and(|value| value.to_ascii_lowercase().contains("attachment"))
    {
        return String::new();
    }

    let content_type = header_value(&header_lines, "content-type")
        .unwrap_or("text/plain")
        .to_string();
    let lower_content_type = content_type.to_ascii_lowercase();
    if lower_content_type.starts_with("multipart/") {
        let Some(boundary) = header_param(&content_type, "boundary") else {
            return String::new();
        };
        budget.remaining_depth -= 1;
        let mut output = String::new();
        for part in multipart_parts(body, &boundary) {
            output.push_str(&extract_mime_text(part.headers, part.body, budget));
            output.push(' ');
            if budget.remaining_parts == 0 {
                break;
            }
        }
        budget.remaining_depth += 1;
        return output;
    }

    let transfer_encoding = header_value(&header_lines, "content-transfer-encoding")
        .unwrap_or("7bit")
        .to_ascii_lowercase();
    let decoded = decode_transfer_body(body, &transfer_encoding);
    if lower_content_type.starts_with("text/html") {
        html_text(&decoded)
    } else if lower_content_type.starts_with("text/")
        || header_value(&header_lines, "content-type").is_none()
    {
        decoded
    } else {
        String::new()
    }
}

#[derive(Debug, Clone, Copy)]
struct MimePart<'a> {
    headers: &'a str,
    body: &'a str,
}

fn multipart_parts<'a>(body: &'a str, boundary: &str) -> Vec<MimePart<'a>> {
    let marker = format!("--{boundary}");
    let closing = format!("--{boundary}--");
    let mut parts = Vec::new();
    let mut current_start = None;
    let mut cursor = 0usize;
    for line in body.split_inclusive('\n') {
        let line_start = cursor;
        cursor += line.len();
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed == marker || trimmed == closing {
            if let Some(start) = current_start {
                push_mime_part(&mut parts, &body[start..line_start]);
            }
            if trimmed == closing {
                return parts;
            }
            current_start = Some(cursor);
        }
    }
    if let Some(start) = current_start {
        push_mime_part(&mut parts, &body[start..]);
    }
    parts
}

fn push_mime_part<'a>(parts: &mut Vec<MimePart<'a>>, part: &'a str) {
    let part = part.trim_matches('\n');
    if let Some((headers, body)) = part.split_once("\n\n") {
        parts.push(MimePart { headers, body });
    }
}

fn unfolded_header_lines(headers: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for line in headers.lines() {
        if line.starts_with(char::is_whitespace) {
            if let Some(previous) = lines.last_mut() {
                previous.push(' ');
                previous.push_str(line.trim());
            }
        } else {
            lines.push(line.to_string());
        }
    }
    lines
}

fn header_value<'a>(headers: &'a [String], name: &str) -> Option<&'a str> {
    headers.iter().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.eq_ignore_ascii_case(name).then_some(value.trim())
    })
}

fn header_param(header_value: &str, name: &str) -> Option<String> {
    header_value.split(';').skip(1).find_map(|param| {
        let (key, value) = param.trim().split_once('=')?;
        if !key.trim().eq_ignore_ascii_case(name) {
            return None;
        }
        Some(value.trim().trim_matches('"').to_string())
    })
}

fn decode_transfer_body(body: &str, encoding: &str) -> String {
    match encoding {
        "quoted-printable" | "7bit" | "8bit" | "binary" => decode_quoted_printable(body),
        "base64" => decode_base64(body),
        _ => body.to_string(),
    }
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

fn decode_base64(input: &str) -> String {
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut quantum = [0u8; 4];
    let mut quantum_len = 0usize;
    for byte in input.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
        let Some(value) = base64_value(byte) else {
            continue;
        };
        quantum[quantum_len] = value;
        quantum_len += 1;
        if quantum_len == 4 {
            output.push((quantum[0] << 2) | (quantum[1] >> 4));
            if quantum[2] != 64 {
                output.push((quantum[1] << 4) | (quantum[2] >> 2));
            }
            if quantum[3] != 64 {
                output.push((quantum[2] << 6) | quantum[3]);
            }
            quantum_len = 0;
        }
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        b'=' => Some(64),
        _ => None,
    }
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

fn truncate_text(text: &str, max_bytes: usize) -> String {
    let end = floor_char_boundary(text, max_bytes);
    text[..end].to_string()
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_html_visible_text() {
        let doc = extract_rich(
            b"<html><style>.x{}</style><body><h1>Hello &amp; welcome</h1><script>hidden()</script><p>needle text</p></body></html>",
            RichKind::Html,
            usize::MAX,
        )
        .unwrap();

        assert_eq!(doc.text, "Hello & welcome needle text");
    }

    #[test]
    fn extracts_rtf_visible_text() {
        let doc = extract_rich(
            br"{\rtf1\ansi Hello\par rich \'74ext}",
            RichKind::Rtf,
            usize::MAX,
        )
        .unwrap();

        assert_eq!(doc.text, "Hello rich text");
    }

    #[test]
    fn extracts_email_headers_and_body() {
        let doc = extract_rich(
            b"From: Ada <ada@example.com>\r\nTo: Team\r\nSubject: Launch\r\n\r\nBody has mailneedle=20text",
            RichKind::Email,
            usize::MAX,
        )
        .unwrap();

        assert_eq!(
            doc.text,
            "Ada <ada@example.com> Team Launch Body has mailneedle text"
        );
    }

    #[test]
    fn extracts_multipart_email_text_without_attachments() {
        let doc = extract_rich(
            br#"From: Ada <ada@example.com>
To: Team
Subject: Launch
Content-Type: multipart/mixed; boundary="outer"

--outer
Content-Type: text/plain; charset=utf-8
Content-Transfer-Encoding: quoted-printable

Plain part has multipartneedle=20text
--outer
Content-Type: text/html
Content-Transfer-Encoding: base64

PGh0bWw+PGJvZHk+PHA+SFRNTCBwYXJ0IGhhcyBodG1sbmVlZGxlPC9wPjwvYm9keT48L2h0bWw+
--outer
Content-Type: application/octet-stream
Content-Disposition: attachment; filename="secret.txt"

attachmentneedle should not index
--outer--
"#,
            RichKind::Email,
            usize::MAX,
        )
        .unwrap();

        assert!(doc.text.contains("multipartneedle text"), "{}", doc.text);
        assert!(
            doc.text.contains("HTML part has htmlneedle"),
            "{}",
            doc.text
        );
        assert!(!doc.text.contains("attachmentneedle"), "{}", doc.text);
    }

    #[test]
    fn extracts_nested_multipart_alternative_email() {
        let doc = extract_rich(
            br#"Subject: Nested
Content-Type: multipart/mixed; boundary=outer

--outer
Content-Type: multipart/alternative; boundary=inner

--inner
Content-Type: text/plain

Nested plain nestedneedle
--inner
Content-Type: text/html

<p>Nested html alternate</p>
--inner--
--outer--
"#,
            RichKind::Email,
            usize::MAX,
        )
        .unwrap();

        assert!(doc.text.contains("nestedneedle"), "{}", doc.text);
        assert!(doc.text.contains("Nested html alternate"), "{}", doc.text);
    }

    #[test]
    fn rich_output_budget_truncates_without_splitting_utf8() {
        let doc = extract_rich(
            b"<p>alpha \xe6\x9d\xb1\xe4\xba\xac beta</p>",
            RichKind::Html,
            "alpha 東".len(),
        )
        .unwrap();

        assert_eq!(doc.bytes_read, "<p>alpha 東京 beta</p>".len());
        assert_eq!(doc.text, "alpha 東");
    }
}
