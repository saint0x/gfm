use crate::*;
use gfm_types::{GfmError, VolumeId};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Barrier};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn pops_highest_priority_first() {
    let mut scheduler = Scheduler::new();
    scheduler.schedule(Priority::Background, "crawl");
    scheduler.schedule(Priority::Interactive, "open folder");

    assert_eq!(scheduler.pop_next().unwrap().label, "open folder");
    assert_eq!(scheduler.pop_next().unwrap().label, "crawl");
}

#[test]
fn drains_ready_jobs_in_priority_order() {
    let mut scheduler = Scheduler::new();
    scheduler.schedule(Priority::Background, "background");
    scheduler.schedule(Priority::Interactive, "interactive");
    scheduler.schedule(Priority::Visible, "visible");

    let labels: Vec<_> = scheduler
        .drain_ready()
        .into_iter()
        .map(|job| job.label)
        .collect();

    assert_eq!(labels, ["interactive", "visible", "background"]);
}

#[test]
fn scheduling_action_strings_are_stable_for_operator_output() {
    assert_eq!(SchedulingAction::Run.as_str(), "Run");
    assert_eq!(SchedulingAction::Throttle.as_str(), "Throttle");
    assert_eq!(SchedulingAction::Defer.as_str(), "Defer");
}

#[test]
fn fairness_planner_interleaves_job_classes_by_quota() {
    let mut scheduler = Scheduler::new();
    for index in 0..4 {
        scheduler.schedule_in_class(
            Priority::Interactive,
            JobClass::Foreground,
            format!("foreground-{index}"),
        );
    }
    for index in 0..3 {
        scheduler.schedule_in_class(
            Priority::Background,
            JobClass::Background,
            format!("background-{index}"),
        );
    }
    scheduler.schedule_in_class(Priority::Background, JobClass::Maintenance, "compact");

    let jobs = scheduler.drain_ready();
    let plan = JobFairnessPlanner::new(
        JobFairnessPolicy::new()
            .with_quota(JobClass::Foreground, 2)
            .with_quota(JobClass::Background, 1)
            .with_quota(JobClass::Maintenance, 1),
    )
    .plan(jobs);

    assert_eq!(
        plan.labels(),
        [
            "foreground-0",
            "foreground-1",
            "background-0",
            "compact",
            "foreground-2",
            "foreground-3",
            "background-1",
            "background-2",
        ]
    );
    assert!(plan.blocked.is_empty());
}

#[test]
fn fairness_planner_honors_dependencies_and_reports_blocked_jobs() {
    let mut scheduler = Scheduler::new();
    let metadata = scheduler.schedule_in_class(
        Priority::Background,
        JobClass::Maintenance,
        "rebuild metadata",
    );
    scheduler.schedule_in_class_with_dependencies(
        Priority::Visible,
        JobClass::Repair,
        "repair search sidecars",
        vec![metadata.id],
    );
    scheduler.schedule_in_class_with_dependencies(
        Priority::Visible,
        JobClass::Repair,
        "repair missing thumbnail",
        vec![JobId::from_raw(99)],
    );

    let plan = JobFairnessPlanner::new(JobFairnessPolicy::default()).plan(scheduler.drain_ready());

    assert_eq!(
        plan.labels(),
        ["rebuild metadata", "repair search sidecars"]
    );
    assert_eq!(plan.blocked.len(), 1);
    assert_eq!(plan.blocked[0].label, "repair missing thumbnail");
    assert_eq!(plan.blocked[0].missing_dependencies, [JobId::from_raw(99)]);
}

