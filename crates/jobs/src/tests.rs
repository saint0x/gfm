use crate::*;
use gfm_types::{GfmError, VolumeId};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Barrier, Mutex};
use std::time::Duration;
use std::time::Instant;
use std::time::{SystemTime, UNIX_EPOCH};

static CWD_LOCK: Mutex<()> = Mutex::new(());

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

    assert_eq!(plan.labels(), ["rebuild metadata"]);
    assert_eq!(plan.blocked.len(), 2);
    assert_eq!(plan.blocked[0].label, "repair search sidecars");
    assert_eq!(plan.blocked[0].missing_dependencies, [metadata.id]);
    assert_eq!(plan.blocked[1].label, "repair missing thumbnail");
    assert_eq!(plan.blocked[1].missing_dependencies, [JobId::from_raw(99)]);
}

#[test]
fn fairness_planner_does_not_head_of_line_block_class_ready_jobs() {
    let mut scheduler = Scheduler::new();
    let missing = JobId::from_raw(77);
    let blocked = scheduler.schedule_in_class_with_dependencies(
        Priority::Visible,
        JobClass::Repair,
        "blocked repair",
        [missing],
    );
    scheduler.schedule_in_class(Priority::Visible, JobClass::Repair, "ready repair");

    let plan = JobFairnessPlanner::new(JobFairnessPolicy::default()).plan(scheduler.drain_ready());

    assert_eq!(plan.labels(), ["ready repair"]);
    assert_eq!(plan.blocked.len(), 1);
    assert_eq!(plan.blocked[0].id, blocked.id);
    assert_eq!(plan.blocked[0].missing_dependencies, [missing]);
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

    assert_eq!(first.labels(), ["rebuild metadata"]);
    assert_eq!(first.blocked.len(), 2);
    assert_eq!(first.blocked[0].missing_dependencies, [metadata.id]);
    assert_eq!(first.blocked[1].id, thumbnail.id);
    assert_eq!(first.blocked[1].missing_dependencies, [JobId::from_raw(99)]);

    let still_blocked = scheduler.drain_fair_ready(JobFairnessPolicy::default(), []);
    assert!(still_blocked.ready.is_empty());
    assert_eq!(still_blocked.blocked.len(), 2);
    assert_eq!(still_blocked.blocked[0].missing_dependencies, [metadata.id]);
    assert_eq!(still_blocked.blocked[1].id, thumbnail.id);

    let sidecar_released = scheduler.drain_fair_ready(JobFairnessPolicy::default(), [metadata.id]);
    assert_eq!(sidecar_released.labels(), ["repair search sidecars"]);
    assert_eq!(sidecar_released.blocked.len(), 1);
    assert_eq!(sidecar_released.blocked[0].id, thumbnail.id);

    let thumbnail_released =
        scheduler.drain_fair_ready(JobFairnessPolicy::default(), [JobId::from_raw(99)]);
    assert_eq!(thumbnail_released.labels(), ["repair missing thumbnail"]);
    assert!(thumbnail_released.blocked.is_empty());
}

#[test]
fn scheduler_fair_drain_uses_persisted_completed_dependencies() {
    let mut scheduler = Scheduler::new();
    let metadata = scheduler.schedule_in_class(
        Priority::Background,
        JobClass::Maintenance,
        "rebuild metadata",
    );
    let sidecar = scheduler.schedule_in_class_with_dependencies(
        Priority::Visible,
        JobClass::Repair,
        "repair derived sidecar",
        [metadata.id],
    );

    let first = scheduler.drain_fair_ready(JobFairnessPolicy::default(), []);

    assert_eq!(first.labels(), ["rebuild metadata"]);
    assert_eq!(first.blocked[0].id, sidecar.id);

    scheduler.mark_completed(metadata.id);
    let released = scheduler.drain_fair_ready(JobFairnessPolicy::default(), []);

    assert_eq!(scheduler.completed_jobs(), [metadata.id]);
    assert_eq!(released.labels(), ["repair derived sidecar"]);
    assert!(released.blocked.is_empty());
}

#[test]
fn scheduler_cancelled_job_does_not_block_completed_dependency_ledger() {
    let mut scheduler = Scheduler::new();
    let metadata = scheduler.schedule_in_class(
        Priority::Background,
        JobClass::Maintenance,
        "rebuild metadata",
    );
    let repair = scheduler.schedule_in_class_with_dependencies(
        Priority::Visible,
        JobClass::Repair,
        "repair derived sidecar",
        [metadata.id],
    );

    scheduler.cancel(metadata.id);
    scheduler.mark_completed(metadata.id);
    let plan = scheduler.drain_fair_ready(JobFairnessPolicy::default(), []);

    assert_eq!(plan.labels(), ["repair derived sidecar"]);
    assert_eq!(plan.ready[0].id, repair.id);
    assert!(plan.blocked.is_empty());
    assert_eq!(scheduler.completed_jobs(), [metadata.id]);
}

#[test]
fn scheduler_mark_completed_checked_preserves_ledger_when_cancelled_first() {
    let mut scheduler = Scheduler::new();
    let metadata = scheduler.schedule_in_class(
        Priority::Background,
        JobClass::Maintenance,
        "rebuild metadata",
    );

    let result = scheduler.mark_completed_checked(metadata.id, || Err(GfmError::Cancelled));

    assert_eq!(result, Err(GfmError::Cancelled));
    assert!(scheduler.completed_jobs().is_empty());
    assert_eq!(
        scheduler
            .drain_fair_ready(JobFairnessPolicy::default(), [])
            .labels(),
        ["rebuild metadata"]
    );
}

