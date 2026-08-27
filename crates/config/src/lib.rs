use gfm_types::{GfmError, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const CURRENT_SCHEMA_VERSION: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GfmConfig {
    pub schema_version: u32,
    pub parity: ParityConfig,
    pub settings: UserSettings,
    pub features: FeatureFlags,
    pub diagnostics: DiagnosticsConfig,
    pub performance: PerformanceControls,
}

impl Default for GfmConfig {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            parity: ParityConfig::default(),
            settings: UserSettings::default(),
            features: FeatureFlags::default(),
            diagnostics: DiagnosticsConfig::default(),
            performance: PerformanceControls::default(),
        }
    }
}

impl GfmConfig {
    pub fn parse(input: &str) -> Result<Self> {
        let mut value: toml::Value = toml::from_str(input)
            .map_err(|err| GfmError::Format(format!("invalid GFM config TOML: {err}")))?;
        let version = value
            .get("schema_version")
            .and_then(toml::Value::as_integer)
            .unwrap_or(1);
        match version {
            1 | 2 => migrate_legacy_to_current(&mut value)?,
            version if version == i64::from(CURRENT_SCHEMA_VERSION) => {}
            other => {
                return Err(GfmError::Format(format!(
                    "unsupported GFM config schema version {other}"
                )));
            }
        }

        let config: Self = value.try_into().map_err(|err| {
            GfmError::Format(format!("invalid GFM config schema after migration: {err}"))
        })?;
        config.validate()?;
        Ok(config)
    }

    pub fn to_toml(&self) -> Result<String> {
        self.validate()?;
        toml::to_string_pretty(self)
            .map_err(|err| GfmError::Format(format!("failed to encode GFM config: {err}")))
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != CURRENT_SCHEMA_VERSION {
            return Err(GfmError::Format(format!(
                "config schema version must be {CURRENT_SCHEMA_VERSION}, got {}",
                self.schema_version
            )));
        }
        self.parity.validate()?;
        self.settings.validate()?;
        self.diagnostics.validate()?;
        self.performance.validate()?;
        Ok(())
    }

