use gfm_config::{ConfigStore, GfmConfig};
use gfm_content::QuarantineFailureKind;
use gfm_index::{
    BatteryState, IndexMountState, IndexVolumeClass, IndexVolumeDescriptor, IoPressure,
    ThermalState, UserActivity,
};
use gfm_jobs::{
    JobBatteryState, JobIoPressure, JobThermalState, JobUserActivity, Priority, SchedulingPressure,
};
use gfm_mac::{
    current_host_profile, current_permission_onboarding, AccessIntent, MountState, SupportMatrix,
    VolumeDescriptor, VolumeDiscoveryReport, VolumeKind,
};
use gfm_types::{GfmError, Result, VolumeId};
use std::env;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

mod access;
mod archive;
mod content;
mod diagnostics;
mod extract;
mod gates;
mod index;
mod interface;
mod jobs;
mod manifest;
mod operation;
mod packaging;
mod permission_refresh;
mod platform;
mod runtime;
mod search;
mod volume;

fn main() {
    if let Err(err) = run() {
        eprintln!("gfm: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some(command) if interface::run(command, &mut args)? => {}
        Some(command) if search::run(command, &mut args)? => {}
        Some(command) if archive::run(command, &mut args)? => {}
        Some(command) if content::run(command, &mut args)? => {}
        Some(command) if index::run(command, &mut args)? => {}
        Some(command) if manifest::run(command, &mut args)? => {}
        Some(command) if diagnostics::run(command, &mut args)? => {}
        Some(command) if jobs::run(command, &mut args)? => {}
        Some("config-path") => {
            println!("{}", ConfigStore::platform_default()?.path().display());
        }
        Some("config-init") => {
            let store = config_store(args.next())?;
            let config = run_config_init(&store)?;
            println!("{}\t{}", config.schema_version, store.path().display());
        }
        Some("config-check") => {
            let store = config_store(args.next())?;
            let config = run_config_check(&store)?;
            println!("{}\t{}", config.schema_version, store.path().display());
        }
        Some("config-dump") => {
            let store = config_store(args.next())?;
            let config = run_config_dump(&store)?;
            print!("{}", config.to_toml()?);
        }
        Some("support-check") => {
            let matrix = SupportMatrix::default();
            let host = current_host_profile()?;
            let evaluation = matrix.evaluate(&host);
            println!(
                "{}\t{}.{}.{}\t{}\t{}\t{}\t{}",
                evaluation.tier.as_str(),
                host.macos_version.major,
                host.macos_version.minor,
                host.macos_version.patch,
                host.build,
                host.hardware.architecture.as_str(),
                host.hardware.memory_bytes,
                host.hardware.logical_cpus
            );
            for reason in evaluation.reasons {
                eprintln!("unsupported\t{reason}");
            }
        }
        Some("permission-onboarding") => {
            let plan = current_permission_onboarding()?;
            println!(
                "{}\t{}\t{}",
                plan.action.as_str(),
                plan.policy.prompt_mode.as_str(),
                plan.finder_parity_default
            );
            for item in plan.readiness {
                println!(
                    "{}\t{}\t{}\t{}",
                    item.scope.as_str(),
                    item.state.as_str(),
                    item.path.display(),
                    escape_output_field(&item.reason)
                );
            }
        }
        Some("permission-invalidation") => {
            let path = args
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(runtime::default_permission_state_path);
            let report = permission_refresh::refresh_permission_state_at_path(&path)?;
            println!("{}", report.as_tsv());
        }
        Some("permission-invalidation-compare") => {
            let previous_path = required_path(
                args.next(),
                "permission-invalidation-compare requires a previous state path",
            )?;
            let current_path = required_path(
                args.next(),
                "permission-invalidation-compare requires a current state path",
            )?;
            let previous_access = ControlPathAccessReport::new(
                previous_path.clone(),
                AccessIntent::Read,
                "permission invalidation previous state",
            );
            let current_access = ControlPathAccessReport::new(
                current_path.clone(),
                AccessIntent::Read,
                "permission invalidation current state",
            );
            previous_access.preflight_volume()?;
            current_access.preflight_volume()?;
            let _previous_guard = previous_access.access_checked(|| Ok(()))?;
            let _current_guard = current_access.access_checked(|| Ok(()))?;
            let previous = gfm_mac::PermissionStateSnapshot::read(&previous_path)?;
            let current = gfm_mac::PermissionStateSnapshot::read(&current_path)?;
            let report =
                gfm_mac::PermissionStateInvalidationReport::evaluate(Some(&previous), &current);
            println!("{}", report.as_tsv());
        }
        Some(command) if platform::run(command, &mut args)? => {}
        Some(command) if gates::run(command, &mut args)? => {}
        Some("release-policy") => packaging::release_policy()?,
        Some("release-toolchain") => packaging::release_toolchain()?,
        Some("release-validate") => packaging::release_validate(&mut args)?,
        Some("bundle-app") => packaging::bundle_app(&mut args)?,
        Some("register-app") => packaging::register_app(&mut args)?,
        Some("notarize-app") => packaging::notarize_app(&mut args)?,
        Some(command) if operation::run(command, &mut args)? => {}
        _ => print_usage(),
    }
    Ok(())
}

pub(crate) fn required_path(value: Option<String>, message: &str) -> Result<PathBuf> {
    value
        .map(PathBuf::from)
        .ok_or_else(|| GfmError::Format(message.to_string()))
}

pub(crate) fn optional_path_arg(value: Option<String>, message: &str) -> Result<Option<PathBuf>> {
    let value = value.ok_or_else(|| GfmError::Format(message.to_string()))?;
    Ok((value != "-").then(|| PathBuf::from(value)))
}

pub(crate) fn required_string(value: Option<String>, message: &str) -> Result<String> {
    value.ok_or_else(|| GfmError::Format(message.to_string()))
}

pub(crate) fn parse_optional_scheduling_pressure(
    args: &mut impl Iterator<Item = String>,
) -> Result<SchedulingPressure> {
    let Some(io) = args.next() else {
        return Ok(SchedulingPressure::default());
    };
    parse_scheduling_pressure_tail(io, args, "adaptive scheduling")
}

pub(crate) fn parse_required_scheduling_pressure(
    args: &mut impl Iterator<Item = String>,
    context: &str,
) -> Result<SchedulingPressure> {
    let io = required_string(
        args.next(),
        &format!("{context} requires adaptive scheduling io pressure"),
    )?;
    parse_scheduling_pressure_tail(io, args, context)
}

fn parse_scheduling_pressure_tail(
    io: String,
    args: &mut impl Iterator<Item = String>,
    context: &str,
) -> Result<SchedulingPressure> {
    let thermal = required_string(
        args.next(),
        &format!("{context} requires adaptive scheduling thermal state"),
    )?;
    let battery = required_string(
        args.next(),
        &format!("{context} requires adaptive scheduling battery state"),
    )?;
    let user_activity = required_string(
        args.next(),
        &format!("{context} requires adaptive scheduling user activity"),
    )?;
    Ok(SchedulingPressure {
        io: match parse_io_pressure(io)? {
            IoPressure::Nominal => JobIoPressure::Nominal,
            IoPressure::Elevated => JobIoPressure::Elevated,
            IoPressure::Saturated => JobIoPressure::Saturated,
        },
        thermal: match parse_thermal_state(thermal)? {
            ThermalState::Nominal => JobThermalState::Nominal,
            ThermalState::Fair => JobThermalState::Fair,
            ThermalState::Serious => JobThermalState::Serious,
            ThermalState::Critical => JobThermalState::Critical,
        },
        battery: match parse_battery_state(battery)? {
            BatteryState::AcPower => JobBatteryState::AcPower,
            BatteryState::Battery => JobBatteryState::Battery,
            BatteryState::LowPower => JobBatteryState::LowPower,
        },
        user_activity: match parse_user_activity(user_activity)? {
            UserActivity::Idle => JobUserActivity::Idle,
            UserActivity::Active => JobUserActivity::Active,
        },
    })
}

pub(crate) fn parse_io_pressure(value: String) -> Result<IoPressure> {
    match value.as_str() {
        "nominal" => Ok(IoPressure::Nominal),
        "elevated" => Ok(IoPressure::Elevated),
        "saturated" => Ok(IoPressure::Saturated),
        _ => Err(GfmError::Format(format!(
            "invalid io pressure `{value}`; expected nominal, elevated, or saturated"
        ))),
    }
}

pub(crate) fn parse_thermal_state(value: String) -> Result<ThermalState> {
    match value.as_str() {
        "nominal" => Ok(ThermalState::Nominal),
        "fair" => Ok(ThermalState::Fair),
        "serious" => Ok(ThermalState::Serious),
        "critical" => Ok(ThermalState::Critical),
        _ => Err(GfmError::Format(format!(
            "invalid thermal state `{value}`; expected nominal, fair, serious, or critical"
        ))),
    }
}

pub(crate) fn parse_battery_state(value: String) -> Result<BatteryState> {
    match value.as_str() {
        "ac" => Ok(BatteryState::AcPower),
        "battery" => Ok(BatteryState::Battery),
        "low" => Ok(BatteryState::LowPower),
        _ => Err(GfmError::Format(format!(
            "invalid battery state `{value}`; expected ac, battery, or low"
        ))),
    }
}

pub(crate) fn parse_user_activity(value: String) -> Result<UserActivity> {
    match value.as_str() {
        "idle" => Ok(UserActivity::Idle),
        "active" => Ok(UserActivity::Active),
        _ => Err(GfmError::Format(format!(
            "invalid user activity `{value}`; expected idle or active"
        ))),
    }
}

pub(crate) fn index_volume_descriptor(volume: &VolumeDescriptor) -> IndexVolumeDescriptor {
    IndexVolumeDescriptor::new(
        volume.label.clone(),
        volume.path.clone(),
        index_volume_class(volume.kind),
        index_mount_state(volume.mount_state),
    )
    .with_volume_id(volume.id)
    .with_stable_identity(volume.stable_identity.clone())
    .with_reachable(volume.reachable)
    .with_read_only(Some(volume.read_only))
    .with_writable(Some(volume.writable))
    .with_ejectable(Some(volume.ejectable))
    .with_mountable(volume.mountable)
    .with_case_sensitive(volume.case_sensitive)
    .with_filesystem_signature(index_volume_filesystem_signature(volume))
}

fn index_volume_filesystem_signature(volume: &VolumeDescriptor) -> String {
    let mut tokens = Vec::new();
    push_signature_str(&mut tokens, "fs", volume.filesystem.as_deref());
    push_signature_str(&mut tokens, "mount-fs", volume.mount_filesystem.as_deref());
    push_signature_str(&mut tokens, "volume-uuid", volume.volume_uuid.as_deref());
    push_signature_str(
        &mut tokens,
        "apfs-container-uuid",
        volume.apfs_container_uuid.as_deref(),
    );
    push_signature_str(
        &mut tokens,
        "apfs-role",
        volume.apfs_role.map(gfm_mac::ApfsVolumeRole::as_str),
    );
    push_signature_str(&mut tokens, "media-uuid", volume.media_uuid.as_deref());
    push_signature_str(
        &mut tokens,
        "resource-uuid",
        volume.resource_uuid.as_deref(),
    );
    push_signature_str(&mut tokens, "bsd", volume.bsd_name.as_deref());
    push_signature_u64(&mut tokens, "bsd-major", volume.bsd_major);
    push_signature_u64(&mut tokens, "bsd-minor", volume.bsd_minor);
    push_signature_u64(&mut tokens, "bsd-unit", volume.bsd_unit);
    push_signature_str(&mut tokens, "mount-from", volume.mount_from.as_deref());
    push_signature_u32(&mut tokens, "mount-flags", volume.mount_flags);
    push_signature_str(
        &mut tokens,
        "media-content",
        volume.media_content.as_deref(),
    );
    push_signature_str(&mut tokens, "media-name", volume.media_name.as_deref());
    push_signature_str(&mut tokens, "media-path", volume.media_path.as_deref());
    push_signature_str(&mut tokens, "volume-type", volume.volume_type.as_deref());
    push_signature_str(&mut tokens, "media-kind", volume.media_kind.as_deref());
    push_signature_str(&mut tokens, "media-type", volume.media_type.as_deref());
    push_signature_bool(&mut tokens, "case-sensitive", volume.case_sensitive);
    push_signature_bool(&mut tokens, "case-preserving", volume.case_preserving);
    push_signature_bool(&mut tokens, "automounted", volume.resource_automounted);
    push_signature_bool(&mut tokens, "browsable", volume.resource_browsable);
    push_signature_bool(&mut tokens, "resource-encrypted", volume.resource_encrypted);
    push_signature_bool(&mut tokens, "resource-reachable", volume.resource_reachable);
    push_signature_bool(
        &mut tokens,
        "root-filesystem",
        volume.resource_root_file_system,
    );
    push_signature_bool(
        &mut tokens,
        "resource-cloning",
        volume.resource_supports_file_cloning,
    );
    push_signature_bool(
        &mut tokens,
        "resource-hard-links",
        volume.resource_supports_hard_links,
    );
    push_signature_bool(
        &mut tokens,
        "resource-sparse",
        volume.resource_supports_sparse_files,
    );
    push_signature_bool(&mut tokens, "local", volume.local);
    push_signature_bool(&mut tokens, "mount-local", volume.mount_local);
    push_signature_bool(&mut tokens, "internal", volume.internal);
    push_signature_bool(&mut tokens, "writable", Some(volume.writable));
    push_signature_bool(&mut tokens, "read-only", Some(volume.read_only));
    push_signature_bool(&mut tokens, "mount-read-only", volume.mount_read_only);
    push_signature_bool(&mut tokens, "ejectable", Some(volume.ejectable));
    push_signature_bool(&mut tokens, "removable", Some(volume.removable));
    push_signature_bool(&mut tokens, "mountable", volume.mountable);
    push_signature_str(
        &mut tokens,
        "remount-url",
        volume.resource_remount_url.as_deref(),
    );
    push_signature_str(&mut tokens, "protocol", volume.device_protocol.as_deref());
    push_signature_str(&mut tokens, "model", volume.device_model.as_deref());
    push_signature_str(&mut tokens, "vendor", volume.device_vendor.as_deref());
    push_signature_str(&mut tokens, "device-path", volume.device_path.as_deref());
    push_signature_bool(
        &mut tokens,
        "encrypted",
        volume.media_encrypted.or(volume.resource_encrypted),
    );
    push_signature_u64(&mut tokens, "block-size", volume.media_block_size_bytes);
    push_signature_u64(&mut tokens, "media-size", volume.media_size_bytes);
    tokens.join("|")
}

fn push_signature_str(tokens: &mut Vec<String>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        tokens.push(format!("{key}={value}"));
    }
}

