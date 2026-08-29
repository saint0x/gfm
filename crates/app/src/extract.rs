use crate::{
    access::{preflight_access_scope, ScopedAccessGuard},
    permission_refresh::refresh_permission_state_at_path,
};
use gfm_content::{
    ExtractionBatteryState, ExtractionBudgetProfile, ExtractionFingerprint, ExtractionQuarantine,
    ExtractionThermalState, ExtractionUserActivity, ExtractionVolumeClass, QuarantineDecision,
    QuarantineFailureKind,
};
use gfm_jobs::{
    Cancellation, JobBatteryState, JobIoPressure, JobThermalState, JobUserActivity,
    SchedulingPressure,
};
use gfm_mac::{AccessIntent, VolumeDescriptor, VolumeDiscoveryReport, VolumeKind};
use gfm_types::{GfmError, Result};
use std::env;
use std::fs;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub(crate) const ADAPTIVE_WORKER_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) fn extraction_budget_profile(
    root: &Path,
    pressure: SchedulingPressure,
) -> ExtractionBudgetProfile {
    ExtractionBudgetProfile {
        volume: extraction_volume_class_for_path(root),
        thermal: match pressure.thermal {
            JobThermalState::Nominal => ExtractionThermalState::Nominal,
            JobThermalState::Fair => ExtractionThermalState::Fair,
            JobThermalState::Serious => ExtractionThermalState::Serious,
            JobThermalState::Critical => ExtractionThermalState::Critical,
        },
        battery: match pressure.battery {
            JobBatteryState::AcPower => ExtractionBatteryState::AcPower,
            JobBatteryState::Battery => ExtractionBatteryState::Battery,
            JobBatteryState::LowPower => ExtractionBatteryState::LowPower,
        },
        user_activity: match pressure.user_activity {
            JobUserActivity::Idle => ExtractionUserActivity::Idle,
            JobUserActivity::Active => ExtractionUserActivity::Active,
        },
    }
}

fn extraction_volume_class_for_path(path: &Path) -> ExtractionVolumeClass {
    let report = VolumeDiscoveryReport::for_containing_path(path);
    extraction_volume_class_from_report(path, &report)
}

fn extraction_volume_class_from_report(
    path: &Path,
    report: &VolumeDiscoveryReport,
) -> ExtractionVolumeClass {
    if let Some(volume) = report.volume_for_path(path) {
        return extraction_volume_class_for_descriptor(volume);
    }
    fallback_extraction_volume_class_for_path(path)
}

fn extraction_volume_class_for_descriptor(volume: &VolumeDescriptor) -> ExtractionVolumeClass {
    if volume.network || volume.local == Some(false) || volume.kind == VolumeKind::Network {
        return ExtractionVolumeClass::Network;
    }
    if descriptor_reports_cloud_storage(volume) {
        return ExtractionVolumeClass::Cloud;
    }
    match volume.kind {
        VolumeKind::External | VolumeKind::Removable | VolumeKind::DiskImage => {
            ExtractionVolumeClass::External
        }
        VolumeKind::System | VolumeKind::Internal | VolumeKind::Unknown => {
            ExtractionVolumeClass::Local
        }
        VolumeKind::Network => ExtractionVolumeClass::Network,
    }
}

fn descriptor_reports_cloud_storage(volume: &VolumeDescriptor) -> bool {
    [
        volume.source.as_str(),
        volume.filesystem.as_deref().unwrap_or_default(),
        volume.volume_type.as_deref().unwrap_or_default(),
        volume.media_kind.as_deref().unwrap_or_default(),
        volume.media_type.as_deref().unwrap_or_default(),
        volume.mount_from.as_deref().unwrap_or_default(),
        volume.resource_remount_url.as_deref().unwrap_or_default(),
    ]
    .into_iter()
    .any(extraction_cloud_token)
}

