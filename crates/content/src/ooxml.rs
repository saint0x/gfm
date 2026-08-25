use crate::{normalize_text, ContentDocument, ExtractionPolicy};
use std::io::{Cursor, Read};
use zip::ZipArchive;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OoxmlKind {
    Docx,
    Xlsx,
    Pptx,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OoxmlExtractStatus {
    Extracted,
    Unsupported,
    TooLarge,
    TooManyEntries,
    EntryTooLarge,
    Corrupt,
}

pub(crate) fn extract_ooxml(
    bytes: &[u8],
    kind: OoxmlKind,
    policy: &ExtractionPolicy,
) -> (OoxmlExtractStatus, Option<ContentDocument>) {
    if bytes.len() as u64 > policy.max_office_bytes {
        return (OoxmlExtractStatus::TooLarge, None);
    }
    let Ok(mut archive) = ZipArchive::new(Cursor::new(bytes)) else {
        return (OoxmlExtractStatus::Corrupt, None);
    };
    if archive.len() > policy.max_office_entries {
        return (OoxmlExtractStatus::TooManyEntries, None);
    }

    let mut text = String::new();
    let mut extracted_parts = 0usize;
    for index in 0..archive.len() {
        let Ok(file) = archive.by_index(index) else {
            return (OoxmlExtractStatus::Corrupt, None);
        };
        let name = file.name().to_string();
        if !is_text_part(kind, &name) {
            continue;
        }
        if file.size() > policy.max_office_entry_bytes {
            return (OoxmlExtractStatus::EntryTooLarge, None);
        }

        let remaining = policy.max_office_text_bytes.saturating_sub(text.len());
        if remaining == 0 {
            break;
        }
        let mut xml = String::new();
        if file
            .take(policy.max_office_entry_bytes)
            .read_to_string(&mut xml)
            .is_err()
        {
            return (OoxmlExtractStatus::Corrupt, None);
        }
        append_xml_text(&xml, &mut text, remaining);
        extracted_parts += 1;
    }

    let text = normalize_text(text.trim());
    if text.is_empty() {
        let status = if extracted_parts == 0 {
            OoxmlExtractStatus::Unsupported
        } else {
            OoxmlExtractStatus::Extracted
        };
        return (status, None);
    }

    (
        OoxmlExtractStatus::Extracted,
        Some(ContentDocument {
            bytes_read: bytes.len(),
            text,
        }),
    )
}

fn is_text_part(kind: OoxmlKind, name: &str) -> bool {
    match kind {
        OoxmlKind::Docx => {
            name == "word/document.xml"
                || name.starts_with("word/header")
                || name.starts_with("word/footer")
                || name.starts_with("word/comments")
                || name.starts_with("docProps/")
        }
        OoxmlKind::Xlsx => {
            name == "xl/sharedStrings.xml"
                || name.starts_with("xl/worksheets/sheet")
                || name.starts_with("xl/chartsheets/sheet")
                || name.starts_with("docProps/")
        }
        OoxmlKind::Pptx => {
            name.starts_with("ppt/slides/slide")
                || name.starts_with("ppt/notesSlides/notesSlide")
                || name.starts_with("ppt/comments/comment")
                || name.starts_with("docProps/")
        }
    }
}

fn append_xml_text(xml: &str, output: &mut String, max_bytes: usize) {
    let mut in_tag = false;
    let mut entity = String::new();
    let mut text = String::new();
    let mut chars = xml.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '<' => {
                flush_text(output, &mut text, max_bytes);
                in_tag = true;
            }
            '>' => in_tag = false,
            '&' if !in_tag => {
                entity.clear();
                while let Some(next) = chars.peek().copied() {
                    chars.next();
                    if next == ';' || entity.len() >= 16 {
                        break;
                    }
                    entity.push(next);
                }
                text.push_str(decode_entity(&entity));
            }
            _ if !in_tag => text.push(ch),
            _ => {}
        }
        if output.len() >= max_bytes {
            return;
        }
    }
    flush_text(output, &mut text, max_bytes);
}

fn flush_text(output: &mut String, text: &mut String, max_bytes: usize) {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    text.clear();
    if normalized.is_empty() || output.len() >= max_bytes {
        return;
    }
    if !output.is_empty() && !output.ends_with(' ') {
        output.push(' ');
    }
    let remaining = max_bytes.saturating_sub(output.len());
    if normalized.len() <= remaining {
        output.push_str(&normalized);
    } else {
        let end = floor_char_boundary(&normalized, remaining);
        output.push_str(&normalized[..end]);
    }
}

fn decode_entity(entity: &str) -> &str {
    match entity {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        _ => " ",
    }
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
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;

    #[test]
    fn extracts_docx_document_text() {
        let bytes = package(&[(
            "word/document.xml",
            "<w:document><w:body><w:p><w:r><w:t>Hello client brief</w:t></w:r></w:p></w:body></w:document>",
        )]);

        let (_, doc) = extract_ooxml(&bytes, OoxmlKind::Docx, &ExtractionPolicy::default());

        assert_eq!(doc.unwrap().text, "Hello client brief");
    }

    #[test]
    fn extracts_xlsx_shared_strings() {
        let bytes = package(&[(
            "xl/sharedStrings.xml",
            "<sst><si><t>Quarter</t></si><si><t>Revenue &amp; Margin</t></si></sst>",
        )]);

        let (_, doc) = extract_ooxml(&bytes, OoxmlKind::Xlsx, &ExtractionPolicy::default());

        assert_eq!(doc.unwrap().text, "Quarter Revenue & Margin");
    }

    #[test]
    fn enforces_entry_budget() {
        let bytes = package(&[("word/document.xml", "<w:t>large body</w:t>")]);
        let policy = ExtractionPolicy {
            max_office_entry_bytes: 4,
            ..ExtractionPolicy::default()
        };

        let (status, doc) = extract_ooxml(&bytes, OoxmlKind::Docx, &policy);

        assert_eq!(status, OoxmlExtractStatus::EntryTooLarge);
        assert!(doc.is_none());
    }

    fn package(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, text) in parts {
            writer.start_file(*name, options).unwrap();
            writer.write_all(text.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }
}
