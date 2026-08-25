use crate::{normalize_text, ContentDocument, ExtractionPolicy};
use flate2::read::GzDecoder;
use std::io::{Cursor, Read};
use zip::ZipArchive;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArchiveKind {
    Tar,
    TarGz,
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
        ArchiveKind::Tar => extract_tar_metadata(bytes, policy),
        ArchiveKind::TarGz => extract_tar_gz_metadata(bytes, policy),
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

fn extract_tar_gz_metadata(
    bytes: &[u8],
    policy: &ExtractionPolicy,
) -> (ArchiveExtractStatus, Option<ContentDocument>) {
    if bytes.len() as u64 > policy.max_archive_bytes {
        return (ArchiveExtractStatus::TooLarge, None);
    }
    let mut decoder = GzDecoder::new(bytes);
    let mut decoded = Vec::new();
    let limit = policy.max_archive_bytes.saturating_add(1);
    match decoder.by_ref().take(limit).read_to_end(&mut decoded) {
        Ok(read) if read as u64 > policy.max_archive_bytes => {
            return (ArchiveExtractStatus::TooLarge, None);
        }
        Ok(_) => {}
        Err(_) => return (ArchiveExtractStatus::Corrupt, None),
    }
    extract_tar_metadata(&decoded, policy)
}

fn extract_tar_metadata(
    bytes: &[u8],
    policy: &ExtractionPolicy,
) -> (ArchiveExtractStatus, Option<ContentDocument>) {
    if bytes.len() as u64 > policy.max_archive_bytes {
        return (ArchiveExtractStatus::TooLarge, None);
    }
    let mut text = String::new();
    let mut cursor = 0usize;
    let mut entries = 0usize;
    while cursor + 512 <= bytes.len() {
        let header = &bytes[cursor..cursor + 512];
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        if !tar_checksum_is_plausible(header) {
            return (ArchiveExtractStatus::Corrupt, None);
        }
        entries += 1;
        if entries > policy.max_archive_entries {
            return (ArchiveExtractStatus::TooManyEntries, None);
        }
        let Some(size) = parse_tar_size(&header[124..136]) else {
            return (ArchiveExtractStatus::Corrupt, None);
        };
        let Some(name) = tar_entry_name(header) else {
            return (ArchiveExtractStatus::Corrupt, None);
        };
        if !name.is_empty() {
            push_entry_metadata(&mut text, &name, size, policy.max_archive_text_bytes);
        }
        let data_blocks = (usize::try_from(size)
            .unwrap_or(usize::MAX)
            .saturating_add(511))
            / 512;
        let Some(next) = cursor.checked_add(512 + data_blocks.saturating_mul(512)) else {
            return (ArchiveExtractStatus::Corrupt, None);
        };
        if next > bytes.len() {
            return (ArchiveExtractStatus::Corrupt, None);
        }
        cursor = next;
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

fn tar_entry_name(header: &[u8]) -> Option<String> {
    let name = tar_string(&header[0..100])?;
    let prefix = tar_string(&header[345..500]).unwrap_or_default();
    if prefix.is_empty() {
        Some(name)
    } else {
        Some(format!("{prefix}/{name}"))
    }
}

fn tar_string(bytes: &[u8]) -> Option<String> {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    let value = std::str::from_utf8(&bytes[..end]).ok()?.trim().to_string();
    Some(value)
}

fn parse_tar_size(bytes: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(bytes).ok()?.trim_matches(['\0', ' ']);
    if text.is_empty() {
        return Some(0);
    }
    u64::from_str_radix(text, 8).ok()
}

fn tar_checksum_is_plausible(header: &[u8]) -> bool {
    if header.len() != 512 {
        return false;
    }
    if header[148..156].iter().all(|byte| *byte == 0) {
        return true;
    }
    let Some(expected) = parse_tar_size(&header[148..156]) else {
        return false;
    };
    let actual: u64 = header
        .iter()
        .enumerate()
        .map(|(index, byte)| {
            if (148..156).contains(&index) {
                u64::from(b' ')
            } else {
                u64::from(*byte)
            }
        })
        .sum();
    actual == expected
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
    use flate2::{write::GzEncoder, Compression};
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
    fn extracts_tar_entry_metadata() {
        let bytes = tar_file(&[("docs/tar-needle.txt", "body")]);

        let (status, doc) =
            extract_archive_metadata(&bytes, ArchiveKind::Tar, &ExtractionPolicy::default());

        assert_eq!(status, ArchiveExtractStatus::Extracted);
        assert!(doc.unwrap().text.contains("docs/tar-needle.txt"));
    }

    #[test]
    fn extracts_compressed_tar_entry_metadata() {
        let bytes = tar_gz_file(&[("docs/targz-needle.txt", "body")]);

        let (status, doc) =
            extract_archive_metadata(&bytes, ArchiveKind::TarGz, &ExtractionPolicy::default());

        assert_eq!(status, ArchiveExtractStatus::Extracted);
        assert!(doc.unwrap().text.contains("docs/targz-needle.txt"));
    }

    #[test]
    fn enforces_archive_entry_budget() {
        let bytes = tar_file(&[("a.txt", "a"), ("b.txt", "b")]);
        let policy = ExtractionPolicy {
            max_archive_entries: 1,
            ..ExtractionPolicy::default()
        };

        let (status, doc) = extract_archive_metadata(&bytes, ArchiveKind::Tar, &policy);

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

    fn tar_gz_file(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&tar_file(parts)).unwrap();
        encoder.finish().unwrap()
    }

    fn tar_file(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for (name, text) in parts {
            let payload = text.as_bytes();
            let mut header = [0u8; 512];
            write_tar_string(&mut header[0..100], name);
            write_tar_octal(&mut header[100..108], 0o644);
            write_tar_octal(&mut header[108..116], 0);
            write_tar_octal(&mut header[116..124], 0);
            write_tar_octal(&mut header[124..136], payload.len() as u64);
            write_tar_octal(&mut header[136..148], 0);
            for byte in &mut header[148..156] {
                *byte = b' ';
            }
            header[156] = b'0';
            header[257..263].copy_from_slice(b"ustar\0");
            header[263..265].copy_from_slice(b"00");
            let checksum: u64 = header.iter().map(|byte| u64::from(*byte)).sum();
            write_tar_octal(&mut header[148..156], checksum);
            bytes.extend_from_slice(&header);
            bytes.extend_from_slice(payload);
            let padding = (512 - payload.len() % 512) % 512;
            bytes.extend(std::iter::repeat_n(0, padding));
        }
        bytes.extend([0u8; 1024]);
        bytes
    }

    fn write_tar_string(field: &mut [u8], value: &str) {
        let bytes = value.as_bytes();
        let len = bytes.len().min(field.len());
        field[..len].copy_from_slice(&bytes[..len]);
    }

    fn write_tar_octal(field: &mut [u8], value: u64) {
        field.fill(0);
        let encoded = format!("{value:0width$o}", width = field.len().saturating_sub(1));
        let bytes = encoded.as_bytes();
        let start = field.len().saturating_sub(1 + bytes.len());
        field[start..start + bytes.len()].copy_from_slice(bytes);
        field[field.len() - 1] = 0;
    }
}