fn fallback_extraction_volume_class_for_path(path: &Path) -> ExtractionVolumeClass {
    let normalized = path
        .to_string_lossy()
        .to_ascii_lowercase()
        .replace('\\', "/");
    if normalized.contains("/network/")
        || normalized.contains("/net/")
        || normalized.contains("/smb/")
        || normalized.contains("/nfs/")
    {
        ExtractionVolumeClass::Network
    } else if normalized.contains("/mobile documents/")
        || normalized.contains("cloud")
        || normalized.contains("fileprovider")
    {
        ExtractionVolumeClass::Cloud
    } else if normalized.starts_with("/volumes/") {
        ExtractionVolumeClass::External
    } else {
        ExtractionVolumeClass::Local
    }
}

fn extraction_cloud_token(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.contains("mobile documents")
        || value.contains("icloud")
        || value.contains("cloud")
        || value.contains("fileprovider")
}

pub(crate) fn run_adaptive_extraction_worker_cancellable(
    path: &Path,
    pressure: SchedulingPressure,
    timeout: Duration,
    cancellation: &Cancellation,
) -> Result<String> {
    cancellation.check()?;
    let _input_access =
        preflight_access_scope(path, AccessIntent::Read, "adaptive extraction worker")?;
    cancellation.check()?;
    let exe = env::current_exe().map_err(|err| {
        GfmError::Format(format!(
            "could not resolve current executable for extraction worker: {err}"
        ))
    })?;
    let stdout_path = worker_temp_path("stdout");
    let stderr_path = worker_temp_path("stderr");
    let permission_state_dir = worker_temp_dir("permission-state");
    let permission_state_path = permission_state_dir.join("state.tsv");
    let _scratch_access =
        retain_worker_scratch_access(&stdout_path, &stderr_path, &permission_state_dir)?;
    std::fs::File::create(&stdout_path).map_err(|err| GfmError::io(&stdout_path, err))?;
    std::fs::File::create(&stderr_path).map_err(|err| GfmError::io(&stderr_path, err))?;
    if let Err(err) = std::fs::create_dir(&permission_state_dir) {
        let _ = std::fs::remove_file(&stdout_path);
        let _ = std::fs::remove_file(&stderr_path);
        return Err(GfmError::io(&permission_state_dir, err));
    }
    if let Err(err) = refresh_permission_state_at_path(&permission_state_path) {
        let _ = std::fs::remove_file(&stdout_path);
        let _ = std::fs::remove_file(&stderr_path);
        let _ = std::fs::remove_dir_all(&permission_state_dir);
        return Err(err);
    }
    if let Err(err) = cancellation.check() {
        let _ = std::fs::remove_file(&stdout_path);
        let _ = std::fs::remove_file(&stderr_path);
        let _ = std::fs::remove_dir_all(&permission_state_dir);
        return Err(err);
    }
    let sandbox = WorkerSandbox::new(
        &exe,
        path,
        &stdout_path,
        &stderr_path,
        &permission_state_path,
    )?;
    let mut command = sandbox.command(&exe, path, pressure, &permission_state_path);
    let output = run_supervised_worker(
        &mut command,
        path,
        timeout,
        &stdout_path,
        &stderr_path,
        &permission_state_dir,
        cancellation,
    )?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(GfmError::Format(format!(
            "adaptive extraction worker failed for {} with status {}: {}",
            path.display(),
            output.status,
            stderr.trim()
        )));
    }
    String::from_utf8(output.stdout).map_err(|err| {
        GfmError::Format(format!(
            "adaptive extraction worker returned non-utf8 output for {}: {err}",
            path.display()
        ))
    })
}

