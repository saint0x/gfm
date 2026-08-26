use crate::{
    decide_invalidation, PreviewInvalidationDecision, PreviewInvalidationEvent, PreviewRequestKey,
};
use gfm_mac::NativeIconDescriptor;
use gfm_types::FileRecord;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconCacheDisposition {
    ReadThrough,
    RefreshMemoryAndDisk,
    RefreshMemoryOnly,
}

impl IconCacheDisposition {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadThrough => "read-through",
            Self::RefreshMemoryAndDisk => "refresh-memory-and-disk",
            Self::RefreshMemoryOnly => "refresh-memory-only",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconPreviewInput {
    pub key: PreviewRequestKey,
    pub record: FileRecord,
    pub invalidation_event: PreviewInvalidationEvent,
}

impl IconPreviewInput {
    pub fn new(key: PreviewRequestKey, record: FileRecord) -> Self {
        Self {
            key,
            record,
            invalidation_event: PreviewInvalidationEvent::default(),
        }
    }

    pub fn with_invalidation(mut self, event: PreviewInvalidationEvent) -> Self {
        self.invalidation_event = event;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconPreviewContract {
    pub key: PreviewRequestKey,
    pub descriptor: NativeIconDescriptor,
    pub invalidation: PreviewInvalidationDecision,
    pub cache_disposition: IconCacheDisposition,
}

impl IconPreviewContract {
    pub fn from_input(input: IconPreviewInput) -> Self {
        let descriptor = NativeIconDescriptor::for_record(&input.record);
        let invalidation = decide_invalidation(input.invalidation_event);
        let cache_disposition = icon_cache_disposition(&invalidation);
        Self {
            key: input.key,
            descriptor,
            invalidation,
            cache_disposition,
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "icon-preview\t{}\t{}\t{}\t{}\tbadges={}\tcache={}\tinvalidate-memory={}\tinvalidate-disk={}",
            self.key.path.display(),
            self.descriptor.role.as_str(),
            self.descriptor.provider.as_str(),
            self.descriptor.type_hint,
            self.descriptor
                .badges
                .iter()
                .map(|badge| badge.as_str())
                .collect::<Vec<_>>()
                .join(","),
            self.cache_disposition.as_str(),
            self.invalidation.invalidate_memory,
            self.invalidation.invalidate_disk
        )
    }
}

fn icon_cache_disposition(invalidation: &PreviewInvalidationDecision) -> IconCacheDisposition {
    if invalidation.invalidate_disk {
        IconCacheDisposition::RefreshMemoryAndDisk
    } else if invalidation.invalidate_memory {
        IconCacheDisposition::RefreshMemoryOnly
    } else {
        IconCacheDisposition::ReadThrough
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PreviewKind;
    use gfm_mac::{NativeIconBadge, NativeIconProvider, NativeIconRole};
    use gfm_types::{FileId, FileKind, VolumeId};
    use std::path::PathBuf;

    #[test]
    fn icon_preview_contract_carries_finder_badge_descriptor() {
        let mut record = record("GFM.app", FileKind::Directory);
        record.tags.push("Important".to_string());
        let contract =
            IconPreviewContract::from_input(IconPreviewInput::new(key("GFM.app"), record));

        assert_eq!(contract.key.kind, PreviewKind::Icon);
        assert_eq!(contract.descriptor.role, NativeIconRole::Application);
        assert_eq!(
            contract.descriptor.provider,
            NativeIconProvider::LaunchServicesApplicationIcon
        );
        assert_eq!(
            contract.descriptor.badges,
            vec![NativeIconBadge::Package, NativeIconBadge::Tagged]
        );
        assert_eq!(
            contract.cache_disposition,
            IconCacheDisposition::ReadThrough
        );
    }

    #[test]
    fn icon_preview_contract_preserves_alias_and_hidden_badges() {
        let mut record = record(".Latest", FileKind::Symlink);
        record.hidden = true;
        let contract =
            IconPreviewContract::from_input(IconPreviewInput::new(key(".Latest"), record));

        assert_eq!(contract.descriptor.role, NativeIconRole::Symlink);
        assert_eq!(
            contract.descriptor.badges,
            vec![NativeIconBadge::Alias, NativeIconBadge::Hidden]
        );
        assert!(contract.as_tsv().contains("badges=alias,hidden"));
    }

    #[test]
    fn icon_metadata_or_tag_changes_refresh_memory_only() {
        let contract = IconPreviewContract::from_input(
            IconPreviewInput::new(key("Report.md"), record("Report.md", FileKind::File))
                .with_invalidation(PreviewInvalidationEvent {
                    tags_changed: true,
                    ..PreviewInvalidationEvent::default()
                }),
        );

        assert_eq!(
            contract.cache_disposition,
            IconCacheDisposition::RefreshMemoryOnly
        );
        assert!(contract.invalidation.invalidate_memory);
        assert!(!contract.invalidation.invalidate_disk);
    }

    fn key(name: &str) -> PreviewRequestKey {
        PreviewRequestKey::new(
            FileId::new(VolumeId(1), 42),
            PathBuf::from("/tmp").join(name),
            PreviewKind::Icon,
        )
    }

    fn record(name: &str, kind: FileKind) -> FileRecord {
        FileRecord {
            id: FileId::new(VolumeId(1), 42),
            parent: None,
            path: PathBuf::from("/tmp").join(name),
            name: name.to_string(),
            kind,
            len: 0,
            mode: 0,
            owner: 0,
            group: 0,
            xattrs_digest: 0,
            created: None,
            modified: None,
            changed: None,
            hidden: name.starts_with('.'),
            tags: Vec::new(),
            finder_comment: None,
        }
    }
}
