use crate::{normalize_text, ContentDocument};
use gfm_types::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RichKind {
    Html,
    Rtf,
    Email,
}

#[cfg(test)]
fn extract_rich(bytes: &[u8], kind: RichKind, max_text_bytes: usize) -> Option<ContentDocument> {
    extract_rich_checked(bytes, kind, max_text_bytes, || Ok(()))
        .expect("non-cancellable rich extraction cannot cancel")
}

pub(crate) fn extract_rich_checked(
    bytes: &[u8],
    kind: RichKind,
    max_text_bytes: usize,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Option<ContentDocument>> {
    check_control()?;
    let raw = String::from_utf8_lossy(bytes);
    let text = match kind {
        RichKind::Html => html_text_checked(&raw, max_text_bytes, &mut check_control),
        RichKind::Rtf => rtf_text_checked(&raw, max_text_bytes, &mut check_control),
        RichKind::Email => email_text_checked(&raw, max_text_bytes, &mut check_control),
    }?;
    check_control()?;
    let text = truncate_text_checked(
        &normalize_text(text.trim()),
        max_text_bytes,
        &mut check_control,
    )?;
    Ok((!text.is_empty()).then_some(ContentDocument {
        bytes_read: bytes.len(),
        text,
    }))
}

fn html_text_checked(
    input: &str,
    max_bytes: usize,
    check_control: &mut dyn FnMut() -> Result<()>,
) -> Result<String> {
    let mut output = String::new();
    let mut tag = String::new();
    let mut in_tag = false;
    let mut skipping: Option<&'static str> = None;
    let mut entity = String::new();
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        check_control()?;
        if output.len() >= max_bytes {
            break;
        }
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
                        push_separator_bounded(&mut output, max_bytes);
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
                push_str_bounded(&mut output, decode_entity(&entity), max_bytes);
            }
            _ if skipping.is_none() => push_char_bounded(&mut output, ch, max_bytes),
            _ => {}
        }
    }
    collapse_whitespace_bounded_checked(&output, max_bytes, check_control)
}

