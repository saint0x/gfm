use crate::runtime::{default_job_journal_path, run_scheduled_volume_task};
use crate::{parent_volume, parse_optional_scheduling_pressure, parse_usize_arg, required_path};
use gfm_jobs::{
    Cancellation, JobClass, JobFairnessPolicy, JobJournal, JobPayloadCatalog, JobPayloadKind,
    JobPayloadRecord, JobProgressSnapshot, JobProgressState, JobProgressStore, Priority,
    RecoveryReason, RetryPolicy, Scheduler,
};
use gfm_types::{GfmError, Result, VolumeId};
use std::path::{Path, PathBuf};

pub(crate) fn run(command: &str, args: &mut impl Iterator<Item = String>) -> Result<bool> {
    match command {
        "jobs-recover" => {
            let journal = args
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(default_job_journal_path);
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
            let store = JobProgressStore::new(&path);
            for snapshot in sample_progress_snapshots() {
                store.upsert(snapshot)?;
            }
            for snapshot in store.restorable()? {
                println!("{}", snapshot.as_tsv());
            }
        }
        "jobs-cancel-tree" => {
            for line in sample_cancellation_tree_report() {
                println!("{line}");
            }
        }
        "jobs-runtime-retry-probe" => {
            let state = required_path(
                args.next(),
                "jobs-runtime-retry-probe requires an attempt state path",
            )?;
            let pressure = parse_optional_scheduling_pressure(args)?;
            let outcome = run_scheduled_volume_task(
                parent_volume(&state),
                Priority::Background,
                "runtime retry probe",
                pressure,
                move || runtime_retry_probe(&state),
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