#[test]
fn scheduler_mark_completed_checked_rolls_back_when_cancelled_after_mutation() {
    let mut scheduler = Scheduler::new();
    let metadata = scheduler.schedule_in_class(
        Priority::Background,
        JobClass::Maintenance,
        "rebuild metadata",
    );
    let repair = scheduler.schedule_in_class_with_dependencies(
        Priority::Visible,
        JobClass::Repair,
        "repair derived sidecar",
        [metadata.id],
    );
    scheduler.cancel(metadata.id);
    let mut calls = 0;

    let result = scheduler.mark_completed_checked(metadata.id, || {
        calls += 1;
        if calls == 2 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });
    let plan = scheduler.drain_fair_ready(JobFairnessPolicy::default(), []);

    assert_eq!(result, Err(GfmError::Cancelled));
    assert!(scheduler.completed_jobs().is_empty());
    assert!(plan.ready.is_empty());
    assert_eq!(plan.blocked[0].id, repair.id);
    assert_eq!(plan.blocked[0].missing_dependencies, [metadata.id]);
}

#[test]
fn scheduler_completed_dependency_ledger_prunes_unreferenced_old_jobs() {
    let mut scheduler = Scheduler::new();
    let mut newest = Vec::new();

    for index in 0..4104 {
        let job = scheduler.schedule(Priority::Background, format!("completed-{index}"));
        scheduler.mark_completed(job.id);
        if index >= 4096 {
            newest.push(job.id);
        }
    }

    let completed = scheduler.completed_jobs();

    assert!(!completed.contains(&JobId::from_raw(1)));
    for id in newest {
        assert!(
            completed.contains(&id),
            "recent completion should be retained"
        );
    }
}

#[test]
fn scheduler_completed_dependency_ledger_keeps_old_ids_referenced_by_queue() {
    let mut scheduler = Scheduler::new();
    let metadata = scheduler.schedule_in_class(
        Priority::Background,
        JobClass::Maintenance,
        "rebuild metadata",
    );
    scheduler.schedule_in_class_with_dependencies(
        Priority::Visible,
        JobClass::Repair,
        "repair derived sidecar",
        [metadata.id],
    );

    scheduler.mark_completed(metadata.id);
    for index in 0..4104 {
        let job = scheduler.schedule(Priority::Background, format!("completed-{index}"));
        scheduler.mark_completed(job.id);
    }
    let plan = scheduler.drain_fair_ready(JobFairnessPolicy::default(), []);

    assert!(scheduler.completed_jobs().contains(&metadata.id));
    assert!(plan.labels().contains(&"repair derived sidecar"));
}

#[test]
fn scheduler_worker_report_completion_releases_blocked_dependency() {
    let mut scheduler = Scheduler::new();
    let metadata = scheduler.schedule_in_class(
        Priority::Background,
        JobClass::Maintenance,
        "rebuild metadata",
    );
    let repair = scheduler.schedule_in_class_with_dependencies(
        Priority::Visible,
        JobClass::Repair,
        "repair derived sidecar",
        [metadata.id],
    );

    let first = scheduler.drain_fair_ready(JobFairnessPolicy::default(), []);
    assert_eq!(first.labels(), ["rebuild metadata"]);
    assert_eq!(first.blocked[0].id, repair.id);

    let ingestion = scheduler.apply_worker_report(&WorkerReport {
        outcomes: vec![TaskOutcome {
            id: metadata.id,
            label: metadata.label,
            status: TaskStatus::Completed,
        }],
    });
    let released = scheduler.drain_fair_ready(JobFairnessPolicy::default(), []);

    assert_eq!(ingestion.completed, [metadata.id]);
    assert!(ingestion.cancelled.is_empty());
    assert!(ingestion.failed.is_empty());
    assert_eq!(
        ingestion.as_tsv(),
        format!(
            "scheduler-ingest\tcompleted=1\tcancelled=0\tfailed=0\tcompleted-ids={}\tcancelled-ids=-\tfailed-ids=-",
            metadata.id.value()
        )
    );
    assert_eq!(released.labels(), ["repair derived sidecar"]);
    assert!(released.blocked.is_empty());
}

