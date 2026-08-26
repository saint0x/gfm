use gfm_content::{
    ExtractionBatteryState, ExtractionBudgetProfile, ExtractionFingerprint, ExtractionQuarantine,
    ExtractionThermalState, ExtractionUserActivity, ExtractionVolumeClass, QuarantineDecision,
    QuarantineFailureKind,
};
use gfm_jobs::{
    Cancellation, JobBatteryState, JobIoPressure, JobThermalState, JobUserActivity,
    SchedulingPressure,
};
use gfm_types::{GfmError, Result};
use std::env;
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

pub(crate) fn run_adaptive_extraction_worker(
    path: &Path,
    pressure: SchedulingPressure,
) -> Result<String> {
    run_adaptive_extraction_worker_with_timeout(path, pressure, ADAPTIVE_WORKER_TIMEOUT)
}

pub(crate) fn run_adaptive_extraction_worker_with_timeout(
    path: &Path,
    pressure: SchedulingPressure,
    timeout: Duration,
) -> Result<String> {
    run_adaptive_extraction_worker_cancellable(path, pressure, timeout, &Cancellation::default())
}

pub(crate) fn run_adaptive_extraction_worker_cancellable(
    path: &Path,
    pressure: SchedulingPressure,
    timeout: Duration,
    cancellation: &Cancellation,
) -> Result<String> {
    cancellation.check()?;
    let exe = env::current_exe().map_err(|err| {
        GfmError::Format(format!(
            "could not resolve current executable for extraction worker: {err}"
        ))
    })?;
    let stdout_path = worker_temp_path("stdout");
    let stderr_path = worker_temp_path("stderr");
    std::fs::File::create(&stdout_path).map_err(|err| GfmError::io(&stdout_path, err))?;
    std::fs::File::create(&stderr_path).map_err(|err| GfmError::io(&stderr_path, err))?;
    if let Err(err) = cancellation.check() {
        let _ = std::fs::remove_file(&stdout_path);
        let _ = std::fs::remove_file(&stderr_path);
        return Err(err);
    }
    let sandbox = WorkerSandbox::new(&exe, path, &stdout_path, &stderr_path)?;
    let mut command = sandbox.command(&exe, path, pressure);
    let output = run_supervised_worker(
        &mut command,
        path,
        timeout,
        &stdout_path,
        &stderr_path,
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

pub(crate) fn run_quarantined_adaptive_extraction_worker(
    path: &Path,
    store: &Path,
    pressure: SchedulingPressure,
    timeout: Duration,
    threshold: u32,
) -> Result<String> {
    let fingerprint = ExtractionFingerprint::for_path(path)?;
    let mut quarantine = read_extraction_quarantine(store, threshold)?;
    let decision = quarantine.before_extract(path, &fingerprint);
    if matches!(decision, QuarantineDecision::Quarantined(_)) {
        return Ok(format!("{}\n", decision.as_tsv()));
    }
    match run_adaptive_extraction_worker_with_timeout(path, pressure, timeout) {
        Ok(report) => {
            let decision = quarantine.record_success(path, &fingerprint);
            quarantine.write(store)?;
            Ok(format!("{report}{}\n", decision.as_tsv()))
        }
        Err(err) => {
            let message = err.to_string();
            let kind = worker_failure_kind(&message);
            let decision =
                quarantine.record_failure(path, &fingerprint, kind, worker_failure_reason(kind));
            quarantine.write(store)?;
            Ok(format!("{}\n", decision.as_tsv()))
        }
    }
}

pub(crate) fn read_extraction_quarantine(
    store: &Path,
    threshold: u32,
) -> Result<ExtractionQuarantine> {
    if store.is_file() {
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

struct WorkerSandbox {
    profile_path: Option<PathBuf>,
}

impl WorkerSandbox {
    fn new(exe: &Path, input: &Path, stdout: &Path, stderr: &Path) -> Result<Self> {
        let Some(sandbox_exec) = sandbox_exec_path() else {
            return Ok(Self { profile_path: None });
        };
        let _ = sandbox_exec;
        let profile = extraction_sandbox_profile(exe, input, stdout, stderr)?;
        let profile_path = env::temp_dir().join(format!(
            "gfm-extract-worker-{}-{}.sb",
            std::process::id(),
            monotonic_nanos()
        ));
        std::fs::write(&profile_path, profile).map_err(|err| GfmError::io(&profile_path, err))?;
        Ok(Self {
            profile_path: Some(profile_path),
        })
    }

    fn command(&self, exe: &Path, input: &Path, pressure: SchedulingPressure) -> Command {
        if let (Some(sandbox_exec), Some(profile_path)) = (sandbox_exec_path(), &self.profile_path)
        {
            let mut command = Command::new(sandbox_exec);
            command
                .arg("-f")
                .arg(profile_path)
                .arg(exe)
                .arg("extract-report-adaptive")
                .arg(input)
                .args(scheduling_pressure_args(pressure));
            command
        } else {
            let mut command = Command::new(exe);
            command
                .arg("extract-report-adaptive")
                .arg(input)
                .args(scheduling_pressure_args(pressure));
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
                return Err(GfmError::Format(format!(
                    "adaptive extraction worker timed out after {} ms for {}",
                    timeout.as_millis(),
                    input.display()
                )));
            }
            Ok(None) => thread::sleep(Duration::from_millis(5)),
            Err(err) => {
                kill_process_group(child.id());
                let _ = child.wait();
                let _ = std::fs::remove_file(stdout_path);
                let _ = std::fs::remove_file(stderr_path);
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

fn worker_temp_path(label: &str) -> PathBuf {
    env::temp_dir().join(format!(
        "gfm-extract-worker-{}-{}-{label}",
        std::process::id(),
        monotonic_nanos()
    ))
}

fn sandbox_exec_path() -> Option<PathBuf> {
    let path = PathBuf::from("/usr/bin/sandbox-exec");
    path.is_file().then_some(path)
}

fn extraction_sandbox_profile(
    exe: &Path,
    input: &Path,
    stdout: &Path,
    stderr: &Path,
) -> Result<String> {
    let _ = canonical_or_self(exe)?;
    let _ = canonical_or_self(input)?;
    let stdout = canonical_or_self(stdout)?;
    let stderr = canonical_or_self(stderr)?;
    Ok(format!(
        "(version 1)\n\
         (allow default)\n\
         (deny file-write*)\n\
         (allow file-write-data (literal \"{}\"))\n\
         (allow file-write-data (literal \"{}\"))\n",
        sandbox_escape(&stdout),
        sandbox_escape(&stderr)
    ))
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
