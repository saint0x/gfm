use crate::access::{
    preflight_access_scope_checked_with_volume_report, preflight_volume_access_scope_with_report,
    ScopedAccessGuard,
};
use crate::runtime::{
    default_job_journal_path, run_scheduled_volume_task_cancellable, run_volume_task_cancellable,
};
use crate::{parse_optional_scheduling_pressure, parse_u64_arg, parse_usize_arg, required_path};
use gfm_jobs::{
    Cancellation, FailureClass, JobClass, JobFairnessPolicy, JobJournal, JobPayloadCatalog,
    JobPayloadKind, JobPayloadRecord, JobProgressCommand, JobProgressSnapshot, JobProgressState,
    JobProgressStore, Priority, RecoveryReason, RetryPolicy, Scheduler,
};
use gfm_mac::{AccessIntent, VolumeDiscoveryReport};
use gfm_types::{GfmError, Result, VolumeId};
use std::collections::HashMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const RUNTIME_RETRY_STATE_MAX_BYTES: usize = 64 * 1024;

pub(crate) fn run(command: &str, args: &mut impl Iterator<Item = String>) -> Result<bool> {
    match command {
        "jobs-recover" => {
            let journal = args
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(default_job_journal_path);
            for line in run_jobs_recover(journal)? {
                println!("{line}");
            }
        }
        "jobs-retry-plan" => {
            let max_attempts =
                parse_usize_arg(args.next(), "jobs-retry-plan requires max attempts")?;
            let attempts = parse_usize_arg(args.next(), "jobs-retry-plan requires attempts")?;
            let message = args.collect::<Vec<_>>().join(" ");
            if message.is_empty() {
                return Err(GfmError::Format(
                    "jobs-retry-plan requires a failure message".to_string(),
                ));
            }
            let policy = RetryPolicy { max_attempts };
            let decision = policy.retry_decision(attempts, &message);
            println!(
                "retry-plan\tclass={}\tretryable={}\tnext-delay-ms={}\tattempts={}\tmax-attempts={}",
                decision.class.as_str(),
                decision.retryable,
                decision.next_delay_ms,
                attempts,
                max_attempts
            );
        }
        "jobs-payload-catalog" => {
            let path = required_path(args.next(), "jobs-payload-catalog requires a catalog path")?;
            for line in run_jobs_payload_catalog(path)? {
                println!("{line}");
            }
        }
        "jobs-fairness-plan" => {
            for line in sample_fairness_plan() {
                println!("{line}");
            }
        }
        "jobs-progress-snapshot" => {
            let path = required_path(
                args.next(),
                "jobs-progress-snapshot requires a progress path",
            )?;
            for line in run_jobs_progress_snapshot(path)? {
                println!("{line}");
            }
        }
        "jobs-progress-restore" => {
            let path = required_path(
                args.next(),
                "jobs-progress-restore requires a progress path",
            )?;
            let updated_ms = parse_optional_timestamp_ms("jobs-progress-restore", args.next())?;
            for line in run_jobs_progress_restore(path, updated_ms)? {
                println!("{line}");
            }
        }
        "jobs-progress-control" => {
            let path = required_path(
                args.next(),
                "jobs-progress-control requires a progress path",
            )?;
            let job_id = parse_u64_arg(args.next(), "jobs-progress-control requires a job id")?;
            let command = parse_progress_command(args.next())?;
            let updated_ms = parse_optional_timestamp_ms("jobs-progress-control", args.next())?;
            for line in run_jobs_progress_control(path, job_id, command, updated_ms)? {
                println!("{line}");
            }
        }
        "jobs-payload-restore-plan" => {
            let catalog_path = required_path(
                args.next(),
                "jobs-payload-restore-plan requires a payload catalog path",
            )?;
            let progress_path = required_path(
                args.next(),
                "jobs-payload-restore-plan requires a progress path",
            )?;
            let updated_ms = parse_optional_timestamp_ms("jobs-payload-restore-plan", args.next())?;
            for line in run_jobs_payload_restore_plan(catalog_path, progress_path, updated_ms)? {
                println!("{line}");
            }
        }
        "jobs-cancel-tree" => {
            for line in sample_cancellation_tree_report() {
                println!("{line}");
            }
        }
        "jobs-cancel-volume" => {
            let volume = VolumeId(parse_u64_arg(
                args.next(),
                "jobs-cancel-volume requires a volume id",
            )?);
            let class = args
                .next()
                .map(|value| {
                    JobClass::parse(&value)
                        .ok_or_else(|| GfmError::Format(format!("unsupported job class `{value}`")))
                })
                .transpose()?;
            println!(
                "{}",
                sample_volume_cancellation_report(volume, class).as_tsv()
            );
        }
        "jobs-runtime-retry-probe" => {
            let state = required_path(
                args.next(),
                "jobs-runtime-retry-probe requires an attempt state path",
            )?;
            let pressure = parse_optional_scheduling_pressure(args)?;
            let access_report = JobPathAccessReport::new_checked(
                write_probe_path(&state)?.to_path_buf(),
                AccessIntent::Write,
                || Ok(()),
            )?;
            access_report.preflight_volume("runtime retry probe")?;
            let outcome = run_scheduled_volume_task_cancellable(
                access_report.volume(),
                Priority::Background,
                "runtime retry probe",
                pressure,
                move |cancellation| {
                    cancellation.check()?;
                    let _access = access_report
                        .access_checked("runtime retry probe", || cancellation.check())?;
                    cancellation.check()?;
                    runtime_retry_probe_cancellable(&state, &cancellation)
                },
            )?;
            if outcome.deferred {
                println!(
                    "runtime-retry-probe\tdeferred\t{:?}",
                    outcome.scheduling_action
                );
            } else {
                println!(
                    "runtime-retry-probe\tcompleted\t{}\t{:?}",
                    outcome.result.unwrap_or_default(),
                    outcome.scheduling_action
                );
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn run_jobs_recover(journal: PathBuf) -> Result<Vec<String>> {
    const WORKER: &str = "jobs recover";
    let access_report =
        JobPathAccessReport::new_checked(journal.clone(), AccessIntent::Read, || Ok(()))?;
    access_report.preflight_volume(WORKER)?;
    let volume = access_report.volume();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = access_report.access_checked(WORKER, || cancellation.check())?;
        cancellation.check()?;
        let lines = JobJournal::new(journal)
            .recoverable_checked(RetryPolicy { max_attempts: 2 }, || cancellation.check())?
            .into_iter()
            .map(|job| {
                format!(
                    "{}\t{}\t{}\tclass={}\tnext-delay-ms={}\t{}",
                    job.id.value(),
                    job.attempts,
                    recovery_reason(job.reason),
                    recovery_failure_class(job.failure_class),
                    job.next_delay_ms,
                    job.label
                )
            })
            .collect();
        Ok(lines)
    })
}

#[derive(Clone)]
struct JobPathAccessReport {
    path: PathBuf,
    intent: AccessIntent,
    volume_report: VolumeDiscoveryReport,
}

impl JobPathAccessReport {
    fn new_checked(
        path: PathBuf,
        intent: AccessIntent,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        check_control()?;
        let volume_report =
            VolumeDiscoveryReport::for_containing_path_checked(&path, &mut check_control)?;
        check_control()?;
        Ok(Self {
            path,
            intent,
            volume_report,
        })
    }

    fn preflight_volume(&self, worker: &str) -> Result<()> {
        preflight_volume_access_scope_with_report(
            &self.path,
            self.intent,
            worker,
            &self.volume_report,
        )
    }

    fn access_checked(
        &self,
        worker: &str,
        check_control: impl FnMut() -> Result<()>,
    ) -> Result<ScopedAccessGuard> {
        preflight_access_scope_checked_with_volume_report(
            &self.path,
            self.intent,
            worker,
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

#[derive(Clone)]
struct JobPathAccessReports {
    entries: Vec<JobPathAccessReport>,
}

impl JobPathAccessReports {
    fn payload_restore(catalog_path: &Path, progress_path: &Path) -> Result<Self> {
        Self::payload_restore_checked(catalog_path, progress_path, || Ok(()))
    }

    fn payload_restore_checked(
        catalog_path: &Path,
        progress_path: &Path,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        check_control()?;
        let progress_probe = write_probe_path(progress_path)?.to_path_buf();
        check_control()?;
        Ok(Self {
            entries: vec![
                JobPathAccessReport::new_checked(
                    catalog_path.to_path_buf(),
                    AccessIntent::Read,
                    &mut check_control,
                )?,
                JobPathAccessReport::new_checked(
                    progress_probe,
                    AccessIntent::Write,
                    &mut check_control,
                )?,
            ],
        })
    }

    fn preflight_volumes(&self, worker: &str) -> Result<()> {
        for report in &self.entries {
            report.preflight_volume(worker)?;
        }
        Ok(())
    }

    fn access_checked(
        &self,
        worker: &str,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Vec<ScopedAccessGuard>> {
        let mut guards = Vec::with_capacity(self.entries.len());
        for report in &self.entries {
            check_control()?;
            guards.push(report.access_checked(worker, &mut check_control)?);
        }
        check_control()?;
        Ok(guards)
    }

    fn first_volume(&self) -> Option<VolumeId> {
        self.entries.iter().find_map(JobPathAccessReport::volume)
    }
}

fn run_jobs_payload_catalog(path: PathBuf) -> Result<Vec<String>> {
    const WORKER: &str = "jobs payload catalog";
    let access_report = JobPathAccessReport::new_checked(
        write_probe_path(&path)?.to_path_buf(),
        AccessIntent::Write,
        || Ok(()),
    )?;
    access_report.preflight_volume(WORKER)?;
    let volume = access_report.volume();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = access_report.access_checked(WORKER, || cancellation.check())?;
        cancellation.check()?;
        let catalog = JobPayloadCatalog::new(&path);
        let records = sample_payload_catalog_records();
        catalog.write_all_checked(&records, || cancellation.check())?;
        cancellation.check()?;
        let lines = catalog
            .read_checked(|| cancellation.check())?
            .into_iter()
            .map(|record| record.as_tsv())
            .collect();
        Ok(lines)
    })
}

fn run_jobs_progress_snapshot(path: PathBuf) -> Result<Vec<String>> {
    const WORKER: &str = "jobs progress snapshot";
    let access_report = JobPathAccessReport::new_checked(
        write_probe_path(&path)?.to_path_buf(),
        AccessIntent::Write,
        || Ok(()),
    )?;
    access_report.preflight_volume(WORKER)?;
    let volume = access_report.volume();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = access_report.access_checked(WORKER, || cancellation.check())?;
        cancellation.check()?;
        let store = JobProgressStore::new(&path);
        for snapshot in sample_progress_snapshots() {
            store.upsert_checked(snapshot, || cancellation.check())?;
        }
        cancellation.check()?;
        let lines = store
            .restorable_checked(|| cancellation.check())?
            .into_iter()
            .map(|snapshot| snapshot.as_tsv())
            .collect();
        Ok(lines)
    })
}

fn run_jobs_progress_restore(path: PathBuf, updated_ms: u64) -> Result<Vec<String>> {
    const WORKER: &str = "jobs progress restore";
    let access_report = JobPathAccessReport::new_checked(
        write_probe_path(&path)?.to_path_buf(),
        AccessIntent::Write,
        || Ok(()),
    )?;
    access_report.preflight_volume(WORKER)?;
    let volume = access_report.volume();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = access_report.access_checked(WORKER, || cancellation.check())?;
        cancellation.check()?;
        let lines = JobProgressStore::new(&path)
            .restore_interrupted_checked(updated_ms, || cancellation.check())?
            .into_iter()
            .map(|snapshot| snapshot.as_tsv())
            .collect();
        Ok(lines)
    })
}

