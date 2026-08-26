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

#[cfg(test)]
pub(crate) fn verify_copy(from: &Path, to: &Path, policy: VerificationPolicy) -> Result<()> {
    verify_copy_checked(from, to, policy, || Ok(()))
}

pub(crate) fn verify_copy_checked(
    from: &Path,
    to: &Path,
    policy: VerificationPolicy,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    check_control()?;
    match policy {
        VerificationPolicy::None => Ok(()),
        VerificationPolicy::Size => verify_copy_size_checked(from, to, &mut check_control),
        VerificationPolicy::Bytes => {
            verify_copy_size_checked(from, to, &mut check_control)?;
            verify_copy_bytes_checked(from, to, &mut check_control)
        }
    }
}

fn verify_copy_size_checked(
    from: &Path,
    to: &Path,
    check_control: &mut impl FnMut() -> Result<()>,
) -> Result<()> {
    check_control()?;
    let source_len = fs::metadata(from)
        .map_err(|err| GfmError::io(from, err))?
        .len();
    check_control()?;
    let destination_len = fs::metadata(to).map_err(|err| GfmError::io(to, err))?.len();
    check_control()?;
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

fn verify_copy_bytes_checked(
    from: &Path,
    to: &Path,
    check_control: &mut impl FnMut() -> Result<()>,
) -> Result<()> {
    const VERIFY_BUFFER_BYTES: usize = 128 * 1024;

    check_control()?;
    let mut source = File::open(from).map_err(|err| GfmError::io(from, err))?;
    check_control()?;
    let mut destination = File::open(to).map_err(|err| GfmError::io(to, err))?;
    let mut source_buffer = vec![0; VERIFY_BUFFER_BYTES];
    let mut destination_buffer = vec![0; VERIFY_BUFFER_BYTES];
    let mut offset = 0_u64;

    loop {
        check_control()?;
        let source_read = source
            .read(&mut source_buffer)
            .map_err(|err| GfmError::io(from, err))?;
        check_control()?;
        let destination_read = destination
            .read(&mut destination_buffer)
            .map_err(|err| GfmError::io(to, err))?;
        check_control()?;
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

    #[test]
    fn checked_size_verification_honors_cancellation() {
        let root = unique_temp_dir("gfm-verify-size-cancel");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::create_dir_all(&root).unwrap();
        fs::write(&source, "same").unwrap();
        fs::write(&destination, "same").unwrap();

        let err = verify_copy_checked(&source, &destination, VerificationPolicy::Size, || {
            Err(GfmError::Cancelled)
        })
        .unwrap_err();

        assert!(matches!(err, GfmError::Cancelled));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn checked_byte_verification_honors_mid_stream_cancellation() {
        let root = unique_temp_dir("gfm-verify-bytes-cancel");
        let source = root.join("source.bin");
        let destination = root.join("destination.bin");
        fs::create_dir_all(&root).unwrap();
        let bytes = vec![17_u8; 384 * 1024];
        fs::write(&source, &bytes).unwrap();
        fs::write(&destination, &bytes).unwrap();
        let mut checks = 0_u32;

        let err = verify_copy_checked(&source, &destination, VerificationPolicy::Bytes, || {
            checks += 1;
            if checks > 9 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        })
        .unwrap_err();

        assert!(matches!(err, GfmError::Cancelled));
        fs::remove_dir_all(root).unwrap();
    }
}
