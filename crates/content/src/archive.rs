use crate::{normalize_text, ContentDocument, ExtractionPolicy};
use flate2::read::GzDecoder;
use gfm_types::Result;
use std::io::{Cursor, Read};
use zip::ZipArchive;

const ARCHIVE_DECODE_CHUNK_BYTES: usize = 256 * 1024;

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

#[cfg(test)]
fn extract_archive_metadata(
    bytes: &[u8],
    kind: ArchiveKind,
    policy: &ExtractionPolicy,
) -> (ArchiveExtractStatus, Option<ContentDocument>) {
    extract_archive_metadata_checked(bytes, kind, policy, || Ok(()))
        .expect("non-cancellable archive metadata extraction cannot cancel")
}

pub(crate) fn extract_archive_metadata_checked(
    bytes: &[u8],
    kind: ArchiveKind,
    policy: &ExtractionPolicy,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<(ArchiveExtractStatus, Option<ContentDocument>)> {
    check_control()?;
    match kind {
        ArchiveKind::Tar => extract_tar_metadata_checked(bytes, policy, check_control),
        ArchiveKind::TarGz => extract_tar_gz_metadata_checked(bytes, policy, check_control),
        ArchiveKind::Zip => extract_zip_metadata_checked(bytes, policy, check_control),
    }
}

fn extract_zip_metadata_checked(
    bytes: &[u8],
    policy: &ExtractionPolicy,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<(ArchiveExtractStatus, Option<ContentDocument>)> {
    check_control()?;
    if bytes.len() as u64 > policy.max_archive_bytes {
        return Ok((ArchiveExtractStatus::TooLarge, None));
    }
    let Ok(mut archive) = ZipArchive::new(Cursor::new(bytes)) else {
        return Ok((ArchiveExtractStatus::Corrupt, None));
    };
    if archive.len() > policy.max_archive_entries {
        return Ok((ArchiveExtractStatus::TooManyEntries, None));
    }

    let mut text = String::new();
    for index in 0..archive.len() {
        check_control()?;
        let Ok(file) = archive.by_index(index) else {
            return Ok((ArchiveExtractStatus::Corrupt, None));
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
        return Ok((ArchiveExtractStatus::Unsupported, None));
    }

    Ok((
        ArchiveExtractStatus::Extracted,
        Some(ContentDocument {
            bytes_read: bytes.len(),
            text,
        }),
    ))
}

fn extract_tar_gz_metadata_checked(
    bytes: &[u8],
    policy: &ExtractionPolicy,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<(ArchiveExtractStatus, Option<ContentDocument>)> {
    check_control()?;
    if bytes.len() as u64 > policy.max_archive_bytes {
        return Ok((ArchiveExtractStatus::TooLarge, None));
    }
    let mut decoder = GzDecoder::new(bytes);
    let mut decoded = Vec::new();
    let limit = policy.max_archive_bytes.saturating_add(1);
    let mut reader = decoder.by_ref().take(limit);
    let mut buffer = [0_u8; ARCHIVE_DECODE_CHUNK_BYTES];
    loop {
        check_control()?;
        let read = match reader.read(&mut buffer) {
            Ok(read) => read,
            Err(_) => return Ok((ArchiveExtractStatus::Corrupt, None)),
        };
        check_control()?;
        if read == 0 {
            break;
        }
        decoded.extend_from_slice(&buffer[..read]);
        if decoded.len() as u64 > policy.max_archive_bytes {
            return Ok((ArchiveExtractStatus::TooLarge, None));
        }
    }
    extract_tar_metadata_checked(&decoded, policy, check_control)
}

fn extract_tar_metadata_checked(
    bytes: &[u8],
    policy: &ExtractionPolicy,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<(ArchiveExtractStatus, Option<ContentDocument>)> {
    check_control()?;
    if bytes.len() as u64 > policy.max_archive_bytes {
        return Ok((ArchiveExtractStatus::TooLarge, None));
    }
    let mut text = String::new();
    let mut cursor = 0usize;
    let mut entries = 0usize;
    let mut pending_path: Option<String> = None;
    while cursor + 512 <= bytes.len() {
        check_control()?;
        let header = &bytes[cursor..cursor + 512];
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        if !tar_checksum_is_plausible(header) {
            return Ok((ArchiveExtractStatus::Corrupt, None));
        }
        let Some(size) = parse_tar_size(&header[124..136]) else {
            return Ok((ArchiveExtractStatus::Corrupt, None));
        };
        let data_blocks = (usize::try_from(size)
            .unwrap_or(usize::MAX)
            .saturating_add(511))
            / 512;
        let Some(next) = cursor.checked_add(512 + data_blocks.saturating_mul(512)) else {
            return Ok((ArchiveExtractStatus::Corrupt, None));
        };
        if next > bytes.len() {
            return Ok((ArchiveExtractStatus::Corrupt, None));
        }
        let payload_start = cursor + 512;
        let payload_end = payload_start + usize::try_from(size).unwrap_or(usize::MAX);
        let payload = bytes.get(payload_start..payload_end).unwrap_or_default();
        match header[156] {
            b'x' | b'g' => {
                if let Some(path) = pax_path(payload) {
                    pending_path = Some(path);
                }
            }
            b'L' => {
                if let Some(path) = gnu_long_name(payload) {
                    pending_path = Some(path);
                }
            }
            _ => {
                entries += 1;
                if entries > policy.max_archive_entries {
                    return Ok((ArchiveExtractStatus::TooManyEntries, None));
                }
                let name = match pending_path.take() {
                    Some(path) => path,
                    None => {
                        let Some(name) = tar_entry_name(header) else {
                            return Ok((ArchiveExtractStatus::Corrupt, None));
                        };
                        name
                    }
                };
                if !name.is_empty() {
                    push_entry_metadata(&mut text, &name, size, policy.max_archive_text_bytes);
                }
            }
        }
        cursor = next;
        if text.len() >= policy.max_archive_text_bytes {
            break;
        }
    }

    let text = normalize_text(text.trim());
    if text.is_empty() {
        return Ok((ArchiveExtractStatus::Unsupported, None));
    }

    Ok((
        ArchiveExtractStatus::Extracted,
        Some(ContentDocument {
            bytes_read: bytes.len(),
            text,
        }),
    ))
}

fn pax_path(payload: &[u8]) -> Option<String> {
    let mut cursor = 0usize;
    while cursor < payload.len() {
        let space = payload[cursor..]
            .iter()
            .position(|byte| *byte == b' ')
            .map(|offset| cursor + offset)?;
        let length = std::str::from_utf8(&payload[cursor..space])
            .ok()?
            .parse::<usize>()
            .ok()?;
        if length == 0 || cursor + length > payload.len() {
            return None;
        }
        let record = &payload[space + 1..cursor + length];
        let record = record.strip_suffix(b"\n").unwrap_or(record);
        if let Some(value) = record.strip_prefix(b"path=") {
            return String::from_utf8(value.to_vec()).ok();
        }
        cursor += length;
    }
    None
}

fn gnu_long_name(payload: &[u8]) -> Option<String> {
    let end = payload
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(payload.len());
    String::from_utf8(payload[..end].to_vec()).ok()
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
    use gfm_types::GfmError;
    use std::cell::Cell;
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
    fn extracts_pax_tar_long_path_metadata() {
        let path = "deep/archive/path/with/pax-long-name-needle.txt";
        let bytes = tar_file_with_pax_path(path, "body");

        let (status, doc) =
            extract_archive_metadata(&bytes, ArchiveKind::Tar, &ExtractionPolicy::default());

        assert_eq!(status, ArchiveExtractStatus::Extracted);
        assert!(doc.unwrap().text.contains(path));
    }

    #[test]
    fn extracts_gnu_tar_long_name_metadata() {
        let path = "deep/archive/path/with/gnu-long-name-needle.txt";
        let bytes = tar_file_with_gnu_long_name(path, "body");

        let (status, doc) =
            extract_archive_metadata(&bytes, ArchiveKind::Tar, &ExtractionPolicy::default());

        assert_eq!(status, ArchiveExtractStatus::Extracted);
        assert!(doc.unwrap().text.contains(path));
    }

    #[test]
    fn extension_headers_do_not_count_as_archive_entries() {
        let path = "deep/archive/path/with/pax-budget-needle.txt";
        let bytes = tar_file_with_pax_path(path, "body");
        let policy = ExtractionPolicy {
            max_archive_entries: 1,
            ..ExtractionPolicy::default()
        };

        let (status, doc) = extract_archive_metadata(&bytes, ArchiveKind::Tar, &policy);

        assert_eq!(status, ArchiveExtractStatus::Extracted);
        assert!(doc.unwrap().text.contains(path));
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

    #[test]
    fn checked_tar_gz_extraction_can_cancel_during_decode() {
        let bytes = tar_gz_file(&[("large.txt", &"payload ".repeat(128 * 1024))]);
        let checks = Cell::new(0);

        let result = extract_archive_metadata_checked(
            &bytes,
            ArchiveKind::TarGz,
            &ExtractionPolicy::default(),
            || {
                let next = checks.get() + 1;
                checks.set(next);
                if next >= 6 {
                    Err(GfmError::Cancelled)
                } else {
                    Ok(())
                }
            },
        );

        assert!(matches!(result, Err(GfmError::Cancelled)));
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
            append_tar_entry(&mut bytes, name, b'0', text.as_bytes());
        }
        bytes.extend([0u8; 1024]);
        bytes
    }

    fn tar_file_with_pax_path(path: &str, text: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        append_tar_entry(
            &mut bytes,
            "./PaxHeaders/gfm",
            b'x',
            pax_path_record(path).as_bytes(),
        );
        append_tar_entry(&mut bytes, "truncated-name.txt", b'0', text.as_bytes());
        bytes.extend([0u8; 1024]);
        bytes
    }

    fn tar_file_with_gnu_long_name(path: &str, text: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut long_name = path.as_bytes().to_vec();
        long_name.push(0);
        append_tar_entry(&mut bytes, "././@LongLink", b'L', &long_name);
        append_tar_entry(&mut bytes, "truncated-name.txt", b'0', text.as_bytes());
        bytes.extend([0u8; 1024]);
        bytes
    }

    fn pax_path_record(path: &str) -> String {
        let mut length = 0usize;
        loop {
            let record = format!("{length} path={path}\n");
            let next = record.len();
            if next == length {
                return record;
            }
            length = next;
        }
    }

    fn append_tar_entry(bytes: &mut Vec<u8>, name: &str, typeflag: u8, payload: &[u8]) {
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
        header[156] = typeflag;
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        let checksum: u64 = header.iter().map(|byte| u64::from(*byte)).sum();
        write_tar_octal(&mut header[148..156], checksum);
        bytes.extend_from_slice(&header);
        bytes.extend_from_slice(payload);
        let padding = (512 - payload.len() % 512) % 512;
        bytes.extend(std::iter::repeat_n(0, padding));
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