fn run_jobs_progress_control(
    path: PathBuf,
    job_id: u64,
    command: JobProgressCommand,
    updated_ms: u64,
) -> Result<Vec<String>> {
    const WORKER: &str = "jobs progress control";
    let access_report = JobPathAccessReport::new_checked(
        write_probe_path(&path)?.to_path_buf(),
        AccessIntent::Write,
        || Ok(()),
    )?;
    access_report.preflight_volume(WORKER)?;
    let volume = access_report.volume();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = access_report.access_checked(WORKER, || cancellation.check())?;
        cancellation.check()?;
        let snapshot = JobProgressStore::new(&path).apply_command_checked(
            gfm_jobs::JobId::from_raw(job_id),
            command,
            updated_ms,
            || cancellation.check(),
        )?;
        Ok(vec![
            format!(
                "progress-control\t{}\tjob={}\tstate={}\tdetail={}",
                command.as_str(),
                snapshot.id.value(),
                snapshot.state.as_str(),
                snapshot.detail
            ),
            snapshot.as_tsv(),
        ])
    })
}

fn run_jobs_payload_restore_plan(
    catalog_path: PathBuf,
    progress_path: PathBuf,
    updated_ms: u64,
) -> Result<Vec<String>> {
    const WORKER: &str = "jobs payload restore plan";
    let access_reports = JobPathAccessReports::payload_restore(&catalog_path, &progress_path)?;
    access_reports.preflight_volumes(WORKER)?;
    let volume = access_reports.first_volume();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = access_reports.access_checked(WORKER, || cancellation.check())?;
        cancellation.check()?;
        let store = JobProgressStore::new(&progress_path);
        let restored = store.restore_interrupted_checked(updated_ms, || cancellation.check())?;
        cancellation.check()?;
        let payloads = JobPayloadCatalog::new(&catalog_path)
            .read_for_ids_checked(restored.iter().map(|snapshot| snapshot.id), || {
                cancellation.check()
            })?
            .into_iter()
            .map(|record| (record.id, record))
            .collect::<HashMap<_, _>>();
        let lines = restored
            .into_iter()
            .map(|snapshot| {
                if let Some(payload) = payloads.get(&snapshot.id) {
                    format!("restore\t{}\t{}", snapshot.state.as_str(), payload.as_tsv())
                } else {
                    format!(
                        "missing-payload\t{}\t{}\t{}",
                        snapshot.id.value(),
                        snapshot.state.as_str(),
                        snapshot.label
                    )
                }
            })
            .collect();
        Ok(lines)
    })
}

