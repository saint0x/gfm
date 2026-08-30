use gfm_search::ShardedSearchIndex;
use gfm_types::{FileRecord, GfmError, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameCorrelationReport {
    pub from: PathBuf,
    pub to: PathBuf,
    pub removed: usize,
    pub inserted: usize,
    pub preserved: usize,
}

impl RenameCorrelationReport {
    pub fn as_tsv(&self) -> String {
        format!(
            "rename-correlation\tfrom={}\tto={}\tremoved={}\tinserted={}\tpreserved={}",
            self.from.display(),
            self.to.display(),
            self.removed,
            self.inserted,
            self.preserved
        )
    }
}

pub fn correlate_rename(
    index: &mut ShardedSearchIndex,
    from: &Path,
    to: &Path,
) -> Result<RenameCorrelationReport> {
    correlate_rename_checked(index, from, to, || Ok(()))
}

pub fn correlate_rename_checked(
    index: &mut ShardedSearchIndex,
    from: &Path,
    to: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<RenameCorrelationReport> {
    check_control()?;
    let removed_records = index.remove_subtree(from);
    let removed = removed_records.len();
    if let Err(err) = check_control() {
        restore_removed(index, removed_records);
        return Err(err);
    }

    if removed_records.is_empty() {
        let record = gfm_fs::record_for_path_checked(to, None, false, &mut check_control)?;
        check_control()?;
        index.insert(record);
        return Ok(RenameCorrelationReport {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
            removed,
            inserted: 1,
            preserved: 0,
        });
    }

    let fresh_root = match gfm_fs::record_for_path_checked(
        to,
        root_parent(&removed_records, from),
        false,
        &mut check_control,
    ) {
        Ok(record) => record,
        Err(err) => {
            return match err {
                GfmError::Io { .. } => Ok(RenameCorrelationReport {
                    from: from.to_path_buf(),
                    to: to.to_path_buf(),
                    removed,
                    inserted: 0,
                    preserved: 0,
                }),
                other => {
                    restore_removed(index, removed_records);
                    Err(other)
                }
            };
        }
    };

    let mut moved_records = Vec::new();
    let mut preserved = 0;
    for old in &removed_records {
        if let Err(err) = check_control() {
            restore_removed(index, removed_records);
            return Err(err);
        }
        if let Some(moved) = moved_record(old.clone(), from, to, &fresh_root) {
            preserved += 1;
            moved_records.push(moved);
        }
    }
    if let Err(err) = check_control() {
        restore_removed(index, removed_records);
        return Err(err);
    }

    let inserted = moved_records.len();
    for moved in moved_records {
        index.insert(moved);
    }

    Ok(RenameCorrelationReport {
        from: from.to_path_buf(),
        to: to.to_path_buf(),
        removed,
        inserted,
        preserved,
    })
}

fn restore_removed(index: &mut ShardedSearchIndex, records: Vec<FileRecord>) {
    for record in records {
        index.insert(record);
    }
}

fn moved_record(
    old: FileRecord,
    from: &Path,
    to: &Path,
    fresh_root: &FileRecord,
) -> Option<FileRecord> {
    let moved_path = renamed_path(&old.path, from, to)?;
    if old.path == from {
        let mut moved = fresh_root.clone();
        moved.id = old.id;
        moved.parent = old.parent;
        Some(moved)
    } else {
        let mut moved = old;
        moved.path = moved_path;
        moved.name = moved
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("")
            .to_string();
        Some(moved)
    }
}

fn renamed_path(path: &Path, from: &Path, to: &Path) -> Option<PathBuf> {
    if path == from {
        return Some(to.to_path_buf());
    }
    let suffix = path.strip_prefix(from).ok()?;
    Some(to.join(suffix))
}

fn root_parent(records: &[FileRecord], from: &Path) -> Option<gfm_types::FileId> {
    records
        .iter()
        .find(|record| record.path == from)
        .and_then(|record| record.parent)
}