fn rtf_text_checked(
    input: &str,
    max_bytes: usize,
    check_control: &mut dyn FnMut() -> Result<()>,
) -> Result<String> {
    let mut output = String::new();
    let mut chars = input.chars().peekable();
    let mut depth = 0usize;
    while let Some(ch) = chars.next() {
        check_control()?;
        if output.len() >= max_bytes {
            break;
        }
        match ch {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            '\\' => match chars.peek().copied() {
                Some('\\' | '{' | '}') => {
                    push_char_bounded(&mut output, chars.next().unwrap(), max_bytes);
                }
                Some('\'') => {
                    chars.next();
                    let hi = chars.next().and_then(|ch| ch.to_digit(16));
                    let lo = chars.next().and_then(|ch| ch.to_digit(16));
                    if let (Some(hi), Some(lo)) = (hi, lo) {
                        push_char_bounded(
                            &mut output,
                            char::from_u32(hi * 16 + lo).unwrap_or(' '),
                            max_bytes,
                        );
                    }
                }
                Some(_) => {
                    let control = take_control_word(&mut chars);
                    match control.as_str() {
                        "par" | "line" | "tab" => push_separator_bounded(&mut output, max_bytes),
                        _ => {}
                    }
                }
                None => {}
            },
            '\n' | '\r' => push_separator_bounded(&mut output, max_bytes),
            _ if depth > 0 => push_char_bounded(&mut output, ch, max_bytes),
            _ => push_char_bounded(&mut output, ch, max_bytes),
        }
    }
    collapse_whitespace_bounded_checked(&output, max_bytes, check_control)
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

fn email_text_checked(
    input: &str,
    max_bytes: usize,
    check_control: &mut dyn FnMut() -> Result<()>,
) -> Result<String> {
    check_control()?;
    let normalized = normalize_newlines_checked(input, check_control)?;
    let (headers, body) = normalized
        .split_once("\n\n")
        .unwrap_or((normalized.as_str(), ""));
    let mut output = String::new();
    for line in unfolded_header_lines_checked(headers, check_control)? {
        check_control()?;
        if output.len() >= max_bytes {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("subject:") || lower.starts_with("from:") || lower.starts_with("to:") {
            push_str_bounded(
                &mut output,
                line.split_once(':')
                    .map(|(_, value)| value)
                    .unwrap_or(&line)
                    .trim(),
                max_bytes,
            );
            push_separator_bounded(&mut output, max_bytes);
        }
    }
    let mut budget = MimeTraversalBudget {
        remaining_parts: 128,
        remaining_depth: 8,
    };
    append_mime_text_checked(
        headers,
        body,
        &mut budget,
        &mut output,
        max_bytes,
        check_control,
    )?;
    collapse_whitespace_bounded_checked(&output, max_bytes, check_control)
}

#[derive(Debug, Clone, Copy)]
struct MimeTraversalBudget {
    remaining_parts: usize,
    remaining_depth: usize,
}

fn append_mime_text_checked(
    headers: &str,
    body: &str,
    budget: &mut MimeTraversalBudget,
    output: &mut String,
    max_bytes: usize,
    check_control: &mut dyn FnMut() -> Result<()>,
) -> Result<()> {
    check_control()?;
    if budget.remaining_parts == 0 || budget.remaining_depth == 0 {
        return Ok(());
    }
    if output.len() >= max_bytes {
        return Ok(());
    }
    budget.remaining_parts -= 1;
    let header_lines = unfolded_header_lines_checked(headers, check_control)?;
    if header_value(&header_lines, "content-disposition")
        .is_some_and(|value| value.to_ascii_lowercase().contains("attachment"))
    {
        return Ok(());
    }

    let content_type = header_value(&header_lines, "content-type")
        .unwrap_or("text/plain")
        .to_string();
    let lower_content_type = content_type.to_ascii_lowercase();
    if lower_content_type.starts_with("multipart/") {
        let Some(boundary) = header_param(&content_type, "boundary") else {
            return Ok(());
        };
        budget.remaining_depth -= 1;
        for part in multipart_parts_checked(body, &boundary, check_control)? {
            check_control()?;
            append_mime_text_checked(
                part.headers,
                part.body,
                budget,
                output,
                max_bytes,
                check_control,
            )?;
            push_separator_bounded(output, max_bytes);
            if budget.remaining_parts == 0 || output.len() >= max_bytes {
                break;
            }
        }
        budget.remaining_depth += 1;
        return Ok(());
    }

    let transfer_encoding = header_value(&header_lines, "content-transfer-encoding")
        .unwrap_or("7bit")
        .to_ascii_lowercase();
    let remaining = remaining_bytes(output, max_bytes);
    let decoded = decode_transfer_body_checked(body, &transfer_encoding, remaining, check_control)?;
    if lower_content_type.starts_with("text/html") {
        let visible = html_text_checked(&decoded, remaining, check_control)?;
        push_str_bounded(output, &visible, max_bytes);
    } else if lower_content_type.starts_with("text/")
        || header_value(&header_lines, "content-type").is_none()
    {
        push_str_bounded(output, &decoded, max_bytes);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct MimePart<'a> {
    headers: &'a str,
    body: &'a str,
}

fn multipart_parts_checked<'a>(
    body: &'a str,
    boundary: &str,
    check_control: &mut dyn FnMut() -> Result<()>,
) -> Result<Vec<MimePart<'a>>> {
    let marker = format!("--{boundary}");
    let closing = format!("--{boundary}--");
    let mut parts = Vec::new();
    let mut current_start = None;
    let mut cursor = 0usize;
    for line in body.split_inclusive('\n') {
        check_control()?;
        let line_start = cursor;
        cursor += line.len();
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed == marker || trimmed == closing {
            if let Some(start) = current_start {
                push_mime_part(&mut parts, &body[start..line_start]);
            }
            if trimmed == closing {
                return Ok(parts);
            }
            current_start = Some(cursor);
        }
    }
    if let Some(start) = current_start {
        push_mime_part(&mut parts, &body[start..]);
    }
    Ok(parts)
}

fn push_mime_part<'a>(parts: &mut Vec<MimePart<'a>>, part: &'a str) {
    let part = part.trim_matches('\n');
    if let Some((headers, body)) = part.split_once("\n\n") {
        parts.push(MimePart { headers, body });
    }
}