#[cfg(test)]
fn retain_payload_restore_access_checked(
    catalog_path: &Path,
    progress_path: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    check_control()?;
    let access_reports = JobPathAccessReports::payload_restore_checked(
        catalog_path,
        progress_path,
        &mut check_control,
    )?;
    access_reports.access_checked("jobs payload restore plan", &mut check_control)
}

fn write_probe_path(path: &Path) -> Result<&Path> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(path),
        Ok(_) => Ok(crate::parent_or_cwd(path)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(crate::parent_or_cwd(path)),
        Err(err) => Err(GfmError::io(
            path,
            format!("jobs write path metadata unavailable: {err}"),
        )),
    }
}

fn parse_progress_command(value: Option<String>) -> Result<JobProgressCommand> {
    let value = value.ok_or_else(|| {
        GfmError::Format("jobs-progress-control requires pause, resume, or stop".to_string())
    })?;
    JobProgressCommand::parse(&value).ok_or_else(|| {
        GfmError::Format(format!(
            "invalid progress command `{value}`; expected pause, resume, or stop"
        ))
    })
}

fn recovery_reason(reason: RecoveryReason) -> &'static str {
    match reason {
        RecoveryReason::Interrupted => "interrupted",
        RecoveryReason::RetryableFailure => "retryable-failure",
    }
}

