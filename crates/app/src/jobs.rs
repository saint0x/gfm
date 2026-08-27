use crate::access::{preflight_access_scope, preflight_volume_access_scope, ScopedAccessGuard};
use crate::runtime::{default_job_journal_path, run_scheduled_volume_task};
use crate::{
    parent_volume, parse_optional_scheduling_pressure, parse_u64_arg, parse_usize_arg,
    required_path,
};
use gfm_jobs::{
    Cancellation, JobClass, JobFairnessPolicy, JobJournal, JobPayloadCatalog, JobPayloadKind,
    JobPayloadRecord, JobProgressCommand, JobProgressSnapshot, JobProgressState, JobProgressStore,
    Priority, RecoveryReason, RetryPolicy, Scheduler,
};
use gfm_mac::AccessIntent;
use gfm_types::{GfmError, Result, VolumeId};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn run(command: &str, args: &mut impl Iterator<Item = String>) -> Result<bool> {
    match command {
        "jobs-recover" => {
            let journal = args
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(default_job_journal_path);
            let _access = preflight_access_scope(&journal, AccessIntent::Read, "jobs recover")?;
            let recoverable =
                JobJournal::new(journal).recoverable(RetryPolicy { max_attempts: 2 })?;
            for job in recoverable {
                println!(
                    "{}\t{}\t{}\t{}",
                    job.id.value(),
                    job.attempts,
                    recovery_reason(job.reason),
                    job.label
                );
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
            let _access = preflight_access_scope(
                write_probe_path(&path),
                AccessIntent::Write,
                "jobs payload catalog",
            )?;
            let catalog = JobPayloadCatalog::new(&path);
            let records = sample_payload_catalog_records();
            catalog.write_all(&records)?;
            for record in catalog.read()? {
                println!("{}", record.as_tsv());
            }
        }
        "jobs-fairness-plan" => {
            let plan = sample_fairness_plan();
            for job in plan.ready {
                println!(
                    "ready\t{}\t{}\t{}\t{}",
                    job.id.value(),
                    job.class.as_str(),
                    priority_name(job.priority),
                    job.label
                );
            }
            for job in plan.blocked {
                let missing = job
                    .missing_dependencies
                    .iter()
                    .map(|dependency| dependency.value().to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                println!(
                    "blocked\t{}\t{}\t{}\t{}",
                    job.id.value(),
                    job.class.as_str(),
                    missing,
                    job.label
                );
            }
        }
        "jobs-progress-snapshot" => {
            let path = required_path(
                args.next(),
                "jobs-progress-snapshot requires a progress path",
            )?;
            let _access = preflight_access_scope(
                write_probe_path(&path),
                AccessIntent::Write,
                "jobs progress snapshot",
            )?;
            let store = JobProgressStore::new(&path);
            for snapshot in sample_progress_snapshots() {
                store.upsert(snapshot)?;
            }
            for snapshot in store.restorable()? {
                println!("{}", snapshot.as_tsv());
            }
        }
        "jobs-progress-restore" => {
            let path = required_path(
                args.next(),
                "jobs-progress-restore requires a progress path",
            )?;
            let updated_ms = parse_optional_timestamp_ms("jobs-progress-restore", args.next())?;
            let _access = preflight_access_scope(
                write_probe_path(&path),
                AccessIntent::Write,
                "jobs progress restore",
            )?;
            let store = JobProgressStore::new(&path);
            for snapshot in store.restore_interrupted(updated_ms)? {
                println!("{}", snapshot.as_tsv());
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
            let _access = preflight_access_scope(
                write_probe_path(&path),
                AccessIntent::Write,
                "jobs progress control",
            )?;
            let store = JobProgressStore::new(&path);
            let snapshot =
                store.apply_command(gfm_jobs::JobId::from_raw(job_id), command, updated_ms)?;
            println!(
                "progress-control\t{}\tjob={}\tstate={}\tdetail={}",
                command.as_str(),
                snapshot.id.value(),
                snapshot.state.as_str(),
                snapshot.detail
            );
            println!("{}", snapshot.as_tsv());
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
            let _access = retain_payload_restore_access(&catalog_path, &progress_path)?;
            let store = JobProgressStore::new(&progress_path);
            let restored = store.restore_interrupted(updated_ms)?;
            let payloads = JobPayloadCatalog::new(&catalog_path)
                .read_for_ids(restored.iter().map(|snapshot| snapshot.id))?
                .into_iter()
                .map(|record| (record.id, record))
                .collect::<HashMap<_, _>>();
            for snapshot in restored {
                if let Some(payload) = payloads.get(&snapshot.id) {
                    println!("restore\t{}\t{}", snapshot.state.as_str(), payload.as_tsv());
                } else {
                    println!(
                        "missing-payload\t{}\t{}\t{}",
                        snapshot.id.value(),
                        snapshot.state.as_str(),
                        snapshot.label
                    );
                }
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
            preflight_volume_access_scope(
                write_probe_path(&state),
                AccessIntent::Write,
                "runtime retry probe",
            )?;
            let outcome = run_scheduled_volume_task(
                parent_volume(&state),
                Priority::Background,
                "runtime retry probe",
                pressure,
                move || {
                    let _access = preflight_access_scope(
                        write_probe_path(&state),
                        AccessIntent::Write,
                        "runtime retry probe",
                    )?;
                    runtime_retry_probe(&state)
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

fn retain_payload_restore_access(
    catalog_path: &Path,
    progress_path: &Path,
) -> Result<Vec<ScopedAccessGuard>> {
    Ok(vec![
        preflight_access_scope(
            catalog_path,
            AccessIntent::Read,
            "jobs payload restore plan",
        )?,
        preflight_access_scope(
            write_probe_path(progress_path),
            AccessIntent::Write,
            "jobs payload restore plan",
        )?,
    ])
}

fn write_probe_path(path: &Path) -> &Path {
    if path.is_dir() {
        return path;
    }
    path.parent().unwrap_or(path)
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

fn sample_fairness_plan() -> gfm_jobs::JobFairnessPlan {
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

    scheduler.drain_fair_ready(
        JobFairnessPolicy::new()
            .with_quota(JobClass::Foreground, 1)
            .with_quota(JobClass::Visible, 1)
            .with_quota(JobClass::Background, 1)
            .with_quota(JobClass::Maintenance, 1)
            .with_quota(JobClass::Repair, 1),
        [],
    )
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

fn runtime_retry_probe(state: &Path) -> Result<usize> {
    let attempt = std::fs::read_to_string(state)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0)
        + 1;
    std::fs::write(state, attempt.to_string()).map_err(|err| GfmError::io(state, err))?;
    if attempt == 1 {
        Err(GfmError::Format("temporary runtime probe busy".to_string()))
    } else {
        Ok(attempt)
    }
}