fn unfolded_header_lines_checked(
    headers: &str,
    check_control: &mut dyn FnMut() -> Result<()>,
) -> Result<Vec<String>> {
    let mut lines: Vec<String> = Vec::new();
    for line in headers.lines() {
        check_control()?;
        if line.starts_with(char::is_whitespace) {
            if let Some(previous) = lines.last_mut() {
                previous.push(' ');
                previous.push_str(line.trim());
            }
        } else {
            lines.push(line.to_string());
        }
    }
    Ok(lines)
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

fn decode_transfer_body_checked(
    body: &str,
    encoding: &str,
    max_bytes: usize,
    check_control: &mut dyn FnMut() -> Result<()>,
) -> Result<String> {
    match encoding {
        "quoted-printable" | "7bit" | "8bit" | "binary" => {
            decode_quoted_printable_checked(body, max_bytes, check_control)
        }
        "base64" => decode_base64_checked(body, max_bytes, check_control),
        _ => truncate_text_checked(body, max_bytes, check_control),
    }
}

fn decode_quoted_printable_checked(
    input: &str,
    max_bytes: usize,
    check_control: &mut dyn FnMut() -> Result<()>,
) -> Result<String> {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len().min(max_bytes));
    let mut index = 0;
    while index < bytes.len() && output.len() < max_bytes {
        check_control()?;
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
                push_byte_bounded(&mut output, (hi << 4) | lo, max_bytes);
                index += 3;
                continue;
            }
        }
        push_byte_bounded(&mut output, bytes[index], max_bytes);
        index += 1;
    }
    Ok(String::from_utf8_lossy(&output).into_owned())
}

fn decode_base64_checked(
    input: &str,
    max_bytes: usize,
    check_control: &mut dyn FnMut() -> Result<()>,
) -> Result<String> {
    let mut output = Vec::with_capacity((input.len() / 4).saturating_mul(3).min(max_bytes));
    let mut quantum = [0u8; 4];
    let mut quantum_len = 0usize;
    for byte in input.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
        check_control()?;
        if output.len() >= max_bytes {
            break;
        }
        let Some(value) = base64_value(byte) else {
            continue;
        };
        quantum[quantum_len] = value;
        quantum_len += 1;
        if quantum_len == 4 {
            if !push_byte_bounded(
                &mut output,
                (quantum[0] << 2) | (quantum[1] >> 4),
                max_bytes,
            ) {
                break;
            }
            if quantum[2] != 64
                && !push_byte_bounded(
                    &mut output,
                    (quantum[1] << 4) | (quantum[2] >> 2),
                    max_bytes,
                )
            {
                break;
            }
            if quantum[3] != 64
                && !push_byte_bounded(&mut output, (quantum[2] << 6) | quantum[3], max_bytes)
            {
                break;
            }
            quantum_len = 0;
        }
    }
    Ok(String::from_utf8_lossy(&output).into_owned())
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

fn collapse_whitespace_bounded_checked(
    input: &str,
    max_bytes: usize,
    check_control: &mut dyn FnMut() -> Result<()>,
) -> Result<String> {
    let mut output = String::new();
    for token in input.split_whitespace() {
        check_control()?;
        if output.len() >= max_bytes {
            break;
        }
        if !output.is_empty() {
            push_char_bounded(&mut output, ' ', max_bytes);
        }
        push_str_bounded(&mut output, token, max_bytes);
    }
    Ok(output)
}

fn remaining_bytes(output: &str, max_bytes: usize) -> usize {
    max_bytes.saturating_sub(output.len())
}

fn push_str_bounded(output: &mut String, value: &str, max_bytes: usize) {
    let remaining = remaining_bytes(output, max_bytes);
    if remaining == 0 {
        return;
    }
    let end = floor_char_boundary(value, remaining);
    output.push_str(&value[..end]);
}

fn push_char_bounded(output: &mut String, ch: char, max_bytes: usize) {
    if output.len().saturating_add(ch.len_utf8()) <= max_bytes {
        output.push(ch);
    }
}

fn push_separator_bounded(output: &mut String, max_bytes: usize) {
    if output.is_empty() || output.ends_with(char::is_whitespace) {
        return;
    }
    push_char_bounded(output, ' ', max_bytes);
}

