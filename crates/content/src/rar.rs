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
const RAR5_HEADER_EXTRA: u64 = 0x0001;
const RAR5_HEADER_DATA: u64 = 0x0002;
const RAR5_DATA_CONTINUES_PREVIOUS: u64 = 0x0008;
const RAR5_DATA_CONTINUES_NEXT: u64 = 0x0010;
const RAR5_MAIN_HEAD: u64 = 1;
const RAR5_FILE_HEAD: u64 = 2;
const RAR5_SERVICE_HEAD: u64 = 3;
const RAR5_ENCRYPTION_HEAD: u64 = 4;
const RAR5_END_HEAD: u64 = 5;
const RAR5_MAIN_VOLUME: u64 = 0x0001;
const RAR5_FILE_DIRECTORY: u64 = 0x0001;
const RAR5_FILE_UNIX_TIME: u64 = 0x0002;
const RAR5_FILE_CRC: u64 = 0x0004;
const RAR5_EXTRA_FILE_ENCRYPTION: u64 = 0x0001;
const RAR5_MAX_HEADER_BYTES: usize = 2 * 1024 * 1024;

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
        return extract_rar5_metadata_checked(bytes, policy, check_control);
    }
    Ok((ArchiveExtractStatus::Corrupt, None))
}

fn extract_rar5_metadata_checked(
    bytes: &[u8],
    policy: &ExtractionPolicy,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<(ArchiveExtractStatus, Option<ContentDocument>)> {
    let mut cursor = RAR5_SIGNATURE.len();
    let mut entries = 0usize;
    let mut text = String::new();

    while cursor < bytes.len() {
        check_control()?;
        let Some(header) = Rar5BlockHeader::parse(bytes, cursor) else {
            return Ok((ArchiveExtractStatus::Corrupt, None));
        };
        if header.header_bytes > RAR5_MAX_HEADER_BYTES
            || header.header_end > bytes.len()
            || header.block_end > bytes.len()
            || header.block_end <= cursor
        {
            return Ok((ArchiveExtractStatus::Corrupt, None));
        }
        if header.flags & RAR5_DATA_CONTINUES_PREVIOUS != 0
            || header.flags & RAR5_DATA_CONTINUES_NEXT != 0
        {
            return Ok((ArchiveExtractStatus::Unsupported, None));
        }

        match header.kind {
            RAR5_MAIN_HEAD => {
                let Some(main) = parse_rar5_main_header(bytes, header) else {
                    return Ok((ArchiveExtractStatus::Corrupt, None));
                };
                if main.volume {
                    return Ok((ArchiveExtractStatus::Unsupported, None));
                }
            }
            RAR5_FILE_HEAD => {
                let Some(entry) = parse_rar5_file_entry(bytes, header) else {
                    return Ok((ArchiveExtractStatus::Corrupt, None));
                };
                match entry.extra_status {
                    Rar5ExtraInspection::Clear => {}
                    Rar5ExtraInspection::Encrypted => {
                        return Ok((ArchiveExtractStatus::Encrypted, None));
                    }
                    Rar5ExtraInspection::Corrupt => {
                        return Ok((ArchiveExtractStatus::Corrupt, None))
                    }
                }
                entries += 1;
                if entries > policy.max_archive_entries {
                    return Ok((ArchiveExtractStatus::TooManyEntries, None));
                }
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
            RAR5_SERVICE_HEAD => match inspect_rar5_extra_area(bytes, header) {
                Rar5ExtraInspection::Clear => {}
                Rar5ExtraInspection::Encrypted => {
                    return Ok((ArchiveExtractStatus::Encrypted, None));
                }
                Rar5ExtraInspection::Corrupt => return Ok((ArchiveExtractStatus::Corrupt, None)),
            },
            RAR5_ENCRYPTION_HEAD => return Ok((ArchiveExtractStatus::Encrypted, None)),
            RAR5_END_HEAD => break,
            _ => {}
        }

        cursor = header.block_end;
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

#[derive(Debug, Clone, Copy)]
struct Rar5BlockHeader {
    kind: u64,
    flags: u64,
    header_bytes: usize,
    body_start: usize,
    extra_start: usize,
    header_end: usize,
    block_end: usize,
}

impl Rar5BlockHeader {
    fn parse(bytes: &[u8], cursor: usize) -> Option<Self> {
        let mut header_cursor = cursor.checked_add(4)?;
        let header_size = read_rar5_vint(bytes, &mut header_cursor)?;
        let header_bytes = usize::try_from(header_size).ok()?;
        let header_start = header_cursor;
        let header_end = header_start.checked_add(header_bytes)?;
        if header_end > bytes.len() {
            return None;
        }
        let mut body_cursor = header_start;
        let kind = read_rar5_vint(bytes, &mut body_cursor)?;
        let flags = read_rar5_vint(bytes, &mut body_cursor)?;
        let extra_size = if flags & RAR5_HEADER_EXTRA != 0 {
            usize::try_from(read_rar5_vint(bytes, &mut body_cursor)?).ok()?
        } else {
            0
        };
        let data_size = if flags & RAR5_HEADER_DATA != 0 {
            usize::try_from(read_rar5_vint(bytes, &mut body_cursor)?).ok()?
        } else {
            0
        };
        let extra_start = header_end.checked_sub(extra_size)?;
        if body_cursor > extra_start {
            return None;
        }
        let block_end = header_end.checked_add(data_size)?;
        Some(Self {
            kind,
            flags,
            header_bytes,
            body_start: body_cursor,
            extra_start,
            header_end,
            block_end,
        })
    }
}

struct Rar5MainHeader {
    volume: bool,
}

struct Rar5FileEntry {
    name: String,
    unpacked_size: u64,
    extra_status: Rar5ExtraInspection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rar5ExtraInspection {
    Clear,
    Encrypted,
    Corrupt,
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

fn parse_rar5_main_header(bytes: &[u8], header: Rar5BlockHeader) -> Option<Rar5MainHeader> {
    let mut cursor = header.body_start;
    let archive_flags = read_rar5_vint_limited(bytes, &mut cursor, header.extra_start)?;
    Some(Rar5MainHeader {
        volume: archive_flags & RAR5_MAIN_VOLUME != 0,
    })
}

fn parse_rar5_file_entry(bytes: &[u8], header: Rar5BlockHeader) -> Option<Rar5FileEntry> {
    let mut cursor = header.body_start;
    let file_flags = read_rar5_vint_limited(bytes, &mut cursor, header.extra_start)?;
    let unpacked_size = read_rar5_vint_limited(bytes, &mut cursor, header.extra_start)?;
    let _attributes = read_rar5_vint_limited(bytes, &mut cursor, header.extra_start)?;
    if file_flags & RAR5_FILE_UNIX_TIME != 0 {
        take_limited(bytes, &mut cursor, 4, header.extra_start)?;
    }
    if file_flags & RAR5_FILE_CRC != 0 {
        take_limited(bytes, &mut cursor, 4, header.extra_start)?;
    }
    let _compression_info = read_rar5_vint_limited(bytes, &mut cursor, header.extra_start)?;
    let _host_os = read_rar5_vint_limited(bytes, &mut cursor, header.extra_start)?;
    let name_len = usize::try_from(read_rar5_vint_limited(
        bytes,
        &mut cursor,
        header.extra_start,
    )?)
    .ok()?;
    let name_bytes = take_limited(bytes, &mut cursor, name_len, header.extra_start)?;
    let name = String::from_utf8(name_bytes.to_vec()).ok()?;
    Some(Rar5FileEntry {
        name,
        unpacked_size: if file_flags & RAR5_FILE_DIRECTORY != 0 {
            0
        } else {
            unpacked_size
        },
        extra_status: inspect_rar5_extra_area(bytes, header),
    })
}

fn inspect_rar5_extra_area(bytes: &[u8], header: Rar5BlockHeader) -> Rar5ExtraInspection {
    let mut cursor = header.extra_start;
    while cursor < header.header_end {
        let Some(record_size) = read_rar5_vint_limited(bytes, &mut cursor, header.header_end)
            .and_then(|size| usize::try_from(size).ok())
        else {
            return Rar5ExtraInspection::Corrupt;
        };
        let Some(record_end) = cursor.checked_add(record_size) else {
            return Rar5ExtraInspection::Corrupt;
        };
        if record_size == 0 || record_end > header.header_end {
            return Rar5ExtraInspection::Corrupt;
        }
        let record_start = cursor;
        if let Some(record_type) = read_rar5_vint_limited(bytes, &mut cursor, record_end) {
            if record_type == RAR5_EXTRA_FILE_ENCRYPTION {
                return Rar5ExtraInspection::Encrypted;
            }
        } else {
            return Rar5ExtraInspection::Corrupt;
        }
        cursor = record_end.max(record_start);
    }
    Rar5ExtraInspection::Clear
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

fn read_rar5_vint(bytes: &[u8], cursor: &mut usize) -> Option<u64> {
    read_rar5_vint_limited(bytes, cursor, bytes.len())
}

fn read_rar5_vint_limited(bytes: &[u8], cursor: &mut usize, limit: usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        if *cursor >= limit {
            return None;
        }
        let byte = *bytes.get(*cursor)?;
        *cursor += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

fn take_limited<'a>(
    bytes: &'a [u8],
    cursor: &mut usize,
    len: usize,
    limit: usize,
) -> Option<&'a [u8]> {
    let end = cursor.checked_add(len)?;
    if end > limit {
        return None;
    }
    let slice = bytes.get(*cursor..end)?;
    *cursor = end;
    Some(slice)
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}