fn recovery_failure_class(class: Option<FailureClass>) -> &'static str {
    class.map(FailureClass::as_str).unwrap_or("-")
}

fn sample_payload_catalog_records() -> Vec<JobPayloadRecord> {
    [
        (
            1,
            JobPayloadKind::Operation,
            "copy operation",
            "operations/copy.gfmjob",
            Some(VolumeId(1)),
            "copy:/source->/target",
        ),
        (
            2,
            JobPayloadKind::Indexing,
            "content indexing",
            "index/content.gfmjob",
            Some(VolumeId(1)),
            "index:/workspace",
        ),
        (
            3,
            JobPayloadKind::Extraction,
            "content extraction",
            "extract/report.gfmjob",
            Some(VolumeId(1)),
            "extract:/workspace/report.pdf",
        ),
        (
            4,
            JobPayloadKind::Thumbnail,
            "thumbnail generation",
            "preview/thumbnail.gfmjob",
            Some(VolumeId(1)),
            "thumbnail:/workspace/image.png",
        ),
        (
            5,
            JobPayloadKind::Preview,
            "quick look preview",
            "preview/quicklook.gfmjob",
            Some(VolumeId(1)),
            "preview:/workspace/report.pdf",
        ),
        (
            6,
            JobPayloadKind::Repair,
            "sidecar repair",
            "repair/sidecar.gfmjob",
            None,
            "repair:sidecars",
        ),
    ]
    .into_iter()
    .map(|(id, kind, label, path, volume, summary)| {
        JobPayloadRecord::new(
            gfm_jobs::JobId::from_raw(id),
            kind,
            label,
            path,
            volume,
            summary,
        )
    })
    .collect()
}