#[test]
fn scheduler_worker_report_cancelled_and_failed_do_not_release_dependencies() {
    let mut scheduler = Scheduler::new();
    let cancelled_metadata = scheduler.schedule_in_class(
        Priority::Background,
        JobClass::Maintenance,
        "cancelled metadata",
    );
    let failed_metadata = scheduler.schedule_in_class(
        Priority::Background,
        JobClass::Maintenance,
        "failed metadata",
    );
    let repair_cancelled = scheduler.schedule_in_class_with_dependencies(
        Priority::Visible,
        JobClass::Repair,
        "repair cancelled sidecar",
        [cancelled_metadata.id],
    );
    let repair_failed = scheduler.schedule_in_class_with_dependencies(
        Priority::Visible,
        JobClass::Repair,
        "repair failed sidecar",
        [failed_metadata.id],
    );

    let first = scheduler.drain_fair_ready(JobFairnessPolicy::default(), []);
    assert_eq!(first.labels(), ["cancelled metadata", "failed metadata"]);

    let ingestion = scheduler.apply_worker_report(&WorkerReport {
        outcomes: vec![
            TaskOutcome {
                id: cancelled_metadata.id,
                label: cancelled_metadata.label,
                status: TaskStatus::Cancelled,
            },
            TaskOutcome {
                id: failed_metadata.id,
                label: failed_metadata.label,
                status: TaskStatus::Failed("provider unavailable".to_string()),
            },
        ],
    });
    let blocked = scheduler.drain_fair_ready(JobFairnessPolicy::default(), []);

    assert!(ingestion.completed.is_empty());
    assert_eq!(ingestion.cancelled, [cancelled_metadata.id]);
    assert_eq!(ingestion.failed, [failed_metadata.id]);
    assert!(blocked.ready.is_empty());
    assert_eq!(blocked.blocked.len(), 2);
    assert_eq!(blocked.blocked[0].id, repair_cancelled.id);
    assert_eq!(
        blocked.blocked[0].missing_dependencies,
        [cancelled_metadata.id]
    );
    assert_eq!(blocked.blocked[1].id, repair_failed.id);
    assert_eq!(
        blocked.blocked[1].missing_dependencies,
        [failed_metadata.id]
    );
}

#[test]
fn scheduler_worker_report_duplicate_outcomes_are_rejected_without_mutation() {
    let mut scheduler = Scheduler::new();
    let metadata = scheduler.schedule_in_class(
        Priority::Background,
        JobClass::Maintenance,
        "rebuild metadata",
    );
    let repair = scheduler.schedule_in_class_with_dependencies(
        Priority::Visible,
        JobClass::Repair,
        "repair derived sidecar",
        [metadata.id],
    );
    let first = scheduler.drain_fair_ready(JobFairnessPolicy::default(), []);
    assert_eq!(first.labels(), ["rebuild metadata"]);

    let result = scheduler.apply_worker_report_checked(
        &WorkerReport {
            outcomes: vec![
                TaskOutcome {
                    id: metadata.id,
                    label: "first".to_string(),
                    status: TaskStatus::Completed,
                },
                TaskOutcome {
                    id: metadata.id,
                    label: "second".to_string(),
                    status: TaskStatus::Cancelled,
                },
            ],
        },
        || Ok(()),
    );
    let plan = scheduler.drain_fair_ready(JobFairnessPolicy::default(), []);

    assert_eq!(
        result,
        Err(GfmError::Format(format!(
            "duplicate worker outcome for job {}",
            metadata.id.value()
        )))
    );
    assert!(scheduler.completed_jobs().is_empty());
    assert!(plan.ready.is_empty());
    assert_eq!(plan.blocked[0].id, repair.id);
    assert_eq!(plan.blocked[0].missing_dependencies, [metadata.id]);
}

#[test]
fn scheduler_worker_report_started_rows_do_not_conflict_with_terminal_outcomes() {
    let mut scheduler = Scheduler::new();
    let metadata = scheduler.schedule_in_class(
        Priority::Background,
        JobClass::Maintenance,
        "rebuild metadata",
    );
    let repair = scheduler.schedule_in_class_with_dependencies(
        Priority::Visible,
        JobClass::Repair,
        "repair derived sidecar",
        [metadata.id],
    );
    let first = scheduler.drain_fair_ready(JobFairnessPolicy::default(), []);
    assert_eq!(first.labels(), ["rebuild metadata"]);

    let ingestion = scheduler.apply_worker_report(&WorkerReport {
        outcomes: vec![
            TaskOutcome {
                id: metadata.id,
                label: "start".to_string(),
                status: TaskStatus::Started,
            },
            TaskOutcome {
                id: metadata.id,
                label: "done".to_string(),
                status: TaskStatus::Completed,
            },
        ],
    });
    let plan = scheduler.drain_fair_ready(JobFairnessPolicy::default(), []);

    assert_eq!(ingestion.completed, [metadata.id]);
    assert_eq!(plan.ready[0].id, repair.id);
    assert!(plan.blocked.is_empty());
}

#[test]
fn scheduler_worker_report_checked_preserves_state_when_cancelled_before_commit() {
    let mut scheduler = Scheduler::new();
    let metadata = scheduler.schedule_in_class(
        Priority::Background,
        JobClass::Maintenance,
        "rebuild metadata",
    );
    let repair = scheduler.schedule_in_class_with_dependencies(
        Priority::Visible,
        JobClass::Repair,
        "repair derived sidecar",
        [metadata.id],
    );
    let first = scheduler.drain_fair_ready(JobFairnessPolicy::default(), []);
    assert_eq!(first.labels(), ["rebuild metadata"]);
    let mut checks = 0usize;

    let result = scheduler.apply_worker_report_checked(
        &WorkerReport {
            outcomes: vec![TaskOutcome {
                id: metadata.id,
                label: metadata.label,
                status: TaskStatus::Completed,
            }],
        },
        || {
            checks += 1;
            if checks == 4 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        },
    );
    let plan = scheduler.drain_fair_ready(JobFairnessPolicy::default(), []);

    assert_eq!(result, Err(GfmError::Cancelled));
    assert!(scheduler.completed_jobs().is_empty());
    assert!(plan.ready.is_empty());
    assert_eq!(plan.blocked[0].id, repair.id);
    assert_eq!(plan.blocked[0].missing_dependencies, [metadata.id]);
}

