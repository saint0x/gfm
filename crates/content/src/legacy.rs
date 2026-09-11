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
#[cfg(test)]
const DIFSECT: u32 = 0xFFFF_FFFC;
const MAX_SECTOR_BYTES: usize = 4096;
const MINI_SECTOR_BYTES: usize = 64;
const MAX_BIFF_TEXT_RECORDS_TO_SCAN: usize = 4096;

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
        if legacy_required_stream_reports_protection(kind, &entry.name, &stream) {
            return Ok((LegacyOfficeExtractStatus::Encrypted, None));
        }
        bytes_read += stream.len();
        push_legacy_office_stream_text(
            kind,
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
    let normalized = name.trim_start_matches(|ch: char| ch.is_control());
    normalized.eq_ignore_ascii_case("EncryptedPackage")
        || normalized.eq_ignore_ascii_case("EncryptionInfo")
        || normalized.eq_ignore_ascii_case("DataSpaces")
}

fn required_legacy_office_stream(kind: LegacyOfficeKind, name: &str) -> bool {
    match kind {
        LegacyOfficeKind::Doc => name == "WordDocument",
        LegacyOfficeKind::Xls => name == "Workbook" || name == "Book",
        LegacyOfficeKind::Ppt => name == "PowerPoint Document",
    }
}

fn legacy_required_stream_reports_protection(
    kind: LegacyOfficeKind,
    name: &str,
    stream: &[u8],
) -> bool {
    match kind {
        LegacyOfficeKind::Doc if name == "WordDocument" => {
            word_document_fib_reports_protection(stream)
        }
        LegacyOfficeKind::Xls if name == "Workbook" || name == "Book" => {
            workbook_biff_reports_filepass(stream)
        }
        _ => false,
    }
}

fn word_document_fib_reports_protection(stream: &[u8]) -> bool {
    const FIB_IDENT_OFFSET: usize = 0x00;
    const FIB_IDENT: u16 = 0xa5ec;
    const FIB_FLAGS_OFFSET: usize = 0x0a;
    const F_ENCRYPTED: u16 = 1 << 8;
    const F_OBFUSCATED: u16 = 1 << 15;

    if read_u16(stream, FIB_IDENT_OFFSET) != Some(FIB_IDENT) {
        return false;
    }
    read_u16(stream, FIB_FLAGS_OFFSET)
        .is_some_and(|flags| flags & (F_ENCRYPTED | F_OBFUSCATED) != 0)
}

fn workbook_biff_reports_filepass(stream: &[u8]) -> bool {
    const BIFF_FILEPASS_RECORD: u16 = 0x002f;
    const MAX_BIFF_RECORDS_TO_SCAN: usize = 4096;

    let mut cursor = 0usize;
    let mut records = 0usize;
    while cursor + 4 <= stream.len() && records < MAX_BIFF_RECORDS_TO_SCAN {
        let Some(kind) = read_u16(stream, cursor) else {
            return false;
        };
        let Some(size) = read_u16(stream, cursor + 2).map(usize::from) else {
            return false;
        };
        let Some(next) = cursor
            .checked_add(4)
            .and_then(|start| start.checked_add(size))
        else {
            return false;
        };
        if next > stream.len() {
            return false;
        }
        if kind == BIFF_FILEPASS_RECORD {
            return true;
        }
        cursor = next;
        records += 1;
    }
    false
}

fn push_legacy_office_stream_text(
    kind: LegacyOfficeKind,
    stream: &[u8],
    max_text_bytes: usize,
    out: &mut String,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    if kind == LegacyOfficeKind::Xls {
        if push_workbook_biff_text_records(stream, max_text_bytes, out, &mut check_control)? {
            return Ok(());
        }
        check_control()?;
    }
    push_salvaged_legacy_text(stream, max_text_bytes, out, &mut check_control)
}