fn push_signature_bool(tokens: &mut Vec<String>, key: &str, value: Option<bool>) {
    if let Some(value) = value {
        tokens.push(format!("{key}={}", if value { "1" } else { "0" }));
    }
}

fn push_signature_u32(tokens: &mut Vec<String>, key: &str, value: Option<u32>) {
    if let Some(value) = value {
        tokens.push(format!("{key}=0x{value:08x}"));
    }
}

fn push_signature_u64(tokens: &mut Vec<String>, key: &str, value: Option<u64>) {
    if let Some(value) = value {
        tokens.push(format!("{key}={value}"));
    }
}

fn index_volume_class(kind: VolumeKind) -> IndexVolumeClass {
    match kind {
        VolumeKind::System => IndexVolumeClass::System,
        VolumeKind::Internal => IndexVolumeClass::Internal,
        VolumeKind::External | VolumeKind::Removable => IndexVolumeClass::External,
        VolumeKind::DiskImage => IndexVolumeClass::Slow,
        VolumeKind::Network => IndexVolumeClass::Network,
        VolumeKind::Unknown => IndexVolumeClass::Unknown,
    }
}

fn index_mount_state(state: MountState) -> IndexMountState {
    match state {
        MountState::Mounted => IndexMountState::Mounted,
        MountState::Unmounted => IndexMountState::Unmounted,
        MountState::Stale => IndexMountState::Stale,
    }
}

