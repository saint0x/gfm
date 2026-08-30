use gfm_jobs::Cancellation;
use gfm_types::{FileId, GfmError, Result, VolumeId};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::fs;
use std::hash::{Hash, Hasher};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
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

    fn parse(value: &str) -> Option<Self> {
        match value {
            "icon" => Some(Self::Icon),
            "thumbnail" => Some(Self::Thumbnail),
            "quick-look" => Some(Self::QuickLook),
            "text" => Some(Self::Text),
            _ => None,
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
    disk_index: HashMap<(PathBuf, PreviewKind), PreviewRequestKey>,
    memory_bytes: usize,
}

impl PreviewCache {
    pub fn new(config: PreviewCacheConfig) -> Result<Self> {
        Self::new_cancellable(config, &Cancellation::default())
    }

    pub fn new_cancellable(
        config: PreviewCacheConfig,
        cancellation: &Cancellation,
    ) -> Result<Self> {
        cancellation.check()?;
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
            cancellation.check()?;
            fs::create_dir_all(&config.disk_root)
                .map_err(|err| GfmError::io(&config.disk_root, err))?;
            cancellation.check()?;
        }
        let disk_index = if config.disk_enabled {
            load_disk_index_cancellable(&config, cancellation)?
        } else {
            HashMap::new()
        };
        cancellation.check()?;
        Ok(Self {
            config,
            memory: HashMap::new(),
            order: VecDeque::new(),
            disk_index,
            memory_bytes: 0,
        })
    }

    pub fn insert(&mut self, entry: PreviewEntry) -> Result<()> {
        self.insert_cancellable(entry, &Cancellation::default())
    }

    pub fn insert_cancellable(
        &mut self,
        entry: PreviewEntry,
        cancellation: &Cancellation,
    ) -> Result<()> {
        cancellation.check()?;
        if entry.bytes.len() > self.config.max_entry_bytes {
            return Err(GfmError::Format(format!(
                "preview entry is {} bytes, above max {}",
                entry.bytes.len(),
                self.config.max_entry_bytes
            )));
        }
        if self.config.disk_enabled {
            cancellation.check()?;
            self.write_disk(&entry)?;
            cancellation.check()?;
            self.write_disk_index_entry_cancellable(&entry.key, cancellation)?;
        }
        cancellation.check()?;
        self.insert_memory(entry);
        Ok(())
    }

    pub fn get(&mut self, key: &PreviewRequestKey) -> Result<Option<CacheHit>> {
        self.get_cancellable(key, &Cancellation::default())
    }

    pub fn get_cancellable(
        &mut self,
        key: &PreviewRequestKey,
        cancellation: &Cancellation,
    ) -> Result<Option<CacheHit>> {
        cancellation.check()?;
        if let Some(entry) = self.memory.get(key).cloned() {
            self.touch(key);
            return Ok(Some(CacheHit::Memory(entry)));
        }
        if self.config.disk_enabled {
            cancellation.check()?;
            if let Some(entry) = self.read_disk(key)? {
                cancellation.check()?;
                self.insert_memory(entry.clone());
                return Ok(Some(CacheHit::Disk(entry)));
            }
        }
        Ok(None)
    }

    pub fn invalidate(&mut self, key: &PreviewRequestKey) -> Result<()> {
        self.invalidate_cancellable(key, &Cancellation::default())
    }

    pub fn invalidate_cancellable(
        &mut self,
        key: &PreviewRequestKey,
        cancellation: &Cancellation,
    ) -> Result<()> {
        cancellation.check()?;
        if let Some(entry) = self.memory.remove(key) {
            self.memory_bytes = self.memory_bytes.saturating_sub(entry.byte_len());
        }
        self.order.retain(|candidate| candidate != key);
        if self.config.disk_enabled {
            cancellation.check()?;
            let path = self.disk_path(key);
            if disk_cache_path_exists(&path)? {
                cancellation.check()?;
                fs::remove_file(&path).map_err(|err| GfmError::io(&path, err))?;
            }
            cancellation.check()?;
            self.remove_disk_index_entry_cancellable(key, cancellation)?;
        }
        Ok(())
    }

    pub fn apply_invalidation(
        &mut self,
        key: &PreviewRequestKey,
        event: PreviewInvalidationEvent,
    ) -> Result<PreviewCacheInvalidationReport> {
        self.apply_invalidation_cancellable(key, event, &Cancellation::default())
    }

    pub fn apply_invalidation_cancellable(
        &mut self,
        key: &PreviewRequestKey,
        event: PreviewInvalidationEvent,
        cancellation: &Cancellation,
    ) -> Result<PreviewCacheInvalidationReport> {
        cancellation.check()?;
        let decision = decide_invalidation(event);
        let removed_memory = if decision.invalidate_memory {
            cancellation.check()?;
            self.remove_memory(key)
        } else {
            false
        };
        let removed_disk = if decision.invalidate_disk {
            cancellation.check()?;
            self.remove_disk_cancellable(key, cancellation)?
        } else {
            false
        };

        Ok(PreviewCacheInvalidationReport {
            key: key.clone(),
            decision,
            removed_memory,
            removed_disk,
        })
    }

    pub fn disk_key_for_path_kind(
        &self,
        path: &Path,
        kind: PreviewKind,
    ) -> Option<PreviewRequestKey> {
        self.disk_index.get(&(path.to_path_buf(), kind)).cloned()
    }

    pub fn memory_bytes(&self) -> usize {
        self.memory_bytes
    }

    fn remove_memory(&mut self, key: &PreviewRequestKey) -> bool {
        let Some(entry) = self.memory.remove(key) else {
            return false;
        };
        self.memory_bytes = self.memory_bytes.saturating_sub(entry.byte_len());
        self.order.retain(|candidate| candidate != key);
        true
    }

    fn remove_disk_cancellable(
        &mut self,
        key: &PreviewRequestKey,
        cancellation: &Cancellation,
    ) -> Result<bool> {
        cancellation.check()?;
        if !self.config.disk_enabled {
            return Ok(false);
        }
        let path = self.disk_path(key);
        if !disk_cache_path_exists(&path)? {
            cancellation.check()?;
            self.remove_disk_index_entry_cancellable(key, cancellation)?;
            return Ok(false);
        }
        cancellation.check()?;
        fs::remove_file(&path).map_err(|err| GfmError::io(&path, err))?;
        cancellation.check()?;
        self.remove_disk_index_entry_cancellable(key, cancellation)?;
        Ok(true)
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

    fn write_disk_index_entry_cancellable(
        &mut self,
        key: &PreviewRequestKey,
        cancellation: &Cancellation,
    ) -> Result<()> {
        if !self.config.disk_enabled {
            return Ok(());
        }
        cancellation.check()?;
        self.disk_index
            .insert((key.path.clone(), key.kind), key.clone());
        write_disk_index_cancellable(&self.config, &self.disk_index, cancellation)
    }

    fn remove_disk_index_entry_cancellable(
        &mut self,
        key: &PreviewRequestKey,
        cancellation: &Cancellation,
    ) -> Result<()> {
        if !self.config.disk_enabled {
            return Ok(());
        }
        cancellation.check()?;
        let removed = self.disk_index.remove(&(key.path.clone(), key.kind));
        if removed.is_some() {
            write_disk_index_cancellable(&self.config, &self.disk_index, cancellation)?;
        }
        Ok(())
    }

    fn read_disk(&self, key: &PreviewRequestKey) -> Result<Option<PreviewEntry>> {
        let path = self.disk_path(key);
        if !disk_cache_file_exists(&path)? {
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

fn disk_cache_path_exists(path: &Path) -> Result<bool> {
    path.try_exists().map_err(|err| {
        GfmError::io(
            path,
            format!("preview disk cache existence unavailable: {err}"),
        )
    })
}

fn disk_cache_file_exists(path: &Path) -> Result<bool> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(metadata.is_file()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(GfmError::io(
            path,
            format!("preview disk cache metadata unavailable: {err}"),
        )),
    }
}

fn preview_cache_index_path(config: &PreviewCacheConfig) -> PathBuf {
    config.disk_root.join("preview-cache-index.tsv")
}

fn load_disk_index_cancellable(
    config: &PreviewCacheConfig,
    cancellation: &Cancellation,
) -> Result<HashMap<(PathBuf, PreviewKind), PreviewRequestKey>> {
    cancellation.check()?;
    let path = preview_cache_index_path(config);
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(err) => {
            return Err(GfmError::io(
                &path,
                format!("preview disk cache index unavailable: {err}"),
            ))
        }
    };
    let mut index = HashMap::new();
    let mut seen_path_kinds = HashSet::new();
    for (line_index, line) in contents.lines().enumerate() {
        cancellation.check()?;
        if line.is_empty() {
            continue;
        }
        let key = parse_disk_index_line(line).ok_or_else(|| {
            GfmError::Format(format!(
                "preview disk cache index corrupt at line {}",
                line_index + 1
            ))
        })?;
        let path_kind = (key.path.clone(), key.kind);
        if !seen_path_kinds.insert(path_kind.clone()) {
            return Err(GfmError::Format(format!(
                "preview disk cache index duplicate path/kind at line {}: {} {}",
                line_index + 1,
                key.kind.as_str(),
                key.path.display()
            )));
        }
        cancellation.check()?;
        if disk_cache_file_exists(&config.disk_root.join(key.stable_name()))? {
            index.insert(path_kind, key);
        }
    }
    cancellation.check()?;
    Ok(index)
}

fn write_disk_index_cancellable(
    config: &PreviewCacheConfig,
    index: &HashMap<(PathBuf, PreviewKind), PreviewRequestKey>,
    cancellation: &Cancellation,
) -> Result<()> {
    cancellation.check()?;
    let path = preview_cache_index_path(config);
    let tmp = path.with_extension("tmp");
    let mut lines = Vec::new();
    for key in index.values() {
        cancellation.check()?;
        lines.push(format_disk_index_line(key));
    }
    lines.sort();
    cancellation.check()?;
    fs::write(&tmp, lines.join("\n")).map_err(|err| GfmError::io(&tmp, err))?;
    cancellation.check()?;
    fs::rename(&tmp, &path).map_err(|err| GfmError::io(&path, err))
}

fn format_disk_index_line(key: &PreviewRequestKey) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}",
        key.kind.as_str(),
        key.file_id.volume.0,
        key.file_id.node,
        key.pixel_size,
        key.scale_factor_milli,
        key.content_epoch,
        hex_encode_path(&key.path)
    )
}

