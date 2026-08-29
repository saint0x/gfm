use crate::copy::copy_path;
use crate::progress::ProgressTracker;
use crate::removal::delete_path_untracked;
use crate::target::{
    ensure_source_exists, metadata_same_file, path_exists_or_symlink, prepare_destination,
    replacement_destination_is_directory, replacement_destination_is_non_directory,
    same_canonical_path, source_is_directory, source_is_regular_file, source_is_symlink,
};
use crate::trashmeta::remove_trash_metadata;
use crate::{
    ConflictPolicy, OperationProgressEvent, OperationVolumeCopyPolicy, VerificationPolicy,
};
use gfm_types::{GfmError, Result};
use std::fs;
use std::io;
use std::path::Path;

pub(crate) fn move_path(
    from: &Path,
    to: &Path,
    conflict: ConflictPolicy,
    verification: VerificationPolicy,
    volume_copy_policy: &OperationVolumeCopyPolicy,
    resuming: bool,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    ensure_source_exists(from)?;
    crate::locked::ensure_unlocked_tree(from, "move")?;
    if resuming && path_exists_or_symlink(to)? {
        copy_path(
            from,
            to,
            ConflictPolicy::Replace,
            verification,
            volume_copy_policy,
            true,
            progress,
        )?;
        delete_path_untracked(from)?;
        return Ok(());
    }
    if conflict == ConflictPolicy::Merge && path_exists_or_symlink(to)? {
        copy_path(
            from,
            to,
            ConflictPolicy::Merge,
            verification,
            volume_copy_policy,
            false,
            progress,
        )?;
        delete_path_untracked(from)?;
        return Ok(());
    }
    if conflict == ConflictPolicy::Replace
        && source_is_regular_file(from)
        && replacement_destination_is_non_directory(to)
    {
        return move_file_replacing_existing(from, to, verification, volume_copy_policy, progress);
    }
    if conflict == ConflictPolicy::Replace
        && source_is_symlink(from)
        && replacement_destination_is_non_directory(to)
    {
        return move_symlink_replacing_existing(
            from,
            to,
            verification,
            volume_copy_policy,
            progress,
        );
    }
    if conflict == ConflictPolicy::Replace
        && source_is_directory(from)
        && replacement_destination_is_directory(to)
    {
        return move_directory_replacing_existing(
            from,
            to,
            verification,
            volume_copy_policy,
            progress,
        );
    }
    prepare_destination(to, conflict, delete_path_untracked)?;
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    if volume_copy_policy.paths_are_known_distinct_volumes(from, to) {
        return move_via_copy_then_delete(
            from,
            to,
            ConflictPolicy::Replace,
            verification,
            volume_copy_policy,
            progress,
            MoveFallbackError::KnownDistinctVolumes,
        );
    }
    match fs::rename(from, to) {
        Ok(()) => progress.complete(),
        Err(rename_err) => move_via_copy_then_delete(
            from,
            to,
            ConflictPolicy::Replace,
            verification,
            volume_copy_policy,
            progress,
            MoveFallbackError::Rename(rename_err),
        ),
    }
}

pub(crate) fn restore_path(
    from: &Path,
    to: &Path,
    conflict: ConflictPolicy,
    volume_copy_policy: &OperationVolumeCopyPolicy,
    metadata_path: Option<&Path>,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    move_path(
        from,
        to,
        conflict,
        VerificationPolicy::Bytes,
        volume_copy_policy,
        false,
        progress,
    )?;
    if let Some(metadata_path) = metadata_path {
        remove_trash_metadata(metadata_path, from)?;
    }
    Ok(())
}

