#[cfg(test)]
use crate::access::preflight_access_scope_checked;
use crate::access::{
    preflight_access_scope_checked_with_volume_report, preflight_volume_access_scope_with_report,
    ScopedAccessGuard,
};
use crate::extract::{
    extraction_budget_profile, extraction_budget_profile_checked,
    preflight_adaptive_extraction_worker_scratch, read_extraction_quarantine_cancellable,
    run_adaptive_extraction_worker_cancellable,
    run_quarantined_adaptive_extraction_worker_cancellable, ADAPTIVE_WORKER_TIMEOUT,
};
use crate::runtime::{
    default_content_job_path, default_extraction_quarantine_path, default_job_journal_path,
    run_retriable_volume_task_cancellable_with_payload_path, run_scheduled_volume_task_cancellable,
    run_scheduled_volume_task_cancellable_with_volume_and_payload_path,
    run_volume_task_cancellable, run_volume_task_cancellable_without_progress,
    runtime_progress_store, RuntimeJobHandle,
};
use crate::{
    optional_path_arg, parse_battery_state, parse_io_pressure, parse_optional_scheduling_pressure,
    parse_quarantine_failure_kind, parse_required_scheduling_pressure, parse_thermal_state,
    parse_u32, parse_u64, parse_user_activity, required_path, required_string,
};
use gfm_content::{CachedExtractor, ExtractionFingerprint, ExtractionQuarantine, Extractor};
use gfm_fs::record_for_path_checked;
use gfm_index::{
    BackgroundContentIndexer, CompactionPressure, ContentIndexJobSpec, ContentIndexReport,
    ContentMaintenanceOptions, ContentMaintenanceReport, ContentMergePolicy, IndexFootprintSpec,
    Indexer, QuarantineContentIndexRequest,
};
use gfm_jobs::{
    Cancellation, FailureClass, JobFairnessPolicy, JobJournal, JobPayloadKind, Priority,
    RecoveryReason, RetriableTask, RetryPolicy, Scheduler, SchedulingAction, SchedulingPressure,
    TaskStatus, WorkerPool,
};
use gfm_mac::{AccessIntent, VolumeDiscoveryReport};
use gfm_store::{atomic_write_checked, read_records_checked, ContentArchiveManifest};
use gfm_types::{GfmError, Result, SearchHit, VolumeId};
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io::{self, Read};
use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

