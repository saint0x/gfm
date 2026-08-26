use crate::progress::ProgressTracker;
use crate::removal::delete_path_untracked;
use crate::target::{
    allocate_replace_backup_path, allocate_replace_stage_path, commit_staged_directory_replace,
    ensure_source_exists, metadata_same_file, path_exists_or_symlink, prepare_destination,
    rename_replacing_file, replacement_destination_is_directory,
    replacement_destination_is_non_directory,
};
#[cfg(test)]
use crate::transfer::copy_file_bytes;
use crate::transfer::{
    clone_fallback_allowed, clone_file, copy_file_bytes_tracked, remove_failed_clone_destination,
};
#[cfg(test)]
use crate::verify::verify_copy;
use crate::verify::verify_copy_checked;
use crate::{
    ConflictPolicy, OperationProgressEvent, OperationVolumeCopyPolicy, VerificationPolicy,
};
use gfm_fs::PackagePolicy;
use gfm_types::{FileKind, GfmError, Result};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyMethod {
    ApfsClone,
    ByteCopy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopyExistingMode {
    Fresh,
    Resume,
    Merge,
}

#[derive(Debug, Default)]
struct CopySession {
    hard_links: BTreeMap<FileIdentity, PathBuf>,
}

impl CopySession {
    fn copied_hard_link_destination(&self, metadata: &fs::Metadata) -> Option<&Path> {
        hard_link_identity(metadata)
            .and_then(|identity| self.hard_links.get(&identity))
            .map(PathBuf::as_path)
    }

    fn remember_hard_link_destination(&mut self, metadata: &fs::Metadata, destination: &Path) {
        if let Some(identity) = hard_link_identity(metadata) {
            self.hard_links
                .entry(identity)
                .or_insert_with(|| destination.to_path_buf());
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

#[derive(Debug, Clone, Copy)]
struct CopyExecution<'a> {
    verification: VerificationPolicy,
    volume_copy_policy: &'a OperationVolumeCopyPolicy,
}

#[cfg(unix)]
fn hard_link_identity(metadata: &fs::Metadata) -> Option<FileIdentity> {
    use std::os::unix::fs::MetadataExt;

    if metadata.is_file() && metadata.nlink() > 1 {
        Some(FileIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    } else {
        None
    }
}

#[cfg(not(unix))]
fn hard_link_identity(_metadata: &fs::Metadata) -> Option<FileIdentity> {
    None
}

pub(crate) fn copy_path(
    from: &Path,
    to: &Path,
    conflict: ConflictPolicy,
    verification: VerificationPolicy,
    volume_copy_policy: &OperationVolumeCopyPolicy,
    resuming: bool,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    let mut session = CopySession::default();
    let execution = CopyExecution {
        verification,
        volume_copy_policy,
    };
    copy_path_with_session(
        from,
        to,
        conflict,
        execution,
        resuming,
        progress,
        &mut session,
    )
}

fn copy_path_with_session(
    from: &Path,
    to: &Path,
    conflict: ConflictPolicy,
    execution: CopyExecution<'_>,
    resuming: bool,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
    session: &mut CopySession,
) -> Result<()> {
    progress.check_cancelled()?;
    ensure_source_exists(from)?;
    let metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
    if resuming && path_exists_or_symlink(to) {
        return copy_path_existing(
            from,
            to,
            &metadata,
            execution,
            CopyExistingMode::Resume,
            progress,
            session,
        );
    }
    if conflict == ConflictPolicy::Merge && metadata.is_dir() && path_exists_or_symlink(to) {
        return copy_path_existing(
            from,
            to,
            &metadata,
            execution,
            CopyExistingMode::Merge,
            progress,
            session,
        );
    }
    if conflict == ConflictPolicy::Replace
        && metadata.is_file()
        && !metadata.file_type().is_symlink()
        && replacement_destination_is_non_directory(to)
    {
        return copy_file_replacing_existing(from, to, execution, progress);
    }
    if conflict == ConflictPolicy::Replace
        && metadata.file_type().is_symlink()
        && replacement_destination_is_non_directory(to)
    {
        return copy_symlink_replacing_existing(from, to, progress);
    }
    if conflict == ConflictPolicy::Replace
        && metadata.is_dir()
        && replacement_destination_is_directory(to)
    {
        return copy_directory_replacing_existing(from, to, execution, progress, session);
    }
    prepare_destination(to, conflict, delete_path_untracked)?;
    if metadata.file_type().is_symlink() {
        copy_symlink(from, to, progress)
    } else if metadata.is_dir() {
        copy_directory(
            from,
            to,
            execution,
            CopyExistingMode::Fresh,
            progress,
            session,
        )
    } else {
        copy_file_with_session(from, to, &metadata, execution, progress, session)?;
        Ok(())
    }
}

fn copy_file_replacing_existing(
    from: &Path,
    to: &Path,
    execution: CopyExecution<'_>,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    let source_metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
    let destination_metadata = fs::symlink_metadata(to).map_err(|err| GfmError::io(to, err))?;
    if metadata_same_file(&source_metadata, &destination_metadata) {
        return progress.advance(&source_metadata);
    }
    let stage = allocate_replace_stage_path(to)?;
    let result = (|| {
        copy_file_tracked(
            from,
            &stage,
            execution.verification,
            execution.volume_copy_policy,
            progress,
        )?;
        rename_replacing_file(&stage, to)
    })();
    if result.is_err() && path_exists_or_symlink(&stage) {
        let _ = delete_path_untracked(&stage);
    }
    result
}

fn copy_directory_replacing_existing(
    from: &Path,
    to: &Path,
    execution: CopyExecution<'_>,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
    session: &mut CopySession,
) -> Result<()> {
    progress.check_cancelled()?;
    let source_metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
    let destination_metadata = fs::symlink_metadata(to).map_err(|err| GfmError::io(to, err))?;
    if metadata_same_file(&source_metadata, &destination_metadata) {
        return progress.complete();
    }
    let stage = allocate_replace_stage_path(to)?;
    let backup = allocate_replace_backup_path(to)?;
    let result = (|| {
        copy_directory(
            from,
            &stage,
            execution,
            CopyExistingMode::Fresh,
            progress,
            session,
        )?;
        commit_staged_directory_replace(&stage, to, &backup, delete_path_untracked)
    })();
    if result.is_err() && path_exists_or_symlink(&stage) {
        let _ = delete_path_untracked(&stage);
    }
    result
}

fn copy_symlink_replacing_existing(
    from: &Path,
    to: &Path,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    let source_metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
    let destination_metadata = fs::symlink_metadata(to).map_err(|err| GfmError::io(to, err))?;
    if metadata_same_file(&source_metadata, &destination_metadata) {
        return progress.advance(&source_metadata);
    }
    let stage = allocate_replace_stage_path(to)?;
    let result = (|| {
        copy_symlink(from, &stage, progress)?;
        rename_replacing_file(&stage, to)
    })();
    if result.is_err() && path_exists_or_symlink(&stage) {
        let _ = delete_path_untracked(&stage);
    }
    result
}

fn copy_directory(
    from: &Path,
    to: &Path,
    execution: CopyExecution<'_>,
    mode: CopyExistingMode,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
    session: &mut CopySession,
) -> Result<()> {
    progress.check_cancelled()?;
    let metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
    let rollback_incomplete_fresh_destination =
        mode == CopyExistingMode::Fresh && metadata.is_dir();
    let mut created_destination = false;
    let result = (|| {
        if mode != CopyExistingMode::Fresh && path_exists_or_symlink(to) {
            let destination_metadata =
                fs::symlink_metadata(to).map_err(|err| GfmError::io(to, err))?;
            if !destination_metadata.is_dir() {
                return Err(GfmError::Conflict {
                    path: to.to_path_buf(),
                    message: format!(
                        "{} destination exists but is not a directory",
                        copy_mode_label(mode)
                    ),
                });
            }
        } else {
            create_new_directory(to)?;
            created_destination = true;
        }
        progress.advance(&metadata)?;

        for entry in fs::read_dir(from).map_err(|err| GfmError::io(from, err))? {
            progress.check_cancelled()?;
            let entry = entry.map_err(|err| GfmError::io(from, err))?;
            let source = entry.path();
            let destination = to.join(entry.file_name());
            let child_metadata =
                fs::symlink_metadata(&source).map_err(|err| GfmError::io(&source, err))?;
            if child_metadata.file_type().is_symlink() {
                if mode == CopyExistingMode::Resume && path_exists_or_symlink(&destination) {
                    copy_path_existing(
                        &source,
                        &destination,
                        &child_metadata,
                        execution,
                        CopyExistingMode::Resume,
                        progress,
                        session,
                    )?;
                } else if mode == CopyExistingMode::Merge && path_exists_or_symlink(&destination) {
                    copy_path_existing(
                        &source,
                        &destination,
                        &child_metadata,
                        execution,
                        CopyExistingMode::Merge,
                        progress,
                        session,
                    )?;
                } else {
                    copy_symlink(&source, &destination, progress)?;
                }
            } else if child_metadata.is_dir() {
                copy_directory(&source, &destination, execution, mode, progress, session)?;
            } else if mode == CopyExistingMode::Resume && path_exists_or_symlink(&destination) {
                copy_path_existing(
                    &source,
                    &destination,
                    &child_metadata,
                    execution,
                    CopyExistingMode::Resume,
                    progress,
                    session,
                )?;
            } else if mode == CopyExistingMode::Merge && path_exists_or_symlink(&destination) {
                copy_path_existing(
                    &source,
                    &destination,
                    &child_metadata,
                    execution,
                    CopyExistingMode::Merge,
                    progress,
                    session,
                )?;
            } else {
                copy_file_with_session(
                    &source,
                    &destination,
                    &child_metadata,
                    execution,
                    progress,
                    session,
                )?;
            }
        }
        crate::preserve::preserve_metadata(from, to, &metadata)?;
        Ok(())
    })();
    if rollback_incomplete_fresh_destination
        && created_destination
        && result
            .as_ref()
            .is_err_and(|err| !matches!(err, GfmError::Paused))
    {
        let _ = delete_path_untracked(to);
    }
    result
}

fn copy_path_existing(
    from: &Path,
    to: &Path,
    metadata: &fs::Metadata,
    execution: CopyExecution<'_>,
    mode: CopyExistingMode,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
    session: &mut CopySession,
) -> Result<()> {
    progress.check_cancelled()?;
    if metadata.file_type().is_symlink() {
        verify_existing_symlink_copy(from, to, mode)?;
        progress.advance(metadata)
    } else if metadata.is_dir() {
        if mode == CopyExistingMode::Merge
            && (is_finder_package_dir(from, metadata) || is_existing_finder_package_dir(to))
        {
            return Err(GfmError::Conflict {
                path: to.to_path_buf(),
                message: "merge destination package already exists".to_string(),
            });
        }
        copy_directory(from, to, execution, mode, progress, session)
    } else if mode == CopyExistingMode::Merge {
        Err(GfmError::Conflict {
            path: to.to_path_buf(),
            message: "merge destination file already exists".to_string(),
        })
    } else {
        verify_copy_with_progress(from, to, execution.verification, progress)?;
        crate::preserve::preserve_metadata(from, to, metadata)?;
        progress.advance(metadata)
    }
}

fn is_existing_finder_package_dir(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| is_finder_package_dir(path, &metadata))
        .unwrap_or(false)
}

fn is_finder_package_dir(path: &Path, metadata: &fs::Metadata) -> bool {
    metadata.is_dir()
        && PackagePolicy::default()
            .classify(path, FileKind::Directory)
            .is_some()
}

fn verify_existing_symlink_copy(from: &Path, to: &Path, mode: CopyExistingMode) -> Result<()> {
    if mode == CopyExistingMode::Merge {
        return Err(GfmError::Conflict {
            path: to.to_path_buf(),
            message: "merge destination symlink already exists".to_string(),
        });
    }
    let source_target = fs::read_link(from).map_err(|err| GfmError::io(from, err))?;
    let destination_target = fs::read_link(to).map_err(|err| GfmError::io(to, err))?;
    if source_target == destination_target {
        Ok(())
    } else {
        Err(GfmError::Conflict {
            path: to.to_path_buf(),
            message: format!(
                "resume symlink target mismatch: {} != {}",
                source_target.display(),
                destination_target.display()
            ),
        })
    }
}

fn create_new_directory(path: &Path) -> Result<()> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Err(GfmError::Conflict {
            path: path.to_path_buf(),
            message: "destination directory already exists".to_string(),
        }),
        Err(err) => Err(GfmError::io(path, err)),
    }
}

fn copy_mode_label(mode: CopyExistingMode) -> &'static str {
    match mode {
        CopyExistingMode::Fresh => "fresh",
        CopyExistingMode::Resume => "resume",
        CopyExistingMode::Merge => "merge",
    }
}

