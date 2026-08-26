use crate::journal::now_nanos;
use gfm_types::{GfmError, Result};
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
pub(crate) fn metadata_same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
pub(crate) fn metadata_same_file(_left: &fs::Metadata, _right: &fs::Metadata) -> bool {
    false
}

pub(crate) fn replacement_destination_is_non_directory(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| !metadata.is_dir())
        .unwrap_or(false)
}

pub(crate) fn replacement_destination_is_directory(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
}

pub(crate) fn allocate_replace_stage_path(to: &Path) -> Result<PathBuf> {
    allocate_hidden_sibling_path(to, "gfm-replace")
}

pub(crate) fn allocate_replace_backup_path(to: &Path) -> Result<PathBuf> {
    allocate_hidden_sibling_path(to, "gfm-replaced")
}

fn allocate_hidden_sibling_path(to: &Path, label: &str) -> Result<PathBuf> {
    let parent = to.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    let file_name = to
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("copy");
    let nonce = now_nanos();
    for attempt in 0..100_u32 {
        let candidate = parent.join(format!(".{}.{}-{}-{}", file_name, label, nonce, attempt));
        if !path_exists_or_symlink(&candidate) {
            return Ok(candidate);
        }
    }
    Err(GfmError::Conflict {
        path: to.to_path_buf(),
        message: format!("could not allocate a safe {label} sibling path"),
    })
}

pub(crate) fn rename_replacing_file(from: &Path, to: &Path) -> Result<()> {
    fs::rename(from, to).map_err(|err| GfmError::io(to, err))
}

pub(crate) fn commit_staged_directory_replace(
    stage: &Path,
    to: &Path,
    backup: &Path,
    cleanup_backup: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    fs::rename(to, backup).map_err(|err| GfmError::io(to, err))?;
    match fs::rename(stage, to) {
        Ok(()) => {
            if let Err(cleanup_err) = cleanup_backup(backup) {
                return Err(GfmError::Format(format!(
                    "replaced {} but failed to remove previous destination backup {}: {}",
                    to.display(),
                    backup.display(),
                    cleanup_err
                )));
            }
            Ok(())
        }
        Err(replace_err) => {
            let restore_result = fs::rename(backup, to);
            if let Err(restore_err) = restore_result {
                return Err(GfmError::Format(format!(
                    "failed to install staged replacement {} -> {}: {}; also failed to restore previous destination {} -> {}: {}",
                    stage.display(),
                    to.display(),
                    replace_err,
                    backup.display(),
                    to.display(),
                    restore_err
                )));
            }
            Err(GfmError::io(to, replace_err))
        }
    }
}

pub(crate) fn source_is_regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        .unwrap_or(false)
}

pub(crate) fn source_is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
}

pub(crate) fn source_is_directory(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
}

pub(crate) fn same_canonical_path(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

pub(crate) fn keep_both_path(path: &Path) -> Result<PathBuf> {
    if !path_exists_or_symlink(path) {
        return Ok(path.to_path_buf());
    }
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .or_else(|| path.file_name().and_then(|name| name.to_str()))
        .ok_or_else(|| {
            GfmError::Format(format!(
                "could not derive keep-both destination name for {}",
                path.display()
            ))
        })?;
    let extension = path.extension().and_then(|extension| extension.to_str());
    for index in 1..=10_000 {
        let suffix = if index == 1 {
            " copy".to_string()
        } else {
            format!(" copy {index}")
        };
        let candidate_name = match extension {
            Some(extension) if path.file_stem().is_some() => {
                format!("{stem}{suffix}.{extension}")
            }
            _ => format!("{stem}{suffix}"),
        };
        let candidate = parent.join(candidate_name);
        if !path_exists_or_symlink(&candidate) {
            return Ok(candidate);
        }
    }
    Err(GfmError::Conflict {
        path: path.to_path_buf(),
        message: "could not allocate a keep-both destination name".to_string(),
    })
}

pub(crate) fn path_exists_or_symlink(path: &Path) -> bool {
    path.exists() || fs::symlink_metadata(path).is_ok()
}