pub(crate) fn run(command: &str, args: &mut impl Iterator<Item = String>) -> Result<bool> {
    match command {
        "index-content" | "index-content-retry-probe" => {
            let root = required_path(args.next(), "index-content requires a root path")?;
            let records = required_path(args.next(), "index-content requires a records path")?;
            let content = required_path(args.next(), "index-content requires a content path")?;
            let retry_probe = if command == "index-content-retry-probe" {
                Some(required_path(
                    args.next(),
                    "index-content-retry-probe requires an attempt state path",
                )?)
            } else {
                None
            };
            let access_reports = ForegroundContentIndexAccessReports::for_paths(
                root.clone(),
                records.clone(),
                content.clone(),
            )?;
            let retry_probe_access_report = retry_probe_access_report(retry_probe.as_deref())?;
            access_reports.preflight_volumes("content index")?;
            if let Some(report) = retry_probe_access_report.as_ref() {
                report.preflight_volume("content index")?;
            }
            let volume = access_reports.first_volume().or_else(|| {
                retry_probe_access_report
                    .as_ref()
                    .and_then(ForegroundContentIndexAccessReport::volume)
            });
            let (records_len, inaccessible_len, indexed) =
                run_retriable_volume_task_cancellable_with_payload_path(
                    volume,
                    Priority::Visible,
                    "content index",
                    content.clone(),
                    move |cancellation| {
                        let root = root.clone();
                        let records = records.clone();
                        let content = content.clone();
                        let retry_probe = retry_probe.clone();
                        let retry_probe_access_report = retry_probe_access_report.clone();
                        cancellation.check()?;
                        if let (Some(retry_probe), Some(retry_probe_access_report)) =
                            (retry_probe.as_ref(), retry_probe_access_report.as_ref())
                        {
                            fail_first_content_retry_probe_attempt(
                                retry_probe_access_report,
                                retry_probe,
                                "content index",
                                &cancellation,
                            )?;
                        }
                        let _access = retain_foreground_content_index_access_checked(
                            &access_reports,
                            "content index",
                            || cancellation.check(),
                        )?;
                        cancellation.check()?;
                        let snapshot = Indexer::default().build_cancellable(root, &cancellation)?;
                        let records_len = snapshot.records.len();
                        let inaccessible_len = snapshot.inaccessible.len();
                        cancellation.check()?;
                        let indexed = snapshot.save_with_content_cancellable(
                            records,
                            content,
                            &Extractor::default(),
                            &cancellation,
                        )?;
                        Ok((records_len, inaccessible_len, indexed))
                    },
                )?;
            eprintln!(
                "indexed {} records; content-indexed {} files; {} inaccessible",
                records_len, indexed, inaccessible_len
            );
        }
        "extract-report" => {
            let path = required_path(args.next(), "extract-report requires a path")?;
            print!(
                "{}",
                run_extraction_report(path, "content extraction", Extractor::default(), None,)?
            );
        }
        "extract-report-retry-probe" => {
            let path = required_path(args.next(), "extract-report-retry-probe requires a path")?;
            let retry_probe = required_path(
                args.next(),
                "extract-report-retry-probe requires an attempt state path",
            )?;
            print!(
                "{}",
                run_extraction_report(
                    path,
                    "content extraction",
                    Extractor::default(),
                    Some(retry_probe),
                )?
            );
        }
        "extract-report-adaptive" => {
            let path = required_path(args.next(), "extract-report-adaptive requires a path")?;
            let pressure = parse_required_scheduling_pressure(args, "extract report")?;
            let root = path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            let extractor =
                Extractor::with_budget_profile(extraction_budget_profile(&root, pressure));
            print!(
                "{}",
                run_extraction_report(path, "adaptive content extraction", extractor, None,)?
            );
        }
        "extract-worker-adaptive" => {
            let path = required_path(args.next(), "extract-worker-adaptive requires a path")?;
            let pressure = parse_required_scheduling_pressure(args, "extract worker")?;
            let _scratch_access = preflight_adaptive_extraction_worker_scratch()?;
            let access_report = ForegroundContentIndexAccessReports::entry_checked(
                path.clone(),
                AccessIntent::Read,
                || Ok(()),
            )?;
            let volume_report = access_report.clone();
            let outcome = run_scheduled_volume_task_cancellable_with_volume_and_payload_path(
                Priority::Background,
                "adaptive extraction",
                pressure,
                move || {
                    volume_report.preflight_volume("adaptive extraction")?;
                    Ok(volume_report.volume())
                },
                path.clone(),
                move |cancellation| {
                    let _access = access_report
                        .access_checked("adaptive extraction worker", || cancellation.check())?;
                    run_adaptive_extraction_worker_cancellable(
                        &path,
                        pressure,
                        ADAPTIVE_WORKER_TIMEOUT,
                        &cancellation,
                    )
                },
            )?;
            if outcome.deferred {
                eprintln!(
                    "adaptive-extraction-deferred\taction={:?}",
                    outcome.scheduling_action
                );
            } else {
                let report = outcome.result.ok_or_else(|| {
                    GfmError::Format("adaptive extraction ran without a report".to_string())
                })?;
                eprintln!(
                    "adaptive-extraction-action\t{:?}",
                    outcome.scheduling_action
                );
                print!("{}", report);
            }
        }
        "extract-worker-cancel-adaptive" => {
            let path = required_path(
                args.next(),
                "extract-worker-cancel-adaptive requires a path",
            )?;
            let pressure = parse_required_scheduling_pressure(args, "extract worker")?;
            let cancellation = Cancellation::default();
            cancellation.cancel();
            match run_adaptive_extraction_worker_cancellable(
                &path,
                pressure,
                ADAPTIVE_WORKER_TIMEOUT,
                &cancellation,
            ) {
                Err(GfmError::Cancelled) => {
                    println!("extract-worker\tstatus=cancelled\treason=cancelled-before-launch")
                }
                Ok(report) => print!("{report}"),
                Err(err) => return Err(err),
            }
        }
        "extract-worker-quarantine-adaptive" => {
            let path = required_path(
                args.next(),
                "extract-worker-quarantine-adaptive requires a path",
            )?;
            let store = required_path(
                args.next(),
                "extract-worker-quarantine-adaptive requires a quarantine store path",
            )?;
            let pressure = parse_required_scheduling_pressure(args, "extract worker")?;
            let timeout = args
                .next()
                .map(|value| parse_u64(&value, "timeout ms"))
                .transpose()?
                .map(Duration::from_millis)
                .unwrap_or(ADAPTIVE_WORKER_TIMEOUT);
            let threshold = args
                .next()
                .map(|value| parse_u32(&value, "failure threshold"))
                .transpose()?
                .unwrap_or(2);
            let _scratch_access = preflight_adaptive_extraction_worker_scratch()?;
            let volume_path = path.clone();
            let volume_store = store.clone();
            let outcome = run_scheduled_volume_task_cancellable_with_volume_and_payload_path(
                Priority::Background,
                "quarantined adaptive extraction",
                pressure,
                move || {
                    let access_reports =
                        ExtractionQuarantineAccessReports::for_paths(&volume_path, &volume_store)?;
                    access_reports.preflight_volumes("quarantined adaptive extraction")?;
                    Ok(access_reports.first_volume())
                },
                path.clone(),
                move |cancellation| {
                    let access_reports =
                        ExtractionQuarantineAccessReports::for_paths(&path, &store)?;
                    let _access = access_reports
                        .access_checked("quarantined adaptive extraction", || {
                            cancellation.check()
                        })?;
                    run_quarantined_adaptive_extraction_worker_cancellable(
                        &path,
                        &store,
                        pressure,
                        timeout,
                        threshold,
                        &cancellation,
                    )
                },
            )?;
            if outcome.deferred {
                eprintln!(
                    "quarantined-adaptive-extraction-deferred\taction={:?}",
                    outcome.scheduling_action
                );
            } else {
                let output = outcome.result.ok_or_else(|| {
                    GfmError::Format(
                        "quarantined adaptive extraction ran without output".to_string(),
                    )
                })?;
                eprintln!(
                    "quarantined-adaptive-extraction-action\t{:?}",
                    outcome.scheduling_action
                );
                print!("{output}");
            }
        }
        "extract-cache" => {
            let path = required_path(args.next(), "extract-cache requires a path")?;
            print!("{}", run_extraction_cache(path)?);
        }
        "extract-quarantine" => {
            let path = required_path(args.next(), "extract-quarantine requires a path")?;
            let store = required_path(
                args.next(),
                "extract-quarantine requires a quarantine store path",
            )?;
            let kind = parse_quarantine_failure_kind(
                args.next().as_deref().unwrap_or("timeout"),
                "failure kind",
            )?;
            let attempts = args
                .next()
                .map(|value| parse_u32(&value, "attempts"))
                .transpose()?
                .unwrap_or(2);
            for line in run_extraction_quarantine(path, store, kind, attempts)? {
                println!("{line}");
            }
        }
        "index-content-segment" | "index-content-segment-retry-probe" => {
            let root = required_path(args.next(), "index-content-segment requires a root path")?;
            let output = required_path(
                args.next(),
                "index-content-segment requires an output segment path",
            )?;
            let retry_probe = if command == "index-content-segment-retry-probe" {
                Some(required_path(
                    args.next(),
                    "index-content-segment-retry-probe requires an attempt state path",
                )?)
            } else {
                None
            };
            let access_reports = ContentSegmentIndexAccessReports::for_paths(&root, &output)?;
            let retry_probe_access_report = retry_probe_access_report(retry_probe.as_deref())?;
            access_reports.preflight_volumes("content segment index")?;
            if let Some(report) = retry_probe_access_report.as_ref() {
                report.preflight_volume("content segment index")?;
            }
            let volume = access_reports.first_volume().or_else(|| {
                retry_probe_access_report
                    .as_ref()
                    .and_then(ForegroundContentIndexAccessReport::volume)
            });
            let (inaccessible_len, indexed) =
                run_retriable_volume_task_cancellable_with_payload_path(
                    volume,
                    Priority::Visible,
                    "content segment index",
                    output.clone(),
                    move |cancellation| {
                        let root = root.clone();
                        let output = output.clone();
                        let retry_probe = retry_probe.clone();
                        let retry_probe_access_report = retry_probe_access_report.clone();
                        cancellation.check()?;
                        if let (Some(retry_probe), Some(retry_probe_access_report)) =
                            (retry_probe.as_ref(), retry_probe_access_report.as_ref())
                        {
                            fail_first_content_retry_probe_attempt(
                                retry_probe_access_report,
                                retry_probe,
                                "content segment index",
                                &cancellation,
                            )?;
                        }
                        let _access = access_reports
                            .access_checked("content segment index", || cancellation.check())?;
                        cancellation.check()?;
                        let snapshot = Indexer::default().build_cancellable(root, &cancellation)?;
                        let inaccessible_len = snapshot.inaccessible.len();
                        cancellation.check()?;
                        let indexed = snapshot.save_content_segment_cancellable(
                            output,
                            &Extractor::default(),
                            Vec::new(),
                            &cancellation,
                        )?;
                        Ok((inaccessible_len, indexed))
                    },
                )?;
            eprintln!(
                "content-segmented {} files; {} inaccessible",
                indexed, inaccessible_len
            );
        }
        "compact-content" => {
            let output = required_path(args.next(), "compact-content requires an output path")?;
            let segments: Vec<PathBuf> = args.map(PathBuf::from).collect();
            if segments.is_empty() {
                return Err(gfm_types::GfmError::Format(
                    "compact-content requires at least one segment path".to_string(),
                ));
            }
            let terms = run_content_compaction(output, segments)?;
            eprintln!("compacted {terms} content terms");
        }
        "compact-content-tiered" => {
            let output = required_path(
                args.next(),
                "compact-content-tiered requires an output path",
            )?;
            let segments: Vec<PathBuf> = args.map(PathBuf::from).collect();
            if segments.is_empty() {
                return Err(gfm_types::GfmError::Format(
                    "compact-content-tiered requires at least one segment path".to_string(),
                ));
            }
            let outcome = run_tiered_content_compaction(output, segments)?;
            eprintln!(
                "tiered-compacted {} content terms; merged {}; retained {}; bytes {}; tombstone-segments {}; tier {:?}",
                outcome.postings.len(),
                outcome.merged_segments.len(),
                outcome.retained_segments.len(),
                outcome.merge_bytes,
                outcome.tombstone_segments,
                outcome.tier
            );
            for segment in outcome.retained_segments {
                println!("retain\t{}", segment.display());
            }
        }
        "content-maintain-segments" => {
            let manifest_path = required_path(
                args.next(),
                "content-maintain-segments requires a manifest path",
            )?;
            let output_archive = required_path(
                args.next(),
                "content-maintain-segments requires an output archive path",
            )?;
            let segments = args.map(PathBuf::from).collect::<Vec<_>>();
            if segments.is_empty() {
                return Err(GfmError::Format(
                    "content-maintain-segments requires at least one segment".to_string(),
                ));
            }
            let report = run_content_segment_maintenance(manifest_path, output_archive, segments)?;
            print_content_maintenance_report(report);
        }
        "content-maintain-segments-adaptive" => {
            let manifest_path = required_path(
                args.next(),
                "content-maintain-segments-adaptive requires a manifest path",
            )?;
            let output_archive = required_path(
                args.next(),
                "content-maintain-segments-adaptive requires an output archive path",
            )?;
            let pressure = parse_required_scheduling_pressure(args, "content maintenance")?;
            let segments = args.map(PathBuf::from).collect::<Vec<_>>();
            if segments.is_empty() {
                return Err(GfmError::Format(
                    "content-maintain-segments-adaptive requires at least one segment".to_string(),
                ));
            }
            let access_reports = ContentSegmentsAccessReports::for_paths(
                Some(&manifest_path),
                &output_archive,
                &segments,
            )?;
            let schedule_access_reports = access_reports.clone();
            let worker = BackgroundContentIndexer::default();
            let outcome = run_scheduled_volume_task_cancellable_with_volume_and_payload_path(
                Priority::Background,
                "content maintenance",
                pressure,
                move || {
                    schedule_access_reports.preflight_volumes("content maintenance")?;
                    Ok(schedule_access_reports.first_volume())
                },
                output_archive.clone(),
                move |cancellation| {
                    cancellation.check()?;
                    let _access = access_reports
                        .access_checked("content maintenance", || cancellation.check())?;
                    cancellation.check()?;
                    worker.maintain_segments_cancellable(
                        &manifest_path,
                        &output_archive,
                        &segments,
                        &ContentMaintenanceOptions::default(),
                        &cancellation,
                    )
                },
            )?;
            if outcome.deferred {
                eprintln!(
                    "content-maintenance-deferred\taction={:?}",
                    outcome.scheduling_action
                );
            } else {
                let report = outcome.result.ok_or_else(|| {
                    GfmError::Format("content maintenance ran without a report".to_string())
                })?;
                eprintln!(
                    "content-maintenance-action\t{:?}",
                    outcome.scheduling_action
                );
                print_content_maintenance_report(report);
            }
        }
        "index-content-background" => {
            let root = required_path(args.next(), "index-content-background requires a root path")?;
            let segment_dir = required_path(
                args.next(),
                "index-content-background requires a segment directory",
            )?;
            let records = required_path(
                args.next(),
                "index-content-background requires a records path",
            )?;
            let content = required_path(
                args.next(),
                "index-content-background requires a content path",
            )?;
            let pressure = parse_optional_scheduling_pressure(args)?;
            let journal = JobJournal::new(default_job_journal_path());
            let mut spec = ContentIndexJobSpec::new(&root, segment_dir, records, content);
            let spec_path = default_content_job_path();
            if pressure.decide(Priority::Background, 1, 1).action == SchedulingAction::Defer {
                let _access = ContentJobAccessReports::for_spec_path_write(&spec_path)?
                    .access_checked("background content index", || Ok(()))?;
            } else {
                let access_reports =
                    ContentJobAccessReports::for_spec(&spec, &spec_path, Some(journal.path()))?;
                access_reports.preflight_volumes("background content index")?;
                let volume = access_reports.first_volume().ok_or_else(|| {
                    GfmError::Format(format!(
                        "could not determine content index volume for {}",
                        spec.root.display()
                    ))
                })?;
                spec = spec.with_volume(volume);
            }
            spec.write(&spec_path)?;
            let outcome = run_content_job(&spec, &journal, pressure, &spec_path)?;
            if outcome.deferred {
                eprintln!(
                    "background-content-deferred action={:?}; journal {}; {} inaccessible",
                    outcome.scheduling_action,
                    journal.path().display(),
                    outcome.inaccessible
                );
            } else {
                let report = outcome.report.ok_or_else(|| {
                    GfmError::Format("background content index ran without a report".to_string())
                })?;
                eprintln!(
                    "background-content-indexed {} files; skipped {}; quarantined {}; unchanged {}; tombstoned {}; segments {}; terms {}; action={:?}; journal {}; {} inaccessible",
                    report.indexed,
                    report.skipped,
                    report.quarantined,
                    report.unchanged,
                    report.tombstoned,
                    report.segments.len(),
                    report.terms,
                    outcome.scheduling_action,
                    journal.path().display(),
                    outcome.inaccessible
                );
            }
        }
        "resume-content-background" => {
            let spec_path = args
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(default_content_job_path);
            let journal = args
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(default_job_journal_path);
            let journal = JobJournal::new(journal);
            let Some((recovery, spec)) =
                load_resumable_content_job_spec(spec_path.clone(), &journal)?
            else {
                eprintln!("no recoverable background content jobs");
                return Ok(true);
            };
            {
                let outcome =
                    run_content_job(&spec, &journal, SchedulingPressure::default(), &spec_path)?;
                if outcome.deferred {
                    eprintln!(
                        "resumed-background-content-deferred action={:?}; {}",
                        outcome.scheduling_action,
                        recovery.as_diagnostics()
                    );
                } else {
                    let report = outcome.report.ok_or_else(|| {
                        GfmError::Format(
                            "resumed background content index ran without a report".to_string(),
                        )
                    })?;
                    eprintln!(
                        "resumed-background-content-indexed {} files; skipped {}; quarantined {}; unchanged {}; tombstoned {}; segments {}; terms {}; action={:?}; {}",
                        report.indexed,
                        report.skipped,
                        report.quarantined,
                        report.unchanged,
                        report.tombstoned,
                        report.segments.len(),
                        report.terms,
                        outcome.scheduling_action,
                        recovery.as_diagnostics()
                    );
                }
            }
        }
        "resume-content-background-adaptive" => {
            let spec_path = required_path(
                args.next(),
                "resume-content-background-adaptive requires a content job spec path",
            )?;
            let journal_path = required_path(
                args.next(),
                "resume-content-background-adaptive requires a job journal path",
            )?;
            let pressure = parse_required_scheduling_pressure(args, "resume content job")?;
            let journal = JobJournal::new(journal_path);
            if pressure.decide(Priority::Background, 1, 1).action == SchedulingAction::Defer {
                let outcome = run_scheduled_volume_task_cancellable(
                    None,
                    Priority::Background,
                    "background content index",
                    pressure,
                    |_| {
                        Ok(ContentJobOutcome {
                            report: None,
                            inaccessible: 0,
                            scheduling_action: SchedulingAction::Defer,
                            deferred: true,
                        })
                    },
                )?;
                eprintln!(
                    "resumed-background-content-deferred action={:?}; recoverable unknown",
                    outcome.scheduling_action
                );
            } else {
                let Some((recovery, spec)) =
                    load_resumable_content_job_spec(spec_path.clone(), &journal)?
                else {
                    eprintln!("no recoverable background content jobs");
                    return Ok(true);
                };
                {
                    let outcome = run_content_job(&spec, &journal, pressure, &spec_path)?;
                    if outcome.deferred {
                        eprintln!(
                            "resumed-background-content-deferred action={:?}; {}",
                            outcome.scheduling_action,
                            recovery.as_diagnostics()
                        );
                    } else {
                        let report = outcome.report.ok_or_else(|| {
                            GfmError::Format(
                                "resumed background content index ran without a report".to_string(),
                            )
                        })?;
                        eprintln!(
                            "resumed-background-content-indexed {} files; skipped {}; quarantined {}; unchanged {}; tombstoned {}; segments {}; terms {}; action={:?}; {}",
                            report.indexed,
                            report.skipped,
                            report.quarantined,
                            report.unchanged,
                            report.tombstoned,
                            report.segments.len(),
                            report.terms,
                            outcome.scheduling_action,
                            recovery.as_diagnostics()
                        );
                    }
                }
            }
        }
        "index-footprint" => {
            let records = required_path(args.next(), "index-footprint requires a records path")?;
            let columns =
                optional_path_arg(args.next(), "index-footprint requires a columns path or -")?;
            let metadata =
                optional_path_arg(args.next(), "index-footprint requires a metadata path or -")?;
            let prefixes =
                optional_path_arg(args.next(), "index-footprint requires a prefixes path or -")?;
            let substrings = optional_path_arg(
                args.next(),
                "index-footprint requires a substrings path or -",
            )?;
            let fuzzy =
                optional_path_arg(args.next(), "index-footprint requires a fuzzy path or -")?;
            let content_manifest = optional_path_arg(
                args.next(),
                "index-footprint requires a content manifest path or -",
            )?;
            let mut spec = IndexFootprintSpec::new(records);
            spec.columns = columns;
            spec.metadata = metadata;
            spec.prefixes = prefixes;
            spec.substrings = substrings;
            spec.fuzzy = fuzzy;
            spec.content_manifest = content_manifest;
            spec.content_segments = args.map(PathBuf::from).collect();
            let report = run_index_footprint_inspect(spec, "index footprint")?;
            eprintln!(
                "index-footprint\trecords={}\ttotal-bytes={}\tbytes-per-record={}\tsegments={}\tsegment-bytes={}\tcompaction-scheduled={}\treason={:?}",
                report.record_count,
                report.total_bytes,
                report.bytes_per_record,
                report.segment_count,
                report.segment_bytes,
                report.compaction.scheduled,
                report.compaction.reason
            );
            println!(
                "records\tcount={}\tbytes={}",
                report.record_count, report.record_bytes
            );
            println!(
                "columns\tcount={}\tbytes={}\tstring-pool-bytes={}",
                report.column_count, report.column_bytes, report.column_string_pool_bytes
            );
            println!(
                "metadata\tterms={}\tbytes={}",
                report.metadata_terms, report.metadata_bytes
            );
            println!(
                "prefixes\tkeys={}\tbytes={}",
                report.prefix_keys, report.prefix_bytes
            );
            println!(
                "substrings\tkeys={}\tbytes={}",
                report.substring_keys, report.substring_bytes
            );
            println!(
                "fuzzy\tkeys={}\tbytes={}",
                report.fuzzy_keys, report.fuzzy_bytes
            );
            println!(
                "content\tarchives={}\tterms={}\tbytes={}",
                report.content_archives, report.content_terms, report.content_bytes
            );
            println!(
                "segments\tcount={}\tbytes={}\tpostings={}\ttombstone-segments={}\ttombstones={}",
                report.segment_count,
                report.segment_bytes,
                report.segment_postings,
                report.tombstone_segments,
                report.tombstones
            );
            println!(
                "compaction\tscheduled={}\ttier={:?}\treason={:?}\tmerge-bytes={}\tmerge-segments={}\tretained-segments={}\ttombstone-segments={}",
                report.compaction.scheduled,
                report.compaction.tier,
                report.compaction.reason,
                report.compaction.merge_bytes,
                report.compaction.merge_segments.len(),
                report.compaction.retained_segments.len(),
                report.compaction.tombstone_segments
            );
            for path in report.compaction.merge_segments {
                println!("merge-segment\t{}", path.display());
            }
            for path in report.compaction.retained_segments {
                println!("retain-segment\t{}", path.display());
            }
        }
        "index-compaction-plan" => {
            let records =
                required_path(args.next(), "index-compaction-plan requires a records path")?;
            let content_manifest = optional_path_arg(
                args.next(),
                "index-compaction-plan requires a content manifest path or -",
            )?;
            let io = parse_io_pressure(required_string(
                args.next(),
                "index-compaction-plan requires io pressure",
            )?)?;
            let thermal = parse_thermal_state(required_string(
                args.next(),
                "index-compaction-plan requires thermal state",
            )?)?;
            let battery = parse_battery_state(required_string(
                args.next(),
                "index-compaction-plan requires battery state",
            )?)?;
            let user_activity = parse_user_activity(required_string(
                args.next(),
                "index-compaction-plan requires user activity",
            )?)?;
            let mut spec = IndexFootprintSpec::new(records);
            spec.content_manifest = content_manifest;
            spec.content_segments = args.map(PathBuf::from).collect();
            spec.compaction_pressure = CompactionPressure {
                io,
                thermal,
                battery,
                user_activity,
            };
            let report = run_index_footprint_inspect(spec, "index compaction plan")?;
            eprintln!(
                "index-compaction-plan\taction={:?}\tscheduled={}\treason={:?}\tpressure={:?}\tmerge-bytes={}\teffective-max-bytes={}",
                report.compaction.action,
                report.compaction.scheduled,
                report.compaction.reason,
                report.compaction.pressure,
                report.compaction.merge_bytes,
                report.compaction.effective_max_merge_bytes
            );
            println!(
                "compaction\taction={:?}\tscheduled={}\ttier={:?}\treason={:?}\tmerge-segments={}\tretained-segments={}\tmerge-bytes={}\teffective-max-bytes={}\tbytes-per-record={}",
                report.compaction.action,
                report.compaction.scheduled,
                report.compaction.tier,
                report.compaction.reason,
                report.compaction.merge_segments.len(),
                report.compaction.retained_segments.len(),
                report.compaction.merge_bytes,
                report.compaction.effective_max_merge_bytes,
                report.bytes_per_record
            );
            for path in report.compaction.merge_segments {
                println!("merge-segment\t{}", path.display());
            }
            for path in report.compaction.retained_segments {
                println!("retain-segment\t{}", path.display());
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}

pub(crate) fn run_content_search(
    root: PathBuf,
    query: String,
    extractor: Extractor,
) -> Result<(usize, Vec<SearchHit>)> {
    let volume_report = VolumeDiscoveryReport::for_containing_path_checked(&root, || Ok(()))?;
    preflight_volume_access_scope_with_report(
        &root,
        AccessIntent::Index,
        "content search",
        &volume_report,
    )?;
    let volume = volume_report.volume_for_path(&root).map(|volume| volume.id);
    run_volume_task_cancellable(
        volume,
        Priority::Visible,
        "content extraction search",
        move |cancellation| {
            cancellation.check()?;
            let _access = preflight_access_scope_checked_with_volume_report(
                &root,
                AccessIntent::Index,
                "content search",
                &volume_report,
                || cancellation.check(),
            )?;
            cancellation.check()?;
            let snapshot = Indexer::default().build_cancellable(root, &cancellation)?;
            let mut live = snapshot.into_live();
            let indexed = live.index_content_cancellable(&extractor, &cancellation)?;
            let hits =
                live.search_with_snippets_cancellable(&query, 50, &extractor, 96, &cancellation)?;
            Ok((indexed, hits))
        },
    )
}

fn run_extraction_report(
    path: PathBuf,
    worker: &'static str,
    extractor: Extractor,
    retry_probe: Option<PathBuf>,
) -> Result<String> {
    let access_report = ForegroundContentIndexAccessReports::entry_checked(
        path.clone(),
        AccessIntent::Read,
        || Ok(()),
    )?;
    let retry_probe_access_report = retry_probe_access_report(retry_probe.as_deref())?;
    access_report.preflight_volume(worker)?;
    if matches!(fs::metadata(&path), Err(err) if err.kind() == io::ErrorKind::NotFound) {
        let _access = access_report.access_checked(worker, || Ok(()))?;
    }
    if let Some(report) = retry_probe_access_report.as_ref() {
        report.preflight_volume(worker)?;
    }
    let volume = access_report.volume().or_else(|| {
        retry_probe_access_report
            .as_ref()
            .and_then(ForegroundContentIndexAccessReport::volume)
    });
    run_retriable_volume_task_cancellable_with_payload_path(
        volume,
        Priority::Visible,
        worker,
        path.clone(),
        move |cancellation| {
            let path = path.clone();
            let extractor = extractor.clone();
            let retry_probe = retry_probe.clone();
            let retry_probe_access_report = retry_probe_access_report.clone();
            cancellation.check()?;
            if let (Some(retry_probe), Some(retry_probe_access_report)) =
                (retry_probe.as_ref(), retry_probe_access_report.as_ref())
            {
                fail_first_content_retry_probe_attempt(
                    retry_probe_access_report,
                    retry_probe,
                    worker,
                    &cancellation,
                )?;
            }
            let _access = access_report.access_checked(worker, || cancellation.check())?;
            cancellation.check()?;
            let report = extractor.extract_path_report_checked(&path, || cancellation.check())?;
            let mut quarantine = ExtractionQuarantine::default();
            let decision = quarantine.record_report(&report);
            Ok(format!("{}\n{}\n", report.as_tsv(), decision.as_tsv()))
        },
    )
}

fn run_extraction_cache(path: PathBuf) -> Result<String> {
    const WORKER: &str = "content extraction cache";
    let access_report = ForegroundContentIndexAccessReports::entry_checked(
        path.clone(),
        AccessIntent::Read,
        || Ok(()),
    )?;
    access_report.preflight_volume(WORKER)?;
    let volume = access_report.volume();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = access_report.access_checked(WORKER, || cancellation.check())?;
        cancellation.check()?;
        let record = record_for_path_checked(&path, None, false, || cancellation.check())?;
        cancellation.check()?;
        let mut cached = CachedExtractor::default();
        let first = cached
            .extract_record_report_checked(&record, || cancellation.check())?
            .as_tsv();
        cancellation.check()?;
        let second = cached
            .extract_record_report_checked(&record, || cancellation.check())?
            .as_tsv();
        Ok(format!("{first}\n{second}\n"))
    })
}

fn run_content_compaction(output: PathBuf, segments: Vec<PathBuf>) -> Result<usize> {
    const WORKER: &str = "content compaction";
    let access_reports = ContentSegmentsAccessReports::for_paths(None, &output, &segments)?;
    access_reports.preflight_volumes(WORKER)?;
    let volume = access_reports.first_volume();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = access_reports.access_checked(WORKER, || cancellation.check())?;
        cancellation.check()?;
        let terms = Indexer::default().compact_content_segments(output, &segments)?;
        cancellation.check()?;
        Ok(terms)
    })
}