fn parse_disk_index_line(line: &str) -> Option<PreviewRequestKey> {
    let mut parts = line.split('\t');
    let kind = PreviewKind::parse(parts.next()?)?;
    let volume = parts.next()?.parse().ok()?;
    let node = parts.next()?.parse().ok()?;
    let pixel_size = parts.next()?.parse().ok()?;
    let scale_factor_milli = parts.next()?.parse().ok()?;
    let content_epoch = parts.next()?.parse().ok()?;
    let path = hex_decode_path(parts.next()?)?;
    if parts.next().is_some() {
        return None;
    }
    Some(PreviewRequestKey {
        file_id: FileId::new(VolumeId(volume), node),
        path,
        kind,
        pixel_size,
        scale_factor_milli,
        content_epoch,
    })
}

fn hex_encode_path(path: &Path) -> String {
    path.as_os_str()
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn hex_decode_path(value: &str) -> Option<PathBuf> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for chunk in value.as_bytes().chunks_exact(2) {
        let hex = std::str::from_utf8(chunk).ok()?;
        bytes.push(u8::from_str_radix(hex, 16).ok()?);
    }
    Some(PathBuf::from(OsString::from_vec(bytes)))
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
        gfm_mac::CloudStorageState::Unknown | gfm_mac::CloudStorageState::Removed => {
            gfm_mac::CloudMaterialization::Unknown
        }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewCacheInvalidationReport {
    pub key: PreviewRequestKey,
    pub decision: PreviewInvalidationDecision,
    pub removed_memory: bool,
    pub removed_disk: bool,
}

impl PreviewCacheInvalidationReport {
    pub fn as_tsv(&self) -> String {
        format!(
            "preview-cache-invalidation\t{}\tkind={}\treason={}\tinvalidate-memory={}\tinvalidate-disk={}\tremoved-memory={}\tremoved-disk={}",
            self.key.path.display(),
            self.key.kind.as_str(),
            self.decision.reason,
            self.decision.invalidate_memory,
            self.decision.invalidate_disk,
            self.removed_memory,
            self.removed_disk
        )
    }
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
        removed: report.current.storage_state == gfm_mac::CloudStorageState::Removed,
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
    fn cancelled_cache_insert_does_not_touch_memory_or_disk() {
        let root = temp_root("cancel-insert");
        let mut cache = PreviewCache::new(PreviewCacheConfig {
            memory_budget_bytes: 16,
            max_entry_bytes: 16,
            disk_root: root.clone(),
            disk_enabled: true,
        })
        .unwrap();
        let key = key("cancelled.png", PreviewKind::Thumbnail);
        let disk_path = cache.disk_path(&key);
        let cancellation = Cancellation::default();
        cancellation.cancel();

        let result = cache.insert_cancellable(
            PreviewEntry::new(key.clone(), b"cancelled".to_vec()),
            &cancellation,
        );

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert_eq!(cache.memory_bytes(), 0);
        assert!(!disk_path.exists());
        assert!(!preview_cache_index_path(&PreviewCacheConfig::new(&root)).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancelled_cache_get_does_not_promote_disk_entry_to_memory() {
        let root = temp_root("cancel-get");
        let key = key("disk.png", PreviewKind::Thumbnail);
        {
            let mut cache = PreviewCache::new(PreviewCacheConfig {
                memory_budget_bytes: 16,
                max_entry_bytes: 16,
                disk_root: root.clone(),
                disk_enabled: true,
            })
            .unwrap();
            cache
                .insert(PreviewEntry::new(key.clone(), b"disk".to_vec()))
                .unwrap();
        }
        let mut reloaded = PreviewCache::new(PreviewCacheConfig {
            memory_budget_bytes: 16,
            max_entry_bytes: 16,
            disk_root: root.clone(),
            disk_enabled: true,
        })
        .unwrap();
        let cancellation = Cancellation::default();
        cancellation.cancel();

        let result = reloaded.get_cancellable(&key, &cancellation);

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert_eq!(reloaded.memory_bytes(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancelled_cache_invalidation_keeps_existing_entry() {
        let root = temp_root("cancel-invalidation");
        let key = key("keep.png", PreviewKind::Thumbnail);
        let mut cache = PreviewCache::new(PreviewCacheConfig {
            memory_budget_bytes: 16,
            max_entry_bytes: 16,
            disk_root: root.clone(),
            disk_enabled: true,
        })
        .unwrap();
        cache
            .insert(PreviewEntry::new(key.clone(), b"keep".to_vec()))
            .unwrap();
        let disk_path = cache.disk_path(&key);
        let cancellation = Cancellation::default();
        cancellation.cancel();

        let result = cache.apply_invalidation_cancellable(
            &key,
            PreviewInvalidationEvent {
                icloud_state_changed: true,
                ..PreviewInvalidationEvent::default()
            },
            &cancellation,
        );

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert_eq!(cache.memory_bytes(), 4);
        assert!(disk_path.exists());
        assert!(cache.disk_key_for_path_kind(&key.path, key.kind).is_some());
        fs::remove_dir_all(root).unwrap();
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
    fn cache_metadata_invalidation_removes_memory_without_dropping_disk() {
        let root = temp_root("metadata-invalidation");
        let key = key("metadata.png", PreviewKind::Thumbnail);
        let mut cache = PreviewCache::new(PreviewCacheConfig {
            memory_budget_bytes: 16,
            max_entry_bytes: 16,
            disk_root: root,
            disk_enabled: true,
        })
        .unwrap();
        cache
            .insert(PreviewEntry::new(key.clone(), b"metadata".to_vec()))
            .unwrap();

        let report = cache
            .apply_invalidation(
                &key,
                PreviewInvalidationEvent {
                    metadata_changed: true,
                    ..PreviewInvalidationEvent::default()
                },
            )
            .unwrap();

        assert!(report.removed_memory);
        assert!(!report.removed_disk);
        assert_eq!(report.decision.reason, "metadata-or-tags");
        assert_eq!(cache.memory_bytes(), 0);
        assert!(matches!(
            cache.get(&key).unwrap(),
            Some(CacheHit::Disk(entry)) if entry.bytes == b"metadata"
        ));
    }

    #[test]
    fn cache_icloud_invalidation_removes_memory_and_disk() {
        let root = temp_root("icloud-invalidation");
        let key = key("remote.icloud", PreviewKind::QuickLook);
        let mut cache = PreviewCache::new(PreviewCacheConfig {
            memory_budget_bytes: 16,
            max_entry_bytes: 16,
            disk_root: root,
            disk_enabled: true,
        })
        .unwrap();
        cache
            .insert(PreviewEntry::new(key.clone(), b"cloud".to_vec()))
            .unwrap();

        let report = cache
            .apply_invalidation(
                &key,
                PreviewInvalidationEvent {
                    icloud_state_changed: true,
                    ..PreviewInvalidationEvent::default()
                },
            )
            .unwrap();

        assert!(report.removed_memory);
        assert!(report.removed_disk);
        assert_eq!(report.decision.reason, "content-or-icloud");
        assert_eq!(cache.get(&key).unwrap(), None);
    }

    #[test]
    fn cache_disk_probe_failures_surface_as_unavailable_io() {
        let root = temp_root("disk-probe-unavailable");
        let path = root.join("preview-disk-cache-unavailable".repeat(64));

        let err = disk_cache_path_exists(&path).unwrap_err();

        assert!(err
            .to_string()
            .contains("preview disk cache existence unavailable"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cache_disk_metadata_failures_surface_as_unavailable_io() {
        let root = temp_root("disk-metadata-unavailable");
        let path = root.join("preview-disk-cache-metadata-unavailable".repeat(64));

        let err = disk_cache_file_exists(&path).unwrap_err();

        assert!(err
            .to_string()
            .contains("preview disk cache metadata unavailable"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cache_allows_missing_disk_index_as_empty() {
        let root = temp_root("missing-disk-index");
        let cache = PreviewCache::new(PreviewCacheConfig {
            memory_budget_bytes: 16,
            max_entry_bytes: 16,
            disk_root: root.clone(),
            disk_enabled: true,
        })
        .unwrap();

        assert_eq!(
            cache.disk_key_for_path_kind(Path::new("missing.png"), PreviewKind::Thumbnail),
            None
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cache_open_honors_pre_cancelled_token_before_directory_creation() {
        let root = std::env::temp_dir().join(format!(
            "gfm-preview-cancelled-open-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let cancellation = Cancellation::default();
        cancellation.cancel();

        let result = PreviewCache::new_cancellable(
            PreviewCacheConfig {
                memory_budget_bytes: 16,
                max_entry_bytes: 16,
                disk_root: root.clone(),
                disk_enabled: true,
            },
            &cancellation,
        )
        .map(|_| ());

        assert_eq!(result, Err(GfmError::Cancelled));
        assert!(!root.exists());
    }

    #[test]
    fn cache_open_honors_pre_cancelled_token_when_disk_is_disabled() {
        let root = std::env::temp_dir().join(format!(
            "gfm-preview-cancelled-memory-open-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let cancellation = Cancellation::default();
        cancellation.cancel();

        let result = PreviewCache::new_cancellable(
            PreviewCacheConfig {
                memory_budget_bytes: 16,
                max_entry_bytes: 16,
                disk_root: root.clone(),
                disk_enabled: false,
            },
            &cancellation,
        )
        .map(|_| ());

        assert_eq!(result, Err(GfmError::Cancelled));
        assert!(!root.exists());
    }

    #[test]
    fn cache_disk_index_read_failures_surface_as_unavailable_io() {
        let root = temp_root("disk-index-unavailable");
        fs::create_dir_all(preview_cache_index_path(&PreviewCacheConfig::new(&root))).unwrap();

        let err = match PreviewCache::new(PreviewCacheConfig {
            memory_budget_bytes: 16,
            max_entry_bytes: 16,
            disk_root: root.clone(),
            disk_enabled: true,
        }) {
            Ok(_) => panic!("preview cache should reject an unreadable disk index"),
            Err(err) => err,
        };

        assert!(err
            .to_string()
            .contains("preview disk cache index unavailable"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cache_disk_index_corruption_surfaces_line_number() {
        let root = temp_root("disk-index-corrupt");
        fs::write(
            preview_cache_index_path(&PreviewCacheConfig::new(&root)),
            "thumbnail\t1\t2\t256\t2000\t0\t72656d6f74652e69636c6f7564\nnot-a-valid-index-row\n",
        )
        .unwrap();

        let err = match PreviewCache::new(PreviewCacheConfig {
            memory_budget_bytes: 16,
            max_entry_bytes: 16,
            disk_root: root.clone(),
            disk_enabled: true,
        }) {
            Ok(_) => panic!("preview cache should reject a corrupt disk index"),
            Err(err) => err,
        };

        assert_eq!(
            err.to_string(),
            "preview disk cache index corrupt at line 2"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cache_disk_index_duplicate_path_kind_surfaces_line_number() {
        let root = temp_root("disk-index-duplicate");
        let path_hex = "72656d6f74652e69636c6f7564";
        fs::write(
            preview_cache_index_path(&PreviewCacheConfig::new(&root)),
            format!(
                "thumbnail\t1\t2\t256\t2000\t0\t{path_hex}\nthumbnail\t1\t3\t512\t2000\t1\t{path_hex}\n"
            ),
        )
        .unwrap();

        let err = match PreviewCache::new(PreviewCacheConfig {
            memory_budget_bytes: 16,
            max_entry_bytes: 16,
            disk_root: root.clone(),
            disk_enabled: true,
        }) {
            Ok(_) => panic!("preview cache should reject a duplicate disk index path/kind"),
            Err(err) => err,
        };

        assert_eq!(
            err.to_string(),
            "preview disk cache index duplicate path/kind at line 2: thumbnail remote.icloud"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reloaded_cache_resolves_disk_key_for_fileprovider_invalidation() {
        let root = temp_root("indexed-icloud-invalidation");
        let key = key("remote.icloud", PreviewKind::Thumbnail);
        let mut cache = PreviewCache::new(PreviewCacheConfig {
            memory_budget_bytes: 16,
            max_entry_bytes: 16,
            disk_root: root.clone(),
            disk_enabled: true,
        })
        .unwrap();
        cache
            .insert(PreviewEntry::new(key.clone(), b"cloud".to_vec()))
            .unwrap();

        let mut reloaded = PreviewCache::new(PreviewCacheConfig {
            memory_budget_bytes: 16,
            max_entry_bytes: 16,
            disk_root: root,
            disk_enabled: true,
        })
        .unwrap();
        let resolved = reloaded
            .disk_key_for_path_kind(&key.path, key.kind)
            .expect("disk index should retain the preview request key");

        let report = reloaded
            .apply_invalidation(
                &resolved,
                PreviewInvalidationEvent {
                    icloud_state_changed: true,
                    ..PreviewInvalidationEvent::default()
                },
            )
            .unwrap();

        assert!(!report.removed_memory);
        assert!(report.removed_disk);
        assert_eq!(reloaded.disk_key_for_path_kind(&key.path, key.kind), None);
        assert_eq!(reloaded.get(&key).unwrap(), None);
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

    #[test]
    fn removed_fileprovider_items_map_to_removed_preview_invalidation() {
        let report = fileprovider_report(
            gfm_mac::CloudStorageState::Downloaded,
            gfm_mac::CloudStorageState::Removed,
            true,
        );

        let event = preview_invalidation_for_fileprovider(&report);
        let decision = decide_invalidation(event);

        assert!(event.removed);
        assert!(event.icloud_state_changed);
        assert_eq!(
            decision,
            PreviewInvalidationDecision {
                invalidate_memory: true,
                invalidate_disk: true,
                reason: "removed"
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
                materialization_confidence: gfm_mac::CloudMaterializationConfidence::Native,
                materialization_reason: Some("test".to_string()),
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