pub(crate) fn parse_quarantine_failure_kind(
    value: &str,
    name: &str,
) -> Result<QuarantineFailureKind> {
    QuarantineFailureKind::parse(value)
        .ok_or_else(|| GfmError::Format(format!("invalid {name}: {value}")))
}

pub(crate) fn parse_u32(value: &str, name: &str) -> Result<u32> {
    value
        .parse()
        .map_err(|_| GfmError::Format(format!("{name} must be an unsigned 32-bit integer")))
}

pub(crate) fn parse_u64(value: &str, name: &str) -> Result<u64> {
    value
        .parse()
        .map_err(|_| GfmError::Format(format!("{name} must be an unsigned 64-bit integer")))
}

pub(crate) fn parse_u32_arg(value: Option<String>, message: &str) -> Result<u32> {
    let value = value.ok_or_else(|| GfmError::Format(message.to_string()))?;
    value
        .parse()
        .map_err(|_| GfmError::Format(format!("{message}; got `{value}`")))
}

pub(crate) fn parse_u64_arg(value: Option<String>, message: &str) -> Result<u64> {
    let value = value.ok_or_else(|| GfmError::Format(message.to_string()))?;
    value
        .parse()
        .map_err(|_| GfmError::Format(format!("{message}; got `{value}`")))
}

pub(crate) fn parse_usize_arg(value: Option<String>, message: &str) -> Result<usize> {
    let value = value.ok_or_else(|| GfmError::Format(message.to_string()))?;
    parse_usize(&value, message)
}

fn parse_usize(value: &str, message: &str) -> Result<usize> {
    value
        .parse()
        .map_err(|_| GfmError::Format(format!("{message}; got `{value}`")))
}

#[derive(Clone)]
struct ControlPathAccessReport {
    path: PathBuf,
    intent: AccessIntent,
    worker: &'static str,
    volume_report: VolumeDiscoveryReport,
}

impl ControlPathAccessReport {
    fn new(path: PathBuf, intent: AccessIntent, worker: &'static str) -> Self {
        let volume_report = VolumeDiscoveryReport::for_containing_path(&path);
        Self {
            path,
            intent,
            worker,
            volume_report,
        }
    }

    fn preflight_volume(&self) -> Result<()> {
        access::preflight_volume_access_scope_with_report(
            &self.path,
            self.intent,
            self.worker,
            &self.volume_report,
        )
    }

    fn access_checked(
        &self,
        check_control: impl FnMut() -> Result<()>,
    ) -> Result<access::ScopedAccessGuard> {
        access::preflight_access_scope_checked_with_volume_report(
            &self.path,
            self.intent,
            self.worker,
            &self.volume_report,
            check_control,
        )
    }

    fn volume(&self) -> Option<VolumeId> {
        self.volume_report
            .volume_for_path(&self.path)
            .map(|volume| volume.id)
    }
}

pub(crate) fn config_store(value: Option<String>) -> Result<ConfigStore> {
    value
        .map(|path| Ok(ConfigStore::new(path)))
        .unwrap_or_else(ConfigStore::platform_default)
}

fn run_config_init(store: &ConfigStore) -> Result<GfmConfig> {
    const WORKER: &str = "config init";
    let access_report = config_init_access_report(store)?;
    access_report.preflight_volume()?;
    let volume = access_report.volume();
    let store = store.clone();
    runtime::run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = access_report.access_checked(|| cancellation.check())?;
        cancellation.check()?;
        store.load_or_create_default_checked(|| cancellation.check())
    })
}

fn run_config_check(store: &ConfigStore) -> Result<GfmConfig> {
    const WORKER: &str = "config check";
    let access_report = config_read_access_report(store, WORKER);
    access_report.preflight_volume()?;
    let volume = access_report.volume();
    let store = store.clone();
    runtime::run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = access_report.access_checked(|| cancellation.check())?;
        cancellation.check()?;
        let config = store.load_checked(|| cancellation.check())?;
        config.validate()?;
        Ok(config)
    })
}

fn run_config_dump(store: &ConfigStore) -> Result<GfmConfig> {
    const WORKER: &str = "config dump";
    let access_report = config_read_access_report(store, WORKER);
    access_report.preflight_volume()?;
    let volume = access_report.volume();
    let store = store.clone();
    runtime::run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = access_report.access_checked(|| cancellation.check())?;
        cancellation.check()?;
        store.load_or_create_default_checked(|| cancellation.check())
    })
}

fn config_init_access_report(store: &ConfigStore) -> Result<ControlPathAccessReport> {
    let probe = if config_path_exists(store.path())? {
        ControlPathAccessReport::new(
            store.path().to_path_buf(),
            AccessIntent::Read,
            "config init",
        )
    } else {
        ControlPathAccessReport::new(
            config_write_probe_path(store.path())?.to_path_buf(),
            AccessIntent::Write,
            "config init",
        )
    };
    Ok(probe)
}

