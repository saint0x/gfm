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
    let preserve_sparse_holes = metadata_has_sparse_holes(&source_metadata);
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