fn push_workbook_biff_text_records(
    stream: &[u8],
    max_text_bytes: usize,
    out: &mut String,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<bool> {
    const BIFF_LABEL_RECORD: u16 = 0x0204;
    const BIFF_SST_RECORD: u16 = 0x00fc;

    let mut cursor = 0usize;
    let mut records = 0usize;
    let initial_len = out.len();
    while cursor + 4 <= stream.len() && records < MAX_BIFF_TEXT_RECORDS_TO_SCAN {
        check_control()?;
        let Some(record_type) = read_u16(stream, cursor) else {
            break;
        };
        let Some(size) = read_u16(stream, cursor + 2).map(usize::from) else {
            break;
        };
        let Some(data_start) = cursor.checked_add(4) else {
            break;
        };
        let Some(next) = data_start.checked_add(size) else {
            break;
        };
        if next > stream.len() {
            break;
        }
        let data = &stream[data_start..next];
        match record_type {
            BIFF_LABEL_RECORD => push_biff_label_record_text(data, max_text_bytes, out),
            BIFF_SST_RECORD => {
                push_biff_sst_record_text(data, max_text_bytes, out, &mut check_control)?
            }
            _ => {}
        }
        if out.len() >= max_text_bytes {
            return Ok(out.len() > initial_len);
        }
        cursor = next;
        records += 1;
    }
    Ok(out.len() > initial_len)
}

fn push_biff_label_record_text(data: &[u8], max_text_bytes: usize, out: &mut String) {
    if data.len() < 8 {
        return;
    }
    let string = parse_biff_string(data, 6);
    append_biff_text(string.as_deref(), max_text_bytes, out);
}

fn push_biff_sst_record_text(
    data: &[u8],
    max_text_bytes: usize,
    out: &mut String,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    if data.len() < 8 {
        return Ok(());
    }
    let declared_unique = read_u32(data, 4).unwrap_or_default() as usize;
    let mut cursor = 8usize;
    for _ in 0..declared_unique.min(MAX_BIFF_TEXT_RECORDS_TO_SCAN) {
        check_control()?;
        let Some((string, next)) = parse_biff_string_at(data, cursor) else {
            break;
        };
        append_biff_text(Some(&string), max_text_bytes, out);
        if out.len() >= max_text_bytes {
            break;
        }
        cursor = next;
    }
    Ok(())
}

fn parse_biff_string(data: &[u8], offset: usize) -> Option<String> {
    parse_biff_string_at(data, offset).map(|(string, _)| string)
}

fn parse_biff_string_at(data: &[u8], offset: usize) -> Option<(String, usize)> {
    let char_count = read_u16(data, offset)? as usize;
    let flags_offset = offset.checked_add(2)?;
    let flags = *data.get(flags_offset)?;
    let mut cursor = flags_offset.checked_add(1)?;
    let has_16_bit_chars = flags & 0x01 != 0;
    let has_rich_text_runs = flags & 0x08 != 0;
    let has_extended_data = flags & 0x04 != 0;
    let rich_text_runs = if has_rich_text_runs {
        let value = read_u16(data, cursor)? as usize;
        cursor = cursor.checked_add(2)?;
        value
    } else {
        0
    };
    let extended_data_bytes = if has_extended_data {
        let value = read_u32(data, cursor)? as usize;
        cursor = cursor.checked_add(4)?;
        value
    } else {
        0
    };
    let string_bytes = if has_16_bit_chars {
        char_count.checked_mul(2)?
    } else {
        char_count
    };
    let string_end = cursor.checked_add(string_bytes)?;
    let raw = data.get(cursor..string_end)?;
    let string = if has_16_bit_chars {
        decode_utf16le_lossy(raw)
    } else {
        decode_biff_compressed_string(raw)
    };
    cursor = string_end;
    cursor = cursor.checked_add(rich_text_runs.checked_mul(4)?)?;
    cursor = cursor.checked_add(extended_data_bytes)?;
    if cursor > data.len() {
        return None;
    }
    Some((string, cursor))
}

fn decode_biff_compressed_string(raw: &[u8]) -> String {
    raw.iter()
        .filter_map(|&byte| {
            let ch = char::from(byte);
            is_text_char(ch).then_some(ch)
        })
        .collect()
}

fn decode_utf16le_lossy(raw: &[u8]) -> String {
    let utf16 = raw
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect::<Vec<_>>();
    String::from_utf16_lossy(&utf16)
}

fn append_biff_text(value: Option<&str>, max_text_bytes: usize, out: &mut String) {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    append_with_limit(out, value, max_text_bytes);
    append_with_limit(out, " ", max_text_bytes);
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
    mini_fat: Vec<u32>,
    mini_stream: Vec<u8>,
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
        let first_mini_fat_sector = read_u32(bytes, 60).unwrap_or(FREESECT);
        let mini_fat_sector_count = read_u32(bytes, 64).unwrap_or_default() as usize;
        let first_difat_sector = read_u32(bytes, 68).unwrap_or(FREESECT);
        let difat_sector_count = read_u32(bytes, 72).unwrap_or_default() as usize;
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
            append_difat_chain_checked(
                OleDifatChain {
                    bytes,
                    sector_bytes,
                    sector_count,
                    first_sector: first_difat_sector,
                    sector_count_hint: difat_sector_count,
                    fat_sector_count,
                },
                &mut difat,
                &mut check_control,
            )?;
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

        let (mini_fat, mini_stream) = read_mini_fat_and_stream_checked(
            bytes,
            sector_bytes,
            &fat,
            &entries,
            first_mini_fat_sector,
            mini_fat_sector_count,
            &mut check_control,
        )?;

        Ok(Some(Self {
            sector_bytes,
            fat,
            mini_fat,
            mini_stream,
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

        if entry.stream_size < self.mini_stream_cutoff {
            if let Some(stream) = self.read_mini_stream_checked(entry, &mut check_control)? {
                return Ok(Some(stream));
            }
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

        Ok(Some(stream))
    }

    fn read_mini_stream_checked(
        &self,
        entry: &OleDirectoryEntry,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Option<Vec<u8>>> {
        check_control()?;
        if self.mini_fat.is_empty() || self.mini_stream.is_empty() {
            return Ok(None);
        }

        let mut seen = HashSet::new();
        let mut sector = entry.start_sector;
        let mut stream = Vec::with_capacity(entry.stream_size.min(self.mini_stream.len()));
        while !matches!(sector, ENDOFCHAIN | FREESECT) {
            check_control()?;
            let sector_index = sector as usize;
            if sector_index >= self.mini_fat.len() || !seen.insert(sector) {
                return Ok(None);
            }
            let offset = sector_index.checked_mul(MINI_SECTOR_BYTES).ok_or_else(|| {
                gfm_types::GfmError::Format("OLE mini sector overflow".to_string())
            })?;
            let end = offset.checked_add(MINI_SECTOR_BYTES).ok_or_else(|| {
                gfm_types::GfmError::Format("OLE mini sector overflow".to_string())
            })?;
            let Some(bytes) = self.mini_stream.get(offset..end) else {
                return Ok(None);
            };
            stream.extend_from_slice(bytes);
            if stream.len() >= entry.stream_size {
                break;
            }
            sector = self.mini_fat[sector_index];
            if matches!(sector, FATSECT) {
                return Ok(None);
            }
        }
        if stream.len() < entry.stream_size {
            return Ok(None);
        }
        stream.truncate(entry.stream_size);
        Ok(Some(stream))
    }
}

struct OleDifatChain<'a> {
    bytes: &'a [u8],
    sector_bytes: usize,
    sector_count: usize,
    first_sector: u32,
    sector_count_hint: usize,
    fat_sector_count: usize,
}

fn append_difat_chain_checked(
    chain: OleDifatChain<'_>,
    difat: &mut Vec<u32>,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    if difat.len() >= chain.fat_sector_count
        || chain.sector_count_hint == 0
        || matches!(chain.first_sector, FREESECT | ENDOFCHAIN)
    {
        return Ok(());
    }

    let entries_per_sector = chain.sector_bytes / 4;
    if entries_per_sector < 2 {
        return Ok(());
    }
    let mut seen = HashSet::new();
    let mut sector = chain.first_sector;
    for _ in 0..chain.sector_count_hint {
        check_control()?;
        let sector_index = sector as usize;
        if sector_index >= chain.sector_count || !seen.insert(sector) {
            return Ok(());
        }
        let Some(difat_sector) = sector_bytes_for(chain.bytes, chain.sector_bytes, sector) else {
            return Ok(());
        };
        for offset in (0..chain.sector_bytes - 4).step_by(4) {
            check_control()?;
            let Some(fat_sector) = read_u32(difat_sector, offset) else {
                return Ok(());
            };
            if fat_sector == FREESECT {
                continue;
            }
            difat.push(fat_sector);
            if difat.len() == chain.fat_sector_count {
                return Ok(());
            }
        }
        let Some(next) = read_u32(difat_sector, chain.sector_bytes - 4) else {
            return Ok(());
        };
        if matches!(next, FREESECT | ENDOFCHAIN) {
            return Ok(());
        }
        sector = next;
    }
    Ok(())
}

fn read_mini_fat_and_stream_checked(
    bytes: &[u8],
    sector_bytes: usize,
    fat: &[u32],
    entries: &[OleDirectoryEntry],
    first_mini_fat_sector: u32,
    mini_fat_sector_count: usize,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<(Vec<u32>, Vec<u8>)> {
    check_control()?;
    if mini_fat_sector_count == 0 || matches!(first_mini_fat_sector, FREESECT | ENDOFCHAIN) {
        return Ok((Vec::new(), Vec::new()));
    }
    let Some(mut mini_fat_bytes) = read_sector_chain_checked(
        bytes,
        sector_bytes,
        fat,
        first_mini_fat_sector,
        &mut check_control,
    )?
    else {
        return Ok((Vec::new(), Vec::new()));
    };
    let expected_mini_fat_bytes = mini_fat_sector_count.saturating_mul(sector_bytes);
    if mini_fat_bytes.len() < expected_mini_fat_bytes {
        return Ok((Vec::new(), Vec::new()));
    }
    mini_fat_bytes.truncate(expected_mini_fat_bytes);
    let mini_fat = mini_fat_bytes
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect::<Vec<_>>();
    let Some(root) = entries.iter().find(|entry| entry.object_type == 5) else {
        return Ok((Vec::new(), Vec::new()));
    };
    if root.stream_size == 0 || matches!(root.start_sector, FREESECT | ENDOFCHAIN | FATSECT) {
        return Ok((mini_fat, Vec::new()));
    }
    let Some(mut mini_stream) = read_sector_chain_checked(
        bytes,
        sector_bytes,
        fat,
        root.start_sector,
        &mut check_control,
    )?
    else {
        return Ok((mini_fat, Vec::new()));
    };
    if mini_stream.len() < root.stream_size {
        return Ok((mini_fat, Vec::new()));
    }
    mini_stream.truncate(root.stream_size);
    Ok((mini_fat, mini_stream))
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
    fn detects_protected_word_document_fib_flags() {
        let mut payload = vec![0_u8; 16];
        payload[0x00..0x02].copy_from_slice(&0xa5ec_u16.to_le_bytes());
        payload[0x0a..0x0c].copy_from_slice(&(1_u16 << 8).to_le_bytes());
        let bytes = legacy_office_compound_file_with_ministream("WordDocument", &payload);

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
    fn detects_obfuscated_word_document_fib_flags() {
        let mut payload = vec![0_u8; 16];
        payload[0x00..0x02].copy_from_slice(&0xa5ec_u16.to_le_bytes());
        payload[0x0a..0x0c].copy_from_slice(&(1_u16 << 15).to_le_bytes());
        let bytes = legacy_office_compound_file_with_ministream("WordDocument", &payload);

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
    fn detects_workbook_filepass_records() {
        let payload = [
            0x09, 0x08, 0x00, 0x00, // BOF with no body in this minimal stream.
            0x2f, 0x00, 0x00, 0x00, // FILEPASS with no body.
        ];
        let bytes = legacy_office_compound_file_with_ministream("Workbook", &payload);

        let (status, document) = extract_legacy_office_document_checked(
            &bytes,
            LegacyOfficeKind::Xls,
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

    #[test]
    fn extracts_workbook_biff_label_records_before_raw_salvage() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&biff_label_record(0, 0, "Q4", false));
        payload.extend_from_slice(&biff_label_record(0, 1, "OK", false));
        let bytes = legacy_office_compound_file_with_ministream("Workbook", &payload);

        let (status, document) = extract_legacy_office_document_checked(
            &bytes,
            LegacyOfficeKind::Xls,
            &ExtractionPolicy::default(),
            || Ok(()),
        )
        .unwrap();

        assert_eq!(status, LegacyOfficeExtractStatus::Extracted);
        let document = document.expect("BIFF labels should extract");
        assert_eq!(document.text, "Q4 OK");
    }

    #[test]
    fn extracts_workbook_biff_sst_strings() {
        let mut payload = Vec::new();
        let mut sst = Vec::new();
        sst.extend_from_slice(&2_u32.to_le_bytes());
        sst.extend_from_slice(&2_u32.to_le_bytes());
        sst.extend_from_slice(&biff_string("A1", false));
        sst.extend_from_slice(&biff_string("Ω2", true));
        payload.extend_from_slice(&biff_record(0x00fc, &sst));
        let bytes = legacy_office_compound_file_with_ministream("Workbook", &payload);

        let (status, document) = extract_legacy_office_document_checked(
            &bytes,
            LegacyOfficeKind::Xls,
            &ExtractionPolicy::default(),
            || Ok(()),
        )
        .unwrap();

        assert_eq!(status, LegacyOfficeExtractStatus::Extracted);
        let document = document.expect("BIFF shared strings should extract");
        assert_eq!(document.text, "A1 Ω2");
    }

    #[test]
    fn workbook_biff_text_record_scan_honors_cancellation() {
        let payload = biff_label_record(0, 0, "Q4", false);
        let bytes = legacy_office_compound_file_with_ministream("Workbook", &payload);
        let mut checks = 0usize;

        let err = extract_legacy_office_document_checked(
            &bytes,
            LegacyOfficeKind::Xls,
            &ExtractionPolicy::default(),
            || {
                checks += 1;
                if checks >= 8 {
                    Err(gfm_types::GfmError::Cancelled)
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();

        assert_eq!(err, gfm_types::GfmError::Cancelled);
    }

    #[test]
    fn extracts_small_legacy_office_streams_from_ministream() {
        let bytes = legacy_office_compound_file_with_ministream(
            "WordDocument",
            b"ministreamneedle launch plan",
        );

        let (status, document) = extract_legacy_office_document_checked(
            &bytes,
            LegacyOfficeKind::Doc,
            &ExtractionPolicy::default(),
            || Ok(()),
        )
        .unwrap();

        assert_eq!(status, LegacyOfficeExtractStatus::Extracted);
        let document = document.expect("ministream WordDocument text should extract");
        assert!(document.text.contains("ministreamneedle launch plan"));
    }

    #[test]
    fn reads_legacy_office_fat_sectors_from_difat_chain() {
        let bytes =
            legacy_office_compound_file_with_difat("WordDocument", b"difatneedle launch plan");

        let (status, document) = extract_legacy_office_document_checked(
            &bytes,
            LegacyOfficeKind::Doc,
            &ExtractionPolicy::default(),
            || Ok(()),
        )
        .unwrap();

        assert_eq!(status, LegacyOfficeExtractStatus::Extracted);
        let document = document.expect("DIFAT-backed WordDocument text should extract");
        assert!(document.text.contains("difatneedle launch plan"));
    }

    fn biff_record(record_type: u16, data: &[u8]) -> Vec<u8> {
        let mut record = Vec::new();
        record.extend_from_slice(&record_type.to_le_bytes());
        record.extend_from_slice(&(data.len() as u16).to_le_bytes());
        record.extend_from_slice(data);
        record
    }

    fn biff_label_record(row: u16, column: u16, text: &str, wide: bool) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&row.to_le_bytes());
        data.extend_from_slice(&column.to_le_bytes());
        data.extend_from_slice(&0_u16.to_le_bytes());
        data.extend_from_slice(&biff_string(text, wide));
        biff_record(0x0204, &data)
    }

    fn biff_string(text: &str, wide: bool) -> Vec<u8> {
        let char_count = text.chars().count();
        let mut data = Vec::new();
        data.extend_from_slice(&(char_count as u16).to_le_bytes());
        data.push(if wide { 0x01 } else { 0x00 });
        if wide {
            for unit in text.encode_utf16() {
                data.extend_from_slice(&unit.to_le_bytes());
            }
        } else {
            for ch in text.chars() {
                data.push(ch as u8);
            }
        }
        data
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

    fn legacy_office_compound_file_with_ministream(stream_name: &str, payload: &[u8]) -> Vec<u8> {
        assert!(payload.len() <= MINI_SECTOR_BYTES);
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
        header[60..64].copy_from_slice(&2_u32.to_le_bytes());
        header[64..68].copy_from_slice(&1_u32.to_le_bytes());
        header[68..72].copy_from_slice(&ENDOFCHAIN.to_le_bytes());
        header[76..80].copy_from_slice(&0_u32.to_le_bytes());
        for offset in (80..HEADER_BYTES).step_by(4) {
            header[offset..offset + 4].copy_from_slice(&FREESECT.to_le_bytes());
        }

        let mut fat = vec![0xff_u8; HEADER_BYTES];
        write_fat_entry(&mut fat, 0, FATSECT);
        write_fat_entry(&mut fat, 1, ENDOFCHAIN);
        write_fat_entry(&mut fat, 2, ENDOFCHAIN);
        write_fat_entry(&mut fat, 3, ENDOFCHAIN);

        let mut directory = vec![0_u8; HEADER_BYTES];
        write_directory_entry_with_stream(
            &mut directory[0..DIRECTORY_ENTRY_BYTES],
            "Root Entry",
            5,
            3,
            MINI_SECTOR_BYTES,
        );
        write_directory_entry_with_stream(
            &mut directory[DIRECTORY_ENTRY_BYTES..DIRECTORY_ENTRY_BYTES * 2],
            stream_name,
            2,
            0,
            payload.len(),
        );

        let mut mini_fat = vec![0xff_u8; HEADER_BYTES];
        write_fat_entry(&mut mini_fat, 0, ENDOFCHAIN);

        let mut mini_stream = vec![0_u8; HEADER_BYTES];
        mini_stream[..payload.len()].copy_from_slice(payload);

        [header, fat, directory, mini_fat, mini_stream].concat()
    }

    fn legacy_office_compound_file_with_difat(stream_name: &str, payload: &[u8]) -> Vec<u8> {
        let mut header = vec![0_u8; HEADER_BYTES];
        header[..8].copy_from_slice(OLE_COMPOUND_FILE_MAGIC);
        header[24..26].copy_from_slice(&0x003e_u16.to_le_bytes());
        header[26..28].copy_from_slice(&0x0003_u16.to_le_bytes());
        header[28..30].copy_from_slice(&0xfffe_u16.to_le_bytes());
        header[30..32].copy_from_slice(&9_u16.to_le_bytes());
        header[32..34].copy_from_slice(&6_u16.to_le_bytes());
        header[44..48].copy_from_slice(&1_u32.to_le_bytes());
        header[48..52].copy_from_slice(&2_u32.to_le_bytes());
        header[56..60].copy_from_slice(&4096_u32.to_le_bytes());
        header[60..64].copy_from_slice(&ENDOFCHAIN.to_le_bytes());
        header[68..72].copy_from_slice(&0_u32.to_le_bytes());
        header[72..76].copy_from_slice(&1_u32.to_le_bytes());
        for offset in (76..HEADER_BYTES).step_by(4) {
            header[offset..offset + 4].copy_from_slice(&FREESECT.to_le_bytes());
        }

        let mut difat = vec![0xff_u8; HEADER_BYTES];
        difat[0..4].copy_from_slice(&1_u32.to_le_bytes());
        for offset in (4..HEADER_BYTES - 4).step_by(4) {
            difat[offset..offset + 4].copy_from_slice(&FREESECT.to_le_bytes());
        }
        difat[HEADER_BYTES - 4..HEADER_BYTES].copy_from_slice(&ENDOFCHAIN.to_le_bytes());

        let mut fat = vec![0xff_u8; HEADER_BYTES];
        write_fat_entry(&mut fat, 0, DIFSECT);
        write_fat_entry(&mut fat, 1, FATSECT);
        write_fat_entry(&mut fat, 2, ENDOFCHAIN);
        write_fat_entry(&mut fat, 3, ENDOFCHAIN);

        let mut directory = vec![0_u8; HEADER_BYTES];
        write_directory_entry(&mut directory[0..DIRECTORY_ENTRY_BYTES], "Root Entry", 5);
        write_directory_entry_with_stream(
            &mut directory[DIRECTORY_ENTRY_BYTES..DIRECTORY_ENTRY_BYTES * 2],
            stream_name,
            2,
            3,
            payload.len(),
        );

        let mut stream = vec![0_u8; HEADER_BYTES];
        stream[..payload.len().min(HEADER_BYTES)]
            .copy_from_slice(&payload[..payload.len().min(HEADER_BYTES)]);

        [header, difat, fat, directory, stream].concat()
    }

    fn write_fat_entry(fat: &mut [u8], index: usize, value: u32) {
        let offset = index * 4;
        fat[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_directory_entry(entry: &mut [u8], name: &str, object_type: u8) {
        write_directory_entry_with_stream(entry, name, object_type, ENDOFCHAIN, 0);
    }

    fn write_directory_entry_with_stream(
        entry: &mut [u8],
        name: &str,
        object_type: u8,
        start_sector: u32,
        stream_size: usize,
    ) {
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
        entry[116..120].copy_from_slice(&start_sector.to_le_bytes());
        entry[120..124].copy_from_slice(&(stream_size as u32).to_le_bytes());
    }
}
