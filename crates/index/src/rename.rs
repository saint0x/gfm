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
    let removed_records = index.remove_subtree(from);
    let removed = removed_records.len();

    if removed_records.is_empty() {
        let record = gfm_fs::record_for_path(to, None, false)?;
        index.insert(record);
        return Ok(RenameCorrelationReport {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
            removed,
            inserted: 1,
            preserved: 0,
        });
    }

    let fresh_root = match gfm_fs::record_for_path(to, root_parent(&removed_records, from), false) {
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
                other => Err(other),
            };
        }
    };

    let mut inserted = 0;
    let mut preserved = 0;
    for old in removed_records {
        if let Some(moved) = moved_record(old, from, to, &fresh_root) {
            preserved += 1;
            inserted += 1;
            index.insert(moved);
        }
    }

    Ok(RenameCorrelationReport {
        from: from.to_path_buf(),
        to: to.to_path_buf(),
        removed,
        inserted,
        preserved,
    })
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