fn config_read_access_report(store: &ConfigStore, worker: &'static str) -> ControlPathAccessReport {
    ControlPathAccessReport::new(store.path().to_path_buf(), AccessIntent::Read, worker)
}

fn config_path_exists(path: &Path) -> Result<bool> {
    path.try_exists()
        .map_err(|err| GfmError::io(path, format!("config path existence unavailable: {err}")))
}

pub(crate) fn existing_read_probe_path(path: &Path) -> Result<&Path> {
    if path
        .try_exists()
        .map_err(|err| GfmError::io(path, format!("read path existence unavailable: {err}")))?
    {
        return Ok(path);
    }
    Ok(parent_or_cwd(path))
}

pub(crate) fn config_write_probe_path(path: &Path) -> Result<&Path> {
    match path.metadata() {
        Ok(metadata) if metadata.is_dir() => Ok(path),
        Ok(_) => Ok(parent_or_cwd(path)),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(parent_or_cwd(path)),
        Err(err) => Err(GfmError::io(
            path,
            format!("config write path metadata unavailable: {err}"),
        )),
    }
}

pub(crate) fn parent_or_cwd(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

pub(crate) fn parent_volume(path: &Path) -> Option<VolumeId> {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => detect_volume_id(parent).ok(),
        Some(_) => detect_volume_id(Path::new(".")).ok(),
        None => None,
    }
}

pub(crate) fn run_preview_contract_cancellable_with_payload_path<T>(
    volume: Option<VolumeId>,
    label: &'static str,
    payload_path: impl Into<PathBuf>,
    build: impl Fn(gfm_jobs::Cancellation) -> Result<T> + Send + Sync + 'static,
) -> Result<T>
where
    T: Send + 'static,
{
    runtime::run_retriable_volume_task_cancellable_with_payload_path(
        volume,
        Priority::Visible,
        label,
        payload_path,
        build,
    )
}

pub(crate) fn run_preview_contract_adaptive_with_volume_and_payload_path<T>(
    priority: Priority,
    label: &'static str,
    pressure: SchedulingPressure,
    volume: impl FnOnce() -> Result<Option<VolumeId>>,
    payload_path: impl Into<PathBuf>,
    build: impl Fn(gfm_jobs::Cancellation) -> Result<T> + Send + Sync + 'static,
) -> Result<runtime::ScheduledTaskOutcome<T>>
where
    T: Send + 'static,
{
    runtime::run_scheduled_volume_task_cancellable_with_volume_and_payload_path(
        priority,
        label,
        pressure,
        volume,
        payload_path,
        build,
    )
}

pub(crate) fn detect_volume_id(path: &Path) -> Result<VolumeId> {
    volume_id_from_metadata(&std::fs::metadata(path).map_err(|err| GfmError::io(path, err))?)
}

#[cfg(unix)]
fn volume_id_from_metadata(metadata: &std::fs::Metadata) -> Result<VolumeId> {
    use std::os::unix::fs::MetadataExt;

    Ok(VolumeId(metadata.dev()))
}

#[cfg(not(unix))]
fn volume_id_from_metadata(_metadata: &std::fs::Metadata) -> Result<VolumeId> {
    Ok(VolumeId(0))
}

fn escape_output_field(input: &str) -> String {
    input
        .chars()
        .map(|ch| match ch {
            '\t' | '\n' | '\r' => ' ',
            other => other,
        })
        .collect()
}