fn sample_fairness_plan() -> Vec<String> {
    let mut scheduler = Scheduler::new();
    scheduler.schedule_in_class(Priority::Interactive, JobClass::Foreground, "open folder");
    scheduler.schedule_in_class(Priority::Visible, JobClass::Visible, "render visible rows");
    scheduler.schedule_in_class(Priority::Background, JobClass::Background, "index content");
    let compact = scheduler.schedule_in_class(
        Priority::Background,
        JobClass::Maintenance,
        "compact sidecars",
    );
    scheduler.schedule_in_class_with_dependencies(
        Priority::Visible,
        JobClass::Repair,
        "repair derived sidecar",
        [compact.id],
    );
    scheduler.schedule_in_class_with_dependencies(
        Priority::Visible,
        JobClass::Repair,
        "repair missing thumbnail",
        [gfm_jobs::JobId::from_raw(999)],
    );

    let first = scheduler.drain_fair_ready(
        JobFairnessPolicy::new()
            .with_quota(JobClass::Foreground, 1)
            .with_quota(JobClass::Visible, 1)
            .with_quota(JobClass::Background, 1)
            .with_quota(JobClass::Maintenance, 1)
            .with_quota(JobClass::Repair, 1),
        [],
    );
    scheduler.mark_completed(compact.id);
    let second = scheduler.drain_fair_ready(
        JobFairnessPolicy::new()
            .with_quota(JobClass::Foreground, 1)
            .with_quota(JobClass::Visible, 1)
            .with_quota(JobClass::Background, 1)
            .with_quota(JobClass::Maintenance, 1)
            .with_quota(JobClass::Repair, 1),
        [],
    );
    let mut lines = format_fairness_plan("first", first);
    lines.extend(format_fairness_plan("after-completion", second));
    lines
}

fn format_fairness_plan(pass: &str, plan: gfm_jobs::JobFairnessPlan) -> Vec<String> {
    let mut lines = Vec::new();
    for job in plan.ready {
        lines.push(format!(
            "ready\t{}\t{}\t{}\t{}\t{}",
            pass,
            job.id.value(),
            job.class.as_str(),
            priority_name(job.priority),
            job.label
        ));
    }
    for job in plan.blocked {
        let missing = job
            .missing_dependencies
            .iter()
            .map(|dependency| dependency.value().to_string())
            .collect::<Vec<_>>()
            .join(",");
        lines.push(format!(
            "blocked\t{}\t{}\t{}\t{}\t{}",
            pass,
            job.id.value(),
            job.class.as_str(),
            missing,
            job.label
        ));
    }
    lines
}

fn sample_volume_cancellation_report(
    volume: VolumeId,
    class: Option<JobClass>,
) -> gfm_jobs::VolumeCancellationReport {
    let mut scheduler = Scheduler::new();
    scheduler.schedule_on_volume_in_class(
        Priority::Background,
        JobClass::Background,
        "index detached volume",
        volume,
    );
    scheduler.schedule_on_volume_in_class(
        Priority::Visible,
        JobClass::Visible,
        "render visible thumbnails",
        volume,
    );
    scheduler.schedule_on_volume_in_class(
        Priority::Background,
        JobClass::Background,
        "index mounted volume",
        VolumeId(volume.0 + 1),
    );
    scheduler.cancel_volume_jobs(volume, class)
}

