use crate::control::OperationCancellation;
use crate::progress::{item_bytes, OperationProgress};
use crate::Operation;
use gfm_types::{GfmError, Result};
use std::fs;
use std::path::Path;

pub fn plan_operation(operation: &Operation) -> Result<OperationProgress> {
    plan_operation_checked(operation, &OperationCancellation::default())
}

pub(crate) fn plan_operation_checked(
    operation: &Operation,
    cancellation: &OperationCancellation,
) -> Result<OperationProgress> {
    match operation {
        Operation::Copy { from, .. }
        | Operation::Move { from, .. }
        | Operation::Rename { from, .. }
        | Operation::Restore { from, .. } => plan_path(from, cancellation),
        Operation::Delete { path } | Operation::Trash { path } => plan_path(path, cancellation),
        Operation::EmptyTrash { path } => plan_empty_trash(path, cancellation),
    }
}

fn plan_path(path: &Path, cancellation: &OperationCancellation) -> Result<OperationProgress> {
    cancellation.check()?;
    let metadata = fs::symlink_metadata(path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            GfmError::io(path, err)
        } else {
            GfmError::io(path, format!("operation path metadata unavailable: {err}"))
        }
    })?;
    let mut progress = OperationProgress {
        total_items: 1,
        total_bytes: item_bytes(&metadata),
        completed_items: 0,
        completed_bytes: 0,
    };
    if metadata.is_dir() {
        for entry in fs::read_dir(path).map_err(|err| GfmError::io(path, err))? {
            cancellation.check()?;
            let entry = entry.map_err(|err| GfmError::io(path, err))?;
            let child = plan_path(&entry.path(), cancellation)?;
            progress.total_items += child.total_items;
            progress.total_bytes += child.total_bytes;
        }
    }
    Ok(progress)
}

fn plan_empty_trash(
    path: &Path,
    cancellation: &OperationCancellation,
) -> Result<OperationProgress> {
    cancellation.check()?;
    let metadata = fs::symlink_metadata(path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            GfmError::io(path, err)
        } else {
            GfmError::io(path, format!("operation path metadata unavailable: {err}"))
        }
    })?;
    if !metadata.is_dir() {
        return Err(GfmError::Format(format!(
            "empty trash requires a directory: {}",
            path.display()
        )));
    }
    let mut progress = OperationProgress::default();
    for entry in fs::read_dir(path).map_err(|err| GfmError::io(path, err))? {
        cancellation.check()?;
        let entry = entry.map_err(|err| GfmError::io(path, err))?;
        let child = plan_path(&entry.path(), cancellation)?;
        progress.total_items += child.total_items;
        progress.total_bytes += child.total_bytes;
    }
    Ok(progress)
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
    fn plans_recursive_copy_totals_before_execution() {
        let root = unique_temp_dir("gfm-plan-copy");
        let source = root.join("source");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("alpha.txt"), "alpha").unwrap();
        fs::write(source.join("nested").join("beta.txt"), "beta").unwrap();

        let progress = plan_operation(&Operation::Copy {
            from: source.clone(),
            to: root.join("destination"),
        })
        .unwrap();

        assert_eq!(progress.total_items, 4);
        assert_eq!(progress.total_bytes, 9);
        assert_eq!(progress.completed_items, 0);
        assert_eq!(progress.completed_bytes, 0);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn empty_trash_plans_children_without_counting_trash_root() {
        let root = unique_temp_dir("gfm-plan-empty-trash");
        let trash = root.join("Trash");
        fs::create_dir_all(trash.join("folder")).unwrap();
        fs::write(trash.join("file.txt"), "file").unwrap();
        fs::write(trash.join("folder").join("note.txt"), "note").unwrap();

        let progress = plan_operation(&Operation::EmptyTrash {
            path: trash.clone(),
        })
        .unwrap();

        assert_eq!(progress.total_items, 3);
        assert_eq!(progress.total_bytes, 8);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancelled_planning_stops_before_filesystem_walk() {
        let cancellation = OperationCancellation::default();
        cancellation.cancel();

        let err = plan_operation_checked(
            &Operation::Delete {
                path: Path::new("/this/path/should/not/be/read").into(),
            },
            &cancellation,
        )
        .unwrap_err();

        assert!(matches!(err, GfmError::Cancelled));
    }
}
