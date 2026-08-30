use crate::{normalize_text, ContentDocument, ExtractionPolicy};
use flate2::read::ZlibDecoder;
use gfm_types::{GfmError, Result};
use std::io::Read;

const PDF_INFLATE_CHUNK_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PdfExtractStatus {
    Extracted,
    Unsupported,
    TooLarge,
    TooManyPages,
    TooManyObjects,
    Encrypted,
    Corrupt,
}

#[cfg(test)]
fn extract_pdf(
    bytes: &[u8],
    policy: &ExtractionPolicy,
) -> (PdfExtractStatus, Option<ContentDocument>) {
    extract_pdf_checked(bytes, policy, || Ok(()))
        .expect("non-cancellable PDF extraction cannot cancel")
}

pub(crate) fn extract_pdf_checked(
    bytes: &[u8],
    policy: &ExtractionPolicy,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<(PdfExtractStatus, Option<ContentDocument>)> {
    check_control()?;
    if bytes.len() as u64 > policy.max_pdf_bytes {
        return Ok((PdfExtractStatus::TooLarge, None));
    }
    if !bytes.starts_with(b"%PDF-") {
        return Ok((PdfExtractStatus::Unsupported, None));
    }
    if has_encryption_dictionary(bytes) {
        return Ok((PdfExtractStatus::Encrypted, None));
    }

    let objects = count_marker(bytes, b" obj");
    if objects > policy.max_pdf_objects {
        return Ok((PdfExtractStatus::TooManyObjects, None));
    }

    let pages = count_pages(bytes);
    if pages > policy.max_pdf_pages {
        return Ok((PdfExtractStatus::TooManyPages, None));
    }

    let mut text = String::new();
    for stream in streams(bytes) {
        check_control()?;
        if stream_has_filter(stream.header, b"LZWDecode")
            || stream_has_filter(stream.header, b"ASCII85Decode")
            || stream_has_filter(stream.header, b"DCTDecode")
        {
            continue;
        }
        if stream_has_filter(stream.header, b"FlateDecode") {
            match inflate_stream_checked(
                stream.body,
                policy.max_pdf_stream_bytes,
                &mut check_control,
            ) {
                Ok(decoded) => extract_text_stream(&decoded, &mut text),
                Err(InflateError::TooLarge) => return Ok((PdfExtractStatus::TooLarge, None)),
                Err(InflateError::Corrupt) => return Ok((PdfExtractStatus::Corrupt, None)),
                Err(InflateError::Control(err)) => return Err(err),
            }
        } else {
            extract_text_stream(stream.body, &mut text);
        }
        check_control()?;
    }

    let text = normalize_text(text.trim());
    if text.is_empty() {
        return Ok((PdfExtractStatus::Unsupported, None));
    }

    Ok((
        PdfExtractStatus::Extracted,
        Some(ContentDocument {
            bytes_read: bytes.len(),
            text,
        }),
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum InflateError {
    TooLarge,
    Corrupt,
    Control(GfmError),
}

fn inflate_stream_checked(
    bytes: &[u8],
    max_bytes: usize,
    mut check_control: impl FnMut() -> Result<()>,
) -> std::result::Result<Vec<u8>, InflateError> {
    check_control().map_err(InflateError::Control)?;
    let limit = u64::try_from(max_bytes).unwrap_or(u64::MAX);
    let mut decoder = ZlibDecoder::new(bytes);
    let mut decoded = Vec::new();
    let mut reader = decoder.by_ref().take(limit.saturating_add(1));
    let mut buffer = [0_u8; PDF_INFLATE_CHUNK_BYTES];
    loop {
        check_control().map_err(InflateError::Control)?;
        let read = reader
            .read(&mut buffer)
            .map_err(|_| InflateError::Corrupt)?;
        check_control().map_err(InflateError::Control)?;
        if read == 0 {
            break;
        }
        decoded.extend_from_slice(&buffer[..read]);
        if decoded.len() > max_bytes {
            return Err(InflateError::TooLarge);
        }
    }
    Ok(decoded)
}

#[derive(Debug, Clone, Copy)]
struct PdfStream<'a> {
    header: &'a [u8],
    body: &'a [u8],
}

fn streams(bytes: &[u8]) -> impl Iterator<Item = PdfStream<'_>> {
    let mut cursor = 0;
    std::iter::from_fn(move || {
        let stream_at = find_from(bytes, b"stream", cursor)?;
        let body_start = stream_body_start(bytes, stream_at + b"stream".len())?;
        let end_at = find_from(bytes, b"endstream", body_start)?;
        let header_start = bytes[..stream_at]
            .iter()
            .rposition(|byte| *byte == b'<')
            .unwrap_or(stream_at.saturating_sub(256));
        cursor = end_at + b"endstream".len();
        Some(PdfStream {
            header: &bytes[header_start..stream_at],
            body: trim_stream_body(&bytes[body_start..end_at]),
        })
    })
}

fn stream_body_start(bytes: &[u8], mut index: usize) -> Option<usize> {
    if index >= bytes.len() {
        return None;
    }
    if bytes.get(index) == Some(&b'\r') && bytes.get(index + 1) == Some(&b'\n') {
        index += 2;
    } else if matches!(bytes.get(index), Some(b'\n' | b'\r')) {
        index += 1;
    }
    Some(index)
}

fn trim_stream_body(mut body: &[u8]) -> &[u8] {
    while body
        .last()
        .is_some_and(|byte| matches!(byte, b'\n' | b'\r'))
    {
        body = &body[..body.len() - 1];
    }
    body
}

fn extract_text_stream(bytes: &[u8], output: &mut String) {
    let mut cursor = 0;
    while let Some(begin) = find_from(bytes, b"BT", cursor) {
        let text_start = begin + 2;
        let Some(end) = find_from(bytes, b"ET", text_start) else {
            break;
        };
        extract_text_section(&bytes[text_start..end], output);
        cursor = end + 2;
    }
}

fn extract_text_section(bytes: &[u8], output: &mut String) {
    let mut cursor = 0;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'(' => {
                if let Some((value, next)) = parse_literal(bytes, cursor) {
                    push_pdf_text(output, &value);
                    cursor = next;
                } else {
                    cursor += 1;
                }
            }
            b'<' if bytes.get(cursor + 1) != Some(&b'<') => {
                if let Some((value, next)) = parse_hex(bytes, cursor) {
                    push_pdf_text(output, &value);
                    cursor = next;
                } else {
                    cursor += 1;
                }
            }
            b'\'' | b'"' => {
                output.push('\n');
                cursor += 1;
            }
            _ => cursor += 1,
        }
    }
}

