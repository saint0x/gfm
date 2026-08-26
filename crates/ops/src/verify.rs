use gfm_types::{GfmError, Result};
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationPolicy {
    None,
    Size,
    Bytes,
}

pub(crate) fn verify_copy(from: &Path, to: &Path, policy: VerificationPolicy) -> Result<()> {
    match policy {
        VerificationPolicy::None => Ok(()),
        VerificationPolicy::Size => verify_copy_size(from, to),
        VerificationPolicy::Bytes => {
            verify_copy_size(from, to)?;
            verify_copy_bytes(from, to)
        }
    }
}

fn verify_copy_size(from: &Path, to: &Path) -> Result<()> {
    let source_len = fs::metadata(from)
        .map_err(|err| GfmError::io(from, err))?
        .len();
    let destination_len = fs::metadata(to).map_err(|err| GfmError::io(to, err))?.len();
    if source_len == destination_len {
        Ok(())
    } else {
        Err(GfmError::Format(format!(
            "copy verification failed for {} -> {}: source size {} != destination size {}",
            from.display(),
            to.display(),
            source_len,
            destination_len
        )))
    }
}

fn verify_copy_bytes(from: &Path, to: &Path) -> Result<()> {
    const VERIFY_BUFFER_BYTES: usize = 128 * 1024;

    let mut source = File::open(from).map_err(|err| GfmError::io(from, err))?;
    let mut destination = File::open(to).map_err(|err| GfmError::io(to, err))?;
    let mut source_buffer = vec![0; VERIFY_BUFFER_BYTES];
    let mut destination_buffer = vec![0; VERIFY_BUFFER_BYTES];
    let mut offset = 0_u64;

    loop {
        let source_read = source
            .read(&mut source_buffer)
            .map_err(|err| GfmError::io(from, err))?;
        let destination_read = destination
            .read(&mut destination_buffer)
            .map_err(|err| GfmError::io(to, err))?;
        if source_read != destination_read {
            return Err(GfmError::Format(format!(
                "copy verification failed for {} -> {}: read length drift at byte {}",
                from.display(),
                to.display(),
                offset
            )));
        }
        if source_read == 0 {
            return Ok(());
        }
        if source_buffer[..source_read] != destination_buffer[..destination_read] {
            return Err(GfmError::Format(format!(
                "copy verification failed for {} -> {}: byte mismatch in block starting at {}",
                from.display(),
                to.display(),
                offset
            )));
        }
        offset += source_read as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "{}-{}-{}",
            prefix,
            std::process::id(),
            crate::journal::now_nanos()
        ))
    }

    #[test]
    fn size_verification_rejects_length_mismatch() {
        let root = unique_temp_dir("gfm-verify-size");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::create_dir_all(&root).unwrap();
        fs::write(&source, "source").unwrap();
        fs::write(&destination, "destination").unwrap();

        let err = verify_copy(&source, &destination, VerificationPolicy::Size).unwrap_err();

        assert!(matches!(err, GfmError::Format(message) if message.contains("source size")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn byte_verification_rejects_equal_size_mismatch() {
        let root = unique_temp_dir("gfm-verify-bytes");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::create_dir_all(&root).unwrap();
        fs::write(&source, "abcdef").unwrap();
        fs::write(&destination, "abcxef").unwrap();

        let err = verify_copy(&source, &destination, VerificationPolicy::Bytes).unwrap_err();

        assert!(matches!(err, GfmError::Format(message) if message.contains("byte mismatch")));
        fs::remove_dir_all(root).unwrap();
    }
}
