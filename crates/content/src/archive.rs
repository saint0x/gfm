use crate::{normalize_text, ContentDocument, ExtractionPolicy};
use std::io::Cursor;
use zip::ZipArchive;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArchiveKind {
    Zip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArchiveExtractStatus {
    Extracted,
    Unsupported,
    TooLarge,
    TooManyEntries,
    Corrupt,
}

pub(crate) fn extract_archive_metadata(
    bytes: &[u8],
    kind: ArchiveKind,
    policy: &ExtractionPolicy,
) -> (ArchiveExtractStatus, Option<ContentDocument>) {
    match kind {
        ArchiveKind::Zip => extract_zip_metadata(bytes, policy),
    }
}

fn extract_zip_metadata(
    bytes: &[u8],
    policy: &ExtractionPolicy,
) -> (ArchiveExtractStatus, Option<ContentDocument>) {
    if bytes.len() as u64 > policy.max_archive_bytes {
        return (ArchiveExtractStatus::TooLarge, None);
    }
    let Ok(mut archive) = ZipArchive::new(Cursor::new(bytes)) else {
        return (ArchiveExtractStatus::Corrupt, None);
    };
    if archive.len() > policy.max_archive_entries {
        return (ArchiveExtractStatus::TooManyEntries, None);
    }

    let mut text = String::new();
    for index in 0..archive.len() {
        let Ok(file) = archive.by_index(index) else {
            return (ArchiveExtractStatus::Corrupt, None);
        };
        push_entry_metadata(
            &mut text,
            file.name(),
            file.size(),
            policy.max_archive_text_bytes,
        );
        if text.len() >= policy.max_archive_text_bytes {
            break;
        }
    }

    let text = normalize_text(text.trim());
    if text.is_empty() {
        return (ArchiveExtractStatus::Unsupported, None);
    }

    (
        ArchiveExtractStatus::Extracted,
        Some(ContentDocument {
            bytes_read: bytes.len(),
            text,
        }),
    )
}

fn push_entry_metadata(output: &mut String, name: &str, size: u64, max_bytes: usize) {
    if output.len() >= max_bytes {
        return;
    }
    if !output.is_empty() {
        output.push(' ');
    }
    let entry = format!("{name} {size} bytes");
    let remaining = max_bytes.saturating_sub(output.len());
    if entry.len() <= remaining {
        output.push_str(&entry);
    } else {
        let end = floor_char_boundary(&entry, remaining);
        output.push_str(&entry[..end]);
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
    fn extracts_zip_entry_metadata() {
        let bytes = zip_file(&[("docs/archive-needle.txt", "body")]);

        let (_, doc) =
            extract_archive_metadata(&bytes, ArchiveKind::Zip, &ExtractionPolicy::default());

        assert!(doc.unwrap().text.contains("docs/archive-needle.txt"));
    }

    #[test]
    fn enforces_archive_entry_budget() {
        let bytes = zip_file(&[("a.txt", "a"), ("b.txt", "b")]);
        let policy = ExtractionPolicy {
            max_archive_entries: 1,
            ..ExtractionPolicy::default()
        };

        let (status, doc) = extract_archive_metadata(&bytes, ArchiveKind::Zip, &policy);

        assert_eq!(status, ArchiveExtractStatus::TooManyEntries);
        assert!(doc.is_none());
    }

    fn zip_file(parts: &[(&str, &str)]) -> Vec<u8> {
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