pub(crate) fn run_quarantined_adaptive_extraction_worker_cancellable(
    path: &Path,
    store: &Path,
    pressure: SchedulingPressure,
    timeout: Duration,
    threshold: u32,
    cancellation: &Cancellation,
) -> Result<String> {
    cancellation.check()?;
    let _access =
        retain_extraction_quarantine_worker_access(path, store, "quarantined extraction worker")?;
    cancellation.check()?;
    let fingerprint = ExtractionFingerprint::for_path(path)?;
    cancellation.check()?;
    let mut quarantine = read_extraction_quarantine(store, threshold)?;
    cancellation.check()?;
    let decision = quarantine.before_extract(path, &fingerprint);
    if matches!(decision, QuarantineDecision::Quarantined(_)) {
        return Ok(format!("{}\n", decision.as_tsv()));
    }
    match run_adaptive_extraction_worker_cancellable(path, pressure, timeout, cancellation) {
        Ok(report) => {
            cancellation.check()?;
            let decision = quarantine.record_success(path, &fingerprint);
            cancellation.check()?;
            quarantine.write(store)?;
            Ok(format!("{report}{}\n", decision.as_tsv()))
        }
        Err(err) => {
            let message = err.to_string();
            let kind = worker_failure_kind(&message);
            let decision =
                quarantine.record_failure(path, &fingerprint, kind, worker_failure_reason(kind));
            cancellation.check()?;
            quarantine.write(store)?;
            Ok(format!("{}\n", decision.as_tsv()))
        }
    }
}

pub(crate) fn read_extraction_quarantine(
    store: &Path,
    threshold: u32,
) -> Result<ExtractionQuarantine> {
    if extraction_path_is_file(store, "quarantine store")? {
        ExtractionQuarantine::read(store)
    } else {
        Ok(ExtractionQuarantine::new(threshold))
    }
}

fn worker_failure_kind(message: &str) -> QuarantineFailureKind {
    if message.contains("timed out") {
        QuarantineFailureKind::Timeout
    } else {
        QuarantineFailureKind::Crash
    }
}

fn worker_failure_reason(kind: QuarantineFailureKind) -> &'static str {
    match kind {
        QuarantineFailureKind::Timeout => "worker-timeout",
        QuarantineFailureKind::Crash => "worker-crash",
        QuarantineFailureKind::Corrupt => "worker-corrupt",
        QuarantineFailureKind::Encrypted => "worker-encrypted",
    }
}

fn retain_extraction_quarantine_worker_access(
    path: &Path,
    store: &Path,
    worker: &str,
) -> Result<Vec<ScopedAccessGuard>> {
    Ok(vec![
        preflight_access_scope(path, AccessIntent::Read, worker)?,
        preflight_access_scope(write_probe_path(store)?, AccessIntent::Write, worker)?,
    ])
}

struct WorkerSandbox {
    sandbox_exec_path: Option<PathBuf>,
    profile_path: Option<PathBuf>,
}

impl WorkerSandbox {
    fn new(
        exe: &Path,
        input: &Path,
        stdout: &Path,
        stderr: &Path,
        permission_state: &Path,
    ) -> Result<Self> {
        let Some(sandbox_exec) = sandbox_exec_path()? else {
            return Ok(Self {
                sandbox_exec_path: None,
                profile_path: None,
            });
        };
        let profile = extraction_sandbox_profile(
            exe,
            input,
            stdout,
            stderr,
            permission_state,
            ExtractionSandboxReadMode::from_env(),
        )?;
        let profile_path = env::temp_dir().join(format!(
            "gfm-extract-worker-{}-{}.sb",
            std::process::id(),
            monotonic_nanos()
        ));
        let _profile_access = preflight_access_scope(
            write_probe_path(&profile_path)?,
            AccessIntent::Write,
            "adaptive extraction sandbox profile",
        )?;
        std::fs::write(&profile_path, profile).map_err(|err| GfmError::io(&profile_path, err))?;
        Ok(Self {
            sandbox_exec_path: Some(sandbox_exec),
            profile_path: Some(profile_path),
        })
    }

