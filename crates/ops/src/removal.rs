use crate::progress::ProgressTracker;
use crate::target::ensure_source_exists;
use crate::trashmeta::{
    append_trash_metadata, reconcile_empty_trash_metadata, remove_trash_metadata,
};
use crate::OperationProgressEvent;
use gfm_types::{GfmError, Result};
use std::fs;
use std::path::Path;

pub(crate) fn delete_path(
    path: &Path,
    metadata_path: Option<&Path>,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    let metadata = fs::symlink_metadata(path).map_err(|err| GfmError::io(path, err))?;
    if metadata.is_dir() {
        fs::remove_dir_all(path).map_err(|err| GfmError::io(path, err))?;
        remove_deleted_trash_metadata(metadata_path, path)?;
        progress.complete()
    } else {
        fs::remove_file(path).map_err(|err| GfmError::io(path, err))?;
        remove_deleted_trash_metadata(metadata_path, path)?;
        progress.advance(&metadata)
    }
}

pub(crate) fn delete_path_untracked(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|err| GfmError::io(path, err))?;
    if metadata.is_dir() {
        fs::remove_dir_all(path).map_err(|err| GfmError::io(path, err))
    } else {
        fs::remove_file(path).map_err(|err| GfmError::io(path, err))
    }
}

pub(crate) fn trash_path(
    path: &Path,
    metadata_path: Option<&Path>,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    ensure_source_exists(path)?;
    if let Some(metadata_path) = metadata_path {
        append_trash_metadata(metadata_path, path)?;
    }
    trash::delete(path).map_err(|err| GfmError::io(path, err))?;
    progress.complete()
}

pub(crate) fn empty_trash_path(
    path: &Path,
    metadata_path: Option<&Path>,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    let metadata = fs::symlink_metadata(path).map_err(|err| GfmError::io(path, err))?;
    if !metadata.is_dir() {
        return Err(GfmError::Format(format!(
            "empty trash requires a directory: {}",
            path.display()
        )));
    }
    let mut entries = fs::read_dir(path)
        .map_err(|err| GfmError::io(path, err))?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|err| GfmError::io(path, err))
        })
        .collect::<Result<Vec<_>>>()?;
    entries.sort();
    for entry in entries {
        delete_trash_child(&entry, metadata_path, progress)?;
    }
    reconcile_empty_trash_metadata(metadata_path, path)?;
    progress.complete()
}

fn delete_trash_child(
    path: &Path,
    metadata_path: Option<&Path>,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    let metadata = fs::symlink_metadata(path).map_err(|err| GfmError::io(path, err))?;
    delete_path_untracked(path)?;
    remove_deleted_trash_metadata(metadata_path, path)?;
    if metadata.is_dir() {
        progress.finish_current_item()
    } else {
        progress.advance(&metadata)
    }
}

pub(crate) fn remove_deleted_trash_metadata(
    metadata_path: Option<&Path>,
    deleted_path: &Path,
) -> Result<()> {
    if let Some(metadata_path) = metadata_path {
        remove_trash_metadata(metadata_path, deleted_path)?;
    }
    Ok(())
}
