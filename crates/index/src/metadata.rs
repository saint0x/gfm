use gfm_types::FileRecord;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataUpdateReport {
    pub path: PathBuf,
    pub existed: bool,
    pub changed: Vec<&'static str>,
}

impl MetadataUpdateReport {
    pub fn from_records(path: &Path, previous: Option<&FileRecord>, current: &FileRecord) -> Self {
        let Some(previous) = previous else {
            return Self {
                path: path.to_path_buf(),
                existed: false,
                changed: vec!["record"],
            };
        };

        let mut changed = Vec::new();
        push_changed(&mut changed, "kind", previous.kind != current.kind);
        push_changed(&mut changed, "size", previous.len != current.len);
        push_changed(&mut changed, "mode", previous.mode != current.mode);
        push_changed(&mut changed, "owner", previous.owner != current.owner);
        push_changed(&mut changed, "group", previous.group != current.group);
        push_changed(
            &mut changed,
            "xattrs",
            previous.xattrs_digest != current.xattrs_digest,
        );
        push_changed(&mut changed, "created", previous.created != current.created);
        push_changed(
            &mut changed,
            "modified",
            previous.modified != current.modified,
        );
        push_changed(&mut changed, "changed", previous.changed != current.changed);
        push_changed(&mut changed, "hidden", previous.hidden != current.hidden);
        push_changed(&mut changed, "tags", previous.tags != current.tags);
        push_changed(
            &mut changed,
            "finder-comment",
            previous.finder_comment != current.finder_comment,
        );

        Self {
            path: path.to_path_buf(),
            existed: true,
            changed,
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "metadata-update\t{}\texisted={}\tchanged={}\tfields={}",
            self.path.display(),
            self.existed,
            self.changed.len(),
            self.changed.join(",")
        )
    }
}

pub fn diff_metadata(
    path: &Path,
    previous: Option<&FileRecord>,
    current: &FileRecord,
) -> MetadataUpdateReport {
    MetadataUpdateReport::from_records(path, previous, current)
}

fn push_changed(changed: &mut Vec<&'static str>, field: &'static str, is_changed: bool) {
    if is_changed {
        changed.push(field);
    }
}