fn run_tiered_content_compaction(
    output: PathBuf,
    segments: Vec<PathBuf>,
) -> Result<gfm_index::ContentMergeOutcome> {
    const WORKER: &str = "tiered content compaction";
    let access_reports = ContentSegmentsAccessReports::for_paths(None, &output, &segments)?;
    access_reports.preflight_volumes(WORKER)?;
    let volume = access_reports.first_volume();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = access_reports.access_checked(WORKER, || cancellation.check())?;
        cancellation.check()?;
        let outcome = Indexer::default().compact_content_segments_with_policy(
            output,
            &segments,
            &ContentMergePolicy::default(),
        )?;
        cancellation.check()?;
        Ok(outcome)
    })
}

fn run_content_segment_maintenance(
    manifest_path: PathBuf,
    output_archive: PathBuf,
    segments: Vec<PathBuf>,
) -> Result<ContentMaintenanceReport> {
    const WORKER: &str = "content maintenance";
    let access_reports =
        ContentSegmentsAccessReports::for_paths(Some(&manifest_path), &output_archive, &segments)?;
    access_reports.preflight_volumes(WORKER)?;
    let volume = access_reports.first_volume();
    let worker = BackgroundContentIndexer::default();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = access_reports.access_checked(WORKER, || cancellation.check())?;
        cancellation.check()?;
        let report = worker.maintain_segments_cancellable(
            &manifest_path,
            &output_archive,
            &segments,
            &ContentMaintenanceOptions::default(),
            &cancellation,
        )?;
        cancellation.check()?;
        Ok(report)
    })
}