#[test]
fn scheduler_fair_drain_does_not_release_same_batch_dependencies() {
    let mut scheduler = Scheduler::new();
    let metadata = scheduler.schedule_in_class(
        Priority::Background,
        JobClass::Maintenance,
        "rebuild metadata",
    );
    let sidecar = scheduler.schedule_in_class_with_dependencies(
        Priority::Visible,
        JobClass::Repair,
        "repair derived sidecar",
        [metadata.id],
    );

    let first = scheduler.drain_fair_ready(JobFairnessPolicy::default(), []);

    assert_eq!(first.labels(), ["rebuild metadata"]);
    assert_eq!(first.blocked.len(), 1);
    assert_eq!(first.blocked[0].id, sidecar.id);
    assert_eq!(first.blocked[0].missing_dependencies, [metadata.id]);

    let still_blocked = scheduler.drain_fair_ready(JobFairnessPolicy::default(), []);
    assert!(still_blocked.ready.is_empty());
    assert_eq!(still_blocked.blocked.len(), 1);
    assert_eq!(still_blocked.blocked[0].id, sidecar.id);

    let released = scheduler.drain_fair_ready(JobFairnessPolicy::default(), [metadata.id]);
    assert_eq!(released.labels(), ["repair derived sidecar"]);
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
fn checked_drain_ready_preserves_queue_when_cancelled_mid_scan() {
    let mut scheduler = Scheduler::new();
    for index in 0..32 {
        scheduler.schedule(Priority::Background, format!("background-{index}"));
    }
    let mut checks = 0usize;
    let err = scheduler
        .drain_ready_checked(|| {
            checks += 1;
            if checks > 10 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        })
        .expect_err("checked drain should stop while scanning queued jobs");

    assert!(matches!(err, GfmError::Cancelled));
    assert_eq!(scheduler.drain_ready().len(), 32);
}

#[test]
fn checked_fair_drain_preserves_queue_when_cancelled_during_planning() {
    let mut scheduler = Scheduler::new();
    let metadata = scheduler.schedule_in_class(
        Priority::Background,
        JobClass::Maintenance,
        "rebuild metadata",
    );
    scheduler.schedule_in_class_with_dependencies(
        Priority::Visible,
        JobClass::Repair,
        "repair derived sidecar",
        [metadata.id],
    );
    for index in 0..8 {
        scheduler.schedule_in_class(
            Priority::Interactive,
            JobClass::Foreground,
            format!("foreground-{index}"),
        );
    }
    let mut checks = 0usize;
    let err = scheduler
        .drain_fair_ready_checked(JobFairnessPolicy::default(), [], || {
            checks += 1;
            if checks > 24 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        })
        .expect_err("checked fair drain should stop without committing queue changes");

    assert!(matches!(err, GfmError::Cancelled));
    let plan = scheduler.drain_fair_ready(JobFairnessPolicy::default(), []);
    assert!(plan.labels().contains(&"rebuild metadata"));
    assert_eq!(plan.blocked.len(), 1);
}

#[test]
fn checked_volume_cancellation_preserves_queue_and_tokens_when_cancelled() {
    let mut scheduler = Scheduler::new();
    let volume = VolumeId(19);
    let target = scheduler.schedule_on_volume_in_class(
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
    let mut checks = 0usize;
    let err = scheduler
        .cancel_volume_jobs_checked(volume, Some(JobClass::Background), || {
            checks += 1;
            if checks > 3 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        })
        .expect_err("checked volume cancellation should stop before committing");

    assert!(matches!(err, GfmError::Cancelled));
    assert!(!target.cancellation().is_cancelled());
    assert_eq!(scheduler.drain_ready().len(), 2);
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
fn progress_store_read_surfaces_path_probe_failures() {
    let root = temp_dir("gfm-job-progress-probe");
    let path = unprobeable_child_path(&root, "job-progress-unavailable", "gfmprogress");
    let store = JobProgressStore::new(&path);

    let err = store.read().unwrap_err();

    assert!(err
        .to_string()
        .contains("job progress store existence unavailable"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn progress_store_checked_read_honors_pre_cancelled_control_before_file_open() {
    let path = temp_path("gfm-job-progress-read-cancel", "gfmprogress");
    let store = JobProgressStore::new(&path);

    let result = store.read_checked(|| Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(!path.exists());
}

#[test]
fn progress_restore_checked_honors_pre_cancelled_control_before_file_open() {
    let path = temp_path("gfm-job-progress-restore-cancel", "gfmprogress");
    let store = JobProgressStore::new(&path);

    let result = store.restore_interrupted_checked(99, || Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(!path.exists());
}

#[test]
fn progress_command_checked_honors_pre_cancelled_control_before_file_open() {
    let path = temp_path("gfm-job-progress-command-cancel", "gfmprogress");
    let store = JobProgressStore::new(&path);

    let result =
        store.apply_command_checked(JobId::from_raw(1), JobProgressCommand::Pause, 99, || {
            Err(GfmError::Cancelled)
        });

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(!path.exists());
}

#[test]
fn progress_restore_interrupted_surfaces_path_probe_failures() {
    let root = temp_dir("gfm-job-progress-restore-probe");
    let path = unprobeable_child_path(&root, "job-progress-restore-unavailable", "gfmprogress");
    let store = JobProgressStore::new(&path);

    let err = store.restore_interrupted(99).unwrap_err();

    assert!(err
        .to_string()
        .contains("job progress store existence unavailable"));
    std::fs::remove_dir_all(root).unwrap();
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
fn cancellation_child_created_after_parent_cancel_is_immediately_cancelled() {
    let root = Cancellation::default();
    root.cancel();

    let child = root.child();
    let grandchild = child.child();

    assert!(child.is_cancelled());
    assert!(grandchild.is_cancelled());
    assert!(matches!(grandchild.check(), Err(GfmError::Cancelled)));
}

#[test]
fn inherited_cancellation_latches_into_descendants_after_observation() {
    let root = Cancellation::default();
    let child = root.child();
    let grandchild = child.child();

    root.cancel();
    assert!(grandchild.is_cancelled());

    drop(root);
    drop(child);

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
fn cancellation_prunes_dropped_child_links_without_losing_live_children() {
    let root = Cancellation::default();
    let live = root.child();
    for _ in 0..(1024 * 2) {
        drop(root.child());
    }

    assert!(
        root.child_link_count_for_tests() < 1024,
        "dropped child links should be pruned before unbounded accumulation"
    );

    root.cancel();

    assert!(live.is_cancelled());
    assert_eq!(root.child_link_count_for_tests(), 1);
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
fn worker_pool_does_not_execute_pre_cancelled_tasks() {
    let mut scheduler = Scheduler::new();
    let job = scheduler.schedule(Priority::Visible, "cancelled visible preview");
    job.cancel();
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_task = Arc::clone(&calls);

    let report = WorkerPool::new(1).run(vec![Task::new(job, move |_| {
        calls_task.fetch_add(1, AtomicOrdering::SeqCst);
        Ok(())
    })]);

    assert_eq!(report.cancelled(), 1);
    assert_eq!(report.completed(), 0);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
}

#[test]
fn isolated_worker_pool_does_not_execute_pre_cancelled_tasks() {
    let mut scheduler = Scheduler::new();
    let job =
        scheduler.schedule_on_volume(Priority::Visible, "cancelled volume thumbnail", VolumeId(9));
    job.cancel();
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_task = Arc::clone(&calls);

    let report = WorkerPool::new(1).run_isolated(
        vec![Task::new(job, move |_| {
            calls_task.fetch_add(1, AtomicOrdering::SeqCst);
            Ok(())
        })],
        VolumeConcurrencyPolicy::new(1),
    );

    assert_eq!(report.cancelled(), 1);
    assert_eq!(report.completed(), 0);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
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
fn worker_pool_enforces_late_bound_volume_concurrency_limit() {
    let mut scheduler = Scheduler::new();
    let volume = VolumeId(11);
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let tasks: Vec<_> = (0..4)
        .map(|index| {
            let job = scheduler.schedule(Priority::Background, format!("preview-{index}"));
            let job = scheduler.bind_volume(job.id, volume).unwrap();
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
fn retriable_worker_does_not_execute_or_journal_pre_cancelled_tasks() {
    let path = temp_path("gfm-job-journal-pre-cancelled", "journal");
    let journal = JobJournal::new(&path);
    let mut scheduler = Scheduler::new();
    let job = scheduler.schedule(Priority::Background, "cancelled background index");
    job.cancel();
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_task = Arc::clone(&calls);

    let report = WorkerPool::new(1).run_retriable(
        vec![RetriableTask::new(job, move |_| {
            calls_task.fetch_add(1, AtomicOrdering::SeqCst);
            Ok(())
        })],
        &journal,
        RetryPolicy { max_attempts: 2 },
    );

    assert_eq!(report.cancelled(), 1);
    assert_eq!(report.completed(), 0);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    assert!(journal.read().unwrap().is_empty());
}

#[test]
fn retriable_worker_stops_retries_when_attempt_cancels_after_failure() {
    let path = temp_path("gfm-job-journal-cancel-after-failure", "journal");
    let journal = JobJournal::new(&path);
    let mut scheduler = Scheduler::new();
    let job = scheduler.schedule(Priority::Background, "cancelled failed extraction");
    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_task = Arc::clone(&attempts);

    let report = WorkerPool::new(1).run_retriable(
        vec![RetriableTask::new(job, move |cancellation| {
            attempts_task.fetch_add(1, AtomicOrdering::SeqCst);
            cancellation.cancel();
            Err(GfmError::Format("temporary extraction failure".to_string()))
        })],
        &journal,
        RetryPolicy { max_attempts: 3 },
    );
    let entries = journal.read().unwrap();

    assert_eq!(report.cancelled(), 1);
    assert_eq!(report.completed(), 0);
    assert_eq!(attempts.load(AtomicOrdering::SeqCst), 1);
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].status, TaskStatus::Started);
    assert!(matches!(entries[1].status, TaskStatus::Failed(_)));
    assert_eq!(entries[2].status, TaskStatus::Cancelled);

    std::fs::remove_file(path).unwrap();
}

#[test]
fn retriable_worker_stops_retry_backoff_when_cancelled() {
    let path = temp_path("gfm-job-journal-cancel-backoff", "journal");
    let journal = JobJournal::new(&path);
    let mut scheduler = Scheduler::new();
    let job = scheduler.schedule(Priority::Background, "cancelled offline index");
    let cancellation = job.cancellation();
    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_task = Arc::clone(&attempts);
    let started = Instant::now();

    std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(20));
            cancellation.cancel();
        });

        let report = WorkerPool::new(1).run_retriable(
            vec![RetriableTask::new(job, move |_| {
                attempts_task.fetch_add(1, AtomicOrdering::SeqCst);
                Err(GfmError::Format(
                    "volume is offline and not mounted".to_string(),
                ))
            })],
            &journal,
            RetryPolicy { max_attempts: 3 },
        );

        assert_eq!(report.cancelled(), 1);
    });
    let entries = journal.read().unwrap();

    assert!(
        started.elapsed() < Duration::from_millis(180),
        "cancelled retry backoff waited too long: {:?}",
        started.elapsed()
    );
    assert_eq!(attempts.load(AtomicOrdering::SeqCst), 1);
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].status, TaskStatus::Started);
    assert!(matches!(entries[1].status, TaskStatus::Failed(_)));
    assert_eq!(entries[2].status, TaskStatus::Cancelled);

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
fn job_journal_append_accepts_relative_leaf_path() {
    let _cwd = CWD_LOCK.lock().unwrap();
    let root = temp_path("gfm-job-journal-relative-root", "root");
    let path = PathBuf::from("jobs.journal");
    let previous = std::env::current_dir().unwrap();
    std::fs::create_dir_all(&root).unwrap();
    std::env::set_current_dir(&root).unwrap();
    let journal = JobJournal::new(&path);
    let entry = JournalEntry {
        id: JobId::from_raw(42),
        label: "relative journal".to_string(),
        attempt: 1,
        status: TaskStatus::Started,
    };

    journal.append(&entry).unwrap();

    assert_eq!(journal.read().unwrap(), vec![entry]);
    std::env::set_current_dir(previous).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn job_journal_append_checked_honors_pre_cancelled_control_before_file_create() {
    let path = temp_path("gfm-job-journal-append-pre-cancel", "journal");
    let journal = JobJournal::new(&path);
    let entry = sample_journal_entry(41, TaskStatus::Started);

    let result = journal.append_checked(&entry, || Err(GfmError::Cancelled));

    assert_eq!(result, Err(GfmError::Cancelled));
    assert!(!path.exists());
}

#[test]
fn job_journal_append_checked_preserves_existing_journal_when_cancelled_before_write() {
    let path = temp_path("gfm-job-journal-append-preserve", "journal");
    let journal = JobJournal::new(&path);
    let existing = sample_journal_entry(41, TaskStatus::Started);
    let next = sample_journal_entry(41, TaskStatus::Completed);
    journal.append(&existing).unwrap();
    let before = std::fs::read_to_string(&path).unwrap();
    let mut checks = 0usize;

    let result = journal.append_checked(&next, || {
        checks += 1;
        if checks >= 4 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert_eq!(result, Err(GfmError::Cancelled));
    assert!(checks >= 4);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    assert_eq!(journal.read().unwrap(), vec![existing]);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn job_journal_append_checked_does_not_report_cancel_after_line_commit() {
    let path = temp_path("gfm-job-journal-append-after-commit", "journal");
    let journal = JobJournal::new(&path);
    let entry = sample_journal_entry(41, TaskStatus::Completed);
    let mut checks = 0usize;

    let result = journal.append_checked(&entry, || {
        checks += 1;
        if checks >= 5 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert_eq!(result, Ok(()));
    assert_eq!(checks, 4);
    assert_eq!(journal.read().unwrap(), vec![entry]);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn job_journal_checked_read_honors_pre_cancelled_control_before_file_open() {
    let path = temp_path("gfm-job-journal-read-pre-cancel", "journal");
    let journal = JobJournal::new(&path);

    let result = journal.read_checked(|| Err(GfmError::Cancelled));

    assert_eq!(result, Err(GfmError::Cancelled));
    assert!(!path.exists());
}

#[test]
fn job_journal_recoverable_checked_honors_cancellation_during_scan() {
    let path = temp_path("gfm-job-journal-recovery-cancel", "journal");
    let journal = JobJournal::new(&path);
    journal
        .append(&sample_journal_entry(41, TaskStatus::Started))
        .unwrap();
    journal
        .append(&sample_journal_entry(
            42,
            TaskStatus::Failed("temporary retry".to_string()),
        ))
        .unwrap();
    let mut checks = 0usize;

    let result = journal.recoverable_checked(RetryPolicy { max_attempts: 2 }, || {
        checks += 1;
        if checks >= 5 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert_eq!(result, Err(GfmError::Cancelled));
    assert!(checks >= 5);
    assert_eq!(journal.read().unwrap().len(), 2);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn job_journal_read_surfaces_path_probe_failures() {
    let root = temp_dir("gfm-job-journal-probe");
    let path = unprobeable_child_path(&root, "job-journal-unavailable", "journal");
    let journal = JobJournal::new(&path);

    let err = journal.read().unwrap_err();

    assert!(err
        .to_string()
        .contains("job journal existence unavailable"));
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
                failure_class: None,
                next_delay_ms: 0,
            },
            RecoveryJob {
                id: failed,
                label: "failed".to_string(),
                attempts: 1,
                reason: RecoveryReason::RetryableFailure,
                failure_class: Some(FailureClass::Transient),
                next_delay_ms: 25,
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

    for message in [
        "network is unreachable",
        "device not configured",
        "stale file handle",
    ] {
        let decision = policy.retry_decision(1, message);
        assert_eq!(decision.class, FailureClass::OfflineVolume);
        assert!(decision.retryable);
        assert_eq!(decision.next_delay_ms, 250);
    }

    for message in [
        "resource temporarily unavailable",
        "interrupted system call",
        "source does not exist",
    ] {
        let decision = policy.retry_decision(1, message);
        assert_eq!(decision.class, FailureClass::Transient);
        assert!(decision.retryable);
        assert_eq!(decision.next_delay_ms, 25);
    }

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
    assert_eq!(recoverable[0].failure_class, Some(FailureClass::Transient));
    assert_eq!(recoverable[0].next_delay_ms, 25);

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
fn payload_catalog_write_all_accepts_relative_leaf_path() {
    let _cwd = CWD_LOCK.lock().unwrap();
    let root = temp_path("gfm-job-payload-catalog-relative-root", "root");
    let path = PathBuf::from("payloads.gfmjobs");
    let previous = std::env::current_dir().unwrap();
    std::fs::create_dir_all(&root).unwrap();
    std::env::set_current_dir(&root).unwrap();
    let catalog = JobPayloadCatalog::new(&path);
    let record = JobPayloadRecord::new(
        JobId::from_raw(9),
        JobPayloadKind::Indexing,
        "relative payload",
        "payloads/index.gfmjob",
        Some(VolumeId(3)),
        "index relative payload",
    );

    catalog.write_all(std::slice::from_ref(&record)).unwrap();

    assert_eq!(catalog.read().unwrap(), vec![record]);
    std::env::set_current_dir(previous).unwrap();
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
fn payload_catalog_write_all_checked_honors_pre_cancelled_control_before_file_create() {
    let path = temp_path("gfm-job-payload-catalog-write-pre-cancel", "gfmjobs");
    let catalog = JobPayloadCatalog::new(&path);
    let record = sample_payload_record(1);

    let result = catalog.write_all_checked(&[record], || Err(GfmError::Cancelled));

    assert_eq!(result, Err(GfmError::Cancelled));
    assert!(!path.exists());
    assert!(!has_payload_catalog_temp_file(&path));
}

#[test]
fn payload_catalog_write_all_checked_removes_temp_file_after_cancelled_record_write() {
    let path = temp_path("gfm-job-payload-catalog-write-temp-cancel", "gfmjobs");
    let catalog = JobPayloadCatalog::new(&path);
    let records = vec![sample_payload_record(1), sample_payload_record(2)];
    let mut checks = 0usize;

    let result = catalog.write_all_checked(&records, || {
        checks += 1;
        if checks >= 5 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert_eq!(result, Err(GfmError::Cancelled));
    assert!(checks >= 5);
    assert!(!path.exists());
    assert!(!has_payload_catalog_temp_file(&path));
}

#[test]
fn payload_catalog_write_all_checked_preserves_existing_catalog_when_cancelled_before_publish() {
    let path = temp_path("gfm-job-payload-catalog-write-preserve", "gfmjobs");
    let catalog = JobPayloadCatalog::new(&path);
    let existing = sample_payload_record(1);
    let replacement = sample_payload_record(2);
    catalog.write_all(std::slice::from_ref(&existing)).unwrap();
    let before = std::fs::read_to_string(&path).unwrap();
    let mut checks = 0usize;

    let result = catalog.write_all_checked(std::slice::from_ref(&replacement), || {
        checks += 1;
        if checks >= 4 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert_eq!(result, Err(GfmError::Cancelled));
    assert!(checks >= 4);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    assert_eq!(catalog.read().unwrap(), vec![existing]);
    assert!(!has_payload_catalog_temp_file(&path));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn payload_catalog_append_checked_honors_pre_cancelled_control_before_file_create() {
    let path = temp_path("gfm-job-payload-catalog-append-pre-cancel", "gfmjobs");
    let catalog = JobPayloadCatalog::new(&path);
    let record = sample_payload_record(1);

    let result = catalog.append_checked(&record, || Err(GfmError::Cancelled));

    assert_eq!(result, Err(GfmError::Cancelled));
    assert!(!path.exists());
    assert!(!has_payload_catalog_temp_file(&path));
}

#[test]
fn payload_catalog_append_checked_preserves_existing_catalog_when_cancelled_before_publish() {
    let path = temp_path("gfm-job-payload-catalog-append-preserve", "gfmjobs");
    let catalog = JobPayloadCatalog::new(&path);
    let existing = sample_payload_record(1);
    let appended = sample_payload_record(2);
    catalog.write_all(std::slice::from_ref(&existing)).unwrap();
    let before = std::fs::read_to_string(&path).unwrap();
    let mut checks = 0usize;

    let result = catalog.append_checked(&appended, || {
        checks += 1;
        if checks >= 11 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert_eq!(result, Err(GfmError::Cancelled));
    assert!(checks >= 11);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    assert_eq!(catalog.read().unwrap(), vec![existing]);
    assert!(!has_payload_catalog_temp_file(&path));
    std::fs::remove_file(path).unwrap();
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

#[test]
fn payload_catalog_filtered_read_uses_latest_legacy_duplicate_record() {
    let path = temp_path("gfm-job-payload-catalog-filtered-duplicate", "gfmjobs");
    let stale = JobPayloadRecord::new(
        JobId::from_raw(3),
        JobPayloadKind::Repair,
        "repair",
        "repair/stale.gfmjob",
        None,
        "repair stale",
    );
    let current = JobPayloadRecord::new(
        JobId::from_raw(3),
        JobPayloadKind::Repair,
        "repair",
        "repair/current.gfmjob",
        Some(VolumeId(11)),
        "repair current",
    );
    std::fs::write(
        &path,
        format!(
            "gfm-job-payload-catalog-v1\n{}\n{}\n",
            stale.as_tsv(),
            current.as_tsv()
        ),
    )
    .unwrap();
    let catalog = JobPayloadCatalog::new(&path);

    assert_eq!(
        catalog.read_for_ids([JobId::from_raw(3)]).unwrap(),
        vec![current]
    );

    std::fs::remove_file(path).unwrap();
}

#[test]
fn payload_catalog_read_surfaces_path_probe_failures() {
    let root = temp_dir("gfm-job-payload-catalog-probe");
    let path = unprobeable_child_path(&root, "job-payload-catalog-unavailable", "gfmjobs");
    let catalog = JobPayloadCatalog::new(&path);

    let err = catalog.read().unwrap_err();

    assert!(err
        .to_string()
        .contains("job payload catalog existence unavailable"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn payload_catalog_checked_read_honors_pre_cancelled_control_before_file_open() {
    let path = temp_path("gfm-job-payload-catalog-read-cancel", "gfmjobs");
    let catalog = JobPayloadCatalog::new(&path);

    let result = catalog.read_checked(|| Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(!path.exists());
}

#[test]
fn payload_catalog_checked_filtered_read_honors_pre_cancelled_control_before_file_open() {
    let path = temp_path("gfm-job-payload-catalog-filter-cancel", "gfmjobs");
    let catalog = JobPayloadCatalog::new(&path);

    let result = catalog.read_for_ids_checked([JobId::from_raw(1)], || Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(!path.exists());
}

#[test]
fn payload_catalog_filtered_read_surfaces_path_probe_failures() {
    let root = temp_dir("gfm-job-payload-catalog-filter-probe");
    let path = unprobeable_child_path(&root, "job-payload-catalog-filter-unavailable", "gfmjobs");
    let catalog = JobPayloadCatalog::new(&path);

    let err = catalog.read_for_ids([JobId::from_raw(1)]).unwrap_err();

    assert!(err
        .to_string()
        .contains("job payload catalog existence unavailable"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn payload_catalog_append_surfaces_path_probe_failures() {
    let root = temp_dir("gfm-job-payload-catalog-append-probe");
    let path = unprobeable_child_path(&root, "job-payload-catalog-append-unavailable", "gfmjobs");
    let catalog = JobPayloadCatalog::new(&path);
    let record = JobPayloadRecord::new(
        JobId::from_raw(1),
        JobPayloadKind::Repair,
        "repair",
        "repair/sidecar.gfmjob",
        None,
        "repair sidecar",
    );

    let err = catalog.append(&record).unwrap_err();

    assert!(err
        .to_string()
        .contains("job payload catalog existence unavailable"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn runtime_stores_write_relative_leaf_paths_in_current_directory() {
    let _cwd = CWD_LOCK.lock().unwrap();
    let root = temp_dir("gfm-job-relative-stores");
    let previous = std::env::current_dir().unwrap();
    std::env::set_current_dir(&root).unwrap();

    let catalog = JobPayloadCatalog::new("payloads.gfmjobs");
    let payload = JobPayloadRecord::new(
        JobId::from_raw(7),
        JobPayloadKind::Preview,
        "preview",
        "preview/job.gfmjob",
        Some(VolumeId(9)),
        "preview selected file",
    );
    catalog.write_all(std::slice::from_ref(&payload)).unwrap();
    assert_eq!(catalog.read().unwrap(), vec![payload]);

    let journal = JobJournal::new("jobs.gfmjournal");
    let entry = JournalEntry {
        id: JobId::from_raw(8),
        label: "copy selected files".to_string(),
        attempt: 1,
        status: TaskStatus::Completed,
    };
    journal.append(&entry).unwrap();
    assert_eq!(journal.read().unwrap(), vec![entry]);

    let progress = JobProgressStore::new("progress.gfmprogress");
    let snapshot = JobProgressSnapshot::new(
        JobId::from_raw(9),
        JobClass::Foreground,
        Priority::Visible,
        "thumbnail generation",
        Some(VolumeId(10)),
        4,
    )
    .with_progress(JobProgressState::Running, 2, "rendering", 3);
    progress.write_all(std::slice::from_ref(&snapshot)).unwrap();
    assert_eq!(progress.read().unwrap(), vec![snapshot]);

    std::env::set_current_dir(previous).unwrap();
    std::fs::remove_dir_all(root).unwrap();
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

fn temp_dir(prefix: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "{}-{}",
        prefix,
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn sample_payload_record(id: u64) -> JobPayloadRecord {
    JobPayloadRecord::new(
        JobId::from_raw(id),
        JobPayloadKind::Preview,
        "quicklook preview",
        format!("preview/{id}.gfmjob"),
        Some(VolumeId(7)),
        format!("preview item {id}"),
    )
}

fn sample_journal_entry(id: u64, status: TaskStatus) -> JournalEntry {
    JournalEntry {
        id: JobId::from_raw(id),
        label: "journalled job".to_string(),
        attempt: 1,
        status,
    }
}

fn has_payload_catalog_temp_file(path: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let prefix = format!("{file_name}.{}.", std::process::id());
    std::fs::read_dir(parent)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .any(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".tmp"))
        })
}

fn unprobeable_child_path(root: &std::path::Path, prefix: &str, extension: &str) -> PathBuf {
    root.join(format!("{}.{}", prefix.repeat(64), extension))
}