    fn command(
        &self,
        exe: &Path,
        input: &Path,
        pressure: SchedulingPressure,
        permission_state: &Path,
    ) -> Command {
        if let (Some(sandbox_exec), Some(profile_path)) =
            (&self.sandbox_exec_path, &self.profile_path)
        {
            let mut command = Command::new(sandbox_exec);
            command
                .arg("-f")
                .arg(profile_path)
                .arg(exe)
                .arg("extract-report-adaptive")
                .arg(input)
                .args(scheduling_pressure_args(pressure));
            command.env("GFM_PERMISSION_STATE", permission_state);
            command.env_remove("GFM_JOB_PAYLOAD_CATALOG");
            command.env_remove("GFM_JOB_PROGRESS_STORE");
            command
        } else {
            let mut command = Command::new(exe);
            command
                .arg("extract-report-adaptive")
                .arg(input)
                .args(scheduling_pressure_args(pressure));
            command.env("GFM_PERMISSION_STATE", permission_state);
            command.env_remove("GFM_JOB_PAYLOAD_CATALOG");
            command.env_remove("GFM_JOB_PROGRESS_STORE");
            command
        }
    }
}

impl Drop for WorkerSandbox {
    fn drop(&mut self) {
        if let Some(path) = self.profile_path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn run_supervised_worker(
    command: &mut Command,
    input: &Path,
    timeout: Duration,
    stdout_path: &Path,
    stderr_path: &Path,
    permission_state_dir: &Path,
    cancellation: &Cancellation,
) -> Result<std::process::Output> {
    cancellation.check()?;
    let stdout_file =
        std::fs::File::create(stdout_path).map_err(|err| GfmError::io(stdout_path, err))?;
    let stderr_file =
        std::fs::File::create(stderr_path).map_err(|err| GfmError::io(stderr_path, err))?;
    let mut child = command
        .process_group(0)
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        .spawn()
        .map_err(|err| {
            GfmError::Format(format!(
                "could not launch adaptive extraction worker for {}: {err}",
                input.display()
            ))
        })?;
    let start = Instant::now();
    loop {
        if let Err(err) = cancellation.check() {
            kill_process_group(child.id());
            let _ = child.wait();
            let _ = std::fs::remove_file(stdout_path);
            let _ = std::fs::remove_file(stderr_path);
            let _ = std::fs::remove_dir_all(permission_state_dir);
            return Err(err);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout =
                    std::fs::read(stdout_path).map_err(|err| GfmError::io(stdout_path, err))?;
                let stderr =
                    std::fs::read(stderr_path).map_err(|err| GfmError::io(stderr_path, err))?;
                let _ = std::fs::remove_file(stdout_path);
                let _ = std::fs::remove_file(stderr_path);
                let _ = std::fs::remove_dir_all(permission_state_dir);
                return Ok(std::process::Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) if start.elapsed() >= timeout => {
                kill_process_group(child.id());
                let _ = child.wait();
                let _ = std::fs::remove_file(stdout_path);
                let _ = std::fs::remove_file(stderr_path);
                let _ = std::fs::remove_dir_all(permission_state_dir);
                return Err(GfmError::Format(format!(
                    "adaptive extraction worker timed out after {} ms for {}",
                    timeout.as_millis(),
                    input.display()
                )));
            }
            Ok(None) => supervised_worker_poll_pause(Duration::from_millis(5), cancellation)?,
            Err(err) => {
                kill_process_group(child.id());
                let _ = child.wait();
                let _ = std::fs::remove_file(stdout_path);
                let _ = std::fs::remove_file(stderr_path);
                let _ = std::fs::remove_dir_all(permission_state_dir);
                return Err(GfmError::Format(format!(
                    "could not supervise adaptive extraction worker for {}: {err}",
                    input.display()
                )));
            }
        }
    }
}

fn kill_process_group(pid: u32) {
    let _ = Command::new("/bin/kill")
        .arg("-KILL")
        .arg(format!("-{pid}"))
        .status();
}

fn supervised_worker_poll_pause(delay: Duration, cancellation: &Cancellation) -> Result<()> {
    const CANCEL_GRANULARITY: Duration = Duration::from_millis(1);
    let deadline = Instant::now() + delay;
    loop {
        cancellation.check()?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        thread::sleep(remaining.min(CANCEL_GRANULARITY));
    }
}

fn worker_temp_path(label: &str) -> PathBuf {
    env::temp_dir().join(format!(
        "gfm-extract-worker-{}-{}-{label}",
        std::process::id(),
        monotonic_nanos()
    ))
}

fn worker_temp_dir(label: &str) -> PathBuf {
    env::temp_dir().join(format!(
        "gfm-extract-worker-{}-{}-{label}.d",
        std::process::id(),
        monotonic_nanos()
    ))
}

fn retain_worker_scratch_access(
    stdout_path: &Path,
    stderr_path: &Path,
    permission_state_dir: &Path,
) -> Result<Vec<ScopedAccessGuard>> {
    Ok(vec![
        preflight_access_scope(
            write_probe_path(stdout_path)?,
            AccessIntent::Write,
            "adaptive extraction stdout",
        )?,
        preflight_access_scope(
            write_probe_path(stderr_path)?,
            AccessIntent::Write,
            "adaptive extraction stderr",
        )?,
        preflight_access_scope(
            write_probe_path(permission_state_dir)?,
            AccessIntent::Write,
            "adaptive extraction permission state",
        )?,
    ])
}

fn extraction_path_is_file(path: &Path, label: &str) -> Result<bool> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(metadata.is_file()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(GfmError::io(
            path,
            format!("extraction {label} metadata unavailable: {err}"),
        )),
    }
}

fn write_probe_path(path: &Path) -> Result<&Path> {
    match path.try_exists() {
        Ok(true) => Ok(path),
        Ok(false) => Ok(crate::parent_or_cwd(path)),
        Err(err) => Err(GfmError::io(
            path,
            format!("extraction write path existence unavailable: {err}"),
        )),
    }
}

fn sandbox_exec_path() -> Result<Option<PathBuf>> {
    let path = PathBuf::from("/usr/bin/sandbox-exec");
    Ok(extraction_path_is_file(&path, "sandbox-exec")?.then_some(path))
}

fn extraction_sandbox_profile(
    exe: &Path,
    input: &Path,
    stdout: &Path,
    stderr: &Path,
    permission_state: &Path,
    read_mode: ExtractionSandboxReadMode,
) -> Result<String> {
    let exe = canonical_or_self(exe)?;
    let input = canonical_or_self(input)?;
    let stdout = canonical_or_self(stdout)?;
    let stderr = canonical_or_self(stderr)?;
    let permission_state = canonical_or_self(permission_state)?;
    let permission_state_dir = permission_state
        .parent()
        .map(canonical_or_self)
        .transpose()?
        .unwrap_or_else(env::temp_dir);
    let mut profile = String::from("(version 1)\n(allow default)\n");
    if read_mode == ExtractionSandboxReadMode::Strict {
        profile.push_str(&format!(
            "(deny file-read*)\n\
             (allow file-read* (literal \"{}\"))\n\
             (allow file-read* (literal \"{}\"))\n\
             (allow file-read* (subpath \"{}\"))\n\
             (allow file-read* (literal \"/dev/null\"))\n\
             (allow file-read* (literal \"/dev/random\"))\n\
             (allow file-read* (literal \"/dev/urandom\"))\n\
             (allow file-read* (subpath \"/usr/lib\"))\n\
             (allow file-read* (subpath \"/usr/share\"))\n\
             (allow file-read* (subpath \"/System/Library\"))\n\
             (allow file-read* (subpath \"/System/Volumes/Preboot/Cryptexes\"))\n\
             (allow file-read* (subpath \"/Library/Apple\"))\n\
             (allow file-read* (subpath \"/private/var/db\"))\n",
            sandbox_escape(&exe),
            sandbox_escape(&input),
            sandbox_escape(&permission_state_dir)
        ));
    }
    profile.push_str(&format!(
        "(deny file-write*)\n\
         (allow file-write-data (literal \"{}\"))\n\
         (allow file-write-data (literal \"{}\"))\n\
         (allow file-write* (subpath \"{}\"))\n",
        sandbox_escape(&stdout),
        sandbox_escape(&stderr),
        sandbox_escape(&permission_state_dir)
    ));
    Ok(profile)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtractionSandboxReadMode {
    Ambient,
    Strict,
}

impl ExtractionSandboxReadMode {
    fn from_env() -> Self {
        match env::var("GFM_EXTRACTION_SANDBOX_READ_MODE") {
            Ok(value) if value == "strict" => Self::Strict,
            _ => Self::Ambient,
        }
    }
}

fn canonical_or_self(path: &Path) -> Result<PathBuf> {
    path.canonicalize().map_err(|err| GfmError::io(path, err))
}

fn sandbox_escape(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

fn monotonic_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

fn scheduling_pressure_args(pressure: SchedulingPressure) -> [&'static str; 4] {
    [
        job_io_pressure_arg(pressure.io),
        job_thermal_state_arg(pressure.thermal),
        job_battery_state_arg(pressure.battery),
        job_user_activity_arg(pressure.user_activity),
    ]
}

fn job_io_pressure_arg(value: JobIoPressure) -> &'static str {
    match value {
        JobIoPressure::Nominal => "nominal",
        JobIoPressure::Elevated => "elevated",
        JobIoPressure::Saturated => "saturated",
    }
}

fn job_thermal_state_arg(value: JobThermalState) -> &'static str {
    match value {
        JobThermalState::Nominal => "nominal",
        JobThermalState::Fair => "fair",
        JobThermalState::Serious => "serious",
        JobThermalState::Critical => "critical",
    }
}

fn job_battery_state_arg(value: JobBatteryState) -> &'static str {
    match value {
        JobBatteryState::AcPower => "ac",
        JobBatteryState::Battery => "battery",
        JobBatteryState::LowPower => "low",
    }
}

