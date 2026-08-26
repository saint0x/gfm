use crate::detect_volume_id;
use crate::extract::{extraction_budget_profile, read_extraction_quarantine};
use crate::runtime::{default_extraction_quarantine_path, run_volume_task, RuntimeJobHandle};
use gfm_content::Extractor;
use gfm_index::{
    BackgroundContentIndexer, ContentIndexJobSpec, ContentIndexReport, Indexer,
    QuarantineContentIndexRequest,
};
use gfm_jobs::{
    JobJournal, JobPayloadKind, Priority, RetriableTask, RetryPolicy, Scheduler, SchedulingAction,
    SchedulingPressure, TaskStatus, WorkerPool,
};
use gfm_store::read_records;
use gfm_types::{GfmError, Result, SearchHit};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub(crate) fn run_content_search(
    root: PathBuf,
    query: String,
    extractor: Extractor,
) -> Result<(usize, Vec<SearchHit>)> {
    let volume = detect_volume_id(&root).ok();
    run_volume_task(
        volume,
        Priority::Visible,
        "content extraction search",
        move || {
            let snapshot = Indexer::default().build(root)?;
            let mut live = snapshot.into_live();
            let indexed = live.index_content(&extractor)?;
            let hits = live.search_with_snippets(&query, 50, &extractor, 96)?;
            Ok((indexed, hits))
        },
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContentJobOutcome {
    pub(crate) report: Option<ContentIndexReport>,
    pub(crate) inaccessible: usize,
    pub(crate) scheduling_action: SchedulingAction,
    pub(crate) deferred: bool,
}

pub(crate) fn run_content_job(
    spec: &ContentIndexJobSpec,
    journal: &JobJournal,
    pressure: SchedulingPressure,
) -> Result<ContentJobOutcome> {
    let snapshot = Indexer::default().build(&spec.root)?;
    let inaccessible = snapshot.inaccessible.len();
    let previous_records = if spec.records_path.is_file() && spec.content_path.is_file() {
        read_records(&spec.records_path)?
    } else {
        Vec::new()
    };
    let volume = spec
        .volume
        .or_else(|| snapshot.records.first().map(|record| record.id.volume))
        .or_else(|| detect_volume_id(&spec.root).ok())
        .ok_or_else(|| {
            GfmError::Format(format!(
                "could not determine content index volume for {}",
                spec.root.display()
            ))
        })?;
    snapshot.save(&spec.records_path)?;
    let scheduling = pressure.decide(Priority::Background, 1, 1);
    if scheduling.action == SchedulingAction::Defer {
        return Ok(ContentJobOutcome {
            report: None,
            inaccessible,
            scheduling_action: scheduling.action,
            deferred: true,
        });
    }
    let extractor = Extractor::with_budget_profile(extraction_budget_profile(&spec.root, pressure));
    let worker = BackgroundContentIndexer::new(extractor, spec.options());
    let quarantine_store = default_extraction_quarantine_path();
    let extraction_quarantine = read_extraction_quarantine(&quarantine_store, 2)?;
    let content_report = Arc::new(Mutex::new(None));
    let content_report_task = Arc::clone(&content_report);
    let mut scheduler = Scheduler::new();
    let label = "background content index";
    let job = scheduler.schedule_on_volume(Priority::Background, label, volume);
    let runtime = RuntimeJobHandle::begin(
        &job,
        JobPayloadKind::Indexing,
        label,
        snapshot.records.len().max(1) as u64,
        format!("index:{}", spec.root.display()),
    )?;
    let tasks: Vec<_> = scheduler
        .drain_ready()
        .into_iter()
        .map(|scheduled| {
            let snapshot = snapshot.clone();
            let previous_records = previous_records.clone();
            let segment_dir = spec.segment_dir.clone();
            let content = spec.content_path.clone();
            let quarantine_store = quarantine_store.clone();
            let extraction_quarantine = extraction_quarantine.clone();
            let worker = worker.clone();
            let content_report_task = Arc::clone(&content_report_task);
            let runtime = runtime.clone();
            RetriableTask::new(scheduled, move |cancellation| {
                runtime.running()?;
                let mut extraction_quarantine = extraction_quarantine.clone();
                let request = QuarantineContentIndexRequest {
                    snapshot: &snapshot,
                    previous_records: &previous_records,
                    previous_content_path: Some(&content),
                    segment_dir: &segment_dir,
                    content_path: &content,
                    cancellation: &cancellation,
                };
                let report = worker.run_incremental_and_compact_with_quarantine(
                    request,
                    &mut extraction_quarantine,
                )?;
                extraction_quarantine.write(&quarantine_store)?;
                *content_report_task
                    .lock()
                    .expect("content index report lock poisoned") = Some(report);
                Ok(())
            })
        })
        .collect();
    let worker_report = WorkerPool::new(scheduling.worker_threads).run_retriable_isolated(
        tasks,
        journal,
        RetryPolicy { max_attempts: 2 },
        scheduling.volume_policy,
    );
    let outcome = worker_report
        .outcomes
        .iter()
        .find(|outcome| outcome.id == job.id)
        .ok_or_else(|| GfmError::Format("background content index job did not run".to_string()))?;
    runtime.finish(&outcome.status)?;
    match &outcome.status {
        TaskStatus::Completed => {}
        TaskStatus::Started => {
            return Err(GfmError::Format(
                "background content index is still running".to_string(),
            ))
        }
        TaskStatus::Cancelled => return Err(GfmError::Cancelled),
        TaskStatus::Failed(message) => {
            return Err(GfmError::Format(format!(
                "background content index failed: {message}"
            )))
        }
    }
    let report = content_report
        .lock()
        .expect("content index report lock poisoned")
        .clone()
        .ok_or_else(|| {
            GfmError::Format("background content index completed without a report".to_string())
        })?;
    Ok(ContentJobOutcome {
        report: Some(report),
        inaccessible,
        scheduling_action: scheduling.action,
        deferred: false,
    })
}