fn sample_progress_snapshots() -> Vec<JobProgressSnapshot> {
    vec![
        JobProgressSnapshot::new(
            gfm_jobs::JobId::from_raw(1),
            JobClass::Foreground,
            Priority::Interactive,
            "copy selected files",
            Some(VolumeId(1)),
            100,
        )
        .with_progress(
            JobProgressState::Running,
            42,
            "copy:/source->/target",
            1_000,
        ),
        JobProgressSnapshot::new(
            gfm_jobs::JobId::from_raw(2),
            JobClass::Background,
            Priority::Background,
            "index content",
            Some(VolumeId(1)),
            250,
        )
        .with_progress(JobProgressState::Paused, 128, "pressure:throttled", 1_001),
        JobProgressSnapshot::new(
            gfm_jobs::JobId::from_raw(3),
            JobClass::Maintenance,
            Priority::Background,
            "compact content segments",
            None,
            7,
        )
        .with_progress(JobProgressState::Completed, 7, "done", 1_002),
    ]
}

fn sample_cancellation_tree_report() -> Vec<String> {
    let root = Cancellation::default();
    let child = root.child();
    let sibling = root.child();
    let grandchild = child.child();
    child.cancel();
    let child_cancelled = [
        ("root", root.is_cancelled()),
        ("child", child.is_cancelled()),
        ("sibling", sibling.is_cancelled()),
        ("grandchild", grandchild.is_cancelled()),
    ];
    root.cancel();
    let root_cancelled = [
        ("root", root.is_cancelled()),
        ("child", child.is_cancelled()),
        ("sibling", sibling.is_cancelled()),
        ("grandchild", grandchild.is_cancelled()),
    ];

    child_cancelled
        .into_iter()
        .map(|(name, cancelled)| format!("after-child-cancel\t{name}\tcancelled={cancelled}"))
        .chain(
            root_cancelled.into_iter().map(|(name, cancelled)| {
                format!("after-root-cancel\t{name}\tcancelled={cancelled}")
            }),
        )
        .collect()
}

fn priority_name(priority: Priority) -> &'static str {
    priority.as_str()
}

fn parse_optional_timestamp_ms(command: &str, value: Option<String>) -> Result<u64> {
    match value {
        Some(value) => value.parse().map_err(|_| {
            GfmError::Format(format!(
                "{command} timestamp must be an unsigned millisecond value; got `{value}`"
            ))
        }),
        None => Ok(SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
            .unwrap_or(0)),
    }
}

fn runtime_retry_probe_cancellable(state: &Path, cancellation: &Cancellation) -> Result<usize> {
    cancellation.check()?;
    let attempt = read_runtime_retry_attempt_checked(state, || cancellation.check())? + 1;
    cancellation.check()?;
    write_runtime_retry_attempt_checked(state, attempt, || cancellation.check())?;
    cancellation.check()?;
    if attempt == 1 {
        Err(GfmError::Format("temporary runtime probe busy".to_string()))
    } else {
        Ok(attempt)
    }
}