fn job_user_activity_arg(value: JobUserActivity) -> &'static str {
    match value {
        JobUserActivity::Idle => "idle",
        JobUserActivity::Active => "active",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn extraction_volume_class_uses_discovered_network_descriptor() {
        let root = unique_temp_dir("gfm-extract-volume-network-descriptor");
        let volume_root = root.join("TeamShare");
        fs::create_dir_all(&volume_root).unwrap();
        fs::write(volume_root.join(".gfm-volume-kind"), "network-smb\n").unwrap();
        let input = volume_root.join("Project.md");
        fs::write(&input, "network").unwrap();
        let report = VolumeDiscoveryReport::from_paths(vec![volume_root.clone()]);

        assert_eq!(
            extraction_volume_class_from_report(&input, &report),
            ExtractionVolumeClass::Network
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extraction_volume_class_uses_discovered_external_descriptor() {
        let root = unique_temp_dir("gfm-extract-volume-external-descriptor");
        let volume_root = root.join("CameraCard");
        fs::create_dir_all(&volume_root).unwrap();
        fs::write(volume_root.join(".gfm-volume-kind"), "external-removable\n").unwrap();
        let input = volume_root.join("Clip.mov");
        fs::write(&input, "external").unwrap();
        let report = VolumeDiscoveryReport::from_paths(vec![volume_root.clone()]);

        assert_eq!(
            extraction_volume_class_from_report(&input, &report),
            ExtractionVolumeClass::External
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extraction_volume_class_uses_cloud_provider_descriptor_before_path_text() {
        let root = unique_temp_dir("gfm-extract-volume-cloud-descriptor");
        let volume_root = root.join("ProviderRoot");
        fs::create_dir_all(&volume_root).unwrap();
        let input = volume_root.join("Remote.md");
        fs::write(&input, "cloud").unwrap();
        let mut report = VolumeDiscoveryReport::from_paths(vec![volume_root.clone()]);
        report.volumes[0].source = "native:fileprovider".to_string();
        report.volumes[0].filesystem = Some("apfs".to_string());
        report.volumes[0].kind = VolumeKind::Internal;

        assert_eq!(
            extraction_volume_class_from_report(&input, &report),
            ExtractionVolumeClass::Cloud
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extraction_sandbox_profile_default_confines_writes_without_read_deny() {
        let fixture = SandboxProfileFixture::new("default");

        let profile = fixture.profile(ExtractionSandboxReadMode::Ambient);

        assert!(!profile.contains("(deny file-read*)"), "{profile}");
        assert!(profile.contains("(deny file-write*)"), "{profile}");
        assert!(profile.contains(&format!(
            "(allow file-write-data (literal \"{}\"))",
            sandbox_escape(&fixture.stdout.canonicalize().unwrap())
        )));
        assert!(profile.contains(&format!(
            "(allow file-write-data (literal \"{}\"))",
            sandbox_escape(&fixture.stderr.canonicalize().unwrap())
        )));
        assert!(profile.contains(&format!(
            "(allow file-write* (subpath \"{}\"))",
            sandbox_escape(&fixture.permission_dir.canonicalize().unwrap())
        )));
    }

    #[test]
    fn extraction_sandbox_profile_strict_mode_denies_ambient_reads_and_writes() {
        let fixture = SandboxProfileFixture::new("strict");

        let profile = fixture.profile(ExtractionSandboxReadMode::Strict);

        assert!(profile.contains("(deny file-read*)"), "{profile}");
        assert!(profile.contains("(deny file-write*)"), "{profile}");
        assert!(profile.contains(&format!(
            "(allow file-read* (literal \"{}\"))",
            sandbox_escape(&fixture.exe.canonicalize().unwrap())
        )));
        assert!(profile.contains(&format!(
            "(allow file-read* (literal \"{}\"))",
            sandbox_escape(&fixture.input.canonicalize().unwrap())
        )));
        assert!(profile.contains(&format!(
            "(allow file-read* (subpath \"{}\"))",
            sandbox_escape(&fixture.permission_dir.canonicalize().unwrap())
        )));
        assert!(
            !profile.contains(&format!(
                "(allow file-read* (subpath \"{}\"))",
                sandbox_escape(&fixture.root)
            )),
            "{profile}"
        );
    }

    #[test]
    fn quarantined_worker_refuses_unreachable_store_before_fingerprinting() {
        let source = unique_temp_dir("gfm-extract-quarantine-worker-source");
        let offline = unique_temp_dir("gfm-extract-quarantine-worker-offline");
        fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
        let input = source.join("document.txt");
        let store = offline.join("quarantine.gfmquarantine");
        fs::write(&input, "worker should not launch").unwrap();
        let cancellation = Cancellation::default();

        let err = run_quarantined_adaptive_extraction_worker_cancellable(
            &input,
            &store,
            SchedulingPressure::default(),
            Duration::from_millis(0),
            2,
            &cancellation,
        )
        .unwrap_err();

        assert!(matches!(err, GfmError::Permission { .. }));
        assert!(err
            .to_string()
            .contains("quarantined extraction worker volume access blocked"));
        assert!(err.to_string().contains("unreachable volume network"));
        assert!(!store.exists());

        fs::remove_dir_all(source).unwrap();
        fs::remove_dir_all(offline).unwrap();
    }

    #[test]
    fn quarantine_reader_surfaces_store_probe_failure() {
        let root = unique_temp_dir("gfm-extract-quarantine-store-probe");
        let store = root.join("quarantine-store-unavailable".repeat(16));

        let err = read_extraction_quarantine(&store, 2).unwrap_err();

        assert!(matches!(err, GfmError::Io { .. }));
        assert!(err
            .to_string()
            .contains("extraction quarantine store metadata unavailable"));
        assert!(err.to_string().contains(&store.display().to_string()));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extraction_write_probe_surfaces_path_probe_failure() {
        let root = unique_temp_dir("gfm-extract-write-probe");
        let output = root.join("extraction-output-unavailable".repeat(16));

        let err = write_probe_path(&output).unwrap_err();

        assert!(matches!(err, GfmError::Io { .. }));
        assert!(err
            .to_string()
            .contains("extraction write path existence unavailable"));
        assert!(err.to_string().contains(&output.display().to_string()));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn adaptive_worker_refuses_missing_input_before_scratch_setup() {
        let root = unique_temp_dir("gfm-extract-worker-input-missing");
        let input = root.join("missing.txt");
        let cancellation = Cancellation::default();
        let scratch_before = worker_scratch_entries();

        let err = run_adaptive_extraction_worker_cancellable(
            &input,
            SchedulingPressure::default(),
            Duration::from_millis(0),
            &cancellation,
        )
        .unwrap_err();

        assert!(matches!(err, GfmError::Permission { .. }));
        assert!(err
            .to_string()
            .contains("adaptive extraction worker access blocked"));
        assert!(err.to_string().contains("path is not present on this host"));
        assert_eq!(scratch_before, worker_scratch_entries());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn supervised_worker_poll_pause_returns_promptly_after_cancellation() {
        let cancellation = Cancellation::default();
        let canceller = cancellation.clone();
        let started = Instant::now();
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(5));
            canceller.cancel();
        });

        let err = supervised_worker_poll_pause(Duration::from_millis(250), &cancellation)
            .expect_err("poll pause should observe cancellation");

        handle.join().unwrap();
        assert_eq!(err, GfmError::Cancelled);
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "cancelled poll pause waited {:?}",
            started.elapsed()
        );
    }

    struct SandboxProfileFixture {
        root: PathBuf,
        exe: PathBuf,
        input: PathBuf,
        stdout: PathBuf,
        stderr: PathBuf,
        permission_dir: PathBuf,
        permission_state: PathBuf,
    }

    impl SandboxProfileFixture {
        fn new(name: &str) -> Self {
            let root = unique_temp_dir(&format!("gfm-extract-sandbox-profile-{name}"));
            let exe = root.join("gfm");
            let input = root.join("input.txt");
            let stdout = root.join("stdout");
            let stderr = root.join("stderr");
            let permission_dir = root.join("permission-state");
            let permission_state = permission_dir.join("state.tsv");
            fs::write(&exe, "binary").unwrap();
            fs::write(&input, "body").unwrap();
            fs::write(&stdout, "").unwrap();
            fs::write(&stderr, "").unwrap();
            fs::create_dir(&permission_dir).unwrap();
            fs::write(&permission_state, "gfm-permission-state-v1\n").unwrap();
            Self {
                root,
                exe,
                input,
                stdout,
                stderr,
                permission_dir,
                permission_state,
            }
        }

        fn profile(&self, read_mode: ExtractionSandboxReadMode) -> String {
            extraction_sandbox_profile(
                &self.exe,
                &self.input,
                &self.stdout,
                &self.stderr,
                &self.permission_state,
                read_mode,
            )
            .unwrap()
        }
    }

    impl Drop for SandboxProfileFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let path = env::temp_dir().join(format!(
            "{}-{}-{}",
            prefix,
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn worker_scratch_entries() -> Vec<PathBuf> {
        let prefix = format!("gfm-extract-worker-{}-", std::process::id());
        let mut entries = fs::read_dir(env::temp_dir())
            .unwrap()
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(&prefix))
            })
            .collect::<Vec<_>>();
        entries.sort();
        entries
    }
}
