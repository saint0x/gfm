use std::collections::BTreeSet;

const TEXT_EXTENSION_LIST: &[&str] = &[
    "bash", "c", "cc", "conf", "cpp", "css", "csv", "go", "h", "hpp", "html", "java", "js", "json",
    "jsx", "log", "md", "mjs", "plist", "py", "rb", "rs", "sh", "sql", "swift", "toml", "ts",
    "tsx", "txt", "xml", "yaml", "yml", "zsh",
];

#[derive(Debug, Clone)]
pub struct ExtractionPolicy {
    pub max_bytes: u64,
    pub max_text_bytes: usize,
    pub max_pdf_bytes: u64,
    pub max_pdf_pages: usize,
    pub max_pdf_objects: usize,
    pub max_pdf_stream_bytes: usize,
    pub max_office_bytes: u64,
    pub max_office_entries: usize,
    pub max_office_entry_bytes: u64,
    pub max_office_text_bytes: usize,
    pub max_archive_bytes: u64,
    pub max_archive_entries: usize,
    pub max_archive_text_bytes: usize,
    pub max_rich_text_bytes: usize,
    pub max_structured_text_bytes: usize,
    pub extensions: BTreeSet<String>,
}

impl ExtractionPolicy {
    pub(crate) fn scaled(&self, percent: u64) -> Self {
        let percent = percent.clamp(1, 100);
        Self {
            max_bytes: scale_u64(self.max_bytes, percent).max(16 * 1024),
            max_text_bytes: scale_usize(self.max_text_bytes, percent).max(16 * 1024),
            max_pdf_bytes: scale_u64(self.max_pdf_bytes, percent).max(256 * 1024),
            max_pdf_pages: scale_usize(self.max_pdf_pages, percent).max(1),
            max_pdf_objects: scale_usize(self.max_pdf_objects, percent).max(128),
            max_pdf_stream_bytes: scale_usize(self.max_pdf_stream_bytes, percent).max(128 * 1024),
            max_office_bytes: scale_u64(self.max_office_bytes, percent).max(512 * 1024),
            max_office_entries: scale_usize(self.max_office_entries, percent).max(64),
            max_office_entry_bytes: scale_u64(self.max_office_entry_bytes, percent).max(128 * 1024),
            max_office_text_bytes: scale_usize(self.max_office_text_bytes, percent).max(64 * 1024),
            max_archive_bytes: scale_u64(self.max_archive_bytes, percent).max(512 * 1024),
            max_archive_entries: scale_usize(self.max_archive_entries, percent).max(64),
            max_archive_text_bytes: scale_usize(self.max_archive_text_bytes, percent)
                .max(64 * 1024),
            max_rich_text_bytes: scale_usize(self.max_rich_text_bytes, percent).max(64 * 1024),
            max_structured_text_bytes: scale_usize(self.max_structured_text_bytes, percent)
                .max(64 * 1024),
            extensions: self.extensions.clone(),
        }
    }
}

impl Default for ExtractionPolicy {
    fn default() -> Self {
        Self {
            max_bytes: 2 * 1024 * 1024,
            max_text_bytes: 2 * 1024 * 1024,
            max_pdf_bytes: 16 * 1024 * 1024,
            max_pdf_pages: 256,
            max_pdf_objects: 20_000,
            max_pdf_stream_bytes: 8 * 1024 * 1024,
            max_office_bytes: 32 * 1024 * 1024,
            max_office_entries: 10_000,
            max_office_entry_bytes: 8 * 1024 * 1024,
            max_office_text_bytes: 4 * 1024 * 1024,
            max_archive_bytes: 64 * 1024 * 1024,
            max_archive_entries: 20_000,
            max_archive_text_bytes: 2 * 1024 * 1024,
            max_rich_text_bytes: 2 * 1024 * 1024,
            max_structured_text_bytes: 4 * 1024 * 1024,
            extensions: text_extensions(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionVolumeClass {
    Local,
    External,
    Network,
    Cloud,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionThermalState {
    Nominal,
    Fair,
    Serious,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionBatteryState {
    AcPower,
    Battery,
    LowPower,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionUserActivity {
    Idle,
    Active,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtractionBudgetProfile {
    pub volume: ExtractionVolumeClass,
    pub thermal: ExtractionThermalState,
    pub battery: ExtractionBatteryState,
    pub user_activity: ExtractionUserActivity,
}

impl Default for ExtractionBudgetProfile {
    fn default() -> Self {
        Self {
            volume: ExtractionVolumeClass::Local,
            thermal: ExtractionThermalState::Nominal,
            battery: ExtractionBatteryState::AcPower,
            user_activity: ExtractionUserActivity::Idle,
        }
    }
}

impl ExtractionBudgetProfile {
    pub fn policy(self) -> ExtractionPolicy {
        self.policy_from(ExtractionPolicy::default())
    }

    pub fn policy_from(self, base: ExtractionPolicy) -> ExtractionPolicy {
        base.scaled(self.scale_percent())
    }

    pub const fn scale_percent(self) -> u64 {
        let mut percent = match self.volume {
            ExtractionVolumeClass::Local => 100,
            ExtractionVolumeClass::External => 80,
            ExtractionVolumeClass::Cloud => 60,
            ExtractionVolumeClass::Network => 50,
        };
        percent = min_percent(
            percent,
            match self.thermal {
                ExtractionThermalState::Nominal => 100,
                ExtractionThermalState::Fair => 80,
                ExtractionThermalState::Serious => 50,
                ExtractionThermalState::Critical => 25,
            },
        );
        percent = min_percent(
            percent,
            match self.battery {
                ExtractionBatteryState::AcPower => 100,
                ExtractionBatteryState::Battery => 80,
                ExtractionBatteryState::LowPower => 50,
            },
        );
        min_percent(
            percent,
            match self.user_activity {
                ExtractionUserActivity::Idle => 100,
                ExtractionUserActivity::Active => 60,
            },
        )
    }
}

pub(crate) fn text_extension_is_known(extension: &str) -> bool {
    TEXT_EXTENSION_LIST
        .iter()
        .any(|candidate| extension.eq_ignore_ascii_case(candidate))
}

fn text_extensions() -> BTreeSet<String> {
    TEXT_EXTENSION_LIST
        .iter()
        .map(|extension| (*extension).to_string())
        .collect()
}

const fn min_percent(left: u64, right: u64) -> u64 {
    if left < right {
        left
    } else {
        right
    }
}

fn scale_u64(value: u64, percent: u64) -> u64 {
    value.saturating_mul(percent).div_ceil(100)
}

fn scale_usize(value: usize, percent: u64) -> usize {
    let scaled = (value as u64).saturating_mul(percent).div_ceil(100);
    scaled.min(usize::MAX as u64) as usize
}