#[test]
fn scheduler_fair_drain_retains_blocked_jobs_until_dependencies_complete() {
    let mut scheduler = Scheduler::new();
    let metadata = scheduler.schedule_in_class(
        Priority::Background,
        JobClass::Maintenance,
        "rebuild metadata",
    );
    scheduler.schedule_in_class_with_dependencies(
        Priority::Visible,
        JobClass::Repair,
        "repair search sidecars",
        [metadata.id],
    );
    let thumbnail = scheduler.schedule_in_class_with_dependencies(
        Priority::Visible,
        JobClass::Repair,
        "repair missing thumbnail",
        [JobId::from_raw(99)],
    );

    let first = scheduler.drain_fair_ready(JobFairnessPolicy::default(), []);

    assert_eq!(
        first.labels(),
        ["rebuild metadata", "repair search sidecars"]
    );
    assert_eq!(first.blocked.len(), 1);
    assert_eq!(first.blocked[0].id, thumbnail.id);
    assert_eq!(first.blocked[0].missing_dependencies, [JobId::from_raw(99)]);

    let still_blocked = scheduler.drain_fair_ready(JobFairnessPolicy::default(), []);
    assert!(still_blocked.ready.is_empty());
    assert_eq!(still_blocked.blocked.len(), 1);
    assert_eq!(still_blocked.blocked[0].id, thumbnail.id);

    let released = scheduler.drain_fair_ready(JobFairnessPolicy::default(), [JobId::from_raw(99)]);
    assert_eq!(released.labels(), ["repair missing thumbnail"]);
    assert!(released.blocked.is_empty());
}

#[test]
fn scheduler_fair_drain_drops_cancelled_jobs_before_planning() {
    let mut scheduler = Scheduler::new();
    scheduler.schedule_in_class(Priority::Visible, JobClass::Visible, "render visible rows");
    let cancelled = scheduler.schedule_in_class(
        Priority::Background,
        JobClass::Background,
        "obsolete content index",
    );
    scheduler.cancel(cancelled.id);

    let plan = scheduler.drain_fair_ready(JobFairnessPolicy::default(), []);

    assert_eq!(plan.labels(), ["render visible rows"]);
    assert!(plan.blocked.is_empty());

    let empty = scheduler.drain_fair_ready(JobFairnessPolicy::default(), []);
    assert!(empty.ready.is_empty());
    assert!(empty.blocked.is_empty());
}

#[test]
fn scheduler_cancels_queued_jobs_for_volume_and_class_only() {
    let mut scheduler = Scheduler::new();
    let target_volume = VolumeId(7);
    let other_volume = VolumeId(8);
    let target_index = scheduler.schedule_on_volume_in_class(
        Priority::Background,
        JobClass::Background,
        "index detached volume",
        target_volume,
    );
    scheduler.schedule_on_volume_in_class(
        Priority::Visible,
        JobClass::Visible,
        "render visible thumbnails",
        target_volume,
    );
    scheduler.schedule_on_volume_in_class(
        Priority::Background,
        JobClass::Background,
        "index other volume",
        other_volume,
    );

    let report = scheduler.cancel_volume_jobs(target_volume, Some(JobClass::Background));

    assert_eq!(report.volume, target_volume);
    assert_eq!(report.class, Some(JobClass::Background));
    assert_eq!(report.cancelled.len(), 1);
    assert_eq!(report.cancelled[0].id, target_index.id);
    assert!(target_index.cancellation().is_cancelled());
    assert!(report.as_tsv().contains("cancelled=1\ncancelled-job\t"));

    let plan = scheduler.drain_fair_ready(JobFairnessPolicy::default(), []);
    assert_eq!(
        plan.labels(),
        ["render visible thumbnails", "index other volume"]
    );
}

#[test]
fn scheduler_volume_cancellation_cancels_every_class_when_unfiltered() {
    let mut scheduler = Scheduler::new();
    let volume = VolumeId(9);
    let visible = scheduler.schedule_on_volume_in_class(
        Priority::Visible,
        JobClass::Visible,
        "preview\nselected",
        volume,
    );
    let repair = scheduler.schedule_on_volume_in_class(
        Priority::Background,
        JobClass::Repair,
        "repair\tindex",
        volume,
    );

    let report = scheduler.cancel_volume_jobs(volume, None);

    assert_eq!(report.cancelled.len(), 2);
    assert!(visible.cancellation().is_cancelled());
    assert!(repair.cancellation().is_cancelled());
    assert!(report.as_tsv().contains("preview\\nselected"));
    assert!(report.as_tsv().contains("repair\\tindex"));
    assert!(scheduler.drain_ready().is_empty());
}

