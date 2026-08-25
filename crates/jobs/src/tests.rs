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