fn parse_literal(bytes: &[u8], start: usize) -> Option<(String, usize)> {
    let mut cursor = start + 1;
    let mut depth = 1usize;
    let mut value = Vec::new();
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' => {
                let escaped = *bytes.get(cursor + 1)?;
                match escaped {
                    b'n' => value.push(b'\n'),
                    b'r' => value.push(b'\r'),
                    b't' => value.push(b'\t'),
                    b'b' => value.push(0x08),
                    b'f' => value.push(0x0c),
                    b'(' | b')' | b'\\' => value.push(escaped),
                    b'\n' => {}
                    b'\r' => {
                        if bytes.get(cursor + 2) == Some(&b'\n') {
                            cursor += 1;
                        }
                    }
                    b'0'..=b'7' => {
                        let (octal, next) = parse_octal(bytes, cursor + 1);
                        value.push(octal);
                        cursor = next.saturating_sub(1);
                    }
                    _ => value.push(escaped),
                }
                cursor += 2;
            }
            b'(' => {
                depth += 1;
                value.push(b'(');
                cursor += 1;
            }
            b')' => {
                depth -= 1;
                cursor += 1;
                if depth == 0 {
                    return Some((String::from_utf8_lossy(&value).into_owned(), cursor));
                }
                value.push(b')');
            }
            byte => {
                value.push(byte);
                cursor += 1;
            }
        }
    }
    None
}

fn parse_octal(bytes: &[u8], start: usize) -> (u8, usize) {
    let mut cursor = start;
    let mut value = 0u16;
    for _ in 0..3 {
        let Some(byte @ b'0'..=b'7') = bytes.get(cursor).copied() else {
            break;
        };
        value = value * 8 + u16::from(byte - b'0');
        cursor += 1;
    }
    (value.min(255) as u8, cursor)
}

fn parse_hex(bytes: &[u8], start: usize) -> Option<(String, usize)> {
    let end = bytes[start + 1..]
        .iter()
        .position(|byte| *byte == b'>')
        .map(|offset| start + 1 + offset)?;
    let mut nibbles: Vec<_> = bytes[start + 1..end]
        .iter()
        .filter(|byte| !byte.is_ascii_whitespace())
        .filter_map(|byte| hex_value(*byte))
        .collect();
    if nibbles.len() % 2 == 1 {
        nibbles.push(0);
    }
    let decoded: Vec<_> = nibbles
        .chunks(2)
        .map(|pair| (pair[0] << 4) | pair[1])
        .collect();
    Some((String::from_utf8_lossy(&decoded).into_owned(), end + 1))
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn push_pdf_text(output: &mut String, value: &str) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    if !output.is_empty() && !output.ends_with([' ', '\n']) {
        output.push(' ');
    }
    output.push_str(value);
}

fn count_pages(bytes: &[u8]) -> usize {
    let explicit_pages = count_marker(bytes, b"/Type /Page");
    explicit_pages.saturating_sub(count_marker(bytes, b"/Type /Pages"))
}

fn count_marker(bytes: &[u8], needle: &[u8]) -> usize {
    let mut cursor = 0;
    let mut count = 0;
    while let Some(position) = find_from(bytes, needle, cursor) {
        count += 1;
        cursor = position + needle.len();
    }
    count
}

fn stream_has_filter(bytes: &[u8], filter: &[u8]) -> bool {
    bytes.windows(filter.len()).any(|window| window == filter)
}