#[test]
fn progress_store_round_trips_and_restores_active_snapshots() {
    let path = temp_path("gfm-job-progress", "gfmprogress");
    let store = JobProgressStore::new(&path);
    let running = JobProgressSnapshot::new(
        JobId::from_raw(7),
        JobClass::Foreground,
        Priority::Interactive,
        "copy user selection",
        Some(VolumeId(2)),
        10,
    )
    .with_progress(JobProgressState::Running, 4, "copied\nwith\ttab", 101);
    let completed = JobProgressSnapshot::new(
        JobId::from_raw(8),
        JobClass::Maintenance,
        Priority::Background,
        "compact content",
        None,
        3,
    )
    .with_progress(JobProgressState::Completed, 3, "done", 102);

    store
        .write_all(&[running.clone(), completed.clone()])
        .unwrap();
    assert_eq!(store.read().unwrap(), [running.clone(), completed]);
    assert_eq!(store.restorable().unwrap(), vec![running.clone()]);
    assert!(running.as_tsv().contains("\\nwith\\ttab"));

    let paused =
        running
            .clone()
            .with_progress(JobProgressState::Paused, 6, "waiting for volume", 103);
    assert!(store.upsert(paused.clone()).unwrap());
    let snapshots = store.read().unwrap();
    assert_eq!(snapshots.len(), 2);
    assert_eq!(snapshots[0], paused);
    assert_eq!(store.restorable().unwrap().len(), 1);

    std::fs::remove_file(path).unwrap();
}

#[test]
fn progress_store_upsert_skips_identical_snapshot_write() {
    let path = temp_path("gfm-job-progress-noop-upsert", "gfmprogress");
    let store = JobProgressStore::new(&path);
    let snapshot = JobProgressSnapshot::new(
        JobId::from_raw(7),
        JobClass::Foreground,
        Priority::Interactive,
        "copy user selection",
        Some(VolumeId(2)),
        10,
    )
    .with_progress(JobProgressState::Running, 4, "copying", 101);
    store.write_all(std::slice::from_ref(&snapshot)).unwrap();
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();

    assert!(!store.upsert(snapshot).unwrap());

    let after = std::fs::metadata(&path).unwrap().modified().unwrap();
    assert_eq!(before, after);

    std::fs::remove_file(path).unwrap();
}

#[test]
fn progress_store_normalizes_interrupted_jobs_for_restore() {
    let path = temp_path("gfm-job-progress-restore", "gfmprogress");
    let store = JobProgressStore::new(&path);
    let planned = JobProgressSnapshot::new(
        JobId::from_raw(1),
        JobClass::Background,
        Priority::Background,
        "pending thumbnail",
        None,
        4,
    )
    .with_progress(JobProgressState::Planned, 0, "", 10);
    let running = JobProgressSnapshot::new(
        JobId::from_raw(2),
        JobClass::Foreground,
        Priority::Interactive,
        "copy selection",
        Some(VolumeId(7)),
        10,
    )
    .with_progress(JobProgressState::Running, 6, "copy:/a->/b", 11);
    let paused = JobProgressSnapshot::new(
        JobId::from_raw(3),
        JobClass::Background,
        Priority::Background,
        "index content",
        Some(VolumeId(7)),
        20,
    )
    .with_progress(JobProgressState::Paused, 8, "pressure:throttled", 12);
    let completed = JobProgressSnapshot::new(
        JobId::from_raw(4),
        JobClass::Repair,
        Priority::Visible,
        "repair sidecar",
        None,
        1,
    )
    .with_progress(JobProgressState::Completed, 1, "completed", 13);
    store
        .write_all(&[
            planned.clone(),
            running.clone(),
            paused.clone(),
            completed.clone(),
        ])
        .unwrap();

    let restored = store.restore_interrupted(99).unwrap();

    assert_eq!(restored.len(), 3);
    assert_eq!(restored[0].state, JobProgressState::Paused);
    assert_eq!(restored[0].detail, "interrupted:planned");
    assert_eq!(restored[0].updated_ms, 99);
    assert_eq!(restored[1].state, JobProgressState::Paused);
    assert_eq!(restored[1].completed_units, running.completed_units);
    assert_eq!(restored[1].detail, "interrupted:running:copy:/a->/b");
    assert_eq!(restored[1].updated_ms, 99);
    assert_eq!(restored[2], paused);

    let snapshots = store.read().unwrap();
    assert_eq!(snapshots.len(), 4);
    assert_eq!(snapshots[3], completed);

    std::fs::remove_file(path).unwrap();
}

