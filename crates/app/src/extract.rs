use crate::{
    access::{
        preflight_access_scope_checked, preflight_volume_access_scope_with_report,
        ScopedAccessGuard,
    },
    permission_refresh::refresh_permission_state_at_path_checked,
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
use std::io::{self, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub(crate) const ADAPTIVE_WORKER_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) fn extraction_budget_profile_checked(
    root: &Path,
    pressure: SchedulingPressure,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<ExtractionBudgetProfile> {
    check_control()?;
    Ok(ExtractionBudgetProfile {
        volume: extraction_volume_class_for_path_checked(root, &mut check_control)?,
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
    })
}

fn extraction_volume_class_for_path_checked(
    path: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<ExtractionVolumeClass> {
    check_control()?;
    let report = VolumeDiscoveryReport::for_containing_path_checked(path, &mut check_control)?;
    check_control()?;
    Ok(extraction_volume_class_from_report(path, &report))
}

fn extraction_volume_class_from_report(
    path: &Path,
    report: &VolumeDiscoveryReport,
) -> ExtractionVolumeClass {
    if let Some(volume) = report.volume_for_path(path) {
        return extraction_volume_class_for_descriptor(volume);
    }
    conservative_unknown_extraction_volume_class()
}

fn extraction_volume_class_for_descriptor(volume: &VolumeDescriptor) -> ExtractionVolumeClass {
    if volume.platform_state_unavailable() {
        return conservative_unknown_extraction_volume_class();
    }
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
        VolumeKind::System | VolumeKind::Internal => ExtractionVolumeClass::Local,
        VolumeKind::Unknown => conservative_unknown_extraction_volume_class(),
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

fn conservative_unknown_extraction_volume_class() -> ExtractionVolumeClass {
    ExtractionVolumeClass::Network
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
    let _input_access = preflight_access_scope_checked(
        path,
        AccessIntent::Read,
        "adaptive extraction worker",
        || cancellation.check(),
    )?;
    cancellation.check()?;
    let exe = env::current_exe().map_err(|err| {
        GfmError::Format(format!(
            "could not resolve current executable for extraction worker: {err}"
        ))
    })?;
    let scratch = WorkerScratch::prepare_checked(|| cancellation.check())?;
    let sandbox = WorkerSandbox::new_checked(
        &exe,
        path,
        &scratch.stdout_path,
        &scratch.stderr_path,
        &scratch.permission_state_path,
        || cancellation.check(),
    )?;
    let mut command = sandbox.command(&exe, path, pressure, &scratch);
    let output = run_supervised_worker(
        &mut command,
        path,
        timeout,
        scratch.stdout_path(),
        scratch.stderr_path(),
        scratch.permission_state_dir(),
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

pub(crate) fn preflight_adaptive_extraction_worker_scratch() -> Result<Vec<ScopedAccessGuard>> {
    preflight_worker_scratch_volume_checked(|| Ok(()))?;
    let stdout_path = worker_temp_path("stdout-probe");
    let stderr_path = worker_temp_path("stderr-probe");
    let permission_state_dir = worker_temp_dir("permission-state-probe");
    retain_worker_scratch_access(&stdout_path, &stderr_path, &permission_state_dir)
}

pub(crate) fn preflight_adaptive_extraction_worker_scratch_checked(
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    check_control()?;
    preflight_worker_scratch_volume_checked(&mut check_control)?;
    check_control()?;
    let stdout_path = worker_temp_path("stdout-probe");
    let stderr_path = worker_temp_path("stderr-probe");
    let permission_state_dir = worker_temp_dir("permission-state-probe");
    retain_worker_scratch_access_checked(
        &stdout_path,
        &stderr_path,
        &permission_state_dir,
        check_control,
    )
}

struct WorkerScratch {
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    permission_state_dir: PathBuf,
    permission_state_path: PathBuf,
    job_journal_path: PathBuf,
    job_payload_catalog_path: PathBuf,
    job_progress_store_path: PathBuf,
    _access_guards: Vec<ScopedAccessGuard>,
}

impl WorkerScratch {
    fn prepare_checked(mut check_control: impl FnMut() -> Result<()>) -> Result<Self> {
        let stdout_path = worker_temp_path("stdout");
        let stderr_path = worker_temp_path("stderr");
        let permission_state_dir = worker_temp_dir("permission-state");
        let permission_state_path = permission_state_dir.join("state.tsv");
        let job_journal_path = permission_state_dir.join("jobs.journal");
        let job_payload_catalog_path = permission_state_dir.join("payloads.gfmjobs");
        let job_progress_store_path = permission_state_dir.join("progress.gfmprogress");
        let access_guards = retain_worker_scratch_access_checked(
            &stdout_path,
            &stderr_path,
            &permission_state_dir,
            &mut check_control,
        )?;
        check_control()?;
        let scratch = Self {
            stdout_path,
            stderr_path,
            permission_state_dir,
            permission_state_path,
            job_journal_path,
            job_payload_catalog_path,
            job_progress_store_path,
            _access_guards: access_guards,
        };
        scratch.create_checked(&mut check_control)?;
        Ok(scratch)
    }

    fn create_checked(&self, mut check_control: impl FnMut() -> Result<()>) -> Result<()> {
        if let Err(err) = self.create_files_checked(&mut check_control) {
            self.cleanup();
            return Err(err);
        }
        Ok(())
    }

    fn create_files_checked(&self, mut check_control: impl FnMut() -> Result<()>) -> Result<()> {
        check_control()?;
        std::fs::File::create(&self.stdout_path)
            .map_err(|err| GfmError::io(&self.stdout_path, err))?;
        check_control()?;
        std::fs::File::create(&self.stderr_path)
            .map_err(|err| GfmError::io(&self.stderr_path, err))?;
        check_control()?;
        std::fs::create_dir(&self.permission_state_dir)
            .map_err(|err| GfmError::io(&self.permission_state_dir, err))?;
        check_control()?;
        refresh_permission_state_at_path_checked(&self.permission_state_path, &mut check_control)?;
        check_control()?;
        Ok(())
    }

    fn stdout_path(&self) -> &Path {
        &self.stdout_path
    }

    fn stderr_path(&self) -> &Path {
        &self.stderr_path
    }

    fn permission_state_dir(&self) -> &Path {
        &self.permission_state_dir
    }

    fn cleanup(&self) {
        let _ = std::fs::remove_file(&self.stdout_path);
        let _ = std::fs::remove_file(&self.stderr_path);
        let _ = std::fs::remove_dir_all(&self.permission_state_dir);
    }
}

impl Drop for WorkerScratch {
    fn drop(&mut self) {
        self.cleanup();
    }
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
    let _access = retain_extraction_quarantine_worker_access_checked(
        path,
        store,
        "quarantined extraction worker",
        || cancellation.check(),
    )?;
    cancellation.check()?;
    let _scratch_access =
        preflight_adaptive_extraction_worker_scratch_checked(|| cancellation.check())?;
    cancellation.check()?;
    let fingerprint = ExtractionFingerprint::for_path_checked(path, || cancellation.check())?;
    cancellation.check()?;
    let mut quarantine = read_extraction_quarantine_cancellable(store, threshold, cancellation)?;
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
            quarantine.write_checked(store, || cancellation.check())?;
            Ok(format!("{report}{}\n", decision.as_tsv()))
        }
        Err(err) => {
            let message = err.to_string();
            let kind = worker_failure_kind(&message);
            let decision =
                quarantine.record_failure(path, &fingerprint, kind, worker_failure_reason(kind));
            cancellation.check()?;
            quarantine.write_checked(store, || cancellation.check())?;
            Ok(format!("{}\n", decision.as_tsv()))
        }
    }
}

pub(crate) fn read_extraction_quarantine_cancellable(
    store: &Path,
    threshold: u32,
    cancellation: &Cancellation,
) -> Result<ExtractionQuarantine> {
    read_extraction_quarantine_checked(store, threshold, || cancellation.check())
}

fn read_extraction_quarantine_checked(
    store: &Path,
    threshold: u32,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<ExtractionQuarantine> {
    check_control()?;
    if extraction_path_is_file(store, "quarantine store")? {
        check_control()?;
        ExtractionQuarantine::read_checked(store, &mut check_control)
    } else {
        check_control()?;
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

fn retain_extraction_quarantine_worker_access_checked(
    path: &Path,
    store: &Path,
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    check_control()?;
    let store_probe = checked_write_probe_path(store, "extraction quarantine", &mut check_control)?;
    check_control()?;
    Ok(vec![
        preflight_access_scope_checked(path, AccessIntent::Read, worker, &mut check_control)?,
        preflight_access_scope_checked(
            &store_probe,
            AccessIntent::Write,
            worker,
            &mut check_control,
        )?,
    ])
}

struct WorkerSandbox {
    sandbox_exec_path: Option<PathBuf>,
    profile_path: Option<PathBuf>,
}

impl WorkerSandbox {
    fn new_checked(
        exe: &Path,
        input: &Path,
        stdout: &Path,
        stderr: &Path,
        permission_state: &Path,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        check_control()?;
        let Some(sandbox_exec) = sandbox_exec_path()? else {
            return Ok(Self {
                sandbox_exec_path: None,
                profile_path: None,
            });
        };
        check_control()?;
        let profile = extraction_sandbox_profile(
            exe,
            input,
            stdout,
            stderr,
            permission_state,
            ExtractionSandboxReadMode::from_env(),
        )?;
        check_control()?;
        let profile_path = env::temp_dir().join(format!(
            "gfm-extract-worker-{}-{}.sb",
            std::process::id(),
            monotonic_nanos()
        ));
        check_control()?;
        let profile_probe = write_probe_path(&profile_path)?.to_path_buf();
        check_control()?;
        let _profile_access = preflight_access_scope_checked(
            &profile_probe,
            AccessIntent::Write,
            "adaptive extraction sandbox profile",
            &mut check_control,
        )?;
        if let Err(err) = write_worker_sandbox_profile_checked(
            &profile_path,
            profile.as_bytes(),
            &mut check_control,
        ) {
            let _ = std::fs::remove_file(&profile_path);
            return Err(err);
        }
        check_control()?;
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
        scratch: &WorkerScratch,
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
            command.env("GFM_PERMISSION_STATE", &scratch.permission_state_path);
            command.env("GFM_JOB_JOURNAL", &scratch.job_journal_path);
            command.env("GFM_JOB_PAYLOAD_CATALOG", &scratch.job_payload_catalog_path);
            command.env("GFM_JOB_PROGRESS_STORE", &scratch.job_progress_store_path);
            command
        } else {
            let mut command = Command::new(exe);
            command
                .arg("extract-report-adaptive")
                .arg(input)
                .args(scheduling_pressure_args(pressure));
            command.env("GFM_PERMISSION_STATE", &scratch.permission_state_path);
            command.env("GFM_JOB_JOURNAL", &scratch.job_journal_path);
            command.env("GFM_JOB_PAYLOAD_CATALOG", &scratch.job_payload_catalog_path);
            command.env("GFM_JOB_PROGRESS_STORE", &scratch.job_progress_store_path);
            command
        }
    }
}

fn write_worker_sandbox_profile_checked(
    path: &Path,
    bytes: &[u8],
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    const CHUNK_BYTES: usize = 64 * 1024;

    check_control()?;
    let mut file = std::fs::File::create(path).map_err(|err| GfmError::io(path, err))?;
    check_control()?;
    for chunk in bytes.chunks(CHUNK_BYTES) {
        check_control()?;
        file.write_all(chunk)
            .map_err(|err| GfmError::io(path, err))?;
        check_control()?;
    }
    file.flush().map_err(|err| GfmError::io(path, err))?;
    check_control()?;
    Ok(())
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
                let output =
                    read_supervised_worker_output(status, stdout_path, stderr_path, cancellation);
                let _ = std::fs::remove_file(stdout_path);
                let _ = std::fs::remove_file(stderr_path);
                let _ = std::fs::remove_dir_all(permission_state_dir);
                return output;
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

fn read_supervised_worker_output(
    status: std::process::ExitStatus,
    stdout_path: &Path,
    stderr_path: &Path,
    cancellation: &Cancellation,
) -> Result<std::process::Output> {
    cancellation.check()?;
    let stdout = read_worker_output_file_checked(stdout_path, || cancellation.check())?;
    cancellation.check()?;
    let stderr = read_worker_output_file_checked(stderr_path, || cancellation.check())?;
    cancellation.check()?;
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

fn read_worker_output_file_checked(
    path: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<u8>> {
    const CHUNK_BYTES: usize = 256 * 1024;

    check_control()?;
    let mut file = std::fs::File::open(path).map_err(|err| GfmError::io(path, err))?;
    check_control()?;
    let mut bytes = Vec::new();
    let mut buffer = [0; CHUNK_BYTES];
    loop {
        check_control()?;
        let len = file
            .read(&mut buffer)
            .map_err(|err| GfmError::io(path, err))?;
        if len == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..len]);
        check_control()?;
    }
    check_control()?;
    Ok(bytes)
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

fn preflight_worker_scratch_volume_checked(
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    check_control()?;
    let temp_dir = env::temp_dir();
    let report = VolumeDiscoveryReport::for_containing_path_checked(&temp_dir, &mut check_control)?;
    check_control()?;
    preflight_volume_access_scope_with_report(
        &temp_dir,
        AccessIntent::Write,
        "adaptive extraction",
        &report,
    )
}

fn retain_worker_scratch_access(
    stdout_path: &Path,
    stderr_path: &Path,
    permission_state_dir: &Path,
) -> Result<Vec<ScopedAccessGuard>> {
    retain_worker_scratch_access_checked(stdout_path, stderr_path, permission_state_dir, || Ok(()))
}

fn retain_worker_scratch_access_checked(
    stdout_path: &Path,
    stderr_path: &Path,
    permission_state_dir: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    check_control()?;
    let stdout_probe = write_probe_path(stdout_path)?.to_path_buf();
    check_control()?;
    let stderr_probe = write_probe_path(stderr_path)?.to_path_buf();
    check_control()?;
    let permission_state_probe = write_probe_path(permission_state_dir)?.to_path_buf();
    check_control()?;
    Ok(vec![
        preflight_access_scope_checked(
            &stdout_probe,
            AccessIntent::Write,
            "adaptive extraction stdout",
            &mut check_control,
        )?,
        preflight_access_scope_checked(
            &stderr_probe,
            AccessIntent::Write,
            "adaptive extraction stderr",
            &mut check_control,
        )?,
        preflight_access_scope_checked(
            &permission_state_probe,
            AccessIntent::Write,
            "adaptive extraction permission state",
            &mut check_control,
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

fn checked_write_probe_path(
    path: &Path,
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<PathBuf> {
    check_control()?;
    preflight_write_target_volume_checked(path, worker, &mut check_control)?;
    check_control()?;
    let probe = write_probe_path(path)?.to_path_buf();
    check_control()?;
    Ok(probe)
}

fn preflight_write_target_volume_checked(
    path: &Path,
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    check_control()?;
    let volume_path = crate::parent_or_cwd(path);
    let volume_report =
        VolumeDiscoveryReport::for_containing_path_checked(volume_path, &mut check_control)?;
    check_control()?;
    preflight_volume_access_scope_with_report(
        volume_path,
        AccessIntent::Write,
        worker,
        &volume_report,
    )
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
    fn extraction_volume_class_constrains_unknown_descriptor() {
        let root = unique_temp_dir("gfm-extract-volume-unknown-descriptor");
        let input = root.join("Project.md");
        fs::write(&input, "unknown").unwrap();
        let mut report = VolumeDiscoveryReport::from_paths(vec![root.clone()]);
        report.volumes[0].kind = VolumeKind::Unknown;
        report.volumes[0].network = false;
        report.volumes[0].local = None;

        assert_eq!(
            extraction_volume_class_from_report(&input, &report),
            ExtractionVolumeClass::Network
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extraction_volume_class_constrains_unavailable_volume_api_state() {
        let root = unique_temp_dir("gfm-extract-volume-unavailable-api");
        let input = root.join("Project.md");
        fs::write(&input, "unavailable").unwrap();
        let mut report = VolumeDiscoveryReport::from_paths(vec![root.clone()]);
        report.volumes[0].kind = VolumeKind::Internal;
        report.volumes[0].network = false;
        report.volumes[0].local = Some(true);
        report.volumes[0].native_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
        report.volumes[0].resource_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
        report.volumes[0].mount_table_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);

        assert_eq!(
            extraction_volume_class_from_report(&input, &report),
            ExtractionVolumeClass::Network
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extraction_volume_class_constrains_missing_descriptor_without_path_guessing() {
        let root = unique_temp_dir("gfm-extract-volume-missing-descriptor");
        let input = root.join("ordinary-local-name.md");
        fs::write(&input, "missing descriptor").unwrap();
        let report = VolumeDiscoveryReport { volumes: vec![] };

        assert_eq!(
            extraction_volume_class_from_report(&input, &report),
            ExtractionVolumeClass::Network
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extraction_budget_profile_checked_honors_pre_cancelled_control_before_volume_discovery() {
        let root = std::env::temp_dir()
            .join(format!(
                "gfm-extract-budget-pre-cancel-{}",
                std::process::id()
            ))
            .join("root-that-should-not-be-probed");

        let result =
            extraction_budget_profile_checked(&root, SchedulingPressure::default(), || {
                Err(GfmError::Cancelled)
            });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!root.exists());
    }

    #[test]
    fn extraction_volume_class_for_path_checked_uses_discovered_descriptor() {
        let root = unique_temp_dir("gfm-extract-volume-checked-descriptor");
        fs::write(root.join(".gfm-volume-kind"), "network-smb\n").unwrap();
        let input = root.join("Project.md");
        fs::write(&input, "network").unwrap();

        let class = extraction_volume_class_for_path_checked(&input, || Ok(())).unwrap();

        assert_eq!(class, ExtractionVolumeClass::Network);
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
    fn worker_output_reader_round_trips_large_output() {
        let root = unique_temp_dir("gfm-extract-output-read");
        let path = root.join("stdout.bin");
        let bytes = vec![17; 300 * 1024];
        fs::write(&path, &bytes).unwrap();

        let read = read_worker_output_file_checked(&path, || Ok(())).unwrap();

        assert_eq!(read, bytes);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn worker_output_reader_honors_cancellation_during_chunked_load() {
        let root = unique_temp_dir("gfm-extract-output-cancel");
        let path = root.join("stdout.bin");
        let bytes = vec![23; 300 * 1024];
        fs::write(&path, &bytes).unwrap();
        let mut checks = 0usize;

        let result = read_worker_output_file_checked(&path, || {
            checks += 1;
            if checks >= 4 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        });

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert!(checks >= 4);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn worker_sandbox_profile_writer_honors_cancellation_during_chunked_write() {
        let root = unique_temp_dir("gfm-extract-sandbox-profile-cancel");
        let path = root.join("profile.sb");
        let bytes = vec![b'a'; 96 * 1024];
        let mut checks = 0usize;

        let result = write_worker_sandbox_profile_checked(&path, &bytes, || {
            checks += 1;
            if checks >= 4 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        });

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert!(checks >= 4);
        assert!(path.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn worker_sandbox_constructor_removes_profile_after_cancelled_write() {
        if sandbox_exec_path().unwrap().is_none() {
            return;
        }
        let fixture = SandboxProfileFixture::new("constructor-cancel");
        let scratch_before = worker_scratch_entries();
        let mut checks = 0usize;

        let result = WorkerSandbox::new_checked(
            &fixture.exe,
            &fixture.input,
            &fixture.stdout,
            &fixture.stderr,
            &fixture.permission_state,
            || {
                checks += 1;
                if checks >= 6 {
                    Err(GfmError::Cancelled)
                } else {
                    Ok(())
                }
            },
        );

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert!(checks >= 6);
        assert_eq!(scratch_before, worker_scratch_entries());
    }

    #[test]
    fn worker_scratch_prepare_removes_partial_outputs_after_cancellation() {
        let scratch_before = worker_scratch_entries();
        let mut checks = 0usize;

        let result = WorkerScratch::prepare_checked(|| {
            checks += 1;
            if checks >= 5 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        });

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert!(checks >= 5);
        assert_eq!(scratch_before, worker_scratch_entries());
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
            .contains("extraction quarantine volume access blocked"));
        assert!(err.to_string().contains("unreachable volume network"));
        assert!(!store.exists());

        fs::remove_dir_all(source).unwrap();
        fs::remove_dir_all(offline).unwrap();
    }

    #[test]
    fn quarantine_worker_access_checked_can_cancel_before_store_preflight() {
        let root = unique_temp_dir("gfm-extract-quarantine-access-cancel");
        let input = root.join("document.txt");
        let store = root.join("quarantine.gfmquarantine");
        fs::write(&input, "worker should not launch").unwrap();
        let mut checks = 0usize;

        let result = retain_extraction_quarantine_worker_access_checked(
            &input,
            &store,
            "quarantined extraction worker",
            || {
                checks += 1;
                if checks >= 4 {
                    Err(GfmError::Cancelled)
                } else {
                    Ok(())
                }
            },
        );

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(checks >= 4);
        assert!(!store.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn quarantine_reader_surfaces_store_probe_failure() {
        let root = unique_temp_dir("gfm-extract-quarantine-store-probe");
        let store = root.join("quarantine-store-unavailable".repeat(16));

        let err = read_extraction_quarantine_checked(&store, 2, || Ok(())).unwrap_err();

        assert!(matches!(err, GfmError::Io { .. }));
        assert!(err
            .to_string()
            .contains("extraction quarantine store metadata unavailable"));
        assert!(err.to_string().contains(&store.display().to_string()));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn quarantine_reader_honors_pre_cancelled_control_before_store_probe() {
        let root = unique_temp_dir("gfm-extract-quarantine-read-cancel");
        let store = root.join("quarantine.gfmquarantine");
        let cancellation = Cancellation::default();
        cancellation.cancel();

        let result = read_extraction_quarantine_cancellable(&store, 2, &cancellation);

        assert_eq!(result, Err(GfmError::Cancelled));
        assert!(!store.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn quarantine_worker_access_honors_cancelled_control_before_store_probe() {
        let root = unique_temp_dir("gfm-extract-quarantine-access-cancel");
        let input = root.join("document.txt");
        let store = root.join("missing").join("quarantine.gfmquarantine");
        fs::write(&input, "cancel before store probe").unwrap();

        let result = retain_extraction_quarantine_worker_access_checked(
            &input,
            &store,
            "quarantined extraction worker",
            || Err(GfmError::Cancelled),
        );

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!store.exists());
        assert!(!store.parent().unwrap().exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn quarantine_worker_access_refuses_unreachable_store_before_write_probe() {
        let root = unique_temp_dir("gfm-extract-quarantine-access-root");
        let store_root = unique_temp_dir("gfm-extract-quarantine-access-store");
        fs::write(store_root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
        let input = root.join("document.txt");
        let store = store_root.join("quarantine-store-unavailable".repeat(16));
        fs::write(&input, "block before store probe").unwrap();

        let err = match retain_extraction_quarantine_worker_access_checked(
            &input,
            &store,
            "quarantined extraction worker",
            || Ok(()),
        ) {
            Ok(_) => panic!("unreachable quarantine store was admitted before volume preflight"),
            Err(err) => err,
        };

        assert!(
            err.to_string().contains(
                "extraction quarantine volume access blocked: unreachable volume network"
            ),
            "{err}"
        );
        assert!(
            !err.to_string()
                .contains("extraction write path existence unavailable"),
            "{err}"
        );
        assert!(!store.exists());
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(store_root).unwrap();
    }

    #[test]
    fn adaptive_worker_scratch_preflight_honors_cancelled_control_before_probe() {
        let before = worker_scratch_entries();

        let result =
            preflight_adaptive_extraction_worker_scratch_checked(|| Err(GfmError::Cancelled));

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert_eq!(worker_scratch_entries(), before);
    }

    #[test]
    fn worker_scratch_access_honors_cancellation_while_resolving_probes() {
        let root = unique_temp_dir("gfm-extract-scratch-access-cancel");
        let stdout = root.join("stdout");
        let stderr = root.join("stderr");
        let permission_state = root.join("permission-state");
        let mut checks = 0usize;

        let result =
            retain_worker_scratch_access_checked(&stdout, &stderr, &permission_state, || {
                checks += 1;
                if checks >= 3 {
                    Err(GfmError::Cancelled)
                } else {
                    Ok(())
                }
            });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(checks >= 3);
        assert!(!stdout.exists());
        assert!(!stderr.exists());
        assert!(!permission_state.exists());
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
