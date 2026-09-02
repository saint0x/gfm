use gfm_store::{metadata_postings_from_records_and_secondary, write_metadata_postings};
use gfm_types::{FileRecord, Result, SecondaryMetadataRecord};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataUpdateReport {
    pub path: PathBuf,
    pub existed: bool,
    pub changed: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecondaryMetadataPublicationReport {
    pub path: PathBuf,
    pub primary_records: usize,
    pub secondary_records: usize,
    pub postings: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderMetadataInvalidationReport {
    pub path: PathBuf,
    pub previous_state: String,
    pub current_state: String,
    pub reindex_metadata: bool,
    pub schedule_metadata_update: bool,
    pub invalidate_query_cache: bool,
    pub reason: String,
}

impl SecondaryMetadataPublicationReport {
    pub fn as_tsv(&self) -> String {
        format!(
            "secondary-metadata-publication\t{}\tprimary_records={}\tsecondary_records={}\tpostings={}",
            self.path.display(),
            self.primary_records,
            self.secondary_records,
            self.postings,
        )
    }
}

impl ProviderMetadataInvalidationReport {
    pub fn from_provider_transition(
        path: impl Into<PathBuf>,
        previous_state: impl Into<String>,
        current_state: impl Into<String>,
        provider_reindex_metadata: bool,
        state_changed: bool,
        provider_reason: impl Into<String>,
    ) -> Self {
        let previous_state = previous_state.into();
        let current_state = current_state.into();
        let provider_reason = provider_reason.into();
        let provider_metadata_changed = matches!(
            provider_reason.as_str(),
            "fileprovider-observed-metadata-changed" | "fileprovider-state-signature-changed"
        );
        let schedule_metadata_update =
            provider_reindex_metadata && (state_changed || provider_metadata_changed);
        let invalidate_query_cache = schedule_metadata_update;
        let reason = if !provider_reindex_metadata {
            "provider-did-not-reindex-metadata".to_string()
        } else if provider_metadata_changed {
            provider_reason
        } else if !state_changed {
            "provider-state-unchanged".to_string()
        } else if previous_state != current_state {
            "provider-metadata-state-changed".to_string()
        } else {
            provider_reason
        };

        Self {
            path: path.into(),
            previous_state,
            current_state,
            reindex_metadata: provider_reindex_metadata,
            schedule_metadata_update,
            invalidate_query_cache,
            reason,
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "provider-metadata-invalidation\t{}\tprevious={}\tcurrent={}\treindex-metadata={}\tschedule-metadata-update={}\tinvalidate-query-cache={}\treason={}",
            self.path.display(),
            self.previous_state,
            self.current_state,
            self.reindex_metadata,
            self.schedule_metadata_update,
            self.invalidate_query_cache,
            self.reason
        )
    }
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

pub fn publish_secondary_metadata(
    records: &[FileRecord],
    secondary: &[SecondaryMetadataRecord],
    metadata_path: impl AsRef<Path>,
) -> Result<SecondaryMetadataPublicationReport> {
    let metadata_path = metadata_path.as_ref();
    let postings = metadata_postings_from_records_and_secondary(records, secondary);
    write_metadata_postings(metadata_path, &postings)?;
    Ok(SecondaryMetadataPublicationReport {
        path: metadata_path.to_path_buf(),
        primary_records: records.len(),
        secondary_records: secondary.len(),
        postings: postings.len(),
    })
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