fn has_encryption_dictionary(bytes: &[u8]) -> bool {
    bytes
        .windows(b"/Encrypt".len())
        .any(|window| window == b"/Encrypt")
}

fn find_from(haystack: &[u8], needle: &[u8], start: usize) -> Option<usize> {
    if needle.is_empty() || start >= haystack.len() {
        return None;
    }
    haystack[start..]
        .windows(needle.len())
        .position(|offset| offset == needle)
        .map(|offset| start + offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn extracts_literal_pdf_text_streams() {
        let pdf = b"%PDF-1.4
1 0 obj
<< /Type /Page /Contents 2 0 R >>
endobj
2 0 obj
<< /Length 44 >>
stream
BT /F1 12 Tf 72 720 Td (Hello \\(PDF\\) text) Tj ET
endstream
endobj
%%EOF";

        let (_, doc) = extract_pdf(pdf, &ExtractionPolicy::default());

        assert_eq!(doc.unwrap().text, "Hello (PDF) text");
    }

    #[test]
    fn isolates_corrupt_pdf_literals() {
        let pdf = b"%PDF-1.4
1 0 obj
<< /Type /Page /Contents 2 0 R >>
endobj
2 0 obj
<< /Length 20 >>
stream
BT (unterminated Tj ET
endstream
endobj";

        let (status, doc) = extract_pdf(pdf, &ExtractionPolicy::default());

        assert_eq!(status, PdfExtractStatus::Unsupported);
        assert!(doc.is_none());
    }

    #[test]
    fn enforces_page_budget() {
        let mut pdf = b"%PDF-1.4\n".to_vec();
        for index in 0..4 {
            pdf.extend(format!("{index} 0 obj << /Type /Page >> endobj\n").as_bytes());
        }
        let policy = ExtractionPolicy {
            max_pdf_pages: 3,
            ..ExtractionPolicy::default()
        };

        let (status, doc) = extract_pdf(&pdf, &policy);

        assert_eq!(status, PdfExtractStatus::TooManyPages);
        assert!(doc.is_none());
    }

    #[test]
    fn extracts_flate_decoded_pdf_text_streams() {
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(
            &mut encoder,
            b"BT /F1 12 Tf 72 720 Td (compressed pdfneedle) Tj ET",
        )
        .unwrap();
        let compressed = encoder.finish().unwrap();
        let mut pdf = b"%PDF-1.4
1 0 obj
<< /Type /Page /Contents 2 0 R >>
endobj
2 0 obj
<< /Length "
            .to_vec();
        pdf.extend(compressed.len().to_string().as_bytes());
        pdf.extend(
            b" /Filter /FlateDecode >>
stream
",
        );
        pdf.extend(compressed);
        pdf.extend(
            b"
endstream
endobj
%%EOF",
        );

        let (status, doc) = extract_pdf(&pdf, &ExtractionPolicy::default());

        assert_eq!(status, PdfExtractStatus::Extracted);
        assert_eq!(doc.unwrap().text, "compressed pdfneedle");
    }

    #[test]
    fn checked_extraction_can_cancel_while_inflating_flate_stream() {
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(
            &mut encoder,
            format!("BT ({}) Tj ET", "pdf body ".repeat(32 * 1024)).as_bytes(),
        )
        .unwrap();
        let compressed = encoder.finish().unwrap();
        let mut pdf = b"%PDF-1.4
1 0 obj
<< /Type /Page /Contents 2 0 R >>
endobj
2 0 obj
<< /Length "
            .to_vec();
        pdf.extend(compressed.len().to_string().as_bytes());
        pdf.extend(
            b" /Filter /FlateDecode >>
stream
",
        );
        pdf.extend(compressed);
        pdf.extend(
            b"
endstream
endobj
%%EOF",
        );
        let checks = Cell::new(0);

        let result = extract_pdf_checked(&pdf, &ExtractionPolicy::default(), || {
            let next = checks.get() + 1;
            checks.set(next);
            if next >= 6 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        });

        assert!(matches!(result, Err(GfmError::Cancelled)));
    }

    #[test]
    fn reports_encrypted_pdfs_without_extracting() {
        let pdf = b"%PDF-1.7
1 0 obj
<< /Encrypt 2 0 R >>
endobj";

        let (status, doc) = extract_pdf(pdf, &ExtractionPolicy::default());

        assert_eq!(status, PdfExtractStatus::Encrypted);
        assert!(doc.is_none());
    }

    #[test]
    fn reports_corrupt_flate_streams() {
        let pdf = b"%PDF-1.4
1 0 obj
<< /Type /Page /Contents 2 0 R >>
endobj
2 0 obj
<< /Length 12 /Filter /FlateDecode >>
stream
not-valid-zlib
endstream
endobj";

        let (status, doc) = extract_pdf(pdf, &ExtractionPolicy::default());

        assert_eq!(status, PdfExtractStatus::Corrupt);
        assert!(doc.is_none());
    }
}