#[test]
fn progress_restore_skips_store_write_when_no_snapshots_are_interrupted() {
    let path = temp_path("gfm-job-progress-restore-noop", "gfmprogress");
    let store = JobProgressStore::new(&path);
    let paused = JobProgressSnapshot::new(
        JobId::from_raw(3),
        JobClass::Background,
        Priority::Background,
        "index content",
        Some(VolumeId(7)),
        20,
    )
    .with_progress(JobProgressState::Paused, 8, "pressure:throttled", 12);
    let completed = JobProgressSnapshot::new(
        JobId::from_raw(4),
        JobClass::Repair,
        Priority::Visible,
        "repair sidecar",
        None,
        1,
    )
    .with_progress(JobProgressState::Completed, 1, "completed", 13);
    store
        .write_all(&[paused.clone(), completed.clone()])
        .unwrap();
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();

    let restored = store.restore_interrupted(99).unwrap();

    let after = std::fs::metadata(&path).unwrap().modified().unwrap();
    assert_eq!(restored, vec![paused.clone()]);
    assert_eq!(store.read().unwrap(), vec![paused, completed]);
    assert_eq!(before, after);

    std::fs::remove_file(path).unwrap();
}

#[test]
fn progress_restore_missing_store_is_noop() {
    let path = temp_path("gfm-job-progress-missing-restore", "gfmprogress");
    let store = JobProgressStore::new(&path);

    let restored = store.restore_interrupted(99).unwrap();

    assert!(restored.is_empty());
    assert!(!path.exists());
}

#[test]
fn scheduling_pressure_defers_background_under_saturated_io() {
    let pressure = SchedulingPressure {
        io: JobIoPressure::Saturated,
        ..SchedulingPressure::default()
    };

    let decision = pressure.decide(Priority::Background, 8, 4);

    assert_eq!(decision.action, SchedulingAction::Defer);
    assert_eq!(decision.worker_threads, 0);
    assert_eq!(decision.volume_policy.default_limit(), 1);
}

#[test]
fn scheduling_pressure_throttles_background_under_active_user_load() {
    let pressure = SchedulingPressure {
        user_activity: JobUserActivity::Active,
        ..SchedulingPressure::default()
    };

    let decision = pressure.decide(Priority::Background, 8, 4);

    assert_eq!(decision.action, SchedulingAction::Throttle);
    assert_eq!(decision.worker_threads, 4);
    assert_eq!(decision.volume_policy.default_limit(), 2);
}

#[test]
fn scheduling_pressure_throttles_background_on_battery_power() {
    let pressure = SchedulingPressure {
        battery: JobBatteryState::Battery,
        ..SchedulingPressure::default()
    };

    let decision = pressure.decide(Priority::Background, 8, 4);

    assert_eq!(decision.action, SchedulingAction::Throttle);
    assert_eq!(decision.worker_threads, 4);
    assert_eq!(decision.volume_policy.default_limit(), 2);
}

#[test]
fn scheduling_pressure_preserves_visible_work_under_host_pressure() {
    let pressure = SchedulingPressure {
        io: JobIoPressure::Saturated,
        thermal: JobThermalState::Critical,
        battery: JobBatteryState::LowPower,
        user_activity: JobUserActivity::Active,
    };

    let decision = pressure.decide(Priority::Visible, 8, 4);

    assert_eq!(decision.action, SchedulingAction::Run);
    assert_eq!(decision.worker_threads, 8);
    assert_eq!(decision.volume_policy.default_limit(), 4);
}

#[test]
fn structured_cancellation_propagates_to_nested_children() {
    let root = Cancellation::default();
    let child = root.child();
    let grandchild = child.child();

    root.cancel();

    assert!(root.is_cancelled());
    assert!(child.is_cancelled());
    assert!(grandchild.is_cancelled());
    assert!(matches!(grandchild.check(), Err(GfmError::Cancelled)));
}

#[test]
fn child_cancellation_does_not_cancel_parent_or_siblings() {
    let root = Cancellation::default();
    let first = root.child();
    let second = root.child();

    first.cancel();

    assert!(first.is_cancelled());
    assert!(!root.is_cancelled());
    assert!(!second.is_cancelled());
    assert!(second.check().is_ok());
}