fn read_runtime_retry_attempt_checked(
    state: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<usize> {
    check_control()?;
    let mut file = match fs::File::open(state) {
        Ok(file) => file,
        Err(_) => return Ok(0),
    };
    check_control()?;
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        check_control()?;
        let read = match file.read(&mut buffer) {
            Ok(read) => read,
            Err(_) => return Ok(0),
        };
        check_control()?;
        if read == 0 {
            break;
        }
        if bytes.len().saturating_add(read) > RUNTIME_RETRY_STATE_MAX_BYTES {
            return Ok(0);
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    let Ok(value) = std::str::from_utf8(&bytes) else {
        return Ok(0);
    };
    Ok(value.trim().parse::<usize>().unwrap_or(0))
}

fn write_runtime_retry_attempt_checked(
    state: &Path,
    attempt: usize,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    const CHUNK_BYTES: usize = 4096;

    check_control()?;
    let encoded = attempt.to_string();
    gfm_store::atomic_write_checked(state, &mut check_control, |writer, check_control| {
        for chunk in encoded.as_bytes().chunks(CHUNK_BYTES) {
            check_control()?;
            writer
                .write_all(chunk)
                .map_err(|err| GfmError::io(state, err))?;
            check_control()?;
        }
        Ok(())
    })?;
    check_control()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_retry_attempt_read_honors_pre_cancelled_control_before_file_open() {
        let state = std::env::temp_dir().join(format!(
            "gfm-runtime-retry-attempt-cancel-{}-{}.state",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        let result = read_runtime_retry_attempt_checked(&state, || Err(GfmError::Cancelled));

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert!(!state.exists());
    }

    #[test]
    fn runtime_retry_attempt_write_honors_pre_cancelled_control_before_mutation() {
        let state = std::env::temp_dir().join(format!(
            "gfm-runtime-retry-attempt-write-cancel-{}-{}.state",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&state, "5").unwrap();

        let result = write_runtime_retry_attempt_checked(&state, 6, || Err(GfmError::Cancelled));

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert_eq!(fs::read_to_string(&state).unwrap(), "5");
        fs::remove_file(state).unwrap();
    }

    #[test]
    fn runtime_retry_attempt_write_preserves_existing_state_after_mid_write_cancel() {
        let state = std::env::temp_dir().join(format!(
            "gfm-runtime-retry-attempt-write-existing-cancel-{}-{}.state",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&state, "5").unwrap();
        let mut checks = 0usize;

        let result = write_runtime_retry_attempt_checked(&state, 6, || {
            checks += 1;
            if checks >= 3 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        });

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert!(checks >= 3);
        assert_eq!(fs::read_to_string(&state).unwrap(), "5");
        let temp_prefix = format!(
            ".{}.{}.",
            state.file_name().unwrap().to_string_lossy(),
            std::process::id()
        );
        let leaked_temp = fs::read_dir(state.parent().unwrap())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .any(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with(&temp_prefix))
            });
        assert!(!leaked_temp);
        fs::remove_file(state).unwrap();
    }

    #[test]
    fn runtime_retry_attempt_write_removes_temporary_file_after_cancellation() {
        let state = std::env::temp_dir().join(format!(
            "gfm-runtime-retry-attempt-write-temp-cancel-{}-{}.state",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut checks = 0usize;

        let result = write_runtime_retry_attempt_checked(&state, 6, || {
            checks += 1;
            if checks >= 3 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        });

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert!(checks >= 3);
        assert!(!state.exists());
        let temp_prefix = format!(
            ".{}.{}.",
            state.file_name().unwrap().to_string_lossy(),
            std::process::id()
        );
        let leaked_temp = fs::read_dir(state.parent().unwrap())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .any(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with(&temp_prefix))
            });
        assert!(!leaked_temp);
    }

    #[test]
    fn job_path_access_report_checked_honors_pre_cancelled_control_before_volume_discovery() {
        let path = std::env::temp_dir()
            .join(format!(
                "gfm-job-path-report-pre-cancel-{}",
                std::process::id()
            ))
            .join("progress.tsv");

        let result = JobPathAccessReport::new_checked(path.clone(), AccessIntent::Read, || {
            Err(GfmError::Cancelled)
        });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!path.exists());
    }

    #[test]
    fn payload_restore_access_checked_honors_pre_cancelled_control() {
        let catalog = std::env::temp_dir().join(format!(
            "gfm-payload-restore-catalog-pre-cancel-{}-{}.tsv",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let progress = catalog.with_extension("progress.tsv");

        let result =
            retain_payload_restore_access_checked(&catalog, &progress, || Err(GfmError::Cancelled));

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!catalog.exists());
        assert!(!progress.exists());
    }

    #[test]
    fn payload_restore_access_checked_can_cancel_before_progress_probe() {
        let catalog = std::env::temp_dir().join(format!(
            "gfm-payload-restore-catalog-mid-cancel-{}-{}.tsv",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let progress = catalog.with_extension("progress.tsv");
        let mut checks = 0usize;

        let result = retain_payload_restore_access_checked(&catalog, &progress, || {
            checks += 1;
            if checks >= 2 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(checks >= 2);
        assert!(!progress.exists());
    }

    #[test]
    fn payload_restore_access_checked_can_cancel_during_access_preflights() {
        let catalog = std::env::temp_dir().join(format!(
            "gfm-payload-restore-catalog-access-cancel-{}-{}.tsv",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let progress = catalog.with_extension("progress.tsv");
        fs::write(&catalog, "payload").unwrap();
        let mut checks = 0usize;

        let result = retain_payload_restore_access_checked(&catalog, &progress, || {
            checks += 1;
            if checks >= 5 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(checks >= 5);
        assert!(!progress.exists());
        fs::remove_file(catalog).unwrap();
    }
}