fn copy_symlink(
    from: &Path,
    to: &Path,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    let target = fs::read_link(from).map_err(|err| GfmError::io(from, err))?;
    create_symlink(&target, to)?;
    let metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
    crate::preserve::preserve_xattrs(from, to)?;
    crate::preserve::preserve_symlink_times(to, &metadata)?;
    progress.advance(&metadata)
}

#[cfg(test)]
pub(crate) fn copy_file(
    from: &Path,
    to: &Path,
    verification: VerificationPolicy,
) -> Result<CopyMethod> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    let metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
    match clone_file(from, to) {
        Ok(()) => {
            crate::preserve::preserve_metadata(from, to, &metadata)?;
            verify_copy(from, to, verification)?;
            Ok(CopyMethod::ApfsClone)
        }
        Err(err) if clone_fallback_allowed(&err) => {
            remove_failed_clone_destination(to)?;
            copy_file_bytes(from, to)?;
            crate::preserve::preserve_metadata(from, to, &metadata)?;
            verify_copy(from, to, verification)?;
            Ok(CopyMethod::ByteCopy)
        }
        Err(err) => Err(GfmError::io(from, err)),
    }
}

fn copy_file_tracked(
    from: &Path,
    to: &Path,
    verification: VerificationPolicy,
    volume_copy_policy: &OperationVolumeCopyPolicy,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<CopyMethod> {
    progress.check_cancelled()?;
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    let metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
    match clone_file(from, to) {
        Ok(()) => {
            crate::preserve::preserve_metadata(from, to, &metadata)?;
            verify_copy_with_progress(from, to, verification, progress)?;
            progress.advance(&metadata)?;
            Ok(CopyMethod::ApfsClone)
        }
        Err(err) if clone_fallback_allowed(&err) => {
            remove_failed_clone_destination(to)?;
            copy_file_bytes_tracked(from, to, volume_copy_policy, progress)?;
            crate::preserve::preserve_metadata(from, to, &metadata)?;
            verify_copy_with_progress(from, to, verification, progress)?;
            progress.finish_current_item()?;
            Ok(CopyMethod::ByteCopy)
        }
        Err(err) => Err(GfmError::io(from, err)),
    }
}

fn verify_copy_with_progress(
    from: &Path,
    to: &Path,
    verification: VerificationPolicy,
    progress: &ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    verify_copy_checked(from, to, verification, || progress.check_cancelled())
}

fn copy_file_with_session(
    from: &Path,
    to: &Path,
    metadata: &fs::Metadata,
    execution: CopyExecution<'_>,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
    session: &mut CopySession,
) -> Result<()> {
    if let Some(existing) = session
        .copied_hard_link_destination(metadata)
        .map(Path::to_path_buf)
    {
        link_existing_hard_link(&existing, to, metadata, progress)?;
        return Ok(());
    }

    copy_file_tracked(
        from,
        to,
        execution.verification,
        execution.volume_copy_policy,
        progress,
    )?;
    session.remember_hard_link_destination(metadata, to);
    Ok(())
}

fn link_existing_hard_link(
    existing: &Path,
    to: &Path,
    metadata: &fs::Metadata,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    fs::hard_link(existing, to).map_err(|err| GfmError::io(to, err))?;
    progress.advance(metadata)
}

#[cfg(unix)]
fn create_symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link).map_err(|err| GfmError::io(link, err))
}

#[cfg(windows)]
fn create_symlink(target: &Path, link: &Path) -> Result<()> {
    if target.is_dir() {
        std::os::windows::fs::symlink_dir(target, link).map_err(|err| GfmError::io(link, err))
    } else {
        std::os::windows::fs::symlink_file(target, link).map_err(|err| GfmError::io(link, err))
    }
}
