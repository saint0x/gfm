use crate::kind::LegacyOfficeKind;
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
    Unsupported,
    TooLarge,
    Encrypted,
    Corrupt,
}

pub(crate) fn extract_legacy_office_checked(
    bytes: &[u8],
    kind: LegacyOfficeKind,
    policy: &ExtractionPolicy,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<LegacyOfficeExtractStatus> {
    check_control()?;
    if bytes.len() as u64 > policy.max_office_bytes {
        return Ok(LegacyOfficeExtractStatus::TooLarge);
    }
    if !bytes.starts_with(OLE_COMPOUND_FILE_MAGIC) {
        return Ok(LegacyOfficeExtractStatus::Unsupported);
    }

    let Some(directory_names) = OleDirectory::parse_checked(bytes, &mut check_control)? else {
        return Ok(LegacyOfficeExtractStatus::Corrupt);
    };
    check_control()?;

    if directory_names
        .iter()
        .any(|name| encrypted_stream_name(name))
    {
        return Ok(LegacyOfficeExtractStatus::Encrypted);
    }
    if !directory_names
        .iter()
        .any(|name| required_legacy_office_stream(kind, name))
    {
        return Ok(LegacyOfficeExtractStatus::Corrupt);
    }

    Ok(LegacyOfficeExtractStatus::Unsupported)
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

struct OleDirectory;

impl OleDirectory {
    fn parse_checked(
        bytes: &[u8],
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Option<Vec<String>>> {
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
        let mut names = Vec::new();
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
                names.push(name);
            }
        }

        Ok(Some(names))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ExtractionPolicy;

    #[test]
    fn detects_encrypted_legacy_office_directory_entries() {
        let bytes = legacy_office_compound_file(&["WordDocument", "EncryptionInfo"]);

        let status = extract_legacy_office_checked(
            &bytes,
            LegacyOfficeKind::Doc,
            &ExtractionPolicy::default(),
            || Ok(()),
        )
        .unwrap();

        assert_eq!(status, LegacyOfficeExtractStatus::Encrypted);
    }

    #[test]
    fn corrupts_compound_files_without_required_office_streams() {
        let bytes = legacy_office_compound_file(&["NotOffice"]);

        let status = extract_legacy_office_checked(
            &bytes,
            LegacyOfficeKind::Doc,
            &ExtractionPolicy::default(),
            || Ok(()),
        )
        .unwrap();

        assert_eq!(status, LegacyOfficeExtractStatus::Corrupt);
    }

    #[test]
    fn keeps_well_formed_legacy_office_unsupported_until_binary_import_lands() {
        let bytes = legacy_office_compound_file(&["Workbook"]);

        let status = extract_legacy_office_checked(
            &bytes,
            LegacyOfficeKind::Xls,
            &ExtractionPolicy::default(),
            || Ok(()),
        )
        .unwrap();

        assert_eq!(status, LegacyOfficeExtractStatus::Unsupported);
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
