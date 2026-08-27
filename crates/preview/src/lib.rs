use gfm_types::{FileId, GfmError, Result};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

mod icon;
mod quicklook;
mod schedule;
mod thumbnail;

pub use icon::{IconCacheDisposition, IconPreviewContract, IconPreviewInput};
pub use quicklook::{QuickLookControllerMode, QuickLookSessionContract, QuickLookSessionInput};
pub use schedule::{
    PreviewPriority, PreviewScheduler, PreviewSchedulingPolicy, PreviewTask, PreviewTaskDecision,
    Rect, Viewport,
};
pub use thumbnail::{
    ThumbnailCacheDisposition, ThumbnailGenerationContract, ThumbnailGenerationInput,
    ThumbnailGeneratorMode,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PreviewKind {
    Icon,
    Thumbnail,
    QuickLook,
    Text,
}

impl PreviewKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Icon => "icon",
            Self::Thumbnail => "thumbnail",
            Self::QuickLook => "quick-look",
            Self::Text => "text",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PreviewRequestKey {
    pub file_id: FileId,
    pub path: PathBuf,
    pub kind: PreviewKind,
    pub pixel_size: u16,
    pub scale_factor_milli: u16,
    pub content_epoch: u64,
}

impl PreviewRequestKey {
    pub fn new(file_id: FileId, path: impl Into<PathBuf>, kind: PreviewKind) -> Self {
        Self {
            file_id,
            path: path.into(),
            kind,
            pixel_size: 256,
            scale_factor_milli: 2_000,
            content_epoch: 0,
        }
    }

    fn stable_name(&self) -> String {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut hasher);
        format!("{}-{:016x}.gfmpreview", self.kind.as_str(), hasher.finish())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewEntry {
    pub key: PreviewRequestKey,
    pub bytes: Vec<u8>,
    pub created: SystemTime,
}

impl PreviewEntry {
    pub fn new(key: PreviewRequestKey, bytes: Vec<u8>) -> Self {
        Self {
            key,
            bytes,
            created: SystemTime::now(),
        }
    }

    pub fn byte_len(&self) -> usize {
        self.bytes.len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheHit {
    Memory(PreviewEntry),
    Disk(PreviewEntry),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewCacheConfig {
    pub memory_budget_bytes: usize,
    pub max_entry_bytes: usize,
    pub disk_root: PathBuf,
    pub disk_enabled: bool,
}

impl PreviewCacheConfig {
    pub fn new(disk_root: impl Into<PathBuf>) -> Self {
        Self {
            memory_budget_bytes: 32 * 1024 * 1024,
            max_entry_bytes: 8 * 1024 * 1024,
            disk_root: disk_root.into(),
            disk_enabled: true,
        }
    }
}

pub struct PreviewCache {
    config: PreviewCacheConfig,
    memory: HashMap<PreviewRequestKey, PreviewEntry>,
    order: VecDeque<PreviewRequestKey>,
    memory_bytes: usize,
}

impl PreviewCache {
    pub fn new(config: PreviewCacheConfig) -> Result<Self> {
        if config.memory_budget_bytes == 0 {
            return Err(GfmError::Format(
                "preview cache memory budget must be non-zero".to_string(),
            ));
        }
        if config.max_entry_bytes == 0 || config.max_entry_bytes > config.memory_budget_bytes {
            return Err(GfmError::Format(
                "preview cache max entry bytes must fit within memory budget".to_string(),
            ));
        }
        if config.disk_enabled {
            fs::create_dir_all(&config.disk_root)
                .map_err(|err| GfmError::io(&config.disk_root, err))?;
        }
        Ok(Self {
            config,
            memory: HashMap::new(),
            order: VecDeque::new(),
            memory_bytes: 0,
        })
    }

    pub fn insert(&mut self, entry: PreviewEntry) -> Result<()> {
        if entry.bytes.len() > self.config.max_entry_bytes {
            return Err(GfmError::Format(format!(
                "preview entry is {} bytes, above max {}",
                entry.bytes.len(),
                self.config.max_entry_bytes
            )));
        }
        if self.config.disk_enabled {
            self.write_disk(&entry)?;
        }
        self.insert_memory(entry);
        Ok(())
    }

    pub fn get(&mut self, key: &PreviewRequestKey) -> Result<Option<CacheHit>> {
        if let Some(entry) = self.memory.get(key).cloned() {
            self.touch(key);
            return Ok(Some(CacheHit::Memory(entry)));
        }
        if self.config.disk_enabled {
            if let Some(entry) = self.read_disk(key)? {
                self.insert_memory(entry.clone());
                return Ok(Some(CacheHit::Disk(entry)));
            }
        }
        Ok(None)
    }

    pub fn invalidate(&mut self, key: &PreviewRequestKey) -> Result<()> {
        if let Some(entry) = self.memory.remove(key) {
            self.memory_bytes = self.memory_bytes.saturating_sub(entry.byte_len());
        }
        self.order.retain(|candidate| candidate != key);
        if self.config.disk_enabled {
            let path = self.disk_path(key);
            if path.exists() {
                fs::remove_file(&path).map_err(|err| GfmError::io(&path, err))?;
            }
        }
        Ok(())
    }

    pub fn memory_bytes(&self) -> usize {
        self.memory_bytes
    }

    fn insert_memory(&mut self, entry: PreviewEntry) {
        if let Some(previous) = self.memory.remove(&entry.key) {
            self.memory_bytes = self.memory_bytes.saturating_sub(previous.byte_len());
            self.order.retain(|key| key != &entry.key);
        }
        self.memory_bytes += entry.byte_len();
        self.order.push_back(entry.key.clone());
        self.memory.insert(entry.key.clone(), entry);
        self.evict_over_budget();
    }

    fn touch(&mut self, key: &PreviewRequestKey) {
        self.order.retain(|candidate| candidate != key);
        self.order.push_back(key.clone());
    }

    fn evict_over_budget(&mut self) {
        while self.memory_bytes > self.config.memory_budget_bytes {
            let Some(key) = self.order.pop_front() else {
                break;
            };
            if let Some(entry) = self.memory.remove(&key) {
                self.memory_bytes = self.memory_bytes.saturating_sub(entry.byte_len());
            }
        }
    }

    fn write_disk(&self, entry: &PreviewEntry) -> Result<()> {
        let path = self.disk_path(&entry.key);
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, &entry.bytes).map_err(|err| GfmError::io(&tmp, err))?;
        fs::rename(&tmp, &path).map_err(|err| GfmError::io(&path, err))
    }

    fn read_disk(&self, key: &PreviewRequestKey) -> Result<Option<PreviewEntry>> {
        let path = self.disk_path(key);
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = fs::read(&path).map_err(|err| GfmError::io(&path, err))?;
        if bytes.len() > self.config.max_entry_bytes {
            fs::remove_file(&path).map_err(|err| GfmError::io(&path, err))?;
            return Ok(None);
        }
        Ok(Some(PreviewEntry::new(key.clone(), bytes)))
    }

    fn disk_path(&self, key: &PreviewRequestKey) -> PathBuf {
        self.config.disk_root.join(key.stable_name())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoalescingDecision {
    StartProducer,
    JoinExisting,
}

impl CoalescingDecision {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StartProducer => "start-producer",
            Self::JoinExisting => "join-existing",
        }
    }
}

#[derive(Default)]
pub struct RequestCoalescer {
    inflight: HashSet<PreviewRequestKey>,
}

impl RequestCoalescer {
    pub fn request(&mut self, key: PreviewRequestKey) -> CoalescingDecision {
        if self.inflight.insert(key) {
            CoalescingDecision::StartProducer
        } else {
            CoalescingDecision::JoinExisting
        }
    }

    pub fn finish(&mut self, key: &PreviewRequestKey) {
        self.inflight.remove(key);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustLevel {
    Trusted,
    Untrusted,
    Blocked,
}

impl TrustLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Trusted => "trusted",
            Self::Untrusted => "untrusted",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewSecurityDecision {
    AllowNative,
    Sandbox,
    MetadataOnly,
    Deny,
}

impl PreviewSecurityDecision {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AllowNative => "allow-native",
            Self::Sandbox => "sandbox",
            Self::MetadataOnly => "metadata-only",
            Self::Deny => "deny",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewSecurityPolicy {
    pub allow_native_for_trusted: bool,
    pub sandbox_untrusted_quicklook: bool,
    pub metadata_only_for_executables: bool,
    pub deny_remote_untrusted: bool,
}

impl Default for PreviewSecurityPolicy {
    fn default() -> Self {
        Self {
            allow_native_for_trusted: true,
            sandbox_untrusted_quicklook: true,
            metadata_only_for_executables: true,
            deny_remote_untrusted: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewSecurityInput {
    pub path: PathBuf,
    pub kind: PreviewKind,
    pub trust: TrustLevel,
    pub is_executable: bool,
    pub is_remote: bool,
}

pub fn decide_preview_security(
    policy: &PreviewSecurityPolicy,
    input: &PreviewSecurityInput,
) -> PreviewSecurityDecision {
    if input.trust == TrustLevel::Blocked {
        return PreviewSecurityDecision::Deny;
    }
    if policy.metadata_only_for_executables && input.is_executable {
        return PreviewSecurityDecision::MetadataOnly;
    }
    if policy.deny_remote_untrusted && input.is_remote && input.trust == TrustLevel::Untrusted {
        return PreviewSecurityDecision::Deny;
    }
    if input.trust == TrustLevel::Untrusted
        && input.kind == PreviewKind::QuickLook
        && policy.sandbox_untrusted_quicklook
    {
        return PreviewSecurityDecision::Sandbox;
    }
    if input.trust == TrustLevel::Trusted && policy.allow_native_for_trusted {
        PreviewSecurityDecision::AllowNative
    } else {
        PreviewSecurityDecision::Sandbox
    }
}

pub fn security_input_for_path(
    path: impl Into<PathBuf>,
    kind: PreviewKind,
) -> PreviewSecurityInput {
    let path = path.into();
    let trust = trust_for_path(&path);
    let is_executable = is_probably_executable(&path);
    let is_remote = path.starts_with("/Volumes") || path.starts_with("/Network");
    PreviewSecurityInput {
        path,
        kind,
        trust,
        is_executable,
        is_remote,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudPreviewDecision {
    NativeEligible,
    MetadataOnly,
    Defer,
    Unavailable,
}

impl CloudPreviewDecision {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NativeEligible => "native-eligible",
            Self::MetadataOnly => "metadata-only",
            Self::Defer => "defer",
            Self::Unavailable => "unavailable",
        }
    }
}

pub fn decide_cloud_preview(state: gfm_mac::CloudStorageState) -> CloudPreviewDecision {
    decide_cloud_preview_for_materialization(cloud_materialization_for_state(state))
}

pub fn cloud_materialization_for_state(
    state: gfm_mac::CloudStorageState,
) -> gfm_mac::CloudMaterialization {
    match state {
        gfm_mac::CloudStorageState::LocalOnly => gfm_mac::CloudMaterialization::NotProviderBacked,
        gfm_mac::CloudStorageState::Downloaded => gfm_mac::CloudMaterialization::Materialized,
        gfm_mac::CloudStorageState::Evicted => gfm_mac::CloudMaterialization::RemotePlaceholder,
        gfm_mac::CloudStorageState::Downloading
        | gfm_mac::CloudStorageState::Uploading
        | gfm_mac::CloudStorageState::Waiting => gfm_mac::CloudMaterialization::InFlight,
        gfm_mac::CloudStorageState::Conflict => gfm_mac::CloudMaterialization::Conflict,
        gfm_mac::CloudStorageState::Offline => gfm_mac::CloudMaterialization::Offline,
        gfm_mac::CloudStorageState::Unknown => gfm_mac::CloudMaterialization::Unknown,
    }
}

pub fn decide_cloud_preview_for_materialization(
    materialization: gfm_mac::CloudMaterialization,
) -> CloudPreviewDecision {
    match materialization {
        gfm_mac::CloudMaterialization::NotProviderBacked
        | gfm_mac::CloudMaterialization::Materialized => CloudPreviewDecision::NativeEligible,
        gfm_mac::CloudMaterialization::RemotePlaceholder
        | gfm_mac::CloudMaterialization::Conflict
        | gfm_mac::CloudMaterialization::Unknown => CloudPreviewDecision::MetadataOnly,
        gfm_mac::CloudMaterialization::InFlight => CloudPreviewDecision::Defer,
        gfm_mac::CloudMaterialization::Offline => CloudPreviewDecision::Unavailable,
    }
}

fn trust_for_path(path: &Path) -> TrustLevel {
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return TrustLevel::Trusted;
    };
    match ext.to_ascii_lowercase().as_str() {
        "app" | "command" | "dmg" | "pkg" | "scpt" | "workflow" => TrustLevel::Untrusted,
        "download" | "part" => TrustLevel::Blocked,
        _ => TrustLevel::Trusted,
    }
}

fn is_probably_executable(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "app" | "command" | "pkg" | "sh" | "zsh" | "bash"
            )
        })
        .unwrap_or(false)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PreviewInvalidationEvent {
    pub content_changed: bool,
    pub metadata_changed: bool,
    pub tags_changed: bool,
    pub icloud_state_changed: bool,
    pub removed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewInvalidationDecision {
    pub invalidate_memory: bool,
    pub invalidate_disk: bool,
    pub reason: &'static str,
}

pub fn decide_invalidation(event: PreviewInvalidationEvent) -> PreviewInvalidationDecision {
    if event.removed {
        return PreviewInvalidationDecision {
            invalidate_memory: true,
            invalidate_disk: true,
            reason: "removed",
        };
    }
    if event.content_changed || event.icloud_state_changed {
        return PreviewInvalidationDecision {
            invalidate_memory: true,
            invalidate_disk: true,
            reason: "content-or-icloud",
        };
    }
    if event.metadata_changed || event.tags_changed {
        return PreviewInvalidationDecision {
            invalidate_memory: true,
            invalidate_disk: false,
            reason: "metadata-or-tags",
        };
    }
    PreviewInvalidationDecision {
        invalidate_memory: false,
        invalidate_disk: false,
        reason: "unchanged",
    }
}

pub fn preview_invalidation_for_fileprovider(
    report: &gfm_mac::FileProviderInvalidationReport,
) -> PreviewInvalidationEvent {
    PreviewInvalidationEvent {
        icloud_state_changed: report.invalidate_preview_memory || report.invalidate_preview_disk,
        metadata_changed: report.reindex_metadata
            && !report.invalidate_preview_memory
            && !report.invalidate_preview_disk,
        ..PreviewInvalidationEvent::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfm_types::VolumeId;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn cache_uses_memory_then_disk_tiers() {
        let root = temp_root("cache");
        let config = PreviewCacheConfig {
            memory_budget_bytes: 8,
            max_entry_bytes: 8,
            disk_root: root.clone(),
            disk_enabled: true,
        };
        let key = key("a.png", PreviewKind::Thumbnail);
        let mut cache = PreviewCache::new(config).unwrap();
        cache
            .insert(PreviewEntry::new(key.clone(), b"123456".to_vec()))
            .unwrap();

        assert!(matches!(
            cache.get(&key).unwrap(),
            Some(CacheHit::Memory(_))
        ));

        let mut reloaded = PreviewCache::new(PreviewCacheConfig {
            memory_budget_bytes: 8,
            max_entry_bytes: 8,
            disk_root: root,
            disk_enabled: true,
        })
        .unwrap();
        assert!(matches!(
            reloaded.get(&key).unwrap(),
            Some(CacheHit::Disk(_))
        ));
    }

    #[test]
    fn cache_evicts_memory_without_losing_disk_entry() {
        let root = temp_root("evict");
        let mut cache = PreviewCache::new(PreviewCacheConfig {
            memory_budget_bytes: 6,
            max_entry_bytes: 6,
            disk_root: root,
            disk_enabled: true,
        })
        .unwrap();
        let first = key("a.png", PreviewKind::Thumbnail);
        let second = key("b.png", PreviewKind::Thumbnail);

        cache
            .insert(PreviewEntry::new(first.clone(), b"1111".to_vec()))
            .unwrap();
        cache
            .insert(PreviewEntry::new(second.clone(), b"2222".to_vec()))
            .unwrap();

        assert!(cache.memory_bytes() <= 6);
        assert!(matches!(
            cache.get(&first).unwrap(),
            Some(CacheHit::Disk(_))
        ));
    }

    #[test]
    fn coalescer_joins_duplicate_requests_until_finished() {
        let mut coalescer = RequestCoalescer::default();
        let key = key("a.pdf", PreviewKind::QuickLook);

        assert_eq!(
            coalescer.request(key.clone()),
            CoalescingDecision::StartProducer
        );
        assert_eq!(
            coalescer.request(key.clone()),
            CoalescingDecision::JoinExisting
        );
        coalescer.finish(&key);
        assert_eq!(coalescer.request(key), CoalescingDecision::StartProducer);
    }

    #[test]
    fn security_sandboxes_untrusted_quicklook_and_blocks_remote_untrusted() {
        let policy = PreviewSecurityPolicy::default();
        let local = security_input_for_path("/tmp/example.app", PreviewKind::QuickLook);
        let remote = security_input_for_path("/Volumes/share/example.dmg", PreviewKind::Thumbnail);

        assert_eq!(
            decide_preview_security(&policy, &local),
            PreviewSecurityDecision::MetadataOnly
        );
        assert_eq!(
            decide_preview_security(&policy, &remote),
            PreviewSecurityDecision::Deny
        );
    }

    #[test]
    fn cloud_preview_uses_materialization_verdicts() {
        assert_eq!(
            decide_cloud_preview_for_materialization(gfm_mac::CloudMaterialization::Materialized),
            CloudPreviewDecision::NativeEligible
        );
        assert_eq!(
            decide_cloud_preview_for_materialization(
                gfm_mac::CloudMaterialization::RemotePlaceholder
            ),
            CloudPreviewDecision::MetadataOnly
        );
        assert_eq!(
            decide_cloud_preview_for_materialization(gfm_mac::CloudMaterialization::InFlight),
            CloudPreviewDecision::Defer
        );
        assert_eq!(
            decide_cloud_preview_for_materialization(gfm_mac::CloudMaterialization::Offline),
            CloudPreviewDecision::Unavailable
        );
    }

    #[test]
    fn invalidation_separates_content_from_metadata() {
        assert_eq!(
            decide_invalidation(PreviewInvalidationEvent {
                tags_changed: true,
                ..PreviewInvalidationEvent::default()
            }),
            PreviewInvalidationDecision {
                invalidate_memory: true,
                invalidate_disk: false,
                reason: "metadata-or-tags"
            }
        );
        assert_eq!(
            decide_invalidation(PreviewInvalidationEvent {
                content_changed: true,
                ..PreviewInvalidationEvent::default()
            }),
            PreviewInvalidationDecision {
                invalidate_memory: true,
                invalidate_disk: true,
                reason: "content-or-icloud"
            }
        );
    }

    #[test]
    fn fileprovider_invalidation_maps_to_preview_icloud_invalidation() {
        let report = fileprovider_report(
            gfm_mac::CloudStorageState::Downloaded,
            gfm_mac::CloudStorageState::Evicted,
            true,
        );

        let event = preview_invalidation_for_fileprovider(&report);
        let decision = decide_invalidation(event);

        assert!(event.icloud_state_changed);
        assert!(!event.metadata_changed);
        assert_eq!(
            decision,
            PreviewInvalidationDecision {
                invalidate_memory: true,
                invalidate_disk: true,
                reason: "content-or-icloud"
            }
        );
    }

    #[test]
    fn fileprovider_metadata_only_invalidation_stays_memory_only() {
        let report = fileprovider_report(
            gfm_mac::CloudStorageState::Downloaded,
            gfm_mac::CloudStorageState::Downloaded,
            false,
        );

        let event = preview_invalidation_for_fileprovider(&report);
        let decision = decide_invalidation(event);

        assert!(!event.icloud_state_changed);
        assert!(event.metadata_changed);
        assert_eq!(
            decision,
            PreviewInvalidationDecision {
                invalidate_memory: true,
                invalidate_disk: false,
                reason: "metadata-or-tags"
            }
        );
    }

    fn key(name: &str, kind: PreviewKind) -> PreviewRequestKey {
        PreviewRequestKey::new(
            FileId::new(VolumeId(1), name.len() as u64),
            PathBuf::from(name),
            kind,
        )
    }

    fn fileprovider_report(
        previous: gfm_mac::CloudStorageState,
        current: gfm_mac::CloudStorageState,
        invalidate_preview: bool,
    ) -> gfm_mac::FileProviderInvalidationReport {
        gfm_mac::FileProviderInvalidationReport {
            path: PathBuf::from("/tmp/Remote.icloud"),
            previous,
            current: gfm_mac::FileProviderStateReport {
                path: PathBuf::from("/tmp/Remote.icloud"),
                domain: gfm_mac::FileProviderDomain::ICloudDrive,
                storage_state: current,
                materialization: cloud_materialization_for_state(current),
                materialization_source: gfm_mac::CloudMaterializationSource::NativeUrlResource,
                progress: gfm_mac::CloudTransferProgress {
                    direction: gfm_mac::CloudTransferDirection::Idle,
                    percent_milli: None,
                    requested: false,
                    complete: false,
                    indeterminate: false,
                    source: "state",
                    reason: Some("test".to_string()),
                },
                badges: Vec::new(),
                commands: gfm_mac::CloudCommandPolicy {
                    download: gfm_mac::CloudCommandState::Hidden,
                    evict: gfm_mac::CloudCommandState::Hidden,
                    reveal_conflict: gfm_mac::CloudCommandState::Hidden,
                    reason: None,
                },
                offline: false,
                conflict: false,
                provider_identifier: None,
                source: "test".to_string(),
            },
            state_changed: previous != current,
            invalidate_icon: invalidate_preview,
            invalidate_preview_memory: invalidate_preview,
            invalidate_preview_disk: invalidate_preview,
            invalidate_sidebar: invalidate_preview,
            reindex_metadata: true,
            reason: "test",
        }
    }

    fn temp_root(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "gfm-preview-{name}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
