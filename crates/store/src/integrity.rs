use gfm_types::{GfmError, Result};
use std::io::Write;
use std::path::Path;

const CRC32_TABLE: [u32; 256] = make_crc32_table();

pub(crate) fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        let index = ((crc ^ u32::from(*byte)) & 0xff) as usize;
        crc = (crc >> 8) ^ CRC32_TABLE[index];
    }
    !crc
}

pub(crate) fn write_checksum_footer(
    mut writer: impl Write,
    bytes_before_footer: &[u8],
    footer_magic: &[u8],
) -> std::io::Result<()> {
    writer.write_all(&crc32(bytes_before_footer).to_le_bytes())?;
    writer.write_all(footer_magic)
}

pub(crate) fn verify_checksum_footer(
    bytes: &[u8],
    footer_magic: &[u8],
    path: &Path,
    store_name: &str,
) -> Result<bool> {
    let footer_len = 4usize
        .checked_add(footer_magic.len())
        .ok_or_else(|| integrity_error(path, store_name, "checksum footer length overflow"))?;
    if bytes.len() < footer_len {
        return Ok(false);
    }
    let footer_start = bytes.len() - footer_len;
    if bytes.get(footer_start + 4..) != Some(footer_magic) {
        return Ok(false);
    }
    let mut encoded = [0u8; 4];
    encoded.copy_from_slice(
        bytes
            .get(footer_start..footer_start + 4)
            .ok_or_else(|| integrity_error(path, store_name, "checksum footer truncated"))?,
    );
    let expected = u32::from_le_bytes(encoded);
    let actual = crc32(&bytes[..footer_start]);
    if actual != expected {
        return Err(integrity_error(
            path,
            store_name,
            "checksum mismatch; archive must be rebuilt",
        ));
    }
    Ok(true)
}

fn integrity_error(path: &Path, store_name: &str, reason: &str) -> GfmError {
    GfmError::Format(format!(
        "invalid {store_name} store {}: {reason}",
        path.display()
    ))
}

const fn make_crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut index = 0;
    while index < 256 {
        let mut crc = index as u32;
        let mut bit = 0;
        while bit < 8 {
            if crc & 1 == 1 {
                crc = 0xedb8_8320u32 ^ (crc >> 1);
            } else {
                crc >>= 1;
            }
            bit += 1;
        }
        table[index] = crc;
        index += 1;
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_standard_vector() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }
}
