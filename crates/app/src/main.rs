use gfm_config::ConfigStore;
use gfm_content::QuarantineFailureKind;
use gfm_index::{
    BatteryState, IndexMountState, IndexVolumeClass, IndexVolumeDescriptor, IoPressure,
    ThermalState, UserActivity,
};
use gfm_jobs::{
    JobBatteryState, JobIoPressure, JobThermalState, JobUserActivity, Priority, SchedulingPressure,
};
use gfm_mac::{
    current_host_profile, current_permission_onboarding, MountState, SupportMatrix,
    VolumeDescriptor, VolumeKind,
};
use gfm_types::{GfmError, Result, VolumeId};
use std::env;
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
mod platform;
mod runtime;
mod search;

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
            let config = store.load_or_create_default()?;
            println!("{}\t{}", config.schema_version, store.path().display());
        }
        Some("config-check") => {
            let store = config_store(args.next())?;
            let config = store.load()?;
            config.validate()?;
            println!("{}\t{}", config.schema_version, store.path().display());
        }
        Some("config-dump") => {
            let store = config_store(args.next())?;
            let config = store.load_or_create_default()?;
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
}

fn index_volume_class(kind: VolumeKind) -> IndexVolumeClass {
    match kind {
        VolumeKind::System => IndexVolumeClass::System,
        VolumeKind::Internal => IndexVolumeClass::Internal,
        VolumeKind::External | VolumeKind::Removable | VolumeKind::DiskImage => {
            IndexVolumeClass::External
        }
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

pub(crate) fn config_store(value: Option<String>) -> Result<ConfigStore> {
    value
        .map(|path| Ok(ConfigStore::new(path)))
        .unwrap_or_else(ConfigStore::platform_default)
}

pub(crate) fn parent_volume(path: &Path) -> Option<VolumeId> {
    path.parent()
        .and_then(|parent| detect_volume_id(parent).ok())
}

pub(crate) fn run_preview_contract_cancellable<T>(
    volume: Option<VolumeId>,
    label: &'static str,
    build: impl FnOnce(gfm_jobs::Cancellation) -> Result<T> + Send + 'static,
) -> Result<T>
where
    T: Send + 'static,
{
    runtime::run_volume_task_cancellable(volume, Priority::Visible, label, build)
}

pub(crate) fn run_preview_contract_adaptive<T>(
    volume: Option<VolumeId>,
    priority: Priority,
    label: &'static str,
    pressure: SchedulingPressure,
    build: impl Fn(gfm_jobs::Cancellation) -> Result<T> + Send + Sync + 'static,
) -> Result<runtime::ScheduledTaskOutcome<T>>
where
    T: Send + 'static,
{
    runtime::run_scheduled_volume_task_cancellable(volume, priority, label, pressure, build)
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
  gfm ui-progress-job-contract <progress.gfmprogress> <job-id>
  gfm ui-fileprovider-conflict-contract <fileprovider-path>
  gfm ui-titlebar-contract [path]
  gfm ui-session-contract [path] [window-session.tsv]
  gfm ui-toolbar-contract [path]
  gfm ui-sidebar-contract [path]
  gfm ui-sidebar-fileprovider-contract [current-path] <fileprovider-path>
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
  gfm security-scope <path> [read|write|index|preview|operate]
  gfm mac-bridges
  gfm native-icon <path>
  gfm native-icon-bridge <path>
  gfm fileprovider-state <path>
  gfm fileprovider-state-with-identity <path>
  gfm fileprovider-domain <path>
  gfm fileprovider-domains
  gfm fileprovider-progress <path>
  gfm fileprovider-conflict <path>
  gfm fileprovider-progress-job <path>
  gfm fileprovider-operation <download|evict> <path>
  gfm fileprovider-invalidation <previous-state> <path>
  gfm volume-discovery [paths...]
  gfm volume-operation <eject|unmount|mount> <path>
  gfm volume-index-policy <external:disabled|opt-in|enabled> <network:disabled|opt-in|enabled> [opt-in:path...] [paths...]
  gfm volume-invalidation <previous-class> <previous-mount> <path>
  gfm volume-topology-diff <previous-paths...> -- <current-paths...>
  gfm spotlight-reconcile <path> [spotlight-fixture.tsv]
  gfm preview-check <path> [icon|thumbnail|quick-look|text]
  gfm icon-preview <path>
  gfm quicklook-session <path>
  gfm quicklook-session-adaptive <path> <nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active>
  gfm quicklook-session-cancel <path>
  gfm thumbnail-generation <path>
  gfm thumbnail-generation-adaptive <path> <nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active>
  gfm thumbnail-generation-cancel <path>
  gfm preview-schedule
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
  gfm jobs-payload-restore-plan <catalog.gfmjobs> <progress.gfmprogress> [updated-ms]
  gfm jobs-cancel-tree
  gfm jobs-runtime-retry-probe <attempt-state> [<nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active>]
  gfm ops-recover [ops.journal] [--retry-failed] [--max-attempts N]
  gfm watch-once <root>
  gfm copy <source> <destination> [--replace|--keep-both|--merge|--skip]
  gfm move <source> <destination> [--replace|--keep-both|--merge|--skip]
  gfm rename <source> <destination> [--replace|--keep-both|--merge|--skip]
  gfm delete <path>
  gfm trash <path>
  gfm empty-trash <trash-dir>
  gfm restore <trash-entry> [original-path] [--replace|--keep-both|--merge|--skip]"
    );
}