    pub fn effective_performance_policy(&self) -> RuntimePerformancePolicy {
        if self.features.internal_power_mode && self.performance.enabled {
            self.performance.policy()
        } else {
            RuntimePerformancePolicy::finder_parity()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ParityConfig {
    pub profile: ParityProfile,
    pub baseline_root: PathBuf,
    pub allowed_dynamic_masks: Vec<String>,
}

impl Default for ParityConfig {
    fn default() -> Self {
        Self {
            profile: ParityProfile::default(),
            baseline_root: PathBuf::from("tests/parity/baselines"),
            allowed_dynamic_masks: Vec::new(),
        }
    }
}

impl ParityConfig {
    fn validate(&self) -> Result<()> {
        self.profile.validate()?;
        if self.baseline_root.as_os_str().is_empty() {
            return Err(GfmError::Format(
                "parity baseline_root must not be empty".to_string(),
            ));
        }
        for mask in &self.allowed_dynamic_masks {
            if mask.trim().is_empty() {
                return Err(GfmError::Format(
                    "parity dynamic masks must not be empty".to_string(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ParityProfile {
    pub macos_build: String,
    pub appearance: Appearance,
    pub scale_factor: ScaleFactor,
    pub reduce_motion: bool,
    pub reduce_transparency: bool,
    pub increase_contrast: bool,
}

impl Default for ParityProfile {
    fn default() -> Self {
        Self {
            macos_build: "current".to_string(),
            appearance: Appearance::System,
            scale_factor: ScaleFactor::Two,
            reduce_motion: false,
            reduce_transparency: false,
            increase_contrast: false,
        }
    }
}

impl ParityProfile {
    fn validate(&self) -> Result<()> {
        if self.macos_build.trim().is_empty() {
            return Err(GfmError::Format(
                "parity macos_build must not be empty".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Appearance {
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScaleFactor {
    One,
    Two,
    Three,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UserSettings {
    pub show_hidden_files: bool,
    pub preserve_finder_default_surface: bool,
    pub default_view: ViewMode,
    pub machine_search: MachineSearchPolicy,
    pub external_volume_indexing: VolumeIndexingPolicy,
    pub network_volume_indexing: VolumeIndexingPolicy,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            show_hidden_files: false,
            preserve_finder_default_surface: true,
            default_view: ViewMode::Icon,
            machine_search: MachineSearchPolicy::default(),
            external_volume_indexing: VolumeIndexingPolicy::OptIn,
            network_volume_indexing: VolumeIndexingPolicy::OptIn,
        }
    }
}

impl UserSettings {
    fn validate(&self) -> Result<()> {
        self.machine_search.validate()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ViewMode {
    Icon,
    List,
    Column,
    Gallery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VolumeIndexingPolicy {
    Disabled,
    OptIn,
    Enabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MachineSearchPolicy {
    pub enabled: bool,
    pub index_home: bool,
    pub index_applications: bool,
    pub index_developer_paths: bool,
    pub excluded_paths: Vec<PathBuf>,
}

impl Default for MachineSearchPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            index_home: true,
            index_applications: true,
            index_developer_paths: true,
            excluded_paths: vec![
                PathBuf::from("~/Library/Caches"),
                PathBuf::from("~/Library/Developer/Xcode/DerivedData"),
            ],
        }
    }
}

impl MachineSearchPolicy {
    fn validate(&self) -> Result<()> {
        for path in &self.excluded_paths {
            if path.as_os_str().is_empty() {
                return Err(GfmError::Format(
                    "machine search excluded_paths must not contain empty paths".to_string(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FeatureFlags {
    pub strict_finder_parity: bool,
    pub machine_search: bool,
    pub content_indexing: bool,
    pub background_indexing: bool,
    pub native_file_operations: bool,
    pub preview_cache: bool,
    pub internal_power_mode: bool,
}

impl Default for FeatureFlags {
    fn default() -> Self {
        Self {
            strict_finder_parity: true,
            machine_search: true,
            content_indexing: true,
            background_indexing: true,
            native_file_operations: true,
            preview_cache: true,
            internal_power_mode: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DiagnosticsConfig {
    pub enabled: bool,
    pub local_export_dir: PathBuf,
    pub include_paths: bool,
    pub include_query_text: bool,
    pub latency_histograms: bool,
    pub frame_timing: bool,
    pub io_counters: bool,
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            local_export_dir: PathBuf::from("~/Library/Application Support/GFM/Diagnostics"),
            include_paths: false,
            include_query_text: false,
            latency_histograms: true,
            frame_timing: true,
            io_counters: true,
        }
    }
}

impl DiagnosticsConfig {
    fn validate(&self) -> Result<()> {
        if self.enabled && self.local_export_dir.as_os_str().is_empty() {
            return Err(GfmError::Format(
                "diagnostics local_export_dir must not be empty when diagnostics are enabled"
                    .to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PerformanceControls {
    pub enabled: bool,
    pub profile: PerformanceProfile,
    pub max_background_index_threads: u16,
    pub max_extractor_threads: u16,
    pub max_thumbnail_threads: u16,
    pub max_io_mib_per_second: u16,
    pub search_keystroke_budget_ms: u16,
    pub visible_directory_budget_ms: u16,
    pub enable_aggressive_prefetch: bool,
    pub enable_mmap_read_ahead: bool,
}

impl Default for PerformanceControls {
    fn default() -> Self {
        Self {
            enabled: false,
            profile: PerformanceProfile::Balanced,
            max_background_index_threads: 2,
            max_extractor_threads: 2,
            max_thumbnail_threads: 2,
            max_io_mib_per_second: 256,
            search_keystroke_budget_ms: 30,
            visible_directory_budget_ms: 50,
            enable_aggressive_prefetch: false,
            enable_mmap_read_ahead: true,
        }
    }
}

impl PerformanceControls {
    fn validate(&self) -> Result<()> {
        validate_range(
            "performance max_background_index_threads",
            self.max_background_index_threads,
            1,
            64,
        )?;
        validate_range(
            "performance max_extractor_threads",
            self.max_extractor_threads,
            1,
            64,
        )?;
        validate_range(
            "performance max_thumbnail_threads",
            self.max_thumbnail_threads,
            1,
            64,
        )?;
        validate_range(
            "performance max_io_mib_per_second",
            self.max_io_mib_per_second,
            1,
            16_384,
        )?;
        validate_range(
            "performance search_keystroke_budget_ms",
            self.search_keystroke_budget_ms,
            1,
            1_000,
        )?;
        validate_range(
            "performance visible_directory_budget_ms",
            self.visible_directory_budget_ms,
            1,
            5_000,
        )?;
        Ok(())
    }

    fn policy(&self) -> RuntimePerformancePolicy {
        let mut policy = RuntimePerformancePolicy {
            profile: self.profile,
            max_background_index_threads: self.max_background_index_threads,
            max_extractor_threads: self.max_extractor_threads,
            max_thumbnail_threads: self.max_thumbnail_threads,
            max_io_bytes_per_second: u64::from(self.max_io_mib_per_second) * 1024 * 1024,
            search_keystroke_budget: std::time::Duration::from_millis(u64::from(
                self.search_keystroke_budget_ms,
            )),
            visible_directory_budget: std::time::Duration::from_millis(u64::from(
                self.visible_directory_budget_ms,
            )),
            aggressive_prefetch: self.enable_aggressive_prefetch,
            mmap_read_ahead: self.enable_mmap_read_ahead,
        };
        match self.profile {
            PerformanceProfile::Conservative => {
                policy.max_background_index_threads = policy.max_background_index_threads.min(1);
                policy.max_extractor_threads = policy.max_extractor_threads.min(1);
                policy.max_thumbnail_threads = policy.max_thumbnail_threads.min(1);
                policy.aggressive_prefetch = false;
            }
            PerformanceProfile::Balanced => {}
            PerformanceProfile::Aggressive => {
                policy.aggressive_prefetch = true;
            }
            PerformanceProfile::Benchmark => {
                policy.aggressive_prefetch = true;
                policy.mmap_read_ahead = true;
            }
        }
        policy
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PerformanceProfile {
    Conservative,
    Balanced,
    Aggressive,
    Benchmark,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePerformancePolicy {
    pub profile: PerformanceProfile,
    pub max_background_index_threads: u16,
    pub max_extractor_threads: u16,
    pub max_thumbnail_threads: u16,
    pub max_io_bytes_per_second: u64,
    pub search_keystroke_budget: std::time::Duration,
    pub visible_directory_budget: std::time::Duration,
    pub aggressive_prefetch: bool,
    pub mmap_read_ahead: bool,
}

impl RuntimePerformancePolicy {
    pub fn finder_parity() -> Self {
        let controls = PerformanceControls::default();
        controls.policy()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn platform_default() -> Result<Self> {
        Ok(Self::new(default_config_path()?))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<GfmConfig> {
        let mut input = String::new();
        File::open(&self.path)
            .map_err(|err| GfmError::io(&self.path, err))?
            .read_to_string(&mut input)
            .map_err(|err| GfmError::io(&self.path, err))?;
        GfmConfig::parse(&input)
    }

    pub fn load_or_create_default(&self) -> Result<GfmConfig> {
        match self.load() {
            Ok(config) => Ok(config),
            Err(GfmError::Io { .. }) if !self.path.exists() => {
                let config = GfmConfig::default();
                self.save(&config)?;
                Ok(config)
            }
            Err(err) => Err(err),
        }
    }

    pub fn save(&self, config: &GfmConfig) -> Result<()> {
        let encoded = config.to_toml()?;
        if let Some(parent) = real_parent(&self.path) {
            fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
        }

        let temp_path = self.temp_path();
        {
            let mut file = File::create(&temp_path).map_err(|err| GfmError::io(&temp_path, err))?;
            file.write_all(encoded.as_bytes())
                .map_err(|err| GfmError::io(&temp_path, err))?;
            file.sync_all()
                .map_err(|err| GfmError::io(&temp_path, err))?;
        }
        fs::rename(&temp_path, &self.path).map_err(|err| GfmError::io(&self.path, err))?;
        sync_parent(&self.path)
    }

    fn temp_path(&self) -> PathBuf {
        let mut temp = self.path.clone();
        let extension = temp
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| format!("{extension}.tmp"))
            .unwrap_or_else(|| "tmp".to_string());
        temp.set_extension(extension);
        temp
    }
}

fn default_config_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| {
        GfmError::Format("HOME is required to resolve the GFM config path".to_string())
    })?;
    Ok(PathBuf::from(home)
        .join("Library")
        .join("Application Support")
        .join("GFM")
        .join("config.toml"))
}

fn sync_parent(path: &Path) -> Result<()> {
    let parent = real_parent(path).unwrap_or_else(|| Path::new("."));
    File::open(parent)
        .and_then(|file| file.sync_all())
        .map_err(|err| GfmError::io(parent, err))?;
    Ok(())
}

fn real_parent(path: &Path) -> Option<&Path> {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
}

fn migrate_legacy_to_current(value: &mut toml::Value) -> Result<()> {
    let table = value
        .as_table_mut()
        .ok_or_else(|| GfmError::Format("GFM config root must be a TOML table".to_string()))?;
    table.insert(
        "schema_version".to_string(),
        toml::Value::Integer(i64::from(CURRENT_SCHEMA_VERSION)),
    );

    ensure_table(table, "parity")?;
    ensure_table(table, "settings")?;
    ensure_table(table, "features")?;
    ensure_table(table, "diagnostics")?;
    ensure_table(table, "performance")?;
    Ok(())
}

fn ensure_table(table: &mut toml::map::Map<String, toml::Value>, key: &str) -> Result<()> {
    match table.get(key) {
        Some(value) if value.is_table() => Ok(()),
        Some(_) => Err(GfmError::Format(format!(
            "config section `{key}` must be a table"
        ))),
        None => {
            table.insert(key.to_string(), toml::Value::Table(toml::map::Map::new()));
            Ok(())
        }
    }
}

fn validate_range(label: &str, value: u16, min: u16, max: u16) -> Result<()> {
    if value < min || value > max {
        Err(GfmError::Format(format!(
            "{label} must be between {min} and {max}, got {value}"
        )))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn default_config_round_trips() {
        let encoded = GfmConfig::default().to_toml().unwrap();
        let parsed = GfmConfig::parse(&encoded).unwrap();

        assert_eq!(parsed, GfmConfig::default());
        assert!(encoded.contains("schema_version = 3"));
        assert!(encoded.contains("strict_finder_parity = true"));
        assert!(encoded.contains("[performance]"));
        assert!(encoded.contains("enabled = false"));
    }

    #[test]
    fn migrates_legacy_config_with_partial_sections() {
        let parsed = GfmConfig::parse(
            r#"
schema_version = 1

[settings]
show_hidden_files = true
default_view = "list"
"#,
        )
        .unwrap();

        assert_eq!(parsed.schema_version, CURRENT_SCHEMA_VERSION);
        assert!(parsed.settings.show_hidden_files);
        assert_eq!(parsed.settings.default_view, ViewMode::List);
        assert!(parsed.features.strict_finder_parity);
        assert_eq!(parsed.performance, PerformanceControls::default());
    }

    #[test]
    fn migrates_v2_config_to_performance_controls() {
        let parsed = GfmConfig::parse(
            r#"
schema_version = 2

[features]
internal_power_mode = true
"#,
        )
        .unwrap();

        assert_eq!(parsed.schema_version, CURRENT_SCHEMA_VERSION);
        assert!(parsed.features.internal_power_mode);
        assert!(!parsed.performance.enabled);
    }

    #[test]
    fn rejects_unknown_keys() {
        let err = GfmConfig::parse(
            r#"
schema_version = 2
unexpected = true
"#,
        )
        .unwrap_err();

        assert!(err.to_string().contains("unexpected"));
    }

    #[test]
    fn rejects_invalid_paths() {
        let mut config = GfmConfig::default();
        config
            .settings
            .machine_search
            .excluded_paths
            .push(PathBuf::new());

        let err = config.to_toml().unwrap_err();

        assert!(err.to_string().contains("excluded_paths"));
    }

    #[test]
    fn store_creates_and_loads_default_atomically() {
        let root = unique_temp_dir("gfm-config-store");
        let store = ConfigStore::new(root.join("nested").join("config.toml"));

        let created = store.load_or_create_default().unwrap();
        let loaded = store.load().unwrap();

        assert_eq!(created, GfmConfig::default());
        assert_eq!(loaded, GfmConfig::default());
        assert!(store.path().exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn store_persists_modified_config() {
        let root = unique_temp_dir("gfm-config-save");
        let store = ConfigStore::new(root.join("config.toml"));
        let mut config = GfmConfig::default();
        config.parity.profile.macos_build = "25A354".to_string();
        config.features.internal_power_mode = true;
        config.performance.enabled = true;
        config.performance.profile = PerformanceProfile::Aggressive;

        store.save(&config).unwrap();
        let loaded = store.load().unwrap();

        assert_eq!(loaded.parity.profile.macos_build, "25A354");
        assert!(loaded.features.internal_power_mode);
        assert!(loaded.performance.enabled);
        assert_eq!(loaded.performance.profile, PerformanceProfile::Aggressive);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn store_saves_relative_leaf_path_in_current_directory() {
        let root = unique_temp_dir("gfm-config-relative-save");
        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(&root).unwrap();
        let store = ConfigStore::new("config.toml");
        let config = GfmConfig::default();

        store.save(&config).unwrap();
        let loaded = store.load().unwrap();

        assert_eq!(loaded, config);
        assert!(root.join("config.toml").exists());
        std::env::set_current_dir(previous).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn internal_performance_controls_are_hidden_until_power_mode_is_enabled() {
        let mut config = GfmConfig::default();
        config.performance.enabled = true;
        config.performance.profile = PerformanceProfile::Benchmark;
        config.performance.max_background_index_threads = 16;
        let hidden = config.effective_performance_policy();

        assert_eq!(hidden.profile, PerformanceProfile::Balanced);
        assert_eq!(hidden.max_background_index_threads, 2);

        config.features.internal_power_mode = true;
        let active = config.effective_performance_policy();

        assert_eq!(active.profile, PerformanceProfile::Benchmark);
        assert_eq!(active.max_background_index_threads, 16);
        assert!(active.aggressive_prefetch);
    }

    #[test]
    fn rejects_invalid_internal_performance_controls() {
        let mut config = GfmConfig::default();
        config.performance.max_extractor_threads = 0;

        let err = config.to_toml().unwrap_err();

        assert!(err.to_string().contains("max_extractor_threads"));
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