fn print_usage() {
    println!(
        "gfm commands:
  gfm app [path]
  gfm ui-contract [path]
  gfm ui-menu-contract
  gfm ui-context-menu-contract [file|folder|volume|sidebar|empty|selection|search-result|trash] [selection-count] [writable] [ejectable] [has-clipboard-items]
  gfm ui-dialog-contract [alert|rename|popover|disclosure|progress|conflict|permission] [running|paused] [true|false]
  gfm ui-permission-onboarding-contract
  gfm ui-permission-access-contract <path> <read|write|index|preview|operate> [worker]
  gfm ui-permission-refresh-compare-contract <previous-state.tsv> <current-state.tsv>
  gfm ui-progress-job-contract <progress.gfmprogress> <job-id>
  gfm ui-fileprovider-conflict-contract <fileprovider-path>
  gfm ui-operation-conflict-contract <copy|move|rename|restore> <source> <target> [fail|replace|keep-both|merge|skip]
  gfm ui-operation-conflict-resolve <operation-conflicts.tsv> <target> <replace|keep-both|merge|skip>
  gfm ui-titlebar-contract [path]
  gfm ui-session-contract [path] [window-session.tsv]
  gfm ui-toolbar-contract [path]
  gfm ui-sidebar-contract [path]
  gfm ui-sidebar-fileprovider-contract [current-path] <fileprovider-path>
  gfm ui-sidebar-fileprovider-invalidation <previous-state> <fileprovider-path>
  gfm ui-sidebar-fileprovider-observed-invalidation <state.tsv> <create|metadata|modify|remove|rescan|other|rename> <path> [rename-to]
  gfm ui-sidebar-fileprovider-observer-probe <state.tsv> <root> <target>
  gfm ui-sidebar-volume-invalidation <appeared|description-changed|disappeared|unavailable> [path]
  gfm ui-sidebar-volume-state-invalidation <previous-paths...> -- <appeared|description-changed|disappeared|unavailable> [path]
  gfm ui-icon-view-contract <path> [columns] [viewport-rows] [scroll-row]
  gfm ui-virtualization-contract <icon-grid|list-rows|column-rows|gallery-filmstrip|search-results|trash-rows> <total> <viewport> <scroll> [columns]
  gfm package-traversal <root> [opaque|traverse]
  gfm finder-metadata <path>
  gfm list [path]
  gfm index <root> <output.gfmidx>
  gfm index-state <root> <records.gfmidx> <state.gfmstate>
  gfm index-state-inspect <state.gfmstate>
  gfm scan-progress <root> <records.gfmidx> <progress.gfmprogress>
  gfm scan-progress-inspect <progress.gfmprogress>
  gfm fair-scan <root> <visible-burst> [visible-root...]
  gfm rename-correlation <source> <destination>
  gfm metadata-update <path> [append-text]
  gfm event-backpressure <capacity> <visible-burst> <background-events> [visible-events]
  gfm fsevents-cursor-checkpoint <state.gfmstate> <cursor.gfmcursor> <last-event-id> [clean|repair-required]
  gfm fsevents-cursor-inspect <cursor.gfmcursor>
  gfm fsevents-cursor-resume <state.gfmstate> <cursor.gfmcursor>
  gfm fsevents-repair-schedule <state.gfmstate> <cursor.gfmcursor> <observed-event-ids|-> [reason|-] [dropped-roots...]
  gfm index-content <root> <records.gfmidx> <content.gfmcontent>
  gfm extract-report <path>
  gfm extract-report-adaptive <path> <nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active>
  gfm extract-worker-adaptive <path> <nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active>
  gfm extract-worker-cancel-adaptive <path> <nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active>
  gfm extract-worker-quarantine-adaptive <path> <store.gfmquarantine> <nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active> [timeout-ms] [failure-threshold]
  gfm extract-cache <path>
  gfm extract-quarantine <path> <store.gfmquarantine> [corrupt|encrypted|crash|timeout] [attempts]
  gfm index-content-segment <root> <output.gfmseg>
  gfm compact-content <output.gfmcontent> <segments.gfmseg...>
  gfm compact-content-tiered <output.gfmcontent> <segments.gfmseg...>
  gfm content-manifest-write <manifest.gfmmanifest> <hot|warm|cold:path...>
  gfm content-manifest-inspect <manifest.gfmmanifest>
  gfm content-manifest-recovery-plan <manifest.gfmmanifest> [hot|warm|cold:path...]
  gfm content-manifest-recover <manifest.gfmmanifest> <quarantine-dir> [hot|warm|cold:path...]
  gfm content-manifest-promote <manifest.gfmmanifest> <hot|warm|cold:path> [retired-archive...]
  gfm content-manifest-promotion-recovery-plan <manifest.gfmmanifest>
  gfm content-manifest-promotion-recover <manifest.gfmmanifest>
  gfm content-manifest-cleanup <manifest.gfmmanifest> <candidate-archive...>
  gfm content-cleanup-plan <manifest.gfmmanifest> <min-retired-archives> <min-retired-bytes> <max-cleanup-archives> <candidate-archive...>
  gfm content-maintain-segments <manifest.gfmmanifest> <output.gfmcontent> <segments.gfmseg...>
  gfm content-maintain-segments-adaptive <manifest.gfmmanifest> <output.gfmcontent> <nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active> <segments.gfmseg...>
  gfm index-content-background <root> <segment-dir> <records.gfmidx> <content.gfmcontent> [<nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active>]
  gfm resume-content-background [content.job] [jobs.journal]
  gfm resume-content-background-adaptive <content.job> <jobs.journal> <nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active>
  gfm search <root> <query>
  gfm search-stream <root> <query>
  gfm search-content <root> <query>
  gfm search-content-adaptive <root> <query> <nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active>
  gfm search-index <index.gfmidx> <query>
  gfm search-index-mmap <index.gfmidx> <query>
  gfm search-index-columns <index.gfmidx> <columns.gfmcols> <query>
  gfm search-index-sidecars <index.gfmidx> <columns.gfmcols> <metadata.gfmmeta> <prefixes.gfmprefix> <substrings.gfmsubstr> <fuzzy.gfmfuzzy> <content.gfmcontent> <query>
  gfm search-index-sidecars-session <index.gfmidx> <columns.gfmcols> <metadata.gfmmeta> <prefixes.gfmprefix> <substrings.gfmsubstr> <fuzzy.gfmfuzzy> <content.gfmcontent> <query>
  gfm search-index-sidecars-budget <index.gfmidx> <columns.gfmcols> <metadata.gfmmeta> <prefixes.gfmprefix> <substrings.gfmsubstr> <fuzzy.gfmfuzzy> <content.gfmcontent> <max-prefix-ids> <max-substring-grams> <max-substring-ids> <max-fuzzy-keys> <max-fuzzy-terms> <max-fuzzy-candidates> <max-content-ids> <query>
  gfm search-index-sidecars-volume-scope <index.gfmidx> <columns.gfmcols> <metadata.gfmmeta> <prefixes.gfmprefix> <substrings.gfmsubstr> <fuzzy.gfmfuzzy> <content.gfmcontent> <volume-ids|-> <query>
  gfm search-index-sidecars-cancel-candidates
  gfm search-query-cancel-parse
  gfm index-footprint <index.gfmidx> <columns.gfmcols|-> <metadata.gfmmeta|-> <prefixes.gfmprefix|-> <substrings.gfmsubstr|-> <fuzzy.gfmfuzzy|-> <content-manifest.gfmmanifest|-> [segments.gfmseg...]
  gfm index-compaction-plan <index.gfmidx> <content-manifest.gfmmanifest|-> <nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active> [segments.gfmseg...]
  gfm archive-schema <records|columns|metadata|prefixes|substrings|fuzzy|dictionary|content|content-manifest> <archive-path>
  gfm archive-rebuild-plan <records.gfmidx> <columns.gfmcols> <metadata.gfmmeta> <prefixes.gfmprefix> <substrings.gfmsubstr> <fuzzy.gfmfuzzy> <dictionary.gfmdict> <content.gfmcontent> <content-manifest.gfmmanifest> [hot|warm|cold:content.gfmcontent...]
  gfm records-migration-plan <records.gfmidx>
  gfm records-migrate <records.gfmidx> <backup-dir>
  gfm content-migration-plan <content.gfmcontent>
  gfm content-migrate <content.gfmcontent> <backup-dir>
  gfm metadata-migration-plan <metadata.gfmmeta>
  gfm metadata-migrate <metadata.gfmmeta> <backup-dir>
  gfm columns-rebuild-plan <records.gfmidx> <columns.gfmcols>
  gfm columns-rebuild <records.gfmidx> <columns.gfmcols> <backup-dir>
  gfm derived-sidecar-rebuild-plan <records.gfmidx> <columns|metadata|prefixes|substrings|fuzzy|dictionary> <sidecar-path>
  gfm derived-sidecar-rebuild <records.gfmidx> <columns|metadata|prefixes|substrings|fuzzy|dictionary> <sidecar-path> <backup-dir>
  gfm records-verify <index.gfmidx>
  gfm index-columns <records.gfmidx> <columns.gfmcols>
  gfm columns-verify <columns.gfmcols>
  gfm columns-lookup <columns.gfmcols> <volume-id> <node-id>
  gfm index-metadata <records.gfmidx> <metadata.gfmmeta>
  gfm index-dictionary <records.gfmidx> <dictionary.gfmdict>
  gfm index-prefixes <records.gfmidx> <prefixes.gfmprefix>
  gfm index-substrings <records.gfmidx> <substrings.gfmsubstr>
  gfm index-fuzzy <records.gfmidx> <fuzzy.gfmfuzzy>
  gfm sidecar-recovery-plan <records.gfmidx> <columns.gfmcols|-> <metadata.gfmmeta|-> <prefixes.gfmprefix|-> <substrings.gfmsubstr|-> <fuzzy.gfmfuzzy|-> <dictionary.gfmdict|->
  gfm sidecar-recover <records.gfmidx> <quarantine-dir> <columns.gfmcols|-> <metadata.gfmmeta|-> <prefixes.gfmprefix|-> <substrings.gfmsubstr|-> <fuzzy.gfmfuzzy|-> <dictionary.gfmdict|->
  gfm sidecar-recover-adaptive <records.gfmidx> <quarantine-dir> <nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active> <columns.gfmcols|-> <metadata.gfmmeta|-> <prefixes.gfmprefix|-> <substrings.gfmsubstr|-> <fuzzy.gfmfuzzy|-> <dictionary.gfmdict|->
  gfm fuzzy-terms-mmap <fuzzy.gfmfuzzy> <key>
  gfm fuzzy-verify <fuzzy.gfmfuzzy>
  gfm prefix-ids-mmap <prefixes.gfmprefix> <prefix>
  gfm prefix-id-block-mmap <prefixes.gfmprefix> <prefix> <block-index>
  gfm prefix-verify <prefixes.gfmprefix>
  gfm substring-ids-mmap <substrings.gfmsubstr> <trigram>
  gfm substring-id-block-mmap <substrings.gfmsubstr> <trigram> <block-index>
  gfm substring-verify <substrings.gfmsubstr>
  gfm dictionary-lookup <dictionary.gfmdict> <term>
  gfm dictionary-verify <dictionary.gfmdict>
  gfm metadata-ids-mmap <metadata.gfmmeta> <tag|comment> <term>
  gfm metadata-id-block-mmap <metadata.gfmmeta> <tag|comment> <term> <block-index>
  gfm metadata-verify <metadata.gfmmeta>
  gfm search-content-index <records.gfmidx> <content.gfmcontent> <query>
  gfm search-content-index-adaptive <records.gfmidx> <content.gfmcontent> <query> <nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active>
  gfm search-content-index-set <records.gfmidx> <query> <content.gfmcontent...>
  gfm search-content-index-set-session <records.gfmidx> <query> <content.gfmcontent...>
  gfm search-content-index-manifest <records.gfmidx> <manifest.gfmmanifest> <query>
  gfm search-content-index-manifest-session <records.gfmidx> <manifest.gfmmanifest> <query>
  gfm content-ids <content.gfmcontent> <term>
  gfm content-ids-mmap <content.gfmcontent> <term>
  gfm content-ids-mmap-set <term> <content.gfmcontent...>
  gfm content-ids-mmap-manifest <manifest.gfmmanifest> <term>
  gfm content-id-block-mmap <content.gfmcontent> <term> <block-index>
  gfm content-verify <content.gfmcontent>
  gfm config-path
  gfm config-init [config.toml]
  gfm config-check [config.toml]
  gfm config-dump [config.toml]
  gfm diagnostics-index-rebuild <root> <records.gfmidx> [content.gfmcontent]
  gfm diagnostics-index-rebuild-adaptive <root> <records.gfmidx> <nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active> [content.gfmcontent]
  gfm diagnostics-index-recovery-plan <root> <records.gfmidx> <state.gfmstate> [quarantine-dir]
  gfm diagnostics-index-recover <root> <records.gfmidx> <state.gfmstate> [quarantine-dir]
  gfm diagnostics-index-recover-adaptive <root> <records.gfmidx> <state.gfmstate> <nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active> [quarantine-dir]
  gfm diagnostics-trace-export <trace.json>
  gfm diagnostics-parity-baseline <config.toml> <baseline-root> <macos-build>
  gfm diagnostics-storage-inspect <records.gfmidx|content.gfmcontent>
  gfm support-check
  gfm permission-onboarding
  gfm permission-invalidation [permission-state.tsv]
  gfm permission-invalidation-unavailable-volume-api <permission-state.tsv> <volume-root>
  gfm permission-invalidation-compare <previous-state.tsv> <current-state.tsv>
  gfm security-scope <path> [read|write|index|preview|operate]
  gfm security-worker-admission <worker-label> <path> [read|write|index|preview|operate]
  gfm security-worker-admission-fanout <path> <worker-label> <read|write|index|preview|operate>...
  gfm security-worker-admission-fanout-unavailable-volume-api <path> <volume-root> <worker-label> <read|write|index|preview|operate>...
  gfm security-worker-admission-unavailable-volume-api <worker-label> <path> <volume-root> [read|write|index|preview|operate]
  gfm security-bookmark-create <path> [read|write|index|preview|operate]
  gfm security-bookmark-reconcile
  gfm mac-bridges
  gfm native-icon <path>
  gfm native-icon-bridge <path>
  gfm native-icon-fileprovider-invalidation <previous-state> <path>
  gfm native-icon-fileprovider-observer-probe <state.tsv> <root> <target>
  gfm fileprovider-state <path>
  gfm fileprovider-state-with-identity <path>
  gfm fileprovider-domain <path>
  gfm fileprovider-domains
  gfm fileprovider-progress <path>
  gfm fileprovider-conflict <path>
  gfm fileprovider-progress-job <path>
  gfm fileprovider-operation <download|evict> <path>
  gfm fileprovider-invalidation <previous-state> <path>
  gfm fileprovider-metadata-invalidation <previous-state> <path>
  gfm preview-cache-fileprovider-invalidation <cache-root> <previous-state> <path> [icon|thumbnail|quick-look|text]
  gfm preview-cache-fileprovider-observed-invalidation <cache-root> <state.tsv> [icon|thumbnail|quick-look|text] <create|metadata|modify|remove|rescan|other|rename> <path> [rename-to]
  gfm preview-cache-fileprovider-observer-probe <cache-root> <state.tsv> [icon|thumbnail|quick-look|text] <root> <target>
  gfm fileprovider-invalidation-scan <state.tsv> <paths...>
  gfm fileprovider-invalidation-event <state.tsv> <create|metadata|modify|remove|rescan|other|rename> <path> [rename-to]
  gfm fileprovider-observed-metadata-invalidation <state.tsv> <create|metadata|modify|remove|rescan|other|rename> <path> [rename-to]
  gfm fileprovider-observer-probe <state.tsv> <root> <target>
  gfm fileprovider-observer-metadata-probe <state.tsv> <root> <target>
  gfm volume-discovery [paths...]
  gfm volume-events-probe
  gfm volume-events-shutdown-probe
  gfm volume-event-invalidation <appeared|description-changed|disappeared|unavailable> [path]
  gfm volume-event-transition-invalidation <appeared|description-changed|disappeared|unavailable> <path> <previous-label> <current-label>
  gfm volume-event-transition-case-sensitivity <previous:true|false> <current:true|false>
  gfm volume-event-transition-api-status
  gfm volume-operation <eject|unmount|mount> <path>
  gfm volume-mount-bsd <bsd-name>
  gfm volume-index-policy <external:disabled|opt-in|enabled> <network:disabled|opt-in|enabled> [opt-in:path...] [paths...]
  gfm volume-invalidation <previous-class> <previous-mount> <path> [previous-read-only previous-writable previous-ejectable previous-mountable previous-case-sensitive previous-stable-id previous-filesystem-signature]
  gfm volume-known-facts-lost-invalidation
  gfm volume-event-index-invalidation <appeared|description-changed|disappeared|unavailable> [path]
  gfm volume-event-state-index-invalidation <previous-paths...> -- <appeared|description-changed|disappeared|unavailable> [path]
  gfm volume-case-sensitivity-invalidation <previous:true|false> <current:true|false>
  gfm volume-event-runtime-invalidation <appeared|description-changed|disappeared|unavailable> [path]
  gfm volume-event-runtime-fanout <previous-paths...> -- <appeared|description-changed|disappeared|unavailable> [path]
  gfm volume-topology-diff <previous-paths...> -- <current-paths...>
  gfm volume-topology-index-invalidation <previous-paths...> -- <current-paths...>
  gfm volume-topology-case-sensitivity <previous:true|false> <current:true|false>
  gfm volume-topology-api-status
  gfm spotlight-reconcile <path> [spotlight-fixture.tsv]
  gfm preview-check <path> [icon|thumbnail|quick-look|text]
  gfm preview-volume-check <path> [icon|thumbnail|quick-look|text]
  gfm preview-volume-scheduling <path> [icon|thumbnail|quick-look|text] <nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active>
  gfm icon-preview <path>
  gfm quicklook-session <path>
  gfm quicklook-session-adaptive <path> <nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active>
  gfm quicklook-session-adaptive-cancel-after-access <path> <nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active>
  gfm quicklook-session-cancel <path>
  gfm thumbnail-generation <path>
  gfm thumbnail-generation-adaptive <path> <nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active>
  gfm thumbnail-generation-adaptive-cancel-after-access <path> <nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active>
  gfm thumbnail-generation-cancel <path>
  gfm preview-schedule
  gfm preview-schedule-retained-capacity
  gfm macrobench <workspace> [smoke|standard]
  gfm macrobench-fixture <workspace> [smoke|standard|million]
  gfm parity-fixture <workspace> [smoke|standard]
  gfm pixel-diff <expected.rgba> <actual.rgba> <width> <height> [mask.tsv]
  gfm pixel-threshold-check <layout|text|icon|selection|focus|hover|toolbar|thumbnail|preview> <expected.rgba|png> <actual.rgba|png> <width> <height> [governed-mask.tsv]
  gfm parity-gate <manifest.tsv>
  gfm parity-review <manifest.tsv> <output-dir>
  gfm parity-profile <macos-build> [system|light|dark] [1x|2x|3x] [srgb|display-p3]
  gfm regression-gate <workspace> [smoke|standard]
  gfm large-sidecar-gate <workspace> <synthetic-records>
  gfm search-typing-benchmark <workspace> <synthetic-records> [repetitions] [query]
  gfm search-typing-session-benchmark <workspace> <synthetic-records> [repetitions] [query]
  gfm release-policy
  gfm release-toolchain
  gfm release-validate <GFM.app> [--allow-unsigned] [--skip-notarization] [--skip-gatekeeper]
  gfm bundle-app <executable> <GFM.icns> <output-dir> [--ad-hoc|--unsigned|developer-id]
  gfm register-app <GFM.app>
  gfm notarize-app <GFM.app> <output-dir> --keychain-profile <profile>
  gfm notarize-app <GFM.app> <output-dir> --apple-id <email> --team-id <team> --password <password>
  gfm notarize-app <GFM.app> <output-dir> --api-key <AuthKey.p8> --key-id <key> --issuer <issuer>
  gfm jobs-recover [jobs.journal]
  gfm jobs-retry-plan <max-attempts> <attempts> <failure-message...>
  gfm jobs-payload-catalog <catalog.gfmjobs>
  gfm jobs-fairness-plan
  gfm jobs-progress-snapshot <progress.gfmprogress>
  gfm jobs-progress-restore <progress.gfmprogress> [updated-ms]
  gfm jobs-progress-control <progress.gfmprogress> <job-id> <pause|resume|stop> [updated-ms]
  gfm jobs-payload-restore-plan <catalog.gfmjobs> <progress.gfmprogress> [updated-ms]
  gfm jobs-cancel-tree
  gfm jobs-cancel-volume <volume-id> [foreground|visible|background|maintenance|repair]
  gfm jobs-runtime-retry-probe <attempt-state> [<nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active>]
  gfm ops-recover [ops.journal] [--retry-failed] [--max-attempts N]
  gfm watch-once <root>
  gfm operation-conflict-apply <operation-conflicts.tsv> <target> <replace|keep-both|merge|skip>
  gfm operation-conflict-apply-all <operation-conflicts.tsv> <replace|keep-both|merge|skip>
  gfm copy <source> <destination> [--replace|--keep-both|--merge|--skip]
  gfm operation-access-unavailable-volume-api <source> <destination> <volume-root>
  gfm move <source> <destination> [--replace|--keep-both|--merge|--skip]
  gfm rename <source> <destination> [--replace|--keep-both|--merge|--skip]
  gfm delete <path>
  gfm trash <path>
  gfm empty-trash <trash-dir>
  gfm restore <trash-entry> [original-path] [--replace|--keep-both|--merge|--skip]
  gfm operation-volume-copy-policy <source> <destination>"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn index_volume_signature_includes_native_apfs_and_media_identity() {
        let root = unique_temp_dir("gfm-app-volume-signature");
        let mut descriptor = VolumeDescriptor::for_path(&root).unwrap();
        descriptor.filesystem = Some("apfs".to_string());
        descriptor.mount_filesystem = Some("apfs".to_string());
        descriptor.volume_uuid = Some("VOLUME-UUID".to_string());
        descriptor.apfs_container_uuid = Some("APFS-CONTAINER-UUID".to_string());
        descriptor.apfs_role = Some(gfm_mac::ApfsVolumeRole::Data);
        descriptor.media_uuid = Some("APFS-CONTAINER-UUID".to_string());
        descriptor.resource_uuid = Some("RESOURCE-UUID".to_string());
        descriptor.bsd_name = Some("disk4s1".to_string());
        descriptor.bsd_major = Some(1);
        descriptor.bsd_minor = Some(2);
        descriptor.bsd_unit = Some(4);
        descriptor.mount_from = Some("/dev/disk4s1".to_string());
        descriptor.mount_flags = Some(0x0000_1000);
        descriptor.media_content = Some("Apple_APFS".to_string());
        descriptor.media_name = Some("Container disk4".to_string());
        descriptor.media_path = Some("IODeviceTree:/PCI0@0/AppleAPFSMedia".to_string());
        descriptor.volume_type = Some("apfs".to_string());
        descriptor.media_kind = Some("IOMedia".to_string());
        descriptor.media_type = Some("Generic".to_string());
        descriptor.case_sensitive = Some(true);
        descriptor.case_preserving = Some(true);
        descriptor.resource_automounted = Some(false);
        descriptor.resource_browsable = Some(true);
        descriptor.resource_encrypted = Some(true);
        descriptor.resource_reachable = Some(true);
        descriptor.resource_supports_file_cloning = Some(false);
        descriptor.resource_supports_hard_links = Some(true);
        descriptor.resource_supports_sparse_files = Some(true);
        descriptor.local = Some(true);
        descriptor.mount_local = Some(true);
        descriptor.internal = Some(false);
        descriptor.resource_remount_url = Some("file:///Volumes/Work".to_string());
        descriptor.writable = true;
        descriptor.read_only = false;
        descriptor.mount_read_only = Some(false);
        descriptor.ejectable = true;
        descriptor.mountable = Some(false);
        descriptor.device_protocol = Some("PCI-Express".to_string());
        descriptor.device_model = Some("External SSD".to_string());
        descriptor.device_vendor = Some("Samsung".to_string());
        descriptor.device_path = Some("IODeviceTree:/PCI0@0".to_string());
        descriptor.media_encrypted = None;
        descriptor.media_block_size_bytes = Some(4096);
        descriptor.media_size_bytes = Some(1024 * 1024 * 1024);

        let signature = index_volume_filesystem_signature(&descriptor);

        for token in [
            "fs=apfs",
            "mount-fs=apfs",
            "volume-uuid=VOLUME-UUID",
            "apfs-container-uuid=APFS-CONTAINER-UUID",
            "apfs-role=data",
            "media-uuid=APFS-CONTAINER-UUID",
            "resource-uuid=RESOURCE-UUID",
            "bsd=disk4s1",
            "bsd-major=1",
            "bsd-minor=2",
            "bsd-unit=4",
            "mount-from=/dev/disk4s1",
            "mount-flags=0x00001000",
            "media-content=Apple_APFS",
            "media-name=Container disk4",
            "media-path=IODeviceTree:/PCI0@0/AppleAPFSMedia",
            "volume-type=apfs",
            "media-kind=IOMedia",
            "media-type=Generic",
            "case-sensitive=1",
            "case-preserving=1",
            "automounted=0",
            "browsable=1",
            "resource-encrypted=1",
            "resource-reachable=1",
            "resource-cloning=0",
            "resource-hard-links=1",
            "resource-sparse=1",
            "local=1",
            "mount-local=1",
            "internal=0",
            "writable=1",
            "read-only=0",
            "mount-read-only=0",
            "ejectable=1",
            "mountable=0",
            "remount-url=file:///Volumes/Work",
            "protocol=PCI-Express",
            "model=External SSD",
            "vendor=Samsung",
            "device-path=IODeviceTree:/PCI0@0",
            "encrypted=1",
            "block-size=4096",
            "media-size=1073741824",
        ] {
            assert!(signature.contains(token), "{signature}");
        }

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn index_volume_filesystem_signature_tracks_bsd_identity_numbers() {
        let root = unique_temp_dir("gfm-app-volume-bsd-number-signature");
        let mut previous = VolumeDescriptor::for_path(&root).unwrap();
        previous.bsd_name = Some("disk4s1".to_string());
        previous.bsd_major = Some(1);
        previous.bsd_minor = Some(2);
        previous.bsd_unit = Some(4);
        let mut current = previous.clone();
        current.bsd_major = Some(8);
        current.bsd_minor = Some(9);
        current.bsd_unit = Some(10);

        let previous_signature = index_volume_filesystem_signature(&previous);
        let current_signature = index_volume_filesystem_signature(&current);

        assert_ne!(previous_signature, current_signature);
        assert!(previous_signature.contains("bsd-major=1"));
        assert!(previous_signature.contains("bsd-minor=2"));
        assert!(previous_signature.contains("bsd-unit=4"));
        assert!(current_signature.contains("bsd-major=8"));
        assert!(current_signature.contains("bsd-minor=9"));
        assert!(current_signature.contains("bsd-unit=10"));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn index_volume_signature_changes_when_operation_capabilities_change() {
        let root = unique_temp_dir("gfm-app-volume-operation-signature");
        let mut previous = VolumeDescriptor::for_path(&root).unwrap();
        previous.writable = true;
        previous.read_only = false;
        previous.ejectable = true;
        previous.mountable = Some(false);
        let mut current = previous.clone();
        current.writable = false;
        current.read_only = true;
        current.ejectable = false;
        current.mountable = Some(true);

        let previous_signature = index_volume_filesystem_signature(&previous);
        let current_signature = index_volume_filesystem_signature(&current);

        assert_ne!(previous_signature, current_signature);
        assert!(previous_signature.contains("writable=1"));
        assert!(previous_signature.contains("read-only=0"));
        assert!(previous_signature.contains("ejectable=1"));
        assert!(previous_signature.contains("mountable=0"));
        assert!(current_signature.contains("writable=0"));
        assert!(current_signature.contains("read-only=1"));
        assert!(current_signature.contains("ejectable=0"));
        assert!(current_signature.contains("mountable=1"));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn index_volume_descriptor_carries_operation_capabilities() {
        let root = unique_temp_dir("gfm-app-volume-operation-descriptor");
        let mut volume = VolumeDescriptor::for_path(&root).unwrap();
        volume.removable = true;
        volume.writable = false;
        volume.read_only = true;
        volume.ejectable = true;
        volume.mountable = Some(false);
        volume.case_sensitive = Some(true);

        let descriptor = index_volume_descriptor(&volume);

        assert_eq!(descriptor.writable, Some(false));
        assert_eq!(descriptor.read_only, Some(true));
        assert_eq!(descriptor.ejectable, Some(true));
        assert_eq!(descriptor.mountable, Some(false));
        assert_eq!(descriptor.case_sensitive, Some(true));
        assert!(descriptor
            .filesystem_signature
            .as_deref()
            .unwrap_or_default()
            .contains("removable=1"));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn index_volume_filesystem_signature_tracks_removable_media_truth() {
        let root = unique_temp_dir("gfm-app-volume-removable-signature");
        let mut previous = VolumeDescriptor::for_path(&root).unwrap();
        previous.removable = false;
        previous.ejectable = true;
        let mut current = previous.clone();
        current.removable = true;

        let previous_signature = index_volume_filesystem_signature(&previous);
        let current_signature = index_volume_filesystem_signature(&current);

        assert_ne!(previous_signature, current_signature);
        assert!(previous_signature.contains("removable=0"));
        assert!(current_signature.contains("removable=1"));
        assert!(previous_signature.contains("ejectable=1"));
        assert!(current_signature.contains("ejectable=1"));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn index_volume_filesystem_signature_tracks_root_filesystem_truth() {
        let root = unique_temp_dir("gfm-app-volume-root-filesystem-signature");
        let mut previous = VolumeDescriptor::for_path(&root).unwrap();
        previous.resource_root_file_system = Some(false);
        let mut current = previous.clone();
        current.resource_root_file_system = Some(true);

        let previous_signature = index_volume_filesystem_signature(&previous);
        let current_signature = index_volume_filesystem_signature(&current);

        assert_ne!(previous_signature, current_signature);
        assert!(previous_signature.contains("root-filesystem=0"));
        assert!(current_signature.contains("root-filesystem=1"));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn index_volume_descriptor_maps_disk_images_to_slow_class() {
        let root = unique_temp_dir("gfm-app-volume-disk-image-index-class");
        let mut volume = VolumeDescriptor::for_path(&root).unwrap();
        volume.kind = VolumeKind::DiskImage;

        let descriptor = index_volume_descriptor(&volume);

        assert_eq!(descriptor.class, IndexVolumeClass::Slow);

        std::fs::remove_dir_all(root).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let id = TEMP_COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("{prefix}-{}-{id}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }
}
