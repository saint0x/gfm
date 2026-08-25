use crate::{normalize_text, ContentDocument, ExtractionPolicy};
use plist::Value;
use std::io::Cursor;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StructuredKind {
    Json,
    Csv,
    Plist,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StructuredExtractStatus {
    Extracted,
    Unsupported,
    TooLarge,
    Corrupt,
}

pub(crate) fn extract_structured(
    bytes: &[u8],
    kind: StructuredKind,
    policy: &ExtractionPolicy,
) -> (StructuredExtractStatus, Option<ContentDocument>) {
    if bytes.len() as u64 > policy.max_bytes {
        return (StructuredExtractStatus::TooLarge, None);
    }
    let text = match kind {
        StructuredKind::Json => String::from_utf8_lossy(bytes)
            .pipe(|input| extract_json_text(&input, policy.max_structured_text_bytes)),
        StructuredKind::Csv => String::from_utf8_lossy(bytes)
            .pipe(|input| extract_csv_text(&input, policy.max_structured_text_bytes)),
        StructuredKind::Plist => {
            let Ok(value) = Value::from_reader(Cursor::new(bytes)) else {
                return (StructuredExtractStatus::Corrupt, None);
            };
            let mut output = String::new();
            append_plist_value(&value, &mut output, policy.max_structured_text_bytes);
            output
        }
    };
    let text = normalize_text(text.trim());
    if text.is_empty() {
        return (StructuredExtractStatus::Unsupported, None);
    }
    (
        StructuredExtractStatus::Extracted,
        Some(ContentDocument {
            bytes_read: bytes.len(),
            text,
        }),
    )
}

fn extract_json_text(input: &str, max_bytes: usize) -> String {
    let mut output = String::new();
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => {
                let value = parse_json_string(&mut chars);
                push_token(&mut output, &value, max_bytes);
            }
            '-' | '0'..='9' => {
                let mut value = String::from(ch);
                while let Some(next) = chars.peek().copied() {
                    if next.is_ascii_digit() || matches!(next, '.' | 'e' | 'E' | '+' | '-') {
                        value.push(next);
                        chars.next();
                    } else {
                        break;
                    }
                }
                push_token(&mut output, &value, max_bytes);
            }
            't' | 'f' | 'n' => {
                let mut value = String::from(ch);
                while let Some(next) = chars.peek().copied() {
                    if next.is_ascii_alphabetic() {
                        value.push(next);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if matches!(value.as_str(), "true" | "false" | "null") {
                    push_token(&mut output, &value, max_bytes);
                }
            }
            _ => {}
        }
        if output.len() >= max_bytes {
            break;
        }
    }
    output
}

fn parse_json_string(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut value = String::new();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => break,
            '\\' => match chars.next() {
                Some('"') => value.push('"'),
                Some('\\') => value.push('\\'),
                Some('/') => value.push('/'),
                Some('b') => value.push(' '),
                Some('f') => value.push(' '),
                Some('n') => value.push(' '),
                Some('r') => value.push(' '),
                Some('t') => value.push(' '),
                Some('u') => {
                    let mut code = String::new();
                    for _ in 0..4 {
                        if let Some(hex) = chars.next() {
                            code.push(hex);
                        }
                    }
                    if let Ok(value_code) = u32::from_str_radix(&code, 16) {
                        if let Some(decoded) = char::from_u32(value_code) {
                            value.push(decoded);
                        }
                    }
                }
                Some(other) => value.push(other),
                None => break,
            },
            other => value.push(other),
        }
    }
    value
}

fn extract_csv_text(input: &str, max_bytes: usize) -> String {
    let mut output = String::new();
    let mut field = String::new();
    let mut chars = input.chars().peekable();
    let mut in_quotes = false;
    while let Some(ch) = chars.next() {
        match ch {
            '"' if in_quotes && chars.peek() == Some(&'"') => {
                chars.next();
                field.push('"');
            }
            '"' => in_quotes = !in_quotes,
            ',' | '\n' | '\r' if !in_quotes => {
                push_token(&mut output, field.trim(), max_bytes);
                field.clear();
            }
            other => field.push(other),
        }
        if output.len() >= max_bytes {
            break;
        }
    }
    push_token(&mut output, field.trim(), max_bytes);
    output
}

fn append_plist_value(value: &Value, output: &mut String, max_bytes: usize) {
    match value {
        Value::Array(values) => {
            for value in values {
                append_plist_value(value, output, max_bytes);
            }
        }
        Value::Dictionary(values) => {
            for (key, value) in values {
                push_token(output, key, max_bytes);
                append_plist_value(value, output, max_bytes);
            }
        }
        Value::String(value) => push_token(output, value, max_bytes),
        Value::Boolean(value) => push_token(output, &value.to_string(), max_bytes),
        Value::Integer(value) => push_token(output, &value.to_string(), max_bytes),
        Value::Real(value) => push_token(output, &value.to_string(), max_bytes),
        Value::Date(value) => push_token(output, &value.to_xml_format(), max_bytes),
        Value::Data(_) | Value::Uid(_) => {}
        _ => {}
    }
}

fn push_token(output: &mut String, value: &str, max_bytes: usize) {
    let value = value.trim();
    if value.is_empty() || output.len() >= max_bytes {
        return;
    }
    if !output.is_empty() {
        output.push(' ');
    }
    let remaining = max_bytes.saturating_sub(output.len());
    if value.len() <= remaining {
        output.push_str(value);
    } else {
        let end = floor_char_boundary(value, remaining);
        output.push_str(&value[..end]);
    }
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}

impl<T> Pipe for T {}

#[cfg(test)]
mod tests {
    use super::*;
    use plist::Dictionary;

    #[test]
    fn extracts_json_keys_and_values() {
        let (_, doc) = extract_structured(
            br#"{"client":"Aperture","items":[{"name":"jsonneedle","enabled":true}],"count":7}"#,
            StructuredKind::Json,
            &ExtractionPolicy::default(),
        );

        let text = doc.unwrap().text;
        assert!(text.contains("client"));
        assert!(text.contains("Aperture"));
        assert!(text.contains("jsonneedle"));
        assert!(text.contains("true"));
    }

    #[test]
    fn extracts_csv_cells() {
        let (_, doc) = extract_structured(
            b"name,notes\nAda,\"csvneedle, quoted\"\n",
            StructuredKind::Csv,
            &ExtractionPolicy::default(),
        );

        assert_eq!(doc.unwrap().text, "name notes Ada csvneedle, quoted");
    }

    #[test]
    fn extracts_binary_plist_keys_and_values() {
        let mut dictionary = Dictionary::new();
        dictionary.insert("Owner".into(), Value::String("plistneedle".into()));
        let mut bytes = Vec::new();
        Value::Dictionary(dictionary)
            .to_writer_binary(&mut bytes)
            .unwrap();

        let (_, doc) =
            extract_structured(&bytes, StructuredKind::Plist, &ExtractionPolicy::default());

        assert_eq!(doc.unwrap().text, "Owner plistneedle");
    }
}