fn load_resumable_content_job_spec(
    spec_path: PathBuf,
    journal: &JobJournal,
) -> Result<Option<(RecoverableContentJobs, ContentIndexJobSpec)>> {
    const WORKER: &str = "resume background content recovery";
    let access_reports = BackgroundContentRecoveryAccessReports::for_paths(&spec_path, journal)?;
    access_reports.preflight_recovery_stores()?;
    let volume = access_reports.first_volume();
    let journal = JobJournal::new(journal.path().to_path_buf());
    run_volume_task_cancellable_without_progress(
        volume,
        Priority::Visible,
        WORKER,
        move |cancellation| {
            cancellation.check()?;
            let recoverable =
                recoverable_background_content_jobs_checked(&journal, &access_reports, || {
                    cancellation.check()
                })?;
            if recoverable.total == 0 {
                return Ok(None);
            }
            access_reports
                .spec
                .preflight_volume("resume background content index")?;
            cancellation.check()?;
            let _access = access_reports
                .spec
                .access_checked("resume background content index", || cancellation.check())?;
            let spec = ContentIndexJobSpec::read_checked(&spec_path, || cancellation.check())?;
            cancellation.check()?;
            Ok(Some((recoverable, spec)))
        },
    )
}

fn run_index_footprint_inspect(
    spec: IndexFootprintSpec,
    worker: &'static str,
) -> Result<gfm_index::IndexFootprintReport> {
    let access_reports = IndexFootprintAccessReports::for_spec_checked(&spec, worker, || Ok(()))?;
    access_reports.preflight_volumes()?;
    let volume = access_reports.first_volume();
    run_volume_task_cancellable(volume, Priority::Visible, worker, move |cancellation| {
        cancellation.check()?;
        let _access = access_reports.access_checked(|| cancellation.check())?;
        let archive_paths = index_footprint_content_archive_paths(&spec, &cancellation)?;
        let archive_access_reports =
            IndexFootprintAccessReports::for_archive_paths_checked(&archive_paths, worker, || {
                cancellation.check()
            })?;
        archive_access_reports.preflight_volumes_checked(|| cancellation.check())?;
        cancellation.check()?;
        let _archive_access = archive_access_reports.access_checked(|| cancellation.check())?;
        cancellation.check()?;
        gfm_index::inspect_index_footprint_checked(&spec, &cancellation)
    })
}

fn index_footprint_content_archive_paths(
    spec: &IndexFootprintSpec,
    cancellation: &Cancellation,
) -> Result<Vec<PathBuf>> {
    let Some(manifest_path) = &spec.content_manifest else {
        return Ok(Vec::new());
    };
    let manifest = ContentArchiveManifest::read_checked(manifest_path, || cancellation.check())?;
    Ok(manifest.resolved_archive_paths(manifest_path))
}

#[derive(Clone)]
struct IndexFootprintAccessReport {
    path: PathBuf,
    worker: String,
    volume_report: VolumeDiscoveryReport,
}

impl IndexFootprintAccessReport {
    fn new_checked(
        path: PathBuf,
        worker: String,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        check_control()?;
        let volume_report =
            VolumeDiscoveryReport::for_containing_path_checked(&path, &mut check_control)?;
        check_control()?;
        Ok(Self {
            path,
            worker,
            volume_report,
        })
    }

    fn preflight_volume(&self) -> Result<()> {
        preflight_volume_access_scope_with_report(
            &self.path,
            AccessIntent::Read,
            &self.worker,
            &self.volume_report,
        )
    }

    fn access_checked(
        &self,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<ScopedAccessGuard> {
        preflight_access_scope_checked_with_volume_report(
            &self.path,
            AccessIntent::Read,
            &self.worker,
            &self.volume_report,
            &mut check_control,
        )
    }

    fn volume(&self) -> Option<gfm_types::VolumeId> {
        self.volume_report
            .volume_for_path(&self.path)
            .map(|volume| volume.id)
    }
}

#[derive(Clone)]
struct IndexFootprintAccessReports {
    entries: Vec<IndexFootprintAccessReport>,
}

impl IndexFootprintAccessReports {
    fn for_spec_checked(
        spec: &IndexFootprintSpec,
        worker: &str,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        let mut entries = Vec::new();
        for (path, role) in unique_index_footprint_paths(spec) {
            check_control()?;
            entries.push(IndexFootprintAccessReport::new_checked(
                path.to_path_buf(),
                format!("{worker} {role}"),
                &mut check_control,
            )?);
        }
        check_control()?;
        Ok(Self { entries })
    }

    fn for_archive_paths_checked(
        paths: &[PathBuf],
        worker: &str,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        let mut entries = Vec::new();
        for path in unique_path_refs(paths.iter().map(PathBuf::as_path)) {
            check_control()?;
            entries.push(IndexFootprintAccessReport::new_checked(
                path.to_path_buf(),
                format!("{worker} content archive"),
                &mut check_control,
            )?);
        }
        check_control()?;
        Ok(Self { entries })
    }