fn move_file_replacing_existing(
    from: &Path,
    to: &Path,
    verification: VerificationPolicy,
    volume_copy_policy: &OperationVolumeCopyPolicy,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    crate::locked::ensure_unlocked_path(to, "replace")?;
    let source_metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
    let destination_metadata = fs::symlink_metadata(to).map_err(|err| GfmError::io(to, err))?;
    if metadata_same_file(&source_metadata, &destination_metadata) {
        if !same_canonical_path(from, to) {
            delete_path_untracked(from)?;
        }
        return progress.complete();
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    if volume_copy_policy.paths_are_known_distinct_volumes(from, to) {
        return move_via_copy_then_delete(
            from,
            to,
            ConflictPolicy::Replace,
            verification,
            volume_copy_policy,
            progress,
            MoveFallbackError::KnownDistinctVolumes,
        );
    }
    match fs::rename(from, to) {
        Ok(()) => progress.complete(),
        Err(rename_err) => move_via_copy_then_delete(
            from,
            to,
            ConflictPolicy::Replace,
            verification,
            volume_copy_policy,
            progress,
            MoveFallbackError::Rename(rename_err),
        ),
    }
}

fn move_directory_replacing_existing(
    from: &Path,
    to: &Path,
    verification: VerificationPolicy,
    volume_copy_policy: &OperationVolumeCopyPolicy,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    crate::locked::ensure_unlocked_tree(to, "replace")?;
    let source_metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
    let destination_metadata = fs::symlink_metadata(to).map_err(|err| GfmError::io(to, err))?;
    if metadata_same_file(&source_metadata, &destination_metadata) {
        return progress.complete();
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    if volume_copy_policy.paths_are_known_distinct_volumes(from, to) {
        return move_via_copy_then_delete(
            from,
            to,
            ConflictPolicy::Replace,
            verification,
            volume_copy_policy,
            progress,
            MoveFallbackError::KnownDistinctVolumes,
        );
    }
    match fs::rename(from, to) {
        Ok(()) => progress.complete(),
        Err(rename_err) => move_via_copy_then_delete(
            from,
            to,
            ConflictPolicy::Replace,
            verification,
            volume_copy_policy,
            progress,
            MoveFallbackError::Rename(rename_err),
        ),
    }
}

fn move_symlink_replacing_existing(
    from: &Path,
    to: &Path,
    verification: VerificationPolicy,
    volume_copy_policy: &OperationVolumeCopyPolicy,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    crate::locked::ensure_unlocked_path(to, "replace")?;
    let source_metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
    let destination_metadata = fs::symlink_metadata(to).map_err(|err| GfmError::io(to, err))?;
    if metadata_same_file(&source_metadata, &destination_metadata) {
        if from != to {
            delete_path_untracked(from)?;
        }
        return progress.complete();
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    if volume_copy_policy.paths_are_known_distinct_volumes(from, to) {
        return move_via_copy_then_delete(
            from,
            to,
            ConflictPolicy::Replace,
            verification,
            volume_copy_policy,
            progress,
            MoveFallbackError::KnownDistinctVolumes,
        );
    }
    match fs::rename(from, to) {
        Ok(()) => progress.complete(),
        Err(rename_err) => move_via_copy_then_delete(
            from,
            to,
            ConflictPolicy::Replace,
            verification,
            volume_copy_policy,
            progress,
            MoveFallbackError::Rename(rename_err),
        ),
    }
}

enum MoveFallbackError {
    KnownDistinctVolumes,
    Rename(io::Error),
}

fn move_via_copy_then_delete(
    from: &Path,
    to: &Path,
    conflict: ConflictPolicy,
    verification: VerificationPolicy,
    volume_copy_policy: &OperationVolumeCopyPolicy,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
    fallback: MoveFallbackError,
) -> Result<()> {
    crate::locked::ensure_unlocked_tree(from, "move")?;
    copy_path(
        from,
        to,
        conflict,
        verification,
        volume_copy_policy,
        false,
        progress,
    )?;
    delete_path_untracked(from).map_err(|delete_err| {
        let reason = match fallback {
            MoveFallbackError::KnownDistinctVolumes => {
                "known distinct volume identities".to_string()
            }
            MoveFallbackError::Rename(rename_err) => format!("original rename error: {rename_err}"),
        };
        GfmError::Format(format!(
            "moved copy to {} but failed to remove source {}: {}; {}",
            to.display(),
            from.display(),
            delete_err,
            reason
        ))
    })
}