#[test]
fn worker_pool_runs_tasks_and_reports_outcomes() {
    let mut scheduler = Scheduler::new();
    let first = scheduler.schedule(Priority::Background, "first");
    let second = scheduler.schedule(Priority::Background, "second");
    second.cancel();

    let report = WorkerPool::new(2).run(vec![
        Task::new(first, |_| Ok(())),
        Task::new(second, |cancellation| cancellation.check()),
    ]);

    assert_eq!(report.completed(), 1);
    assert_eq!(report.cancelled(), 1);
    assert_eq!(report.failed(), 0);
}

#[test]
fn worker_pool_allows_nested_child_cancellation_checks() {
    let mut scheduler = Scheduler::new();
    let job = scheduler.schedule(Priority::Visible, "nested preview");
    let job_cancel = job.cancellation();

    let report = WorkerPool::new(1).run(vec![Task::new(job, move |_| {
        let child = job_cancel.child();
        job_cancel.cancel();
        child.check()
    })]);

    assert_eq!(report.cancelled(), 1);
}

#[test]
fn worker_pool_enforces_per_volume_concurrency_limit() {
    let mut scheduler = Scheduler::new();
    let volume = VolumeId(7);
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let tasks: Vec<_> = (0..4)
        .map(|index| {
            let job =
                scheduler.schedule_on_volume(Priority::Background, format!("copy-{index}"), volume);
            let active = Arc::clone(&active);
            let peak = Arc::clone(&peak);
            Task::new(job, move |_| {
                let current = active.fetch_add(1, AtomicOrdering::SeqCst) + 1;
                peak.fetch_max(current, AtomicOrdering::SeqCst);
                std::thread::sleep(Duration::from_millis(5));
                active.fetch_sub(1, AtomicOrdering::SeqCst);
                Ok(())
            })
        })
        .collect();

    let report = WorkerPool::new(4).run_isolated(tasks, VolumeConcurrencyPolicy::new(1));

    assert_eq!(report.completed(), 4);
    assert_eq!(peak.load(AtomicOrdering::SeqCst), 1);
}

#[test]
fn worker_pool_runs_independent_volumes_concurrently() {
    let mut scheduler = Scheduler::new();
    let barrier = Arc::new(Barrier::new(2));
    let first_barrier = Arc::clone(&barrier);
    let second_barrier = Arc::clone(&barrier);
    let first = scheduler.schedule_on_volume(Priority::Visible, "visible-a", VolumeId(1));
    let second = scheduler.schedule_on_volume(Priority::Visible, "visible-b", VolumeId(2));

    let report = WorkerPool::new(2).run_isolated(
        vec![
            Task::new(first, move |_| {
                first_barrier.wait();
                Ok(())
            }),
            Task::new(second, move |_| {
                second_barrier.wait();
                Ok(())
            }),
        ],
        VolumeConcurrencyPolicy::new(1),
    );

    assert_eq!(report.completed(), 2);
}

#[test]
fn isolated_retriable_worker_enforces_per_volume_limit() {
    let path = temp_path("gfm-isolated-job-journal", "journal");
    let journal = JobJournal::new(&path);
    let mut scheduler = Scheduler::new();
    let volume = VolumeId(17);
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let tasks: Vec<_> = (0..3)
        .map(|index| {
            let job = scheduler.schedule_on_volume(
                Priority::Background,
                format!("content-{index}"),
                volume,
            );
            let active = Arc::clone(&active);
            let peak = Arc::clone(&peak);
            RetriableTask::new(job, move |_| {
                let current = active.fetch_add(1, AtomicOrdering::SeqCst) + 1;
                peak.fetch_max(current, AtomicOrdering::SeqCst);
                std::thread::sleep(Duration::from_millis(5));
                active.fetch_sub(1, AtomicOrdering::SeqCst);
                Ok(())
            })
        })
        .collect();

    let report = WorkerPool::new(3).run_retriable_isolated(
        tasks,
        &journal,
        RetryPolicy { max_attempts: 2 },
        VolumeConcurrencyPolicy::new(1),
    );

    assert_eq!(report.completed(), 3);
    assert_eq!(peak.load(AtomicOrdering::SeqCst), 1);
    assert_eq!(journal.read().unwrap().len(), 6);

    std::fs::remove_file(path).unwrap();
}