    fn preflight_volumes(&self) -> Result<()> {
        for report in &self.entries {
            report.preflight_volume()?;
        }
        Ok(())
    }

    fn preflight_volumes_checked(
        &self,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<()> {
        for report in &self.entries {
            check_control()?;
            report.preflight_volume()?;
        }
        check_control()?;
        Ok(())
    }

    fn access_checked(
        &self,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Vec<ScopedAccessGuard>> {
        let mut guards = Vec::with_capacity(self.entries.len());
        for report in &self.entries {
            check_control()?;
            guards.push(report.access_checked(&mut check_control)?);
        }
        check_control()?;
        Ok(guards)
    }

    fn first_volume(&self) -> Option<gfm_types::VolumeId> {
        self.entries
            .iter()
            .find_map(IndexFootprintAccessReport::volume)
    }
}

#[cfg(test)]
fn retain_index_footprint_access_checked(
    spec: &IndexFootprintSpec,
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    let mut guards = Vec::new();
    for (path, role) in unique_index_footprint_paths(spec) {
        check_control()?;
        guards.push(preflight_access_scope_checked(
            path,
            AccessIntent::Read,
            &format!("{worker} {role}"),
            &mut check_control,
        )?);
    }
    check_control()?;
    Ok(guards)
}

#[cfg(test)]
fn retain_index_footprint_archive_access_checked(
    paths: &[PathBuf],
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    let mut guards = Vec::new();
    for path in unique_path_refs(paths.iter().map(PathBuf::as_path)) {
        check_control()?;
        guards.push(preflight_access_scope_checked(
            path,
            AccessIntent::Read,
            &format!("{worker} content archive"),
            &mut check_control,
        )?);
    }
    check_control()?;
    Ok(guards)
}

#[cfg(test)]
fn preflight_index_footprint_archive_volumes_checked(
    paths: &[PathBuf],
    worker: &str,
    check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    IndexFootprintAccessReports::for_archive_paths_checked(paths, worker, || Ok(()))?
        .preflight_volumes_checked(check_control)
}

fn index_footprint_paths_with_roles(spec: &IndexFootprintSpec) -> Vec<(&Path, &'static str)> {
    let mut paths = vec![(spec.records.as_path(), "records")];
    if let Some(path) = &spec.columns {
        paths.push((path.as_path(), "columns"));
    }
    if let Some(path) = &spec.metadata {
        paths.push((path.as_path(), "metadata"));
    }
    if let Some(path) = &spec.prefixes {
        paths.push((path.as_path(), "prefixes"));
    }
    if let Some(path) = &spec.substrings {
        paths.push((path.as_path(), "substrings"));
    }
    if let Some(path) = &spec.fuzzy {
        paths.push((path.as_path(), "fuzzy"));
    }
    if let Some(path) = &spec.content_manifest {
        paths.push((path.as_path(), "content manifest"));
    }
    paths.extend(
        spec.content_segments
            .iter()
            .map(|path| (path.as_path(), "content segment")),
    );
    paths
}

fn unique_index_footprint_paths(spec: &IndexFootprintSpec) -> Vec<(&Path, &'static str)> {
    unique_path_roles(index_footprint_paths_with_roles(spec))
}

fn unique_path_roles<'a>(
    paths: impl IntoIterator<Item = (&'a Path, &'static str)>,
) -> Vec<(&'a Path, &'static str)> {
    let mut seen = BTreeSet::new();
    paths
        .into_iter()
        .filter(|(path, _)| seen.insert((*path).to_path_buf()))
        .collect()
}

fn unique_path_refs<'a>(paths: impl IntoIterator<Item = &'a Path>) -> Vec<&'a Path> {
    let mut seen = BTreeSet::new();
    paths
        .into_iter()
        .filter(|path| seen.insert((*path).to_path_buf()))
        .collect()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct RecoverableContentJobs {
    total: usize,
    interrupted: usize,
    retryable_failures: usize,
    restorable_progress: usize,
    transient_failures: usize,
    offline_volume_failures: usize,
    next_delay_ms: u64,
}

impl RecoverableContentJobs {
    fn add_journal_job(
        &mut self,
        reason: RecoveryReason,
        failure_class: Option<FailureClass>,
        next_delay_ms: u64,
    ) {
        self.total += 1;
        match reason {
            RecoveryReason::Interrupted => self.interrupted += 1,
            RecoveryReason::RetryableFailure => {
                self.retryable_failures += 1;
                self.next_delay_ms = self.next_delay_ms.max(next_delay_ms);
                match failure_class {
                    Some(FailureClass::Transient) => self.transient_failures += 1,
                    Some(FailureClass::OfflineVolume) => self.offline_volume_failures += 1,
                    Some(_) | None => {}
                }
            }
        }
    }

    fn add_progress_job(&mut self) {
        self.total += 1;
        self.restorable_progress += 1;
    }

    fn as_diagnostics(&self) -> String {
        format!(
            "recoverable {}; recovery-interrupted {}; recovery-retryable {}; recovery-progress {}; recovery-classes {}; next-delay-ms {}",
            self.total,
            self.interrupted,
            self.retryable_failures,
            self.restorable_progress,
            self.class_summary(),
            self.next_delay_ms
        )
    }

