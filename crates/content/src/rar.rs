use crate::archive::ArchiveExtractStatus;
use crate::{normalize_text_checked, ContentDocument, ExtractionPolicy};
use gfm_types::Result;

const RAR4_SIGNATURE: &[u8; 7] = b"Rar!\x1a\x07\x00";
const RAR5_SIGNATURE: &[u8; 8] = b"Rar!\x1a\x07\x01\x00";
const RAR4_MARK_HEAD: u8 = 0x72;
const RAR4_MAIN_HEAD: u8 = 0x73;
const RAR4_FILE_HEAD: u8 = 0x74;
const RAR4_LONG_BLOCK: u16 = 0x8000;
const RAR4_MAIN_PASSWORD: u16 = 0x0080;
const RAR4_FILE_ENCRYPTED: u16 = 0x0004;
const RAR4_FILE_LARGE: u16 = 0x0100;

pub(crate) fn extract_rar_metadata_checked(
    bytes: &[u8],
    policy: &ExtractionPolicy,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<(ArchiveExtractStatus, Option<ContentDocument>)> {
    check_control()?;
    if bytes.len() as u64 > policy.max_archive_bytes {
        return Ok((ArchiveExtractStatus::TooLarge, None));
    }
    if bytes.starts_with(RAR4_SIGNATURE) {
        return extract_rar4_metadata_checked(bytes, policy, check_control);
    }
    if bytes.starts_with(RAR5_SIGNATURE) {
        return Ok((ArchiveExtractStatus::Unsupported, None));
    }
    Ok((ArchiveExtractStatus::Corrupt, None))
}

fn extract_rar4_metadata_checked(
    bytes: &[u8],
    policy: &ExtractionPolicy,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<(ArchiveExtractStatus, Option<ContentDocument>)> {
    let mut cursor = 0usize;
    let mut entries = 0usize;
    let mut text = String::new();

    while cursor < bytes.len() {
        check_control()?;
        let Some(header) = Rar4BlockHeader::parse(bytes, cursor) else {
            return Ok((ArchiveExtractStatus::Corrupt, None));
        };
        if header.header_size < 7 || header.end > bytes.len() {
            return Ok((ArchiveExtractStatus::Corrupt, None));
        }

        match header.kind {
            RAR4_MARK_HEAD => {
                if cursor != 0 || header.header_size != RAR4_SIGNATURE.len() {
                    return Ok((ArchiveExtractStatus::Corrupt, None));
                }
            }
            RAR4_MAIN_HEAD => {
                if header.flags & RAR4_MAIN_PASSWORD != 0 {
                    return Ok((ArchiveExtractStatus::Encrypted, None));
                }
            }
            RAR4_FILE_HEAD => {
                if header.flags & RAR4_FILE_ENCRYPTED != 0 {
                    return Ok((ArchiveExtractStatus::Encrypted, None));
                }
                entries += 1;
                if entries > policy.max_archive_entries {
                    return Ok((ArchiveExtractStatus::TooManyEntries, None));
                }
                let Some(entry) = parse_rar4_file_entry(bytes, header) else {
                    return Ok((ArchiveExtractStatus::Corrupt, None));
                };
                push_entry_metadata(
                    &mut text,
                    &entry.name,
                    entry.unpacked_size,
                    policy.max_archive_text_bytes,
                );
                if text.len() >= policy.max_archive_text_bytes {
                    break;
                }
            }
            _ => {}
        }

        if header.end <= cursor {
            return Ok((ArchiveExtractStatus::Corrupt, None));
        }
        cursor = header.end;
    }

    let text = normalize_text_checked(text.trim(), &mut check_control)?;
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

#[derive(Debug, Clone, Copy)]
struct Rar4BlockHeader {
    start: usize,
    kind: u8,
    flags: u16,
    header_size: usize,
    end: usize,
}

impl Rar4BlockHeader {
    fn parse(bytes: &[u8], cursor: usize) -> Option<Self> {
        let common = bytes.get(cursor..cursor + 7)?;
        let kind = common[2];
        let flags = u16::from_le_bytes([common[3], common[4]]);
        let header_size = u16::from_le_bytes([common[5], common[6]]) as usize;
        let add_size = if flags & RAR4_LONG_BLOCK != 0 {
            let start = cursor.checked_add(7)?;
            let extra = bytes.get(start..start + 4)?;
            u32::from_le_bytes([extra[0], extra[1], extra[2], extra[3]]) as usize
        } else {
            0
        };
        let end = cursor.checked_add(header_size)?.checked_add(add_size)?;
        Some(Self {
            start: cursor,
            kind,
            flags,
            header_size,
            end,
        })
    }
}

struct Rar4FileEntry {
    name: String,
    unpacked_size: u64,
}

fn parse_rar4_file_entry(bytes: &[u8], header: Rar4BlockHeader) -> Option<Rar4FileEntry> {
    let body = bytes.get(header.start + 7..header.start + header.header_size)?;
    if body.len() < 25 {
        return None;
    }
    let unpacked_size_low = read_u32(body, 4)? as u64;
    let name_size = read_u16(body, 19)? as usize;
    let name_offset = if header.flags & RAR4_FILE_LARGE != 0 {
        33
    } else {
        25
    };
    let unpacked_size = if header.flags & RAR4_FILE_LARGE != 0 {
        let high_unp = read_u32(body, 29)? as u64;
        unpacked_size_low | (high_unp << 32)
    } else {
        unpacked_size_low
    };
    let name = body.get(name_offset..name_offset + name_size)?;
    let name = String::from_utf8(name.to_vec()).ok()?;
    Some(Rar4FileEntry {
        name,
        unpacked_size,
    })
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

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let bytes = bytes.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let bytes = bytes.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}