#[test]
fn retriable_worker_journals_attempts_until_success() {
    let path = temp_path("gfm-job-journal", "journal");
    let journal = JobJournal::new(&path);
    let mut scheduler = Scheduler::new();
    let job = scheduler.schedule(Priority::Background, "retry content");
    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_task = Arc::clone(&attempts);

    let report = WorkerPool::new(1).run_retriable(
        vec![RetriableTask::new(job, move |_| {
            if attempts_task.fetch_add(1, AtomicOrdering::SeqCst) == 0 {
                Err(GfmError::Format("temporary failure".to_string()))
            } else {
                Ok(())
            }
        })],
        &journal,
        RetryPolicy { max_attempts: 2 },
    );
    let entries = journal.read().unwrap();

    assert_eq!(report.completed(), 1);
    assert_eq!(attempts.load(AtomicOrdering::SeqCst), 2);
    assert_eq!(entries.len(), 4);
    assert_eq!(entries[0].status, TaskStatus::Started);
    assert!(matches!(entries[1].status, TaskStatus::Failed(_)));
    assert_eq!(entries[2].status, TaskStatus::Started);
    assert_eq!(entries[3].status, TaskStatus::Completed);

    std::fs::remove_file(path).unwrap();
}

#[test]
fn job_journal_append_creates_parent_directory() {
    let root = temp_path("gfm-job-journal-parent", "root");
    let path = root.join("nested").join("jobs.journal");
    let journal = JobJournal::new(&path);
    let entry = JournalEntry {
        id: JobId::from_raw(41),
        label: "resume indexing".to_string(),
        attempt: 1,
        status: TaskStatus::Started,
    };

    journal.append(&entry).unwrap();

    assert_eq!(journal.read().unwrap(), vec![entry]);

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn journal_identifies_interrupted_and_retryable_jobs() {
    let path = temp_path("gfm-job-recovery", "journal");
    let journal = JobJournal::new(&path);
    let interrupted = JobId::from_raw(10);
    let failed = JobId::from_raw(11);
    let completed = JobId::from_raw(12);

    journal
        .append(&JournalEntry {
            id: interrupted,
            label: "interrupted".to_string(),
            attempt: 1,
            status: TaskStatus::Started,
        })
        .unwrap();
    journal
        .append(&JournalEntry {
            id: failed,
            label: "failed".to_string(),
            attempt: 1,
            status: TaskStatus::Started,
        })
        .unwrap();
    journal
        .append(&JournalEntry {
            id: failed,
            label: "failed".to_string(),
            attempt: 1,
            status: TaskStatus::Failed("transient".to_string()),
        })
        .unwrap();
    journal
        .append(&JournalEntry {
            id: completed,
            label: "completed".to_string(),
            attempt: 1,
            status: TaskStatus::Started,
        })
        .unwrap();
    journal
        .append(&JournalEntry {
            id: completed,
            label: "completed".to_string(),
            attempt: 1,
            status: TaskStatus::Completed,
        })
        .unwrap();

    let recoverable = journal
        .recoverable(RetryPolicy { max_attempts: 2 })
        .unwrap();

    assert_eq!(
        recoverable,
        vec![
            RecoveryJob {
                id: interrupted,
                label: "interrupted".to_string(),
                attempts: 1,
                reason: RecoveryReason::Interrupted,
            },
            RecoveryJob {
                id: failed,
                label: "failed".to_string(),
                attempts: 1,
                reason: RecoveryReason::RetryableFailure,
            },
        ]
    );

    std::fs::remove_file(path).unwrap();
}

#[test]
fn retry_policy_classifies_failures_and_backoff() {
    let policy = RetryPolicy { max_attempts: 3 };

    let transient = policy.retry_decision(1, "temporary busy timeout");
    assert_eq!(transient.class, FailureClass::Transient);
    assert!(transient.retryable);
    assert_eq!(transient.next_delay_ms, 25);

    let offline = policy.retry_decision(2, "volume is offline and not mounted");
    assert_eq!(offline.class, FailureClass::OfflineVolume);
    assert!(offline.retryable);
    assert_eq!(offline.next_delay_ms, 500);

    for (message, class) in [
        ("permission denied by tcc", FailureClass::Permission),
        ("missing source: no such file", FailureClass::MissingFile),
        ("corrupt archive checksum", FailureClass::CorruptFile),
        ("destination conflict", FailureClass::Permanent),
    ] {
        let decision = policy.retry_decision(1, message);
        assert_eq!(decision.class, class);
        assert!(!decision.retryable);
        assert_eq!(decision.next_delay_ms, 0);
    }
}

#[test]
fn journal_skips_non_retryable_failed_jobs() {
    let path = temp_path("gfm-job-recovery-classified", "journal");
    let journal = JobJournal::new(&path);
    let transient = JobId::from_raw(31);
    let permission = JobId::from_raw(32);

    for (id, label, message) in [
        (transient, "transient", "temporary failure"),
        (permission, "permission", "permission denied"),
    ] {
        journal
            .append(&JournalEntry {
                id,
                label: label.to_string(),
                attempt: 1,
                status: TaskStatus::Started,
            })
            .unwrap();
        journal
            .append(&JournalEntry {
                id,
                label: label.to_string(),
                attempt: 1,
                status: TaskStatus::Failed(message.to_string()),
            })
            .unwrap();
    }

    let recoverable = journal
        .recoverable(RetryPolicy { max_attempts: 2 })
        .unwrap();
    assert_eq!(recoverable.len(), 1);
    assert_eq!(recoverable[0].id, transient);

    std::fs::remove_file(path).unwrap();
}

#[test]
fn journal_does_not_recover_exhausted_failures() {
    let path = temp_path("gfm-job-recovery-exhausted", "journal");
    let journal = JobJournal::new(&path);
    let failed = JobId::from_raw(21);

    journal
        .append(&JournalEntry {
            id: failed,
            label: "failed".to_string(),
            attempt: 1,
            status: TaskStatus::Started,
        })
        .unwrap();
    journal
        .append(&JournalEntry {
            id: failed,
            label: "failed".to_string(),
            attempt: 1,
            status: TaskStatus::Failed("permanent".to_string()),
        })
        .unwrap();

    assert!(journal
        .recoverable(RetryPolicy { max_attempts: 1 })
        .unwrap()
        .is_empty());

    std::fs::remove_file(path).unwrap();
}

#[test]
fn payload_catalog_round_trips_all_job_families() {
    let path = temp_path("gfm-job-payload-catalog", "gfmjobs");
    let catalog = JobPayloadCatalog::new(&path);
    let records = vec![
        JobPayloadRecord::new(
            JobId::from_raw(1),
            JobPayloadKind::Operation,
            "copy",
            "ops/copy.gfmjob",
            Some(VolumeId(7)),
            "copy source to target",
        ),
        JobPayloadRecord::new(
            JobId::from_raw(2),
            JobPayloadKind::Indexing,
            "index",
            "index/content.gfmjob",
            Some(VolumeId(7)),
            "index content",
        ),
        JobPayloadRecord::new(
            JobId::from_raw(3),
            JobPayloadKind::Extraction,
            "extract",
            "extract/doc.gfmjob",
            Some(VolumeId(7)),
            "extract document",
        ),
        JobPayloadRecord::new(
            JobId::from_raw(4),
            JobPayloadKind::Thumbnail,
            "thumbnail",
            "preview/thumb.gfmjob",
            Some(VolumeId(7)),
            "make thumbnail",
        ),
        JobPayloadRecord::new(
            JobId::from_raw(5),
            JobPayloadKind::Preview,
            "preview",
            "preview/quicklook.gfmjob",
            Some(VolumeId(7)),
            "make preview",
        ),
        JobPayloadRecord::new(
            JobId::from_raw(6),
            JobPayloadKind::Repair,
            "repair",
            "repair/sidecar.gfmjob",
            None,
            "repair sidecar",
        ),
    ];

    catalog.write_all(&records).unwrap();
    assert_eq!(catalog.read().unwrap(), records);

    let appended = JobPayloadRecord::new(
        JobId::from_raw(7),
        JobPayloadKind::Repair,
        "repair escaped",
        "repair/escaped.gfmjob",
        None,
        "line\nwith\ttabs",
    );
    catalog.append(&appended).unwrap();
    let read = catalog.read().unwrap();
    assert_eq!(read.last(), Some(&appended));

    std::fs::remove_file(path).unwrap();
}

#[test]
fn payload_catalog_write_all_creates_parent_directory() {
    let root = temp_path("gfm-job-payload-catalog-parent", "root");
    let path = root.join("nested").join("payloads.gfmjobs");
    let catalog = JobPayloadCatalog::new(&path);
    let record = JobPayloadRecord::new(
        JobId::from_raw(8),
        JobPayloadKind::Preview,
        "preview",
        "preview/item.gfmjob",
        None,
        "render preview",
    );

    catalog.write_all(std::slice::from_ref(&record)).unwrap();

    assert_eq!(catalog.read().unwrap(), vec![record]);

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn payload_catalog_append_replaces_existing_job_id() {
    let path = temp_path("gfm-job-payload-catalog-replace", "gfmjobs");
    let catalog = JobPayloadCatalog::new(&path);
    let first = JobPayloadRecord::new(
        JobId::from_raw(8),
        JobPayloadKind::Preview,
        "quicklook preview",
        "preview/old.gfmjob",
        Some(VolumeId(7)),
        "preview:/old",
    );
    let second = JobPayloadRecord::new(
        JobId::from_raw(8),
        JobPayloadKind::Preview,
        "quicklook preview",
        "preview/current.gfmjob",
        Some(VolumeId(9)),
        "preview:/current",
    );

    catalog.append(&first).unwrap();
    catalog.append(&second).unwrap();

    let records = catalog.read().unwrap();
    assert_eq!(records, vec![second]);
    let before_noop = std::fs::metadata(&path).unwrap().modified().unwrap();
    let current = catalog.read().unwrap().remove(0);
    catalog.append(&current).unwrap();
    let after_noop = std::fs::metadata(&path).unwrap().modified().unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    assert_eq!(before_noop, after_noop);
    assert_eq!(raw.matches("\npayload\t8\tpreview\t").count(), 1);
    assert!(!raw.contains("preview/old.gfmjob"), "{raw}");
    assert!(raw.contains("preview/current.gfmjob"), "{raw}");

    std::fs::remove_file(path).unwrap();
}

#[test]
fn payload_catalog_temp_paths_are_unique_within_process() {
    let path = temp_path("gfm-job-payload-catalog-unique-temp", "gfmjobs");
    let catalog = JobPayloadCatalog::new(&path);

    let first = catalog.temp_path();
    let second = catalog.temp_path();

    assert_ne!(first, second);
    assert_eq!(first.parent(), path.parent());
    assert_eq!(second.parent(), path.parent());
    assert!(first
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".tmp")));
    assert!(second
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".tmp")));
}

