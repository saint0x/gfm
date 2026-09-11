use crate::kind::LegacyOfficeKind;
use crate::report::ContentDocument;
use crate::ExtractionPolicy;
use gfm_types::Result;
use std::collections::HashSet;

const OLE_COMPOUND_FILE_MAGIC: &[u8; 8] = b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1";
const HEADER_BYTES: usize = 512;
const DIRECTORY_ENTRY_BYTES: usize = 128;
const FREESECT: u32 = 0xFFFF_FFFF;
const ENDOFCHAIN: u32 = 0xFFFF_FFFE;
const FATSECT: u32 = 0xFFFF_FFFD;
const MAX_SECTOR_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LegacyOfficeExtractStatus {
    Extracted,
    Unsupported,
    TooLarge,
    Encrypted,
    Corrupt,
}

pub(crate) fn extract_legacy_office_document_checked(
    bytes: &[u8],
    kind: LegacyOfficeKind,
    policy: &ExtractionPolicy,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<(LegacyOfficeExtractStatus, Option<ContentDocument>)> {
    check_control()?;
    if bytes.len() as u64 > policy.max_office_bytes {
        return Ok((LegacyOfficeExtractStatus::TooLarge, None));
    }
    if !bytes.starts_with(OLE_COMPOUND_FILE_MAGIC) {
        return Ok((LegacyOfficeExtractStatus::Unsupported, None));
    }

    let Some(directory) = OleDirectory::parse_checked(bytes, &mut check_control)? else {
        return Ok((LegacyOfficeExtractStatus::Corrupt, None));
    };
    check_control()?;

    if directory
        .entries
        .iter()
        .any(|entry| encrypted_stream_name(&entry.name))
    {
        return Ok((LegacyOfficeExtractStatus::Encrypted, None));
    }
    let required = directory
        .entries
        .iter()
        .filter(|entry| required_legacy_office_stream(kind, &entry.name))
        .collect::<Vec<_>>();
    if required.is_empty() {
        return Ok((LegacyOfficeExtractStatus::Corrupt, None));
    }

    let mut text = String::new();
    let mut bytes_read = 0;
    for entry in required {
        check_control()?;
        if entry.stream_size as u64 > policy.max_office_entry_bytes {
            continue;
        }
        let Some(stream) = directory.read_stream_checked(bytes, entry, &mut check_control)? else {
            continue;
        };
        bytes_read += stream.len();
        push_salvaged_legacy_text(
            &stream,
            policy.max_office_text_bytes,
            &mut text,
            &mut check_control,
        )?;
        if text.len() >= policy.max_office_text_bytes {
            break;
        }
    }

    let text = normalize_legacy_text(&text, policy.max_office_text_bytes);
    if text.is_empty() {
        return Ok((LegacyOfficeExtractStatus::Unsupported, None));
    }
    Ok((
        LegacyOfficeExtractStatus::Extracted,
        Some(ContentDocument { bytes_read, text }),
    ))
}

fn encrypted_stream_name(name: &str) -> bool {
    matches!(
        name,
        "EncryptedPackage" | "EncryptionInfo" | "\u{0006}DataSpaces"
    )
}

fn required_legacy_office_stream(kind: LegacyOfficeKind, name: &str) -> bool {
    match kind {
        LegacyOfficeKind::Doc => name == "WordDocument",
        LegacyOfficeKind::Xls => name == "Workbook" || name == "Book",
        LegacyOfficeKind::Ppt => name == "PowerPoint Document",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OleDirectoryEntry {
    name: String,
    object_type: u8,
    start_sector: u32,
    stream_size: usize,
}

struct OleDirectory {
    sector_bytes: usize,
    fat: Vec<u32>,
    mini_stream_cutoff: usize,
    entries: Vec<OleDirectoryEntry>,
}

impl OleDirectory {
    fn parse_checked(
        bytes: &[u8],
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Option<Self>> {
        check_control()?;
        if bytes.len() < HEADER_BYTES {
            return Ok(None);
        }

        let sector_shift = read_u16(bytes, 30).unwrap_or_default();
        let sector_bytes = match sector_shift {
            9 => 512,
            12 => 4096,
            _ => return Ok(None),
        };
        if sector_bytes > MAX_SECTOR_BYTES {
            return Ok(None);
        }

        let fat_sector_count = read_u32(bytes, 44).unwrap_or_default() as usize;
        let first_directory_sector = read_u32(bytes, 48).unwrap_or(FREESECT);
        let mini_stream_cutoff = read_u32(bytes, 56).unwrap_or(4096) as usize;
        if matches!(first_directory_sector, FREESECT | ENDOFCHAIN) {
            return Ok(None);
        }

        let sector_count = bytes.len().saturating_sub(HEADER_BYTES) / sector_bytes;
        if sector_count == 0 || fat_sector_count == 0 {
            return Ok(None);
        }

        let mut difat = Vec::new();
        for offset in (76..HEADER_BYTES).step_by(4) {
            let Some(sector) = read_u32(bytes, offset) else {
                return Ok(None);
            };
            if sector == FREESECT {
                continue;
            }
            difat.push(sector);
            if difat.len() == fat_sector_count {
                break;
            }
        }
        if difat.len() < fat_sector_count {
            return Ok(None);
        }

        let mut fat = Vec::new();
        for fat_sector in difat.into_iter().take(fat_sector_count) {
            check_control()?;
            let Some(sector) = sector_bytes_for(bytes, sector_bytes, fat_sector) else {
                return Ok(None);
            };
            for chunk in sector.chunks_exact(4) {
                fat.push(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
            }
        }

        let Some(directory_stream) = read_sector_chain_checked(
            bytes,
            sector_bytes,
            &fat,
            first_directory_sector,
            &mut check_control,
        )?
        else {
            return Ok(None);
        };
        let mut entries = Vec::new();
        for entry in directory_stream.chunks_exact(DIRECTORY_ENTRY_BYTES) {
            check_control()?;
            let object_type = entry[66];
            if object_type == 0 {
                continue;
            }
            let Some(name) = parse_directory_name(entry) else {
                return Ok(None);
            };
            if !name.is_empty() {
                entries.push(OleDirectoryEntry {
                    name,
                    object_type,
                    start_sector: read_u32(entry, 116).unwrap_or(ENDOFCHAIN),
                    stream_size: read_u64_low_usize(entry, 120).unwrap_or_default(),
                });
            }
        }

        Ok(Some(Self {
            sector_bytes,
            fat,
            mini_stream_cutoff,
            entries,
        }))
    }

    fn read_stream_checked(
        &self,
        bytes: &[u8],
        entry: &OleDirectoryEntry,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Option<Vec<u8>>> {
        check_control()?;
        if entry.object_type != 2
            || entry.stream_size == 0
            || matches!(entry.start_sector, FREESECT | ENDOFCHAIN | FATSECT)
        {
            return Ok(None);
        }

        let Some(mut stream) = read_sector_chain_checked(
            bytes,
            self.sector_bytes,
            &self.fat,
            entry.start_sector,
            &mut check_control,
        )?
        else {
            return Ok(None);
        };
        if stream.len() < entry.stream_size {
            return Ok(None);
        }
        stream.truncate(entry.stream_size);

        // Real small OLE streams usually live in the ministream. Some legacy
        // producers write regular chains anyway, so accept decodable content.
        if entry.stream_size < self.mini_stream_cutoff && !stream_has_salvageable_text(&stream) {
            return Ok(None);
        }
        Ok(Some(stream))
    }
}

fn read_sector_chain_checked(
    bytes: &[u8],
    sector_bytes: usize,
    fat: &[u32],
    first_sector: u32,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Option<Vec<u8>>> {
    let sector_count = bytes.len().saturating_sub(HEADER_BYTES) / sector_bytes;
    let mut seen = HashSet::new();
    let mut sector = first_sector;
    let mut stream = Vec::new();

    while !matches!(sector, ENDOFCHAIN | FREESECT) {
        check_control()?;
        let sector_index = sector as usize;
        if sector_index >= sector_count || sector_index >= fat.len() || !seen.insert(sector) {
            return Ok(None);
        }
        let Some(bytes) = sector_bytes_for(bytes, sector_bytes, sector) else {
            return Ok(None);
        };
        stream.extend_from_slice(bytes);
        sector = fat[sector_index];
        if matches!(sector, FATSECT) {
            return Ok(None);
        }
    }

    Ok(Some(stream))
}

fn sector_bytes_for(bytes: &[u8], sector_bytes: usize, sector: u32) -> Option<&[u8]> {
    let offset = HEADER_BYTES.checked_add((sector as usize).checked_mul(sector_bytes)?)?;
    let end = offset.checked_add(sector_bytes)?;
    bytes.get(offset..end)
}

fn parse_directory_name(entry: &[u8]) -> Option<String> {
    let name_len = read_u16(entry, 64)? as usize;
    if !(2..=64 * 2).contains(&name_len) || !name_len.is_multiple_of(2) {
        return None;
    }
    let name_bytes = entry.get(..name_len.saturating_sub(2))?;
    let utf16 = name_bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect::<Vec<_>>();
    String::from_utf16(&utf16).ok()
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let bytes = bytes.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let bytes = bytes.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_u64_low_usize(bytes: &[u8], offset: usize) -> Option<usize> {
    let low = read_u32(bytes, offset)? as usize;
    let high = read_u32(bytes, offset + 4).unwrap_or_default();
    if high != 0 {
        return None;
    }
    Some(low)
}

fn push_salvaged_legacy_text(
    bytes: &[u8],
    max_text_bytes: usize,
    out: &mut String,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    check_control()?;
    push_ascii_runs(bytes, max_text_bytes, out, &mut check_control)?;
    check_control()?;
    push_utf16le_runs(bytes, max_text_bytes, out, &mut check_control)
}

fn push_ascii_runs(
    bytes: &[u8],
    max_text_bytes: usize,
    out: &mut String,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    let mut run = Vec::new();
    for chunk in bytes.chunks(4096) {
        check_control()?;
        for &byte in chunk {
            if is_ascii_text_byte(byte) {
                run.push(byte);
            } else {
                flush_ascii_run(&mut run, max_text_bytes, out);
            }
            if out.len() >= max_text_bytes {
                return Ok(());
            }
        }
    }
    flush_ascii_run(&mut run, max_text_bytes, out);
    Ok(())
}

fn flush_ascii_run(run: &mut Vec<u8>, max_text_bytes: usize, out: &mut String) {
    const MIN_ASCII_RUN: usize = 4;
    if run.len() >= MIN_ASCII_RUN {
        append_with_limit(out, &String::from_utf8_lossy(run), max_text_bytes);
        append_with_limit(out, " ", max_text_bytes);
    }
    run.clear();
}

fn push_utf16le_runs(
    bytes: &[u8],
    max_text_bytes: usize,
    out: &mut String,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    let mut run = String::new();
    for chunk in bytes.chunks(4096) {
        check_control()?;
        for pair in chunk.chunks_exact(2) {
            let unit = u16::from_le_bytes([pair[0], pair[1]]);
            let Some(ch) = char::from_u32(unit as u32) else {
                flush_utf16_run(&mut run, max_text_bytes, out);
                continue;
            };
            if is_text_char(ch) {
                run.push(ch);
            } else {
                flush_utf16_run(&mut run, max_text_bytes, out);
            }
            if out.len() >= max_text_bytes {
                return Ok(());
            }
        }
    }
    flush_utf16_run(&mut run, max_text_bytes, out);
    Ok(())
}

fn flush_utf16_run(run: &mut String, max_text_bytes: usize, out: &mut String) {
    const MIN_UTF16_RUN_CHARS: usize = 4;
    if run.chars().count() >= MIN_UTF16_RUN_CHARS {
        append_with_limit(out, run, max_text_bytes);
        append_with_limit(out, " ", max_text_bytes);
    }
    run.clear();
}

fn append_with_limit(out: &mut String, value: &str, max_text_bytes: usize) {
    if out.len() >= max_text_bytes {
        return;
    }
    for ch in value.chars() {
        if out.len() + ch.len_utf8() > max_text_bytes {
            break;
        }
        out.push(ch);
    }
}

fn normalize_legacy_text(text: &str, max_text_bytes: usize) -> String {
    let mut out = String::new();
    let mut previous_space = true;
    for ch in text.chars() {
        if out.len() >= max_text_bytes {
            break;
        }
        if ch.is_whitespace() {
            if !previous_space {
                append_with_limit(&mut out, " ", max_text_bytes);
                previous_space = true;
            }
        } else {
            append_with_limit(&mut out, &ch.to_string(), max_text_bytes);
            previous_space = false;
        }
    }
    out.trim().to_string()
}

fn stream_has_salvageable_text(bytes: &[u8]) -> bool {
    let mut run = 0;
    for byte in bytes {
        if is_ascii_text_byte(*byte) {
            run += 1;
            if run >= 4 {
                return true;
            }
        } else {
            run = 0;
        }
    }
    let mut utf16_run = 0;
    for pair in bytes.chunks_exact(2) {
        let unit = u16::from_le_bytes([pair[0], pair[1]]);
        if let Some(ch) = char::from_u32(unit as u32) {
            if is_text_char(ch) {
                utf16_run += 1;
                if utf16_run >= 4 {
                    return true;
                }
                continue;
            }
        }
        utf16_run = 0;
    }
    false
}

fn is_ascii_text_byte(byte: u8) -> bool {
    byte == b'\t' || byte == b'\n' || byte == b'\r' || (0x20..=0x7e).contains(&byte)
}

fn is_text_char(ch: char) -> bool {
    !ch.is_control() || ch.is_whitespace()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ExtractionPolicy;

    #[test]
    fn detects_encrypted_legacy_office_directory_entries() {
        let bytes = legacy_office_compound_file(&["WordDocument", "EncryptionInfo"]);

        let (status, document) = extract_legacy_office_document_checked(
            &bytes,
            LegacyOfficeKind::Doc,
            &ExtractionPolicy::default(),
            || Ok(()),
        )
        .unwrap();

        assert_eq!(status, LegacyOfficeExtractStatus::Encrypted);
        assert!(document.is_none());
    }

    #[test]
    fn corrupts_compound_files_without_required_office_streams() {
        let bytes = legacy_office_compound_file(&["NotOffice"]);

        let (status, document) = extract_legacy_office_document_checked(
            &bytes,
            LegacyOfficeKind::Doc,
            &ExtractionPolicy::default(),
            || Ok(()),
        )
        .unwrap();

        assert_eq!(status, LegacyOfficeExtractStatus::Corrupt);
        assert!(document.is_none());
    }

    #[test]
    fn keeps_well_formed_legacy_office_unsupported_until_binary_import_lands() {
        let bytes = legacy_office_compound_file(&["Workbook"]);

        let (status, document) = extract_legacy_office_document_checked(
            &bytes,
            LegacyOfficeKind::Xls,
            &ExtractionPolicy::default(),
            || Ok(()),
        )
        .unwrap();

        assert_eq!(status, LegacyOfficeExtractStatus::Unsupported);
        assert!(document.is_none());
    }

    pub(crate) fn legacy_office_compound_file(streams: &[&str]) -> Vec<u8> {
        let mut header = vec![0_u8; HEADER_BYTES];
        header[..8].copy_from_slice(OLE_COMPOUND_FILE_MAGIC);
        header[24..26].copy_from_slice(&0x003e_u16.to_le_bytes());
        header[26..28].copy_from_slice(&0x0003_u16.to_le_bytes());
        header[28..30].copy_from_slice(&0xfffe_u16.to_le_bytes());
        header[30..32].copy_from_slice(&9_u16.to_le_bytes());
        header[32..34].copy_from_slice(&6_u16.to_le_bytes());
        header[44..48].copy_from_slice(&1_u32.to_le_bytes());
        header[48..52].copy_from_slice(&1_u32.to_le_bytes());
        header[56..60].copy_from_slice(&4096_u32.to_le_bytes());
        header[60..64].copy_from_slice(&ENDOFCHAIN.to_le_bytes());
        header[68..72].copy_from_slice(&ENDOFCHAIN.to_le_bytes());
        header[76..80].copy_from_slice(&0_u32.to_le_bytes());
        for offset in (80..HEADER_BYTES).step_by(4) {
            header[offset..offset + 4].copy_from_slice(&FREESECT.to_le_bytes());
        }

        let mut fat = vec![0xff_u8; HEADER_BYTES];
        write_fat_entry(&mut fat, 0, FATSECT);
        write_fat_entry(&mut fat, 1, ENDOFCHAIN);

        let mut directory = vec![0_u8; HEADER_BYTES];
        write_directory_entry(&mut directory[0..DIRECTORY_ENTRY_BYTES], "Root Entry", 5);
        for (index, stream) in streams.iter().take(3).enumerate() {
            let offset = (index + 1) * DIRECTORY_ENTRY_BYTES;
            write_directory_entry(
                &mut directory[offset..offset + DIRECTORY_ENTRY_BYTES],
                stream,
                2,
            );
        }

        [header, fat, directory].concat()
    }

    fn write_fat_entry(fat: &mut [u8], index: usize, value: u32) {
        let offset = index * 4;
        fat[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_directory_entry(entry: &mut [u8], name: &str, object_type: u8) {
        let mut utf16 = name.encode_utf16().collect::<Vec<_>>();
        utf16.push(0);
        for (index, unit) in utf16.iter().enumerate() {
            let offset = index * 2;
            entry[offset..offset + 2].copy_from_slice(&unit.to_le_bytes());
        }
        entry[64..66].copy_from_slice(&((utf16.len() * 2) as u16).to_le_bytes());
        entry[66] = object_type;
        entry[67] = 1;
        entry[68..72].copy_from_slice(&FREESECT.to_le_bytes());
        entry[72..76].copy_from_slice(&FREESECT.to_le_bytes());
        entry[76..80].copy_from_slice(&FREESECT.to_le_bytes());
        entry[116..120].copy_from_slice(&ENDOFCHAIN.to_le_bytes());
    }
}
