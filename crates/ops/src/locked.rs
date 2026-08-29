use gfm_types::{GfmError, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) fn ensure_unlocked_path(path: &Path, action: &'static str) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|err| GfmError::io(path, err))?;
    ensure_metadata_unlocked(path, &metadata, action)
}

pub(crate) fn ensure_unlocked_tree(path: &Path, action: &'static str) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|err| GfmError::io(path, err))?;
    ensure_metadata_unlocked(path, &metadata, action)?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        for child in sorted_children(path)? {
            ensure_unlocked_tree(&child, action)?;
        }
    }
    Ok(())
}

fn sorted_children(path: &Path) -> Result<Vec<PathBuf>> {
    let mut children = fs::read_dir(path)
        .map_err(|err| GfmError::io(path, err))?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|err| GfmError::io(path, err))
        })
        .collect::<Result<Vec<_>>>()?;
    children.sort();
    Ok(children)
}

#[cfg(target_vendor = "apple")]
fn ensure_metadata_unlocked(
    path: &Path,
    metadata: &fs::Metadata,
    action: &'static str,
) -> Result<()> {
    use std::os::darwin::fs::MetadataExt;

    let flags = metadata.st_flags();
    let locked = flags & locked_flag_mask() != 0;
    if locked {
        return Err(GfmError::Conflict {
            path: path.to_path_buf(),
            message: format!(
                "{action} requires locked-item confirmation: flags={}",
                locked_flag_names(flags)
            ),
        });
    }
    Ok(())
}

#[cfg(not(target_vendor = "apple"))]
fn ensure_metadata_unlocked(
    _path: &Path,
    _metadata: &fs::Metadata,
    _action: &'static str,
) -> Result<()> {
    Ok(())
}

#[cfg(target_vendor = "apple")]
fn locked_flag_mask() -> u32 {
    libc::UF_IMMUTABLE | libc::SF_IMMUTABLE | libc::UF_APPEND | libc::SF_APPEND
}

#[cfg(target_vendor = "apple")]
fn locked_flag_names(flags: u32) -> String {
    let mut names = Vec::new();
    if flags & libc::UF_IMMUTABLE != 0 {
        names.push("user-immutable");
    }
    if flags & libc::SF_IMMUTABLE != 0 {
        names.push("system-immutable");
    }
    if flags & libc::UF_APPEND != 0 {
        names.push("user-append-only");
    }
    if flags & libc::SF_APPEND != 0 {
        names.push("system-append-only");
    }
    names.join(",")
}
