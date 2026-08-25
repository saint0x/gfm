use gfm_types::{GfmError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseChannel {
    Stable,
    Beta,
    Nightly,
}

impl ReleaseChannel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
            Self::Nightly => "nightly",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateMode {
    Disabled,
    ManualCheck,
    BackgroundCheck,
}

impl UpdateMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::ManualCheck => "manual-check",
            Self::BackgroundCheck => "background-check",
        }
    }

    const fn requires_feed(self) -> bool {
        !matches!(self, Self::Disabled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdatePolicy {
    pub mode: UpdateMode,
    pub feed_url: Option<String>,
    pub minimum_interval_secs: u64,
    pub staged_rollout_percent: u8,
    pub require_notarized: bool,
    pub allow_downgrade: bool,
}

impl Default for UpdatePolicy {
    fn default() -> Self {
        Self {
            mode: UpdateMode::Disabled,
            feed_url: None,
            minimum_interval_secs: 86_400,
            staged_rollout_percent: 100,
            require_notarized: true,
            allow_downgrade: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RollbackPolicy {
    pub enabled: bool,
    pub retained_versions: u8,
    pub require_signed_previous: bool,
    pub preserve_user_state: bool,
}

impl Default for RollbackPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            retained_versions: 3,
            require_signed_previous: true,
            preserve_user_state: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashReportMode {
    LocalOnly,
    RemoteOptIn,
    Disabled,
}

impl CrashReportMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LocalOnly => "local-only",
            Self::RemoteOptIn => "remote-opt-in",
            Self::Disabled => "disabled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrashReportPolicy {
    pub mode: CrashReportMode,
    pub endpoint: Option<String>,
    pub explicit_consent: bool,
    pub include_minidump: bool,
    pub include_paths: bool,
    pub retention_days: u16,
}

impl Default for CrashReportPolicy {
    fn default() -> Self {
        Self {
            mode: CrashReportMode::LocalOnly,
            endpoint: None,
            explicit_consent: false,
            include_minidump: true,
            include_paths: false,
            retention_days: 14,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticMode {
    LocalOnly,
    RemoteOptIn,
    Disabled,
}

impl DiagnosticMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LocalOnly => "local-only",
            Self::RemoteOptIn => "remote-opt-in",
            Self::Disabled => "disabled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticsPolicy {
    pub mode: DiagnosticMode,
    pub endpoint: Option<String>,
    pub explicit_consent: bool,
    pub include_paths: bool,
    pub include_queries: bool,
    pub include_user_identifiers: bool,
    pub retention_days: u16,
}

impl Default for DiagnosticsPolicy {
    fn default() -> Self {
        Self {
            mode: DiagnosticMode::LocalOnly,
            endpoint: None,
            explicit_consent: false,
            include_paths: false,
            include_queries: false,
            include_user_identifiers: false,
            retention_days: 14,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleasePolicy {
    pub channel: ReleaseChannel,
    pub updates: UpdatePolicy,
    pub rollback: RollbackPolicy,
    pub crash_reports: CrashReportPolicy,
    pub diagnostics: DiagnosticsPolicy,
}

impl Default for ReleasePolicy {
    fn default() -> Self {
        Self {
            channel: ReleaseChannel::Stable,
            updates: UpdatePolicy::default(),
            rollback: RollbackPolicy::default(),
            crash_reports: CrashReportPolicy::default(),
            diagnostics: DiagnosticsPolicy::default(),
        }
    }
}

impl ReleasePolicy {
    pub fn production(feed_url: impl Into<String>) -> Self {
        Self {
            updates: UpdatePolicy {
                mode: UpdateMode::BackgroundCheck,
                feed_url: Some(feed_url.into()),
                ..UpdatePolicy::default()
            },
            ..Self::default()
        }
    }

    pub fn validate(&self) -> Result<()> {
        validate_update_policy(&self.updates)?;
        validate_rollback_policy(&self.rollback)?;
        validate_crash_policy(&self.crash_reports)?;
        validate_diagnostics_policy(&self.diagnostics)?;
        Ok(())
    }

    pub fn evaluate_update(&self, current: &str, available: &str) -> Result<UpdateDecision> {
        self.validate()?;
        if matches!(self.updates.mode, UpdateMode::Disabled) {
            return Ok(UpdateDecision::NoCheck);
        }
        match compare_versions(current, available)? {
            VersionOrdering::Older => Ok(match self.updates.mode {
                UpdateMode::ManualCheck => UpdateDecision::Available {
                    version: available.to_string(),
                },
                UpdateMode::BackgroundCheck => UpdateDecision::DownloadAndStage {
                    version: available.to_string(),
                    require_notarized: self.updates.require_notarized,
                },
                UpdateMode::Disabled => UpdateDecision::NoCheck,
            }),
            VersionOrdering::Equal => Ok(UpdateDecision::Current),
            VersionOrdering::Newer if self.updates.allow_downgrade => {
                Ok(UpdateDecision::DownloadAndStage {
                    version: available.to_string(),
                    require_notarized: self.updates.require_notarized,
                })
            }
            VersionOrdering::Newer if self.rollback.enabled => Ok(UpdateDecision::RollbackOnly {
                version: available.to_string(),
                retained_versions: self.rollback.retained_versions,
            }),
            VersionOrdering::Newer => Ok(UpdateDecision::RejectDowngrade {
                current: current.to_string(),
                available: available.to_string(),
            }),
        }
    }

    pub fn remote_crash_upload_allowed(&self) -> bool {
        matches!(self.crash_reports.mode, CrashReportMode::RemoteOptIn)
            && self.crash_reports.explicit_consent
            && self.crash_reports.endpoint.is_some()
            && !self.crash_reports.include_paths
    }

    pub fn remote_diagnostics_upload_allowed(&self) -> bool {
        matches!(self.diagnostics.mode, DiagnosticMode::RemoteOptIn)
            && self.diagnostics.explicit_consent
            && self.diagnostics.endpoint.is_some()
            && !self.diagnostics.include_paths
            && !self.diagnostics.include_queries
            && !self.diagnostics.include_user_identifiers
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateDecision {
    NoCheck,
    Current,
    Available {
        version: String,
    },
    DownloadAndStage {
        version: String,
        require_notarized: bool,
    },
    RollbackOnly {
        version: String,
        retained_versions: u8,
    },
    RejectDowngrade {
        current: String,
        available: String,
    },
}

fn validate_update_policy(policy: &UpdatePolicy) -> Result<()> {
    if policy.mode.requires_feed() {
        let feed = policy.feed_url.as_deref().ok_or_else(|| {
            GfmError::Format("enabled updates require an HTTPS feed URL".to_string())
        })?;
        validate_https_url("update feed", feed)?;
    }
    if policy.mode.requires_feed() && policy.minimum_interval_secs == 0 {
        return Err(GfmError::Format(
            "update minimum interval must be non-zero".to_string(),
        ));
    }
    if policy.staged_rollout_percent == 0 || policy.staged_rollout_percent > 100 {
        return Err(GfmError::Format(
            "staged rollout percent must be between 1 and 100".to_string(),
        ));
    }
    Ok(())
}

fn validate_rollback_policy(policy: &RollbackPolicy) -> Result<()> {
    if policy.enabled && policy.retained_versions == 0 {
        return Err(GfmError::Format(
            "enabled rollback requires at least one retained version".to_string(),
        ));
    }
    if policy.retained_versions > 10 {
        return Err(GfmError::Format(
            "rollback retained versions cannot exceed 10".to_string(),
        ));
    }
    Ok(())
}

fn validate_crash_policy(policy: &CrashReportPolicy) -> Result<()> {
    if policy.retention_days == 0 {
        return Err(GfmError::Format(
            "crash report retention must be non-zero".to_string(),
        ));
    }
    if policy.include_paths {
        return Err(GfmError::Format(
            "crash reports must not include file paths".to_string(),
        ));
    }
    if matches!(policy.mode, CrashReportMode::RemoteOptIn) {
        if !policy.explicit_consent {
            return Err(GfmError::Format(
                "remote crash reporting requires explicit user consent".to_string(),
            ));
        }
        validate_https_url(
            "crash report endpoint",
            policy.endpoint.as_deref().ok_or_else(|| {
                GfmError::Format("remote crash reporting requires an endpoint".to_string())
            })?,
        )?;
    }
    Ok(())
}

fn validate_diagnostics_policy(policy: &DiagnosticsPolicy) -> Result<()> {
    if policy.retention_days == 0 {
        return Err(GfmError::Format(
            "diagnostics retention must be non-zero".to_string(),
        ));
    }
    if policy.include_paths || policy.include_queries || policy.include_user_identifiers {
        return Err(GfmError::Format(
            "diagnostics artifacts must not include paths, queries, or user identifiers"
                .to_string(),
        ));
    }
    if matches!(policy.mode, DiagnosticMode::RemoteOptIn) {
        if !policy.explicit_consent {
            return Err(GfmError::Format(
                "remote diagnostics require explicit user consent".to_string(),
            ));
        }
        validate_https_url(
            "diagnostics endpoint",
            policy.endpoint.as_deref().ok_or_else(|| {
                GfmError::Format("remote diagnostics require an endpoint".to_string())
            })?,
        )?;
    }
    Ok(())
}

fn validate_https_url(label: &str, value: &str) -> Result<()> {
    if value.starts_with("https://") && value.len() > "https://".len() {
        Ok(())
    } else {
        Err(GfmError::Format(format!("{label} must be an HTTPS URL")))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VersionOrdering {
    Older,
    Equal,
    Newer,
}

fn compare_versions(current: &str, available: &str) -> Result<VersionOrdering> {
    let mut current = parse_version(current)?;
    let mut available = parse_version(available)?;
    normalize_version(&mut current);
    normalize_version(&mut available);
    Ok(if current < available {
        VersionOrdering::Older
    } else if current > available {
        VersionOrdering::Newer
    } else {
        VersionOrdering::Equal
    })
}

fn parse_version(value: &str) -> Result<Vec<u64>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(GfmError::Format("version cannot be empty".to_string()));
    }
    trimmed
        .split('.')
        .map(|part| {
            if part.is_empty() {
                return Err(GfmError::Format(format!("invalid version `{value}`")));
            }
            part.parse::<u64>()
                .map_err(|_| GfmError::Format(format!("invalid numeric version `{value}`")))
        })
        .collect()
}

fn normalize_version(version: &mut Vec<u64>) {
    while version.len() > 1 && version.last() == Some(&0) {
        version.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_is_local_private_and_valid() {
        let policy = ReleasePolicy::default();

        policy.validate().expect("default policy validates");

        assert_eq!(policy.updates.mode, UpdateMode::Disabled);
        assert!(policy.rollback.enabled);
        assert_eq!(policy.crash_reports.mode, CrashReportMode::LocalOnly);
        assert_eq!(policy.diagnostics.mode, DiagnosticMode::LocalOnly);
        assert!(!policy.remote_crash_upload_allowed());
        assert!(!policy.remote_diagnostics_upload_allowed());
    }

    #[test]
    fn production_policy_enables_https_notarized_update_checks() {
        let policy = ReleasePolicy::production("https://updates.example.com/gfm.json");

        policy.validate().expect("production policy validates");

        assert_eq!(policy.updates.mode, UpdateMode::BackgroundCheck);
        assert!(policy.updates.require_notarized);
        assert_eq!(
            policy.evaluate_update("1.0.0", "1.1.0").unwrap(),
            UpdateDecision::DownloadAndStage {
                version: "1.1.0".to_string(),
                require_notarized: true
            }
        );
    }

    #[test]
    fn remote_crash_reporting_requires_consent_endpoint_and_no_paths() {
        let mut policy = ReleasePolicy::default();
        policy.crash_reports.mode = CrashReportMode::RemoteOptIn;
        policy.crash_reports.endpoint = Some("https://crash.example.com/report".to_string());

        let err = policy.validate().expect_err("missing consent fails");
        assert!(err.to_string().contains("explicit user consent"));

        policy.crash_reports.explicit_consent = true;
        policy.validate().expect("consented remote crash policy");
        assert!(policy.remote_crash_upload_allowed());

        policy.crash_reports.include_paths = true;
        let err = policy.validate().expect_err("paths fail");
        assert!(err.to_string().contains("must not include file paths"));
    }

    #[test]
    fn remote_diagnostics_reject_sensitive_payload_fields() {
        let mut policy = ReleasePolicy::default();
        policy.diagnostics.mode = DiagnosticMode::RemoteOptIn;
        policy.diagnostics.endpoint = Some("https://diagnostics.example.com/report".to_string());
        policy.diagnostics.explicit_consent = true;
        policy.diagnostics.include_queries = true;

        let err = policy.validate().expect_err("queries fail");
        assert!(err
            .to_string()
            .contains("paths, queries, or user identifiers"));
    }

    #[test]
    fn rollback_path_rejects_downgrades_as_update_installs() {
        let policy = ReleasePolicy::production("https://updates.example.com/gfm.json");

        assert_eq!(
            policy.evaluate_update("2.0.0", "1.9.0").unwrap(),
            UpdateDecision::RollbackOnly {
                version: "1.9.0".to_string(),
                retained_versions: 3
            }
        );
    }

    #[test]
    fn version_comparison_normalizes_trailing_zero_parts() {
        let policy = ReleasePolicy::production("https://updates.example.com/gfm.json");

        assert_eq!(
            policy.evaluate_update("1.0", "1.0.0").unwrap(),
            UpdateDecision::Current
        );
    }

    #[test]
    fn enabled_updates_require_https_feed() {
        let mut policy = ReleasePolicy::default();
        policy.updates.mode = UpdateMode::ManualCheck;
        policy.updates.feed_url = Some("http://updates.example.com/gfm.json".to_string());

        let err = policy.validate().expect_err("http feed fails");
        assert!(err.to_string().contains("HTTPS URL"));
    }
}
