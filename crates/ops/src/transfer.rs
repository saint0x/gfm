use crate::progress::ProgressTracker;
#[cfg(test)]
use crate::volume::COPY_BUFFER_BYTES;
use crate::{OperationProgressEvent, OperationVolumeCopyPolicy};
use gfm_types::{GfmError, Result};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

#[cfg(test)]
pub(crate) fn copy_file_bytes(from: &Path, to: &Path) -> Result<u64> {
    let mut source = File::open(from).map_err(|err| GfmError::io(from, err))?;
    let source_metadata = source.metadata().map_err(|err| GfmError::io(from, err))?;
    let preserve_sparse_holes = metadata_has_sparse_holes(&source_metadata);
    let mut destination = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)
        .map_err(|err| GfmError::io(to, err))?;
    let mut buffer = vec![0; COPY_BUFFER_BYTES];
    let mut written = 0_u64;

    let result = loop {
        let read = match source.read(&mut buffer) {
            Ok(read) => read,
            Err(err) => break Err(GfmError::io(from, err)),
        };
        if read == 0 {
            break Ok(written);
        }
        if let Err(err) = write_copy_chunk(&mut destination, &buffer[..read], preserve_sparse_holes)
        {
            break Err(GfmError::io(to, err));
        }
        written += read as u64;
    };

    let result = result.and_then(|written| {
        destination
            .set_len(written)
            .map_err(|err| GfmError::io(to, err))?;
        Ok(written)
    });

    if result.is_err() {
        let _ = fs::remove_file(to);
    }
    result
}

pub(crate) fn copy_file_bytes_tracked(
    from: &Path,
    to: &Path,
    volume_copy_policy: &OperationVolumeCopyPolicy,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<u64> {
    let mut source = File::open(from).map_err(|err| GfmError::io(from, err))?;
    let source_metadata = source.metadata().map_err(|err| GfmError::io(from, err))?;
    let preserve_sparse_holes =
        preserve_sparse_holes_for_copy(&source_metadata, volume_copy_policy, to);
    let mut destination = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)
        .map_err(|err| GfmError::io(to, err))?;
    let mut buffer = vec![0; volume_copy_policy.copy_buffer_bytes_for_paths(from, to)];
    let mut written = 0_u64;

    let result = loop {
        progress.check_cancelled()?;
        let read = match source.read(&mut buffer) {
            Ok(read) => read,
            Err(err) => break Err(GfmError::io(from, err)),
        };
        if read == 0 {
            break Ok(written);
        }
        if let Err(err) = write_copy_chunk(&mut destination, &buffer[..read], preserve_sparse_holes)
        {
            break Err(GfmError::io(to, err));
        }
        written += read as u64;
        if let Err(err) = progress.advance_bytes(read as u64) {
            break Err(err);
        }
    };

    let result = result.and_then(|written| {
        destination
            .set_len(written)
            .map_err(|err| GfmError::io(to, err))?;
        Ok(written)
    });

    if result.is_err() {
        let _ = fs::remove_file(to);
    }
    result
}

fn write_copy_chunk(
    destination: &mut File,
    chunk: &[u8],
    preserve_sparse_holes: bool,
) -> io::Result<()> {
    if preserve_sparse_holes {
        write_sparse_chunk(destination, chunk)
    } else {
        destination.write_all(chunk)
    }
}

fn write_sparse_chunk(destination: &mut File, chunk: &[u8]) -> io::Result<()> {
    let mut cursor = 0;
    while cursor < chunk.len() {
        let run_start = cursor;
        if chunk[cursor] == 0 {
            while cursor < chunk.len() && chunk[cursor] == 0 {
                cursor += 1;
            }
            destination.seek(SeekFrom::Current((cursor - run_start) as i64))?;
        } else {
            while cursor < chunk.len() && chunk[cursor] != 0 {
                cursor += 1;
            }
            destination.write_all(&chunk[run_start..cursor])?;
        }
    }
    Ok(())
}

pub(crate) fn metadata_has_sparse_holes(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        metadata.len() > 0 && metadata.blocks().saturating_mul(512) < metadata.len()
    }

    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}

fn preserve_sparse_holes_for_copy(
    source_metadata: &fs::Metadata,
    volume_copy_policy: &OperationVolumeCopyPolicy,
    destination: &Path,
) -> bool {
    metadata_has_sparse_holes(source_metadata)
        && volume_copy_policy.sparse_files_supported_for_path(destination)
}

#[cfg(target_os = "macos")]
pub(crate) fn clone_file(from: &Path, to: &Path) -> io::Result<()> {
    let source = File::open(from)?;
    rustix::fs::fclonefileat(
        &source,
        rustix::fs::CWD,
        to,
        rustix::fs::CloneFlags::empty(),
    )
    .map_err(io::Error::from)
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn clone_file(_from: &Path, _to: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "native clonefile is only available on macOS",
    ))
}

pub(crate) fn clone_fallback_allowed(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(libc::ENOTSUP) | Some(libc::EXDEV) | Some(libc::EINVAL)
    ) || matches!(
        err.kind(),
        io::ErrorKind::Unsupported | io::ErrorKind::InvalidInput
    )
}

pub(crate) fn remove_failed_clone_destination(to: &Path) -> Result<()> {
    match fs::symlink_metadata(to) {
        Ok(metadata) if metadata.is_dir() => {
            fs::remove_dir_all(to).map_err(|err| GfmError::io(to, err))
        }
        Ok(_) => fs::remove_file(to).map_err(|err| GfmError::io(to, err)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(GfmError::io(to, err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OperationVolumeClass, OperationVolumeCopyPolicy};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn sparse_hole_preservation_respects_destination_volume_support() {
        let root = unique_temp_dir("gfm-ops-transfer-sparse-policy");
        let source = root.join("source.bin");
        let supported_destination = root.join("sparse").join("copy.bin");
        let unsupported_root = root.join("legacy");
        let unsupported_destination = unsupported_root.join("copy.bin");
        fs::create_dir_all(supported_destination.parent().unwrap()).unwrap();
        fs::create_dir_all(&unsupported_root).unwrap();

        let file = File::create(&source).unwrap();
        file.set_len(1024 * 1024).unwrap();
        let metadata = fs::metadata(&source).unwrap();
        assert!(metadata_has_sparse_holes(&metadata));
        let policy = OperationVolumeCopyPolicy::default()
            .with_root(&unsupported_root, OperationVolumeClass::External)
            .with_root_sparse_file_support(&unsupported_root, false);

        assert!(preserve_sparse_holes_for_copy(
            &metadata,
            &policy,
            &supported_destination
        ));
        assert!(!preserve_sparse_holes_for_copy(
            &metadata,
            &policy,
            &unsupported_destination
        ));

        fs::remove_dir_all(root).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
        let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{unique}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }
}