    fn class_summary(&self) -> String {
        let mut classes = Vec::new();
        if self.offline_volume_failures > 0 {
            classes.push(format!("offline-volume:{}", self.offline_volume_failures));
        }
        if self.transient_failures > 0 {
            classes.push(format!("transient:{}", self.transient_failures));
        }
        if classes.is_empty() {
            "-".to_string()
        } else {
            classes.join(",")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn unique_index_footprint_paths_preserves_first_role_for_repeated_paths() {
        let root = PathBuf::from("/tmp/gfm-footprint");
        let shared_sidecar = root.join("shared.gfmidx");
        let segment = root.join("segment.gfmseg");
        let mut spec = IndexFootprintSpec::new(shared_sidecar.clone());
        spec.columns = Some(shared_sidecar.clone());
        spec.metadata = Some(root.join("metadata.gfmmeta"));
        spec.prefixes = Some(shared_sidecar);
        spec.content_segments = vec![segment.clone(), segment.clone(), root.join("other.gfmseg")];

        let unique = unique_index_footprint_paths(&spec)
            .into_iter()
            .map(|(path, role)| (path.to_path_buf(), role))
            .collect::<Vec<_>>();

        assert_eq!(
            unique,
            vec![
                (root.join("shared.gfmidx"), "records"),
                (root.join("metadata.gfmmeta"), "metadata"),
                (root.join("segment.gfmseg"), "content segment"),
                (root.join("other.gfmseg"), "content segment"),
            ]
        );
    }

    #[test]
    fn unique_path_refs_preserves_first_occurrence_order() {
        let first = PathBuf::from("/tmp/gfm-first");
        let second = PathBuf::from("/tmp/gfm-second");
        let unique = unique_path_refs([first.as_path(), second.as_path(), first.as_path()])
            .into_iter()
            .map(Path::to_path_buf)
            .collect::<Vec<_>>();

        assert_eq!(unique, vec![first, second]);
    }

    #[test]
    fn index_footprint_access_checked_honors_pre_cancelled_control() {
        let root = unique_temp_dir("gfm-footprint-access-pre-cancel");
        let records = root.join("records.gfmidx");
        fs::write(&records, "records").unwrap();
        let spec = IndexFootprintSpec::new(records);

        let result = retain_index_footprint_access_checked(&spec, "index footprint", || {
            Err(GfmError::Cancelled)
        });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn index_footprint_archive_volume_preflight_checked_can_cancel_between_archives() {
        let root = unique_temp_dir("gfm-footprint-archive-volume-cancel");
        let first = root.join("first.gfmcontent");
        let second = root.join("second.gfmcontent");
        fs::write(&first, "first").unwrap();
        fs::write(&second, "second").unwrap();
        let mut checks = 0usize;

        let result = preflight_index_footprint_archive_volumes_checked(
            &[first, second],
            "index footprint",
            || {
                checks += 1;
                if checks >= 2 {
                    Err(GfmError::Cancelled)
                } else {
                    Ok(())
                }
            },
        );

        assert_eq!(result, Err(GfmError::Cancelled));
        assert!(checks >= 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn index_footprint_archive_access_checked_can_cancel_between_archives() {
        let root = unique_temp_dir("gfm-footprint-archive-access-cancel");
        let first = root.join("first.gfmcontent");
        let second = root.join("second.gfmcontent");
        fs::write(&first, "first").unwrap();
        fs::write(&second, "second").unwrap();
        let mut checks = 0usize;

        let result = retain_index_footprint_archive_access_checked(
            &[first, second],
            "index footprint",
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
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn content_input_file_exists_checked_honors_pre_cancelled_control_before_metadata() {
        let path = std::env::temp_dir().join(format!(
            "gfm-content-input-exists-cancelled-{}",
            std::process::id()
        ));

        let result = content_input_file_exists_checked(&path, "background content index", || {
            Err(GfmError::Cancelled)
        });

        assert_eq!(result, Err(GfmError::Cancelled));
        assert!(!path.exists());
    }

    #[test]
    fn content_input_file_exists_checked_reports_missing_file_as_false() {
        let path = std::env::temp_dir().join(format!(
            "gfm-content-input-exists-missing-{}",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);

        let exists =
            content_input_file_exists_checked(&path, "background content index", || Ok(()))
                .unwrap();

        assert!(!exists);
    }

    #[test]
    fn foreground_content_index_access_checked_honors_pre_cancelled_control() {
        let root = unique_temp_dir("gfm-content-index-access-pre-cancel");
        let records = root.join("records.gfmidx");
        let content = root.join("content.gfmcontent");
        let reports =
            ForegroundContentIndexAccessReports::for_paths(root.clone(), records, content).unwrap();

        let result =
            retain_foreground_content_index_access_checked(&reports, "content index", || {
                Err(GfmError::Cancelled)
            });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn foreground_content_index_reports_checked_honor_pre_cancelled_control_before_probe() {
        let root = std::env::temp_dir()
            .join(format!(
                "gfm-content-index-report-pre-cancel-{}",
                std::process::id()
            ))
            .join("root");
        let records = root.join("records.gfmidx");
        let content = root.join("content.gfmcontent");

        let result = ForegroundContentIndexAccessReports::for_paths_checked(
            root.clone(),
            records,
            content,
            || Err(GfmError::Cancelled),
        );

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!root.exists());
    }

    #[test]
    fn foreground_content_index_reports_checked_can_cancel_between_outputs() {
        let root = unique_temp_dir("gfm-content-index-report-cancel");
        let records = root.join("records.gfmidx");
        let content = root.join("content.gfmcontent");
        let mut checks = 0;

        let result = ForegroundContentIndexAccessReports::for_paths_checked(
            root.clone(),
            records,
            content,
            || {
                checks += 1;
                if checks > 4 {
                    Err(GfmError::Cancelled)
                } else {
                    Ok(())
                }
            },
        );

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn retry_probe_report_checked_honors_pre_cancelled_control_before_probe() {
        let path = std::env::temp_dir()
            .join(format!(
                "gfm-content-retry-report-pre-cancel-{}",
                std::process::id()
            ))
            .join("attempt.state");

        let result = retry_probe_access_report_checked(Some(&path), || Err(GfmError::Cancelled));

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!path.exists());
    }

    #[test]
    fn content_segments_access_checked_can_cancel_before_output_probe() {
        let root = unique_temp_dir("gfm-content-segments-access-pre-cancel");
        let output = root.join("output.gfmcontent");

        let result = retain_content_segments_access_checked(
            None,
            &output,
            &[],
            "content compaction",
            || Err(GfmError::Cancelled),
        );

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!output.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn content_segments_access_checked_can_cancel_during_segment_walk() {
        let root = unique_temp_dir("gfm-content-segments-access-walk-cancel");
        let output = root.join("output.gfmcontent");
        let first = root.join("first.gfmcontent");
        let second = root.join("second.gfmcontent");
        fs::write(&first, "first").unwrap();
        fs::write(&second, "second").unwrap();
        let mut checks = 0usize;

        let result = retain_content_segments_access_checked(
            None,
            &output,
            &[first, second],
            "content compaction",
            || {
                checks += 1;
                if checks >= 8 {
                    Err(GfmError::Cancelled)
                } else {
                    Ok(())
                }
            },
        );

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(checks >= 8);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn content_segment_index_reports_checked_honor_pre_cancelled_control_before_probe() {
        let root = std::env::temp_dir()
            .join(format!(
                "gfm-content-segment-report-pre-cancel-{}",
                std::process::id()
            ))
            .join("root");
        let output = root.join("segment.gfmcontent");

        let result = ContentSegmentIndexAccessReports::for_paths_checked(&root, &output, || {
            Err(GfmError::Cancelled)
        });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!root.exists());
    }

    #[test]
    fn content_segments_reports_checked_can_cancel_between_segments() {
        let root = unique_temp_dir("gfm-content-segments-report-cancel");
        let output = root.join("output.gfmcontent");
        let segments = vec![
            root.join("first.gfmcontent"),
            root.join("second.gfmcontent"),
        ];
        let mut checks = 0;

        let result =
            ContentSegmentsAccessReports::for_paths_checked(None, &output, &segments, || {
                checks += 1;
                if checks > 5 {
                    Err(GfmError::Cancelled)
                } else {
                    Ok(())
                }
            });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn content_job_access_checked_honors_pre_cancelled_control_before_preflight() {
        let root = unique_temp_dir("gfm-content-job-access-pre-cancel");
        let spec = ContentIndexJobSpec::new(
            root.join("missing-root"),
            root.join("segments"),
            root.join("records.gfmidx"),
            root.join("content.gfmcontent"),
        );
        let spec_path = root.join("job.tsv");

        let result = retain_content_job_access_checked(
            &spec,
            &spec_path,
            "background content index",
            || Err(GfmError::Cancelled),
        );

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!spec_path.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn content_job_reports_checked_can_cancel_between_outputs() {
        let root = unique_temp_dir("gfm-content-job-report-cancel");
        let spec = ContentIndexJobSpec::new(
            root.join("input"),
            root.join("segments"),
            root.join("records.gfmidx"),
            root.join("content.gfmcontent"),
        );
        let spec_path = root.join("job.tsv");
        let journal_path = root.join("journal.tsv");
        let mut checks = 0;

        let result = ContentJobAccessReports::for_spec_checked(
            &spec,
            &spec_path,
            Some(&journal_path),
            || {
                checks += 1;
                if checks > 6 {
                    Err(GfmError::Cancelled)
                } else {
                    Ok(())
                }
            },
        );

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn optional_recovery_store_access_checked_honors_pre_cancelled_control() {
        let root = unique_temp_dir("gfm-optional-recovery-access-pre-cancel");
        let store = root.join("journal.tsv");
        let reports = OptionalRecoveryStoreAccessReports::for_path(&store).unwrap();

        let result = reports.access_checked("background content recovery journal", || {
            Err(GfmError::Cancelled)
        });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!store.exists());
        fs::remove_dir_all(root).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "{}-{}-{}",
            prefix,
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
}

fn recoverable_background_content_jobs_checked(
    journal: &JobJournal,
    access_reports: &BackgroundContentRecoveryAccessReports,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<RecoverableContentJobs> {
    let _journal_access = access_reports
        .journal
        .access_checked("background content recovery journal", &mut check_control)?;
    check_control()?;
    let mut ids = HashSet::new();
    let mut recoverable = RecoverableContentJobs::default();
    for job in journal.recoverable(RetryPolicy { max_attempts: 2 })? {
        check_control()?;
        if job.label == "background content index" && ids.insert(job.id) {
            recoverable.add_journal_job(job.reason, job.failure_class, job.next_delay_ms);
        }
    }
    if let Some((store, progress_access_reports)) =
        runtime_progress_store().zip(access_reports.progress.as_ref())
    {
        check_control()?;
        let _progress_access = progress_access_reports
            .access_checked("background content recovery progress", &mut check_control)?;
        for snapshot in store.restorable()? {
            check_control()?;
            if snapshot.label == "background content index" && ids.insert(snapshot.id) {
                recoverable.add_progress_job();
            }
        }
    }
    check_control()?;
    Ok(recoverable)
}

#[derive(Clone)]
struct BackgroundContentRecoveryAccessReports {
    journal: OptionalRecoveryStoreAccessReports,
    progress: Option<OptionalRecoveryStoreAccessReports>,
    spec: ForegroundContentIndexAccessReport,
}

impl BackgroundContentRecoveryAccessReports {
    fn for_paths(spec_path: &Path, journal: &JobJournal) -> Result<Self> {
        Ok(Self {
            journal: OptionalRecoveryStoreAccessReports::for_path(journal.path())?,
            progress: runtime_progress_store()
                .map(|store| OptionalRecoveryStoreAccessReports::for_path(store.path()))
                .transpose()?,
            spec: ForegroundContentIndexAccessReports::entry_checked(
                spec_path.to_path_buf(),
                AccessIntent::Read,
                || Ok(()),
            )?,
        })
    }

    fn preflight_recovery_stores(&self) -> Result<()> {
        self.journal
            .preflight_volumes("background content recovery journal")?;
        if let Some(progress) = &self.progress {
            progress.preflight_volumes("background content recovery progress")?;
        }
        Ok(())
    }

    fn first_volume(&self) -> Option<VolumeId> {
        self.journal
            .first_volume()
            .or_else(|| {
                self.progress
                    .as_ref()
                    .and_then(OptionalRecoveryStoreAccessReports::first_volume)
            })
            .or_else(|| self.spec.volume())
    }
}

#[derive(Clone)]
struct OptionalRecoveryStoreAccessReports {
    parent: ForegroundContentIndexAccessReport,
    store: ForegroundContentIndexAccessReport,
}

impl OptionalRecoveryStoreAccessReports {
    fn for_path(path: &Path) -> Result<Self> {
        Ok(Self {
            parent: ForegroundContentIndexAccessReports::entry_checked(
                crate::parent_or_cwd(path).to_path_buf(),
                AccessIntent::Read,
                || Ok(()),
            )?,
            store: ForegroundContentIndexAccessReports::entry_checked(
                path.to_path_buf(),
                AccessIntent::Read,
                || Ok(()),
            )?,
        })
    }

    fn preflight_volumes(&self, worker: &str) -> Result<()> {
        self.parent.preflight_volume(worker)?;
        if optional_recovery_store_exists(&self.store.path, worker)? {
            self.store.preflight_volume(worker)?;
        }
        Ok(())
    }

    fn access_checked(
        &self,
        worker: &str,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Vec<ScopedAccessGuard>> {
        let mut guards = Vec::with_capacity(2);
        check_control()?;
        guards.push(self.parent.access_checked(worker, &mut check_control)?);
        check_control()?;
        if optional_recovery_store_exists_checked(&self.store.path, worker, &mut check_control)? {
            check_control()?;
            guards.push(self.store.access_checked(worker, &mut check_control)?);
        }
        check_control()?;
        Ok(guards)
    }

    fn first_volume(&self) -> Option<VolumeId> {
        self.parent.volume().or_else(|| self.store.volume())
    }
}

fn optional_recovery_store_exists(path: &Path, worker: &str) -> Result<bool> {
    path.try_exists()
        .map_err(|err| GfmError::io(path, format!("{worker} existence unavailable: {err}")))
}

fn optional_recovery_store_exists_checked(
    path: &Path,
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<bool> {
    check_control()?;
    let exists = optional_recovery_store_exists(path, worker)?;
    check_control()?;
    Ok(exists)
}

#[derive(Clone)]
struct ExtractionQuarantineAccessReports {
    entries: [ForegroundContentIndexAccessReport; 2],
}

impl ExtractionQuarantineAccessReports {
    fn for_paths(path: &Path, store: &Path) -> Result<Self> {
        Ok(Self {
            entries: [
                ForegroundContentIndexAccessReports::entry_checked(
                    path.to_path_buf(),
                    AccessIntent::Read,
                    || Ok(()),
                )?,
                ForegroundContentIndexAccessReports::entry_checked(
                    checked_write_probe_path(store)?.to_path_buf(),
                    AccessIntent::Write,
                    || Ok(()),
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

    fn first_volume(&self) -> Option<gfm_types::VolumeId> {
        self.entries
            .iter()
            .find_map(ForegroundContentIndexAccessReport::volume)
    }
}

fn run_extraction_quarantine(
    path: PathBuf,
    store: PathBuf,
    kind: gfm_content::QuarantineFailureKind,
    attempts: u32,
) -> Result<Vec<String>> {
    const WORKER: &str = "extraction quarantine";
    let access_reports = ExtractionQuarantineAccessReports::for_paths(&path, &store)?;
    access_reports.preflight_volumes(WORKER)?;
    let volume = access_reports.first_volume();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = access_reports.access_checked(WORKER, || cancellation.check())?;
        cancellation.check()?;
        let fingerprint = ExtractionFingerprint::for_path_checked(&path, || cancellation.check())?;
        let mut quarantine = ExtractionQuarantine::new(2);
        let mut decision = quarantine.before_extract(&path, &fingerprint);
        for _ in 0..attempts {
            decision = quarantine.record_failure(
                &path,
                &fingerprint,
                kind,
                format!("worker-{}", kind.as_str()),
            );
        }
        cancellation.check()?;
        quarantine.write_checked(&store, || cancellation.check())?;
        let reloaded = ExtractionQuarantine::read_checked(&store, || cancellation.check())?;
        cancellation.check()?;
        Ok(vec![
            decision.as_tsv(),
            reloaded.before_extract(&path, &fingerprint).as_tsv(),
        ])
    })
}

fn retain_foreground_content_index_access_checked(
    reports: &ForegroundContentIndexAccessReports,
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    let mut guards = Vec::with_capacity(reports.entries.len());
    for report in &reports.entries {
        check_control()?;
        guards.push(preflight_access_scope_checked_with_volume_report(
            &report.path,
            report.intent,
            worker,
            &report.volume_report,
            &mut check_control,
        )?);
    }
    check_control()?;
    Ok(guards)
}

#[derive(Clone)]
struct ForegroundContentIndexAccessReport {
    path: PathBuf,
    intent: AccessIntent,
    volume_report: VolumeDiscoveryReport,
}

impl ForegroundContentIndexAccessReport {
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

    fn volume(&self) -> Option<gfm_types::VolumeId> {
        self.volume_report
            .volume_for_path(&self.path)
            .map(|volume| volume.id)
    }
}

#[derive(Clone)]
struct ForegroundContentIndexAccessReports {
    entries: Vec<ForegroundContentIndexAccessReport>,
}

impl ForegroundContentIndexAccessReports {
    fn for_paths(root: PathBuf, records: PathBuf, content: PathBuf) -> Result<Self> {
        Self::for_paths_checked(root, records, content, || Ok(()))
    }

    fn for_paths_checked(
        root: PathBuf,
        records: PathBuf,
        content: PathBuf,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        check_control()?;
        let records_probe = write_probe_path(&records)?.to_path_buf();
        check_control()?;
        let content_probe = write_probe_path(&content)?.to_path_buf();
        check_control()?;
        Ok(Self {
            entries: vec![
                Self::entry_checked(root, AccessIntent::Index, &mut check_control)?,
                Self::entry_checked(records_probe, AccessIntent::Write, &mut check_control)?,
                Self::entry_checked(content_probe, AccessIntent::Write, &mut check_control)?,
            ],
        })
    }

    fn entry_checked(
        path: PathBuf,
        intent: AccessIntent,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<ForegroundContentIndexAccessReport> {
        check_control()?;
        let volume_report =
            VolumeDiscoveryReport::for_containing_path_checked(&path, &mut check_control)?;
        check_control()?;
        Ok(ForegroundContentIndexAccessReport {
            path,
            intent,
            volume_report,
        })
    }

    fn preflight_volumes(&self, worker: &str) -> Result<()> {
        for report in &self.entries {
            report.preflight_volume(worker)?;
        }
        Ok(())
    }

    fn first_volume(&self) -> Option<gfm_types::VolumeId> {
        self.entries
            .iter()
            .find_map(ForegroundContentIndexAccessReport::volume)
    }
}

fn retry_probe_access_report(
    retry_probe: Option<&Path>,
) -> Result<Option<ForegroundContentIndexAccessReport>> {
    retry_probe_access_report_checked(retry_probe, || Ok(()))
}

fn retry_probe_access_report_checked(
    retry_probe: Option<&Path>,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Option<ForegroundContentIndexAccessReport>> {
    retry_probe
        .map(|retry_probe| {
            check_control()?;
            let retry_probe = write_probe_path(retry_probe)?.to_path_buf();
            check_control()?;
            ForegroundContentIndexAccessReports::entry_checked(
                retry_probe,
                AccessIntent::Write,
                &mut check_control,
            )
        })
        .transpose()
}

#[derive(Clone)]
struct ContentSegmentsAccessReports {
    entries: Vec<ForegroundContentIndexAccessReport>,
}

#[derive(Clone)]
struct ContentJobAccessReports {
    entries: Vec<ForegroundContentIndexAccessReport>,
}

#[derive(Clone)]
struct ContentSegmentIndexAccessReports {
    entries: [ForegroundContentIndexAccessReport; 2],
}

impl ContentSegmentIndexAccessReports {
    fn for_paths(root: &Path, output: &Path) -> Result<Self> {
        Self::for_paths_checked(root, output, || Ok(()))
    }

    fn for_paths_checked(
        root: &Path,
        output: &Path,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        check_control()?;
        let output_probe = write_probe_path(output)?.to_path_buf();
        check_control()?;
        Ok(Self {
            entries: [
                ForegroundContentIndexAccessReports::entry_checked(
                    root.to_path_buf(),
                    AccessIntent::Index,
                    &mut check_control,
                )?,
                ForegroundContentIndexAccessReports::entry_checked(
                    output_probe,
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

    fn first_volume(&self) -> Option<gfm_types::VolumeId> {
        self.entries
            .iter()
            .find_map(ForegroundContentIndexAccessReport::volume)
    }
}

impl ContentSegmentsAccessReports {
    fn for_paths(
        manifest_path: Option<&Path>,
        output_archive: &Path,
        segments: &[PathBuf],
    ) -> Result<Self> {
        Self::for_paths_checked(manifest_path, output_archive, segments, || Ok(()))
    }

    fn for_paths_checked(
        manifest_path: Option<&Path>,
        output_archive: &Path,
        segments: &[PathBuf],
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        let mut entries =
            Vec::with_capacity(segments.len() + 1 + usize::from(manifest_path.is_some()));
        if let Some(manifest_path) = manifest_path {
            check_control()?;
            entries.push(ForegroundContentIndexAccessReports::entry_checked(
                manifest_path.to_path_buf(),
                AccessIntent::Read,
                &mut check_control,
            )?);
        }
        check_control()?;
        let output_archive = write_probe_path(output_archive)?.to_path_buf();
        check_control()?;
        entries.push(ForegroundContentIndexAccessReports::entry_checked(
            output_archive,
            AccessIntent::Write,
            &mut check_control,
        )?);
        for segment in segments {
            check_control()?;
            entries.push(ForegroundContentIndexAccessReports::entry_checked(
                segment.clone(),
                AccessIntent::Read,
                &mut check_control,
            )?);
        }
        Ok(Self { entries })
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

    fn first_volume(&self) -> Option<gfm_types::VolumeId> {
        self.entries
            .iter()
            .find_map(ForegroundContentIndexAccessReport::volume)
    }
}

impl ContentJobAccessReports {
    fn for_spec(
        spec: &ContentIndexJobSpec,
        spec_path: &Path,
        journal_path: Option<&Path>,
    ) -> Result<Self> {
        Self::for_spec_checked(spec, spec_path, journal_path, || Ok(()))
    }

    fn for_spec_checked(
        spec: &ContentIndexJobSpec,
        spec_path: &Path,
        journal_path: Option<&Path>,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        let quarantine_path = default_extraction_quarantine_path();
        let mut entries = Vec::with_capacity(6 + usize::from(journal_path.is_some()));
        check_control()?;
        entries.push(ForegroundContentIndexAccessReports::entry_checked(
            spec.root.clone(),
            AccessIntent::Index,
            &mut check_control,
        )?);
        for path in [
            spec.segment_dir.as_path(),
            spec.records_path.as_path(),
            spec.content_path.as_path(),
            spec_path,
            quarantine_path.as_path(),
        ] {
            check_control()?;
            let path = write_probe_path(path)?.to_path_buf();
            check_control()?;
            entries.push(ForegroundContentIndexAccessReports::entry_checked(
                path,
                AccessIntent::Write,
                &mut check_control,
            )?);
        }
        if let Some(journal_path) = journal_path {
            check_control()?;
            let journal_path = write_probe_path(journal_path)?.to_path_buf();
            check_control()?;
            entries.push(ForegroundContentIndexAccessReports::entry_checked(
                journal_path,
                AccessIntent::Write,
                &mut check_control,
            )?);
        }
        Ok(Self { entries })
    }

    fn for_spec_path_write(spec_path: &Path) -> Result<ForegroundContentIndexAccessReport> {
        Self::for_spec_path_write_checked(spec_path, || Ok(()))
    }

    fn for_spec_path_write_checked(
        spec_path: &Path,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<ForegroundContentIndexAccessReport> {
        check_control()?;
        let spec_path = write_probe_path(spec_path)?.to_path_buf();
        check_control()?;
        ForegroundContentIndexAccessReports::entry_checked(
            spec_path,
            AccessIntent::Write,
            &mut check_control,
        )
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

    fn first_volume(&self) -> Option<gfm_types::VolumeId> {
        self.entries
            .iter()
            .find_map(ForegroundContentIndexAccessReport::volume)
    }
}

#[cfg(test)]
fn retain_content_segments_access_checked(
    manifest_path: Option<&Path>,
    output_archive: &Path,
    segments: &[PathBuf],
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    let access_reports = ContentSegmentsAccessReports::for_paths_checked(
        manifest_path,
        output_archive,
        segments,
        &mut check_control,
    )?;
    access_reports.access_checked(worker, &mut check_control)
}

#[cfg(test)]
fn retain_content_job_access_checked(
    spec: &ContentIndexJobSpec,
    spec_path: &Path,
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    check_control()?;
    ContentJobAccessReports::for_spec_checked(spec, spec_path, None, &mut check_control)?
        .access_checked(worker, &mut check_control)
}

fn write_probe_path(path: &Path) -> Result<&Path> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(path),
        Ok(_) => validate_write_file_name(path).map(|()| crate::parent_or_cwd(path)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            validate_write_file_name(path).map(|()| crate::parent_or_cwd(path))
        }
        Err(err) => Err(GfmError::io(
            path,
            format!("content write path metadata unavailable: {err}"),
        )),
    }
}

fn checked_write_probe_path(path: &Path) -> Result<&Path> {
    write_probe_path(path)
}

fn validate_write_file_name(path: &Path) -> Result<()> {
    let Some(file_name) = path.file_name() else {
        return Ok(());
    };
    let limit = 255;
    if file_name.as_encoded_bytes().len() > limit {
        return Err(GfmError::io(
            path,
            format!(
                "content write filename too long: {} bytes exceeds {limit}",
                file_name.as_encoded_bytes().len()
            ),
        ));
    }
    Ok(())
}

fn content_input_file_exists_checked(
    path: &Path,
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<bool> {
    check_control()?;
    match fs::metadata(path) {
        Ok(metadata) => {
            check_control()?;
            Ok(metadata.is_file())
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            check_control()?;
            Ok(false)
        }
        Err(err) => Err(GfmError::io(
            path,
            format!("{worker} previous index metadata unavailable: {err}"),
        )),
    }
}

fn fail_first_content_retry_probe_attempt(
    access_report: &ForegroundContentIndexAccessReport,
    attempt_state: &Path,
    worker: &str,
    cancellation: &Cancellation,
) -> Result<()> {
    cancellation.check()?;
    let _access = access_report.access_checked(worker, || cancellation.check())?;
    cancellation.check()?;
    let attempts =
        read_content_retry_probe_attempt_checked(attempt_state, || cancellation.check())?;
    cancellation.check()?;
    write_content_retry_probe_attempt_checked(attempt_state, attempts + 1, || {
        cancellation.check()
    })?;
    cancellation.check()?;
    if attempts == 0 {
        return Err(GfmError::Format(format!(
            "temporary {worker} retry probe busy"
        )));
    }
    Ok(())
}

fn read_content_retry_probe_attempt_checked(
    path: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<usize> {
    check_control()?;
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(GfmError::io(path, err)),
    };
    check_control()?;
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        check_control()?;
        let read = file
            .read(&mut buffer)
            .map_err(|err| GfmError::io(path, err))?;
        check_control()?;
        if read == 0 {
            break;
        }
        if bytes.len().saturating_add(read) > 4096 {
            return Ok(0);
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    let Ok(value) = std::str::from_utf8(&bytes) else {
        return Ok(0);
    };
    Ok(value.trim().parse::<usize>().unwrap_or(0))
}

fn write_content_retry_probe_attempt_checked(
    path: &Path,
    attempt: usize,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    check_control()?;
    let encoded = attempt.to_string();
    atomic_write_checked(path, &mut check_control, |writer, check_control| {
        for chunk in encoded.as_bytes().chunks(4096) {
            check_control()?;
            writer
                .write_all(chunk)
                .map_err(|err| GfmError::io(path, err))?;
            check_control()?;
        }
        Ok(())
    })?;
    check_control()?;
    Ok(())
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
    spec_path: &Path,
) -> Result<ContentJobOutcome> {
    let scheduling = pressure.decide(Priority::Background, 1, 1);
    let label = "background content index";
    if scheduling.action == SchedulingAction::Defer {
        let mut scheduler = Scheduler::new();
        let volume = spec.volume;
        let job = if let Some(volume) = volume {
            scheduler.schedule_on_volume(Priority::Background, label, volume)
        } else {
            scheduler.schedule(Priority::Background, label)
        };
        let runtime = RuntimeJobHandle::begin_with_payload_path(
            &job,
            JobPayloadKind::Indexing,
            label,
            spec_path,
            1,
            format!("index:{}", spec.root.display()),
        )?;
        runtime.deferred(scheduling.action)?;
        return Ok(ContentJobOutcome {
            report: None,
            inaccessible: 0,
            scheduling_action: scheduling.action,
            deferred: true,
        });
    }
    let access_reports = ContentJobAccessReports::for_spec(spec, spec_path, Some(journal.path()))?;
    access_reports.preflight_volumes(label)?;
    let volume = spec
        .volume
        .or_else(|| access_reports.first_volume())
        .ok_or_else(|| {
            GfmError::Format(format!(
                "could not determine content index volume for {}",
                spec.root.display()
            ))
        })?;
    let job_spec = spec.clone();
    let (job_result_tx, job_result_rx) = mpsc::sync_channel(1);
    let mut scheduler = Scheduler::new();
    let job = scheduler.schedule_on_volume(Priority::Background, label, volume);
    let runtime = RuntimeJobHandle::begin_with_payload_path(
        &job,
        JobPayloadKind::Indexing,
        label,
        spec_path,
        1,
        format!("index:{}", spec.root.display()),
    )?;
    let plan = scheduler.drain_fair_ready(JobFairnessPolicy::default(), []);
    if let Some(blocked) = plan.blocked.first() {
        return Err(GfmError::Format(format!(
            "background content index job {} is blocked by missing dependencies",
            blocked.label
        )));
    }
    let tasks: Vec<_> = plan
        .ready
        .into_iter()
        .map(|scheduled| {
            let job_spec = job_spec.clone();
            let access_reports = access_reports.clone();
            let job_result_tx = job_result_tx.clone();
            let runtime = runtime.clone();
            RetriableTask::new(scheduled, move |cancellation| {
                runtime.running_checked(|| cancellation.check())?;
                cancellation.check()?;
                let _access = access_reports.access_checked(label, || cancellation.check())?;
                cancellation.check()?;
                let snapshot =
                    Indexer::default().build_cancellable(&job_spec.root, &cancellation)?;
                let inaccessible = snapshot.inaccessible.len();
                runtime.resize_checked(
                    snapshot.records.len().max(1) as u64,
                    format!("index:{}", job_spec.root.display()),
                    || cancellation.check(),
                )?;
                let previous_records = if content_input_file_exists_checked(
                    &job_spec.records_path,
                    "background content index",
                    || cancellation.check(),
                )? && content_input_file_exists_checked(
                    &job_spec.content_path,
                    "background content index",
                    || cancellation.check(),
                )? {
                    read_records_checked(&job_spec.records_path, || cancellation.check())?
                } else {
                    Vec::new()
                };
                snapshot.save_checked(&job_spec.records_path, || cancellation.check())?;
                let extractor = Extractor::with_budget_profile(extraction_budget_profile_checked(
                    &job_spec.root,
                    pressure,
                    || cancellation.check(),
                )?);
                let worker = BackgroundContentIndexer::new(extractor, job_spec.options());
                let quarantine_store = default_extraction_quarantine_path();
                let mut extraction_quarantine =
                    read_extraction_quarantine_cancellable(&quarantine_store, 2, &cancellation)?;
                let request = QuarantineContentIndexRequest {
                    snapshot: &snapshot,
                    previous_records: &previous_records,
                    previous_content_path: Some(&job_spec.content_path),
                    segment_dir: &job_spec.segment_dir,
                    content_path: &job_spec.content_path,
                    cancellation: &cancellation,
                };
                let report = worker.run_incremental_and_compact_with_quarantine(
                    request,
                    &mut extraction_quarantine,
                )?;
                extraction_quarantine.write_checked(&quarantine_store, || cancellation.check())?;
                job_result_tx.send((report, inaccessible)).map_err(|_| {
                    GfmError::Format("background content index result receiver dropped".to_string())
                })?;
                Ok(())
            })
        })
        .collect();
    drop(job_result_tx);
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
    let (report, inaccessible) = job_result_rx.try_recv().map_err(|_| {
        GfmError::Format("background content index completed without a report".to_string())
    })?;
    Ok(ContentJobOutcome {
        report: Some(report),
        inaccessible,
        scheduling_action: scheduling.action,
        deferred: false,
    })
}

fn print_content_maintenance_report(report: ContentMaintenanceReport) {
    eprintln!(
        "content-maintenance\tscheduled={}\tterms={}\tmerged={}\tretained={}\tmanifest-archives={}\ttier={:?}\tbytes={}\ttombstone-segments={}",
        report.scheduled,
        report.terms,
        report.merged_segments.len(),
        report.retained_segments.len(),
        report.manifest_archives,
        report.tier,
        report.merge_bytes,
        report.tombstone_segments
    );
    if let Some(path) = report.published_archive {
        println!("published\t{}", path.display());
    }
    for path in report.merged_segments {
        println!("merged-segment\t{}", path.display());
    }
    for path in report.retained_segments {
        println!("retain-segment\t{}", path.display());
    }
}
