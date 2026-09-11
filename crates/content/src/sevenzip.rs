use crate::archive::ArchiveExtractStatus;
use crate::{normalize_text_checked, ContentDocument, ExtractionPolicy};
use gfm_types::Result;

const SIGNATURE: &[u8; 6] = b"7z\xbc\xaf\x27\x1c";
const HEADER_BYTES: usize = 32;
const K_END: u8 = 0x00;
const K_HEADER: u8 = 0x01;
const K_FILES_INFO: u8 = 0x05;
const K_PACK_INFO: u8 = 0x06;
const K_UNPACK_INFO: u8 = 0x07;
const K_SIZE: u8 = 0x09;
const K_CRC: u8 = 0x0a;
const K_FOLDER: u8 = 0x0b;
const K_CODERS_UNPACK_SIZE: u8 = 0x0c;
const K_NAME: u8 = 0x11;
const K_ENCODED_HEADER: u8 = 0x17;
const MAX_7Z_FILES: u64 = 1_000_000;
const MAX_7Z_CODERS: u64 = 1_024;
const SEVENZIP_AES_METHOD_ID: &[u8] = &[0x06, 0xf1, 0x07, 0x01];

pub(crate) fn extract_7z_metadata_checked(
    bytes: &[u8],
    policy: &ExtractionPolicy,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<(ArchiveExtractStatus, Option<ContentDocument>)> {
    check_control()?;
    if bytes.len() as u64 > policy.max_archive_bytes {
        return Ok((ArchiveExtractStatus::TooLarge, None));
    }
    if !bytes.starts_with(SIGNATURE) || bytes.len() < HEADER_BYTES {
        return Ok((ArchiveExtractStatus::Corrupt, None));
    }

    let next_header_offset = read_u64(bytes, 12).unwrap_or(u64::MAX);
    let next_header_size = read_u64(bytes, 20).unwrap_or(u64::MAX);
    let next_header_start = HEADER_BYTES as u64 + next_header_offset;
    let next_header_end = next_header_start.saturating_add(next_header_size);
    if next_header_size == 0 {
        return Ok((ArchiveExtractStatus::Unsupported, None));
    }
    if next_header_end > bytes.len() as u64 {
        return Ok((ArchiveExtractStatus::Corrupt, None));
    }

    let header = &bytes[next_header_start as usize..next_header_end as usize];
    let names =
        match parse_7z_names_checked(header, policy.max_archive_entries, &mut check_control)? {
            SevenZipNames::Names(names) => names,
            SevenZipNames::Unsupported => return Ok((ArchiveExtractStatus::Unsupported, None)),
            SevenZipNames::Encrypted => return Ok((ArchiveExtractStatus::Encrypted, None)),
            SevenZipNames::TooManyEntries => {
                return Ok((ArchiveExtractStatus::TooManyEntries, None))
            }
            SevenZipNames::Corrupt => return Ok((ArchiveExtractStatus::Corrupt, None)),
        };

    names_to_document(bytes.len(), names, policy, check_control)
}

enum SevenZipNames {
    Names(Vec<String>),
    Unsupported,
    Encrypted,
    TooManyEntries,
    Corrupt,
}

fn parse_7z_names_checked(
    header: &[u8],
    max_entries: usize,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<SevenZipNames> {
    check_control()?;
    let mut cursor = 0usize;
    let Some(kind) = read_byte(header, &mut cursor) else {
        return Ok(SevenZipNames::Corrupt);
    };
    if kind == K_ENCODED_HEADER {
        return parse_encoded_header_checked(&header[cursor..], check_control);
    }
    if kind != K_HEADER {
        return Ok(SevenZipNames::Corrupt);
    }
    while cursor < header.len() {
        check_control()?;
        let Some(id) = read_byte(header, &mut cursor) else {
            return Ok(SevenZipNames::Corrupt);
        };
        match id {
            K_END => return Ok(SevenZipNames::Unsupported),
            K_FILES_INFO => break,
            _ => {
                if !skip_7z_property(header, &mut cursor) {
                    return Ok(SevenZipNames::Corrupt);
                }
            }
        }
    }

    let Some(file_count) = read_7z_uint(header, &mut cursor) else {
        return Ok(SevenZipNames::Corrupt);
    };
    if file_count > MAX_7Z_FILES {
        return Ok(SevenZipNames::Corrupt);
    }
    if file_count as usize > max_entries {
        return Ok(SevenZipNames::TooManyEntries);
    }

    while cursor < header.len() {
        check_control()?;
        let Some(id) = read_byte(header, &mut cursor) else {
            return Ok(SevenZipNames::Corrupt);
        };
        match id {
            K_END => return Ok(SevenZipNames::Unsupported),
            K_NAME => {
                let Some(size) = read_7z_uint(header, &mut cursor) else {
                    return Ok(SevenZipNames::Corrupt);
                };
                let Some(external) = read_byte(header, &mut cursor) else {
                    return Ok(SevenZipNames::Corrupt);
                };
                if external != 0 {
                    return Ok(SevenZipNames::Unsupported);
                }
                let Some(payload_len) = size
                    .checked_sub(1)
                    .and_then(|value| usize::try_from(value).ok())
                else {
                    return Ok(SevenZipNames::Corrupt);
                };
                let Some(payload) = take_bytes(header, &mut cursor, payload_len) else {
                    return Ok(SevenZipNames::Corrupt);
                };
                return Ok(parse_utf16le_names(payload, file_count as usize));
            }
            _ => {
                if !skip_7z_property(header, &mut cursor) {
                    return Ok(SevenZipNames::Corrupt);
                }
            }
        }
    }

    Ok(SevenZipNames::Corrupt)
}

fn parse_encoded_header_checked(
    streams_info: &[u8],
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<SevenZipNames> {
    let mut cursor = 0usize;
    while cursor < streams_info.len() {
        check_control()?;
        let Some(id) = read_byte(streams_info, &mut cursor) else {
            return Ok(SevenZipNames::Corrupt);
        };
        match id {
            K_END => return Ok(SevenZipNames::Unsupported),
            K_PACK_INFO => {
                if !skip_pack_info(streams_info, &mut cursor) {
                    return Ok(SevenZipNames::Corrupt);
                }
            }
            K_UNPACK_INFO => {
                return parse_unpack_info_for_encryption_checked(
                    streams_info,
                    &mut cursor,
                    check_control,
                );
            }
            _ => return Ok(SevenZipNames::Unsupported),
        }
    }
    Ok(SevenZipNames::Corrupt)
}

fn parse_unpack_info_for_encryption_checked(
    bytes: &[u8],
    cursor: &mut usize,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<SevenZipNames> {
    let Some(folder_id) = read_byte(bytes, cursor) else {
        return Ok(SevenZipNames::Corrupt);
    };
    if folder_id != K_FOLDER {
        return Ok(SevenZipNames::Unsupported);
    }
    let Some(folder_count) = read_7z_uint(bytes, cursor) else {
        return Ok(SevenZipNames::Corrupt);
    };
    if folder_count > MAX_7Z_FILES {
        return Ok(SevenZipNames::Corrupt);
    }
    let Some(external) = read_byte(bytes, cursor) else {
        return Ok(SevenZipNames::Corrupt);
    };
    if external != 0 {
        return Ok(SevenZipNames::Unsupported);
    }

    let mut folder_out_streams = Vec::new();
    for _ in 0..folder_count {
        check_control()?;
        let Some(folder) = parse_folder_for_encryption(bytes, cursor) else {
            return Ok(SevenZipNames::Corrupt);
        };
        if folder.encrypted {
            return Ok(SevenZipNames::Encrypted);
        }
        folder_out_streams.push(folder.out_streams);
    }

    while *cursor < bytes.len() {
        check_control()?;
        let Some(id) = read_byte(bytes, cursor) else {
            return Ok(SevenZipNames::Corrupt);
        };
        match id {
            K_END => return Ok(SevenZipNames::Unsupported),
            K_CODERS_UNPACK_SIZE => {
                for out_streams in &folder_out_streams {
                    for _ in 0..*out_streams {
                        if read_7z_uint(bytes, cursor).is_none() {
                            return Ok(SevenZipNames::Corrupt);
                        }
                    }
                }
            }
            K_CRC => {
                if !skip_digests(bytes, cursor, folder_count) {
                    return Ok(SevenZipNames::Corrupt);
                }
            }
            _ => {
                if !skip_7z_property(bytes, cursor) {
                    return Ok(SevenZipNames::Corrupt);
                }
            }
        }
    }

    Ok(SevenZipNames::Corrupt)
}

struct SevenZipFolder {
    encrypted: bool,
    out_streams: u64,
}

fn parse_folder_for_encryption(bytes: &[u8], cursor: &mut usize) -> Option<SevenZipFolder> {
    let coder_count = read_7z_uint(bytes, cursor)?;
    if coder_count == 0 || coder_count > MAX_7Z_CODERS {
        return None;
    }

    let mut encrypted = false;
    let mut total_in_streams = 0_u64;
    let mut total_out_streams = 0_u64;
    for _ in 0..coder_count {
        let flags = read_byte(bytes, cursor)?;
        if flags & 0x80 != 0 || flags & 0x40 != 0 {
            return None;
        }
        let method_id_size = usize::from(flags & 0x0f);
        if method_id_size == 0 || method_id_size > 8 {
            return None;
        }
        let method_id = take_bytes(bytes, cursor, method_id_size)?;
        if method_id == SEVENZIP_AES_METHOD_ID {
            encrypted = true;
        }

        let (in_streams, out_streams) = if flags & 0x10 != 0 {
            (read_7z_uint(bytes, cursor)?, read_7z_uint(bytes, cursor)?)
        } else {
            (1, 1)
        };
        if in_streams == 0 || out_streams == 0 {
            return None;
        }
        total_in_streams = total_in_streams.checked_add(in_streams)?;
        total_out_streams = total_out_streams.checked_add(out_streams)?;

        if flags & 0x20 != 0 {
            let properties_size = usize::try_from(read_7z_uint(bytes, cursor)?).ok()?;
            take_bytes(bytes, cursor, properties_size)?;
        }
    }

    let bind_pairs = total_out_streams.checked_sub(1)?;
    for _ in 0..bind_pairs {
        read_7z_uint(bytes, cursor)?;
        read_7z_uint(bytes, cursor)?;
    }

    let packed_streams = total_in_streams.checked_sub(bind_pairs)?;
    if packed_streams > 1 {
        for _ in 0..packed_streams {
            read_7z_uint(bytes, cursor)?;
        }
    }

    Some(SevenZipFolder {
        encrypted,
        out_streams: total_out_streams,
    })
}

fn skip_pack_info(bytes: &[u8], cursor: &mut usize) -> bool {
    let Some(_pack_pos) = read_7z_uint(bytes, cursor) else {
        return false;
    };
    let Some(stream_count) = read_7z_uint(bytes, cursor) else {
        return false;
    };
    if stream_count > MAX_7Z_FILES {
        return false;
    }

    while *cursor < bytes.len() {
        let Some(id) = read_byte(bytes, cursor) else {
            return false;
        };
        match id {
            K_END => return true,
            K_SIZE => {
                for _ in 0..stream_count {
                    if read_7z_uint(bytes, cursor).is_none() {
                        return false;
                    }
                }
            }
            K_CRC => {
                if !skip_digests(bytes, cursor, stream_count) {
                    return false;
                }
            }
            _ => {
                if !skip_7z_property(bytes, cursor) {
                    return false;
                }
            }
        }
    }
    false
}

fn skip_digests(bytes: &[u8], cursor: &mut usize, stream_count: u64) -> bool {
    let Some(all_defined) = read_byte(bytes, cursor) else {
        return false;
    };
    let defined_count = if all_defined == 0 {
        let Some(bitmap_len) = bitmap_len(stream_count) else {
            return false;
        };
        let Some(bitmap) = take_bytes(bytes, cursor, bitmap_len) else {
            return false;
        };
        bitmap
            .iter()
            .map(|byte| u64::from(byte.count_ones()))
            .sum::<u64>()
            .min(stream_count)
    } else {
        stream_count
    };
    let Some(crc_bytes) = defined_count
        .checked_mul(4)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    take_bytes(bytes, cursor, crc_bytes).is_some()
}

fn bitmap_len(bits: u64) -> Option<usize> {
    usize::try_from(bits.checked_add(7)? / 8).ok()
}

fn parse_utf16le_names(payload: &[u8], file_count: usize) -> SevenZipNames {
    if !payload.len().is_multiple_of(2) {
        return SevenZipNames::Corrupt;
    }
    let mut names = Vec::new();
    let mut current = Vec::new();
    for chunk in payload.chunks_exact(2) {
        let unit = u16::from_le_bytes([chunk[0], chunk[1]]);
        if unit == 0 {
            let Ok(name) = String::from_utf16(&current) else {
                return SevenZipNames::Corrupt;
            };
            names.push(name);
            current.clear();
        } else {
            current.push(unit);
        }
    }
    if !current.is_empty() || names.len() != file_count {
        return SevenZipNames::Corrupt;
    }
    SevenZipNames::Names(names)
}

fn skip_7z_property(bytes: &[u8], cursor: &mut usize) -> bool {
    let Some(size) = read_7z_uint(bytes, cursor) else {
        return false;
    };
    let Ok(size) = usize::try_from(size) else {
        return false;
    };
    let Some(next) = cursor.checked_add(size) else {
        return false;
    };
    if next > bytes.len() {
        return false;
    }
    *cursor = next;
    true
}

fn names_to_document(
    bytes_read: usize,
    names: Vec<String>,
    policy: &ExtractionPolicy,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<(ArchiveExtractStatus, Option<ContentDocument>)> {
    let mut text = String::new();
    for name in names {
        check_control()?;
        if !name.is_empty() {
            push_name(&mut text, &name, policy.max_archive_text_bytes);
        }
        if text.len() >= policy.max_archive_text_bytes {
            break;
        }
    }

    let text = normalize_text_checked(text.trim(), &mut check_control)?;
    if text.is_empty() {
        return Ok((ArchiveExtractStatus::Unsupported, None));
    }
    Ok((
        ArchiveExtractStatus::Extracted,
        Some(ContentDocument { bytes_read, text }),
    ))
}

fn push_name(output: &mut String, name: &str, max_bytes: usize) {
    if output.len() >= max_bytes {
        return;
    }
    if !output.is_empty() {
        output.push(' ');
    }
    let remaining = max_bytes.saturating_sub(output.len());
    if name.len() <= remaining {
        output.push_str(name);
    } else {
        let end = floor_char_boundary(name, remaining);
        output.push_str(&name[..end]);
    }
}

fn read_byte(bytes: &[u8], cursor: &mut usize) -> Option<u8> {
    let byte = *bytes.get(*cursor)?;
    *cursor += 1;
    Some(byte)
}

fn take_bytes<'a>(bytes: &'a [u8], cursor: &mut usize, len: usize) -> Option<&'a [u8]> {
    let end = cursor.checked_add(len)?;
    let slice = bytes.get(*cursor..end)?;
    *cursor = end;
    Some(slice)
}

fn read_7z_uint(bytes: &[u8], cursor: &mut usize) -> Option<u64> {
    let first = u64::from(read_byte(bytes, cursor)?);
    let mut mask = 0x80_u64;
    let mut value = 0_u64;
    for bytes_after_first in 0..8 {
        if first & mask == 0 {
            return Some(value | ((first & (mask - 1)) << (8 * bytes_after_first)));
        }
        let next = u64::from(read_byte(bytes, cursor)?);
        value |= next << (8 * bytes_after_first);
        mask >>= 1;
    }
    Some(value)
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    let bytes = bytes.get(offset..offset + 8)?;
    Some(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}