fn push_byte_bounded(output: &mut Vec<u8>, byte: u8, max_bytes: usize) -> bool {
    if output.len() >= max_bytes {
        return false;
    }
    output.push(byte);
    true
}

fn truncate_text(text: &str, max_bytes: usize) -> String {
    let end = floor_char_boundary(text, max_bytes);
    text[..end].to_string()
}

fn truncate_text_checked(
    text: &str,
    max_bytes: usize,
    check_control: &mut dyn FnMut() -> Result<()>,
) -> Result<String> {
    check_control()?;
    Ok(truncate_text(text, max_bytes))
}

fn normalize_newlines_checked(
    input: &str,
    check_control: &mut dyn FnMut() -> Result<()>,
) -> Result<String> {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        check_control()?;
        if ch == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            output.push('\n');
        } else {
            output.push(ch);
        }
    }
    Ok(output)
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
    use gfm_types::GfmError;
    use std::cell::Cell;

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

    #[test]
    fn html_budget_stops_before_late_visible_text() {
        let doc = extract_rich(
            b"<p>alpha</p><p>beta</p><p>gamma</p>",
            RichKind::Html,
            "alpha beta".len(),
        )
        .unwrap();

        assert_eq!(doc.text, "alpha beta");
        assert!(!doc.text.contains("gamma"), "{}", doc.text);
    }

    #[test]
    fn email_budget_bounds_decoded_base64_body() {
        let doc = extract_rich(
            b"Subject: S\r\nContent-Transfer-Encoding: base64\r\n\r\nYWxwaGEgYmV0YSBnYW1tYQ==",
            RichKind::Email,
            "S alpha beta".len(),
        )
        .unwrap();

        assert_eq!(doc.text, "S alpha beta");
        assert!(!doc.text.contains("gamma"), "{}", doc.text);
    }

    #[test]
    fn checked_html_extraction_can_cancel_during_parse() {
        let html = format!(
            "<html><body>{}</body></html>",
            "<p>htmlneedle</p>".repeat(4096)
        );
        let checks = Cell::new(0);

        let result = extract_rich_checked(html.as_bytes(), RichKind::Html, usize::MAX, || {
            let next = checks.get() + 1;
            checks.set(next);
            if next >= 512 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        });

        assert!(matches!(result, Err(GfmError::Cancelled)));
    }

    #[test]
    fn checked_rtf_extraction_can_cancel_during_parse() {
        let rtf = format!(r"{{\rtf1\ansi {}}}", r"rtfneedle\par ".repeat(4096));
        let checks = Cell::new(0);

        let result = extract_rich_checked(rtf.as_bytes(), RichKind::Rtf, usize::MAX, || {
            let next = checks.get() + 1;
            checks.set(next);
            if next >= 512 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        });

        assert!(matches!(result, Err(GfmError::Cancelled)));
    }

    #[test]
    fn checked_email_extraction_can_cancel_during_multipart_parse() {
        let mut email =
            String::from("Subject: Large\nContent-Type: multipart/mixed; boundary=outer\n\n");
        for index in 0..1024 {
            email.push_str("--outer\nContent-Type: text/plain\n\n");
            email.push_str(&format!("part {index} emailneedle\n"));
        }
        email.push_str("--outer--\n");
        let checks = Cell::new(0);

        let result = extract_rich_checked(email.as_bytes(), RichKind::Email, usize::MAX, || {
            let next = checks.get() + 1;
            checks.set(next);
            if next >= 512 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        });

        assert!(matches!(result, Err(GfmError::Cancelled)));
    }

    #[test]
    fn checked_email_extraction_can_cancel_during_base64_decode() {
        let body = "YWxwaGEgYmV0YSBnYW1tYQ==\n".repeat(4096);
        let email = format!("Subject: Encoded\nContent-Transfer-Encoding: base64\n\n{body}");
        let checks = Cell::new(0);

        let result = extract_rich_checked(email.as_bytes(), RichKind::Email, usize::MAX, || {
            let next = checks.get() + 1;
            checks.set(next);
            if next >= 512 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        });

        assert!(matches!(result, Err(GfmError::Cancelled)));
    }
}
