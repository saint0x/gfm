use gfm_types::{GfmError, Result};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MacOsVersion {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl MacOsVersion {
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    pub fn parse(input: &str) -> Result<Self> {
        let mut parts = input.trim().split('.');
        let major = parse_version_part(parts.next(), "major")?;
        let minor = parts
            .next()
            .map(|value| parse_version_part(Some(value), "minor"))
            .transpose()?
            .unwrap_or(0);
        let patch = parts
            .next()
            .map(|value| parse_version_part(Some(value), "patch"))
            .transpose()?
            .unwrap_or(0);
        if parts.next().is_some() {
            return Err(GfmError::Format(format!(
                "unsupported macOS version format `{input}`"
            )));
        }
        Ok(Self {
            major,
            minor,
            patch,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuArchitecture {
    AppleSilicon,
    Intel64,
    Unsupported,
}

impl CpuArchitecture {
    pub fn parse(input: &str) -> Self {
        match input.trim() {
            "arm64" | "arm64e" => Self::AppleSilicon,
            "x86_64" => Self::Intel64,
            _ => Self::Unsupported,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AppleSilicon => "apple-silicon",
            Self::Intel64 => "intel64",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportTier {
    Primary,
    Compatible,
    Unsupported,
}

impl SupportTier {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Compatible => "compatible",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardwareProfile {
    pub architecture: CpuArchitecture,
    pub memory_bytes: u64,
    pub logical_cpus: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostProfile {
    pub macos_version: MacOsVersion,
    pub build: String,
    pub hardware: HardwareProfile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostSchedulingPressureReport {
    pub thermal_status: HostPressureSignalStatus,
    pub thermal_state: HostThermalState,
    pub thermal_reason: Option<String>,
    pub battery_status: HostPressureSignalStatus,
    pub battery_state: HostBatteryState,
    pub battery_reason: Option<String>,
    pub io_status: HostPressureSignalStatus,
    pub io_state: HostIoPressure,
    pub io_reason: Option<String>,
    pub user_activity_status: HostPressureSignalStatus,
    pub user_activity: HostUserActivity,
    pub user_activity_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostPressureSignalStatus {
    Available,
    Unsupported,
    Unavailable,
}

impl HostPressureSignalStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Unsupported => "unsupported",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostIoPressure {
    Nominal,
    Elevated,
    Saturated,
}

impl HostIoPressure {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Nominal => "nominal",
            Self::Elevated => "elevated",
            Self::Saturated => "saturated",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostThermalState {
    Nominal,
    Fair,
    Serious,
    Critical,
}

impl HostThermalState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Nominal => "nominal",
            Self::Fair => "fair",
            Self::Serious => "serious",
            Self::Critical => "critical",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostBatteryState {
    AcPower,
    Battery,
    LowPower,
}

impl HostBatteryState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AcPower => "ac",
            Self::Battery => "battery",
            Self::LowPower => "low",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostUserActivity {
    Idle,
    Active,
}

impl HostUserActivity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Active => "active",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportEvaluation {
    pub tier: SupportTier,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportMatrix {
    pub primary_minimum: MacOsVersion,
    pub compatible_minimum: MacOsVersion,
    pub minimum_memory_bytes: u64,
    pub minimum_logical_cpus: u16,
    pub primary_architectures: Vec<CpuArchitecture>,
    pub compatible_architectures: Vec<CpuArchitecture>,
}

impl Default for SupportMatrix {
    fn default() -> Self {
        Self {
            primary_minimum: MacOsVersion::new(15, 0, 0),
            compatible_minimum: MacOsVersion::new(14, 0, 0),
            minimum_memory_bytes: 8 * 1024 * 1024 * 1024,
            minimum_logical_cpus: 4,
            primary_architectures: vec![CpuArchitecture::AppleSilicon],
            compatible_architectures: vec![CpuArchitecture::Intel64],
        }
    }
}

impl SupportMatrix {
    pub fn evaluate(&self, host: &HostProfile) -> SupportEvaluation {
        let mut reasons = Vec::new();
        if host.macos_version < self.compatible_minimum {
            reasons.push(format!(
                "macOS {}.{}.{} is below minimum {}.{}.{}",
                host.macos_version.major,
                host.macos_version.minor,
                host.macos_version.patch,
                self.compatible_minimum.major,
                self.compatible_minimum.minor,
                self.compatible_minimum.patch
            ));
        }
        if host.hardware.architecture == CpuArchitecture::Unsupported {
            reasons.push("CPU architecture is unsupported".to_string());
        }
        if host.hardware.memory_bytes < self.minimum_memory_bytes {
            reasons.push(format!(
                "memory {} bytes is below minimum {} bytes",
                host.hardware.memory_bytes, self.minimum_memory_bytes
            ));
        }
        if host.hardware.logical_cpus < self.minimum_logical_cpus {
            reasons.push(format!(
                "logical CPU count {} is below minimum {}",
                host.hardware.logical_cpus, self.minimum_logical_cpus
            ));
        }
        if !reasons.is_empty() {
            return SupportEvaluation {
                tier: SupportTier::Unsupported,
                reasons,
            };
        }

        if host.macos_version >= self.primary_minimum
            && self
                .primary_architectures
                .contains(&host.hardware.architecture)
        {
            SupportEvaluation {
                tier: SupportTier::Primary,
                reasons: Vec::new(),
            }
        } else if self
            .compatible_architectures
            .contains(&host.hardware.architecture)
            || host.macos_version < self.primary_minimum
        {
            SupportEvaluation {
                tier: SupportTier::Compatible,
                reasons: Vec::new(),
            }
        } else {
            SupportEvaluation {
                tier: SupportTier::Unsupported,
                reasons: vec!["host does not match a supported architecture tier".to_string()],
            }
        }
    }
}

pub fn current_host_profile() -> Result<HostProfile> {
    Ok(HostProfile {
        macos_version: MacOsVersion::parse(&command_output("sw_vers", &["-productVersion"])?)?,
        build: command_output("sw_vers", &["-buildVersion"])?,
        hardware: HardwareProfile {
            architecture: CpuArchitecture::parse(&command_output("uname", &["-m"])?),
            memory_bytes: command_output("sysctl", &["-n", "hw.memsize"])?
                .trim()
                .parse()
                .map_err(|err| GfmError::Format(format!("invalid hw.memsize: {err}")))?,
            logical_cpus: command_output("sysctl", &["-n", "hw.logicalcpu"])?
                .trim()
                .parse()
                .map_err(|err| GfmError::Format(format!("invalid hw.logicalcpu: {err}")))?,
        },
    })
}

pub fn current_host_scheduling_pressure() -> HostSchedulingPressureReport {
    HostSchedulingPressureReport::from_native(gfm_mac_sys::copy_host_pressure())
}

impl HostSchedulingPressureReport {
    pub fn from_native(native: gfm_mac_sys::NativeHostPressure) -> Self {
        let (thermal_status, thermal_state, thermal_reason) = map_thermal_signal(
            native.thermal_status,
            native.thermal_state,
            native.thermal_reason,
        );
        let (battery_status, battery_state, battery_reason) = map_low_power_signal(
            native.low_power_status,
            native.low_power_enabled,
            native.low_power_reason,
        );
        Self {
            thermal_status,
            thermal_state,
            thermal_reason,
            battery_status,
            battery_state,
            battery_reason,
            io_status: HostPressureSignalStatus::Unsupported,
            io_state: HostIoPressure::Nominal,
            io_reason: Some("macOS IO pressure source is not wired".to_string()),
            user_activity_status: HostPressureSignalStatus::Unsupported,
            user_activity: HostUserActivity::Idle,
            user_activity_reason: Some("UI user-activity source is not wired".to_string()),
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "host-scheduling-pressure\tio-status={}\tio={}\tio-reason={}\tthermal-status={}\tthermal={}\tthermal-reason={}\tbattery-status={}\tbattery={}\tbattery-reason={}\tuser-activity-status={}\tuser-activity={}\tuser-activity-reason={}",
            self.io_status.as_str(),
            self.io_state.as_str(),
            optional_host_reason(self.io_reason.as_deref()),
            self.thermal_status.as_str(),
            self.thermal_state.as_str(),
            optional_host_reason(self.thermal_reason.as_deref()),
            self.battery_status.as_str(),
            self.battery_state.as_str(),
            optional_host_reason(self.battery_reason.as_deref()),
            self.user_activity_status.as_str(),
            self.user_activity.as_str(),
            optional_host_reason(self.user_activity_reason.as_deref()),
        )
    }
}

fn map_thermal_signal(
    status: gfm_mac_sys::NativeHostSignalStatus,
    state: Option<gfm_mac_sys::NativeThermalState>,
    reason: Option<String>,
) -> (HostPressureSignalStatus, HostThermalState, Option<String>) {
    match (status, state) {
        (gfm_mac_sys::NativeHostSignalStatus::Available, Some(state)) => (
            HostPressureSignalStatus::Available,
            match state {
                gfm_mac_sys::NativeThermalState::Nominal => HostThermalState::Nominal,
                gfm_mac_sys::NativeThermalState::Fair => HostThermalState::Fair,
                gfm_mac_sys::NativeThermalState::Serious => HostThermalState::Serious,
                gfm_mac_sys::NativeThermalState::Critical => HostThermalState::Critical,
            },
            reason,
        ),
        (gfm_mac_sys::NativeHostSignalStatus::Unsupported, _) => (
            HostPressureSignalStatus::Unsupported,
            HostThermalState::Nominal,
            reason,
        ),
        _ => (
            HostPressureSignalStatus::Unavailable,
            HostThermalState::Nominal,
            reason,
        ),
    }
}

fn map_low_power_signal(
    status: gfm_mac_sys::NativeHostSignalStatus,
    enabled: Option<bool>,
    reason: Option<String>,
) -> (HostPressureSignalStatus, HostBatteryState, Option<String>) {
    match (status, enabled) {
        (gfm_mac_sys::NativeHostSignalStatus::Available, Some(true)) => (
            HostPressureSignalStatus::Available,
            HostBatteryState::LowPower,
            reason,
        ),
        (gfm_mac_sys::NativeHostSignalStatus::Available, Some(false)) => (
            HostPressureSignalStatus::Available,
            HostBatteryState::AcPower,
            reason,
        ),
        (gfm_mac_sys::NativeHostSignalStatus::Unsupported, _) => (
            HostPressureSignalStatus::Unsupported,
            HostBatteryState::AcPower,
            reason,
        ),
        _ => (
            HostPressureSignalStatus::Unavailable,
            HostBatteryState::AcPower,
            reason,
        ),
    }
}

fn optional_host_reason(reason: Option<&str>) -> String {
    reason
        .filter(|reason| !reason.trim().is_empty())
        .map(escape_host_field)
        .unwrap_or_else(|| "-".to_string())
}

fn escape_host_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn parse_version_part(value: Option<&str>, label: &str) -> Result<u16> {
    let value =
        value.ok_or_else(|| GfmError::Format(format!("missing macOS version {label} part")))?;
    value
        .parse()
        .map_err(|err| GfmError::Format(format!("invalid macOS version {label} `{value}`: {err}")))
}

fn command_output(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|err| GfmError::Format(format!("failed to run {program}: {err}")))?;
    if !output.status.success() {
        return Err(GfmError::Format(format!(
            "{program} {:?} failed with status {}: {}",
            args,
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_string())
        .map_err(|err| GfmError::Format(format!("{program} returned non-UTF-8 output: {err}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_macos_versions() {
        assert_eq!(
            MacOsVersion::parse("15.6.1").unwrap(),
            MacOsVersion::new(15, 6, 1)
        );
        assert_eq!(
            MacOsVersion::parse("14").unwrap(),
            MacOsVersion::new(14, 0, 0)
        );
        assert!(MacOsVersion::parse("15.6.1.2").is_err());
    }

    #[test]
    fn evaluates_primary_compatible_and_unsupported_hosts() {
        let matrix = SupportMatrix::default();
        let primary = HostProfile {
            macos_version: MacOsVersion::new(15, 1, 0),
            build: "24B83".to_string(),
            hardware: HardwareProfile {
                architecture: CpuArchitecture::AppleSilicon,
                memory_bytes: 16 * 1024 * 1024 * 1024,
                logical_cpus: 8,
            },
        };
        let compatible = HostProfile {
            macos_version: MacOsVersion::new(14, 7, 0),
            build: "23H124".to_string(),
            hardware: HardwareProfile {
                architecture: CpuArchitecture::Intel64,
                memory_bytes: 16 * 1024 * 1024 * 1024,
                logical_cpus: 8,
            },
        };
        let unsupported = HostProfile {
            macos_version: MacOsVersion::new(13, 6, 0),
            build: "22G120".to_string(),
            hardware: HardwareProfile {
                architecture: CpuArchitecture::AppleSilicon,
                memory_bytes: 4 * 1024 * 1024 * 1024,
                logical_cpus: 2,
            },
        };

        assert_eq!(matrix.evaluate(&primary).tier, SupportTier::Primary);
        assert_eq!(matrix.evaluate(&compatible).tier, SupportTier::Compatible);
        let rejected = matrix.evaluate(&unsupported);
        assert_eq!(rejected.tier, SupportTier::Unsupported);
        assert_eq!(rejected.reasons.len(), 3);
    }

    #[test]
    fn maps_native_pressure_to_scheduler_ready_host_report() {
        let report = HostSchedulingPressureReport::from_native(gfm_mac_sys::NativeHostPressure {
            thermal_status: gfm_mac_sys::NativeHostSignalStatus::Available,
            thermal_state: Some(gfm_mac_sys::NativeThermalState::Critical),
            thermal_reason: None,
            low_power_status: gfm_mac_sys::NativeHostSignalStatus::Available,
            low_power_enabled: Some(true),
            low_power_reason: None,
        });

        assert_eq!(report.thermal_status, HostPressureSignalStatus::Available);
        assert_eq!(report.thermal_state, HostThermalState::Critical);
        assert_eq!(report.battery_status, HostPressureSignalStatus::Available);
        assert_eq!(report.battery_state, HostBatteryState::LowPower);
        assert_eq!(report.io_status, HostPressureSignalStatus::Unsupported);
        assert_eq!(report.io_state, HostIoPressure::Nominal);
        assert_eq!(
            report.io_reason.as_deref(),
            Some("macOS IO pressure source is not wired")
        );
        assert_eq!(
            report.user_activity_status,
            HostPressureSignalStatus::Unsupported
        );
        assert_eq!(report.user_activity, HostUserActivity::Idle);
    }

    #[test]
    fn pressure_report_tsv_escapes_unavailable_signal_reasons() {
        let report = HostSchedulingPressureReport::from_native(gfm_mac_sys::NativeHostPressure {
            thermal_status: gfm_mac_sys::NativeHostSignalStatus::Unavailable,
            thermal_state: None,
            thermal_reason: Some("thermal\tunknown\nnow\r".to_string()),
            low_power_status: gfm_mac_sys::NativeHostSignalStatus::Unsupported,
            low_power_enabled: None,
            low_power_reason: Some("low power\tunsupported".to_string()),
        });

        let tsv = report.as_tsv();

        assert!(tsv.starts_with("host-scheduling-pressure\t"));
        assert!(tsv.contains("\tio-status=unsupported\tio=nominal\t"));
        assert!(tsv.contains("\tthermal-status=unavailable\tthermal=nominal\t"));
        assert!(tsv.contains("\tthermal-reason=thermal\\tunknown\\nnow\\r\t"));
        assert!(tsv.contains("\tbattery-status=unsupported\tbattery=ac\t"));
        assert!(tsv.contains("\tbattery-reason=low power\\tunsupported\t"));
        assert!(tsv.contains("\tuser-activity-status=unsupported\tuser-activity=idle\t"));
    }
}