#[test]
fn payload_catalog_reads_requested_records_only() {
    let path = temp_path("gfm-job-payload-catalog-filtered", "gfmjobs");
    let catalog = JobPayloadCatalog::new(&path);
    let records = vec![
        JobPayloadRecord::new(
            JobId::from_raw(1),
            JobPayloadKind::Operation,
            "copy",
            "ops/copy.gfmjob",
            Some(VolumeId(7)),
            "copy source to target",
        ),
        JobPayloadRecord::new(
            JobId::from_raw(2),
            JobPayloadKind::Indexing,
            "index",
            "index/content.gfmjob",
            Some(VolumeId(7)),
            "index content",
        ),
        JobPayloadRecord::new(
            JobId::from_raw(3),
            JobPayloadKind::Repair,
            "repair",
            "repair/sidecar.gfmjob",
            None,
            "repair sidecar",
        ),
    ];
    catalog.write_all(&records).unwrap();

    assert_eq!(
        catalog
            .read_for_ids([JobId::from_raw(3), JobId::from_raw(1)])
            .unwrap(),
        vec![records[0].clone(), records[2].clone()]
    );
    assert!(catalog.read_for_ids([]).unwrap().is_empty());

    std::fs::remove_file(path).unwrap();
}

fn temp_path(prefix: &str, extension: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "{}-{}.{}",
        prefix,
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        extension
    ))
}
