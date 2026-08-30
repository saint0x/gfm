use crate::access::{preflight_access_scope, preflight_volume_access_scope, ScopedAccessGuard};
use crate::extract::{
    extraction_budget_profile, preflight_adaptive_extraction_worker_scratch,
    read_extraction_quarantine_cancellable, run_adaptive_extraction_worker_cancellable,
    run_quarantined_adaptive_extraction_worker_cancellable, ADAPTIVE_WORKER_TIMEOUT,
};
use crate::runtime::{
    default_content_job_path, default_extraction_quarantine_path, default_job_journal_path,
    run_scheduled_volume_task_cancellable,
    run_scheduled_volume_task_cancellable_with_volume_and_payload_path,
    run_volume_task_cancellable, run_volume_task_cancellable_without_progress,
    runtime_progress_store, RuntimeJobHandle,
};
use crate::{
    detect_volume_id, optional_path_arg, parent_volume, parse_battery_state, parse_io_pressure,
    parse_optional_scheduling_pressure, parse_quarantine_failure_kind,
    parse_required_scheduling_pressure, parse_thermal_state, parse_u32, parse_u64,
    parse_user_activity, path_volume, required_path, required_string,
};
use gfm_content::{CachedExtractor, ExtractionFingerprint, ExtractionQuarantine, Extractor};
use gfm_fs::record_for_path;
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
use gfm_mac::AccessIntent;
use gfm_store::{read_records_checked, ContentArchiveManifest};
use gfm_types::{GfmError, Result, SearchHit};
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

pub(crate) fn run(command: &str, args: &mut impl Iterator<Item = String>) -> Result<bool> {
    match command {
        "index-content" => {
            let root = required_path(args.next(), "index-content requires a root path")?;
            let records = required_path(args.next(), "index-content requires a records path")?;
            let content = required_path(args.next(), "index-content requires a content path")?;
            preflight_foreground_content_index_volumes(&root, &records, &content, "content index")?;
            let volume = detect_volume_id(&root)
                .ok()
                .or_else(|| parent_volume(&root));
            let (records_len, inaccessible_len, indexed) = run_volume_task_cancellable(
                volume,
                Priority::Visible,
                "content index",
                move |cancellation| {
                    let _access = retain_foreground_content_index_access(
                        &root,
                        &records,
                        &content,
                        "content index",
                    )?;
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
                run_extraction_report(path, "content extraction", Extractor::default(),)?
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
                run_extraction_report(path, "adaptive content extraction", extractor,)?
            );
        }
        "extract-worker-adaptive" => {
            let path = required_path(args.next(), "extract-worker-adaptive requires a path")?;
            let pressure = parse_required_scheduling_pressure(args, "extract worker")?;
            let _scratch_access = preflight_adaptive_extraction_worker_scratch()?;
            let volume_path = path.clone();
            let outcome = run_scheduled_volume_task_cancellable_with_volume_and_payload_path(
                Priority::Background,
                "adaptive extraction",
                pressure,
                move || {
                    preflight_volume_access_scope(
                        &volume_path,
                        AccessIntent::Read,
                        "adaptive extraction",
                    )?;
                    Ok(detect_volume_id(&volume_path)
                        .ok()
                        .or_else(|| parent_volume(&volume_path)))
                },
                path.clone(),
                move |cancellation| {
                    let _access = preflight_access_scope(
                        &path,
                        AccessIntent::Read,
                        "adaptive extraction worker",
                    )?;
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
                    preflight_volume_access_scope(
                        &volume_path,
                        AccessIntent::Read,
                        "quarantined adaptive extraction",
                    )?;
                    preflight_volume_access_scope(
                        write_probe_path(&volume_store)?,
                        AccessIntent::Write,
                        "quarantined adaptive extraction",
                    )?;
                    Ok(detect_volume_id(&volume_path)
                        .ok()
                        .or_else(|| parent_volume(&volume_path)))
                },
                path.clone(),
                move |cancellation| {
                    let _access = retain_extraction_quarantine_access(
                        &path,
                        &store,
                        "quarantined adaptive extraction",
                    )?;
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
        "index-content-segment" => {
            let root = required_path(args.next(), "index-content-segment requires a root path")?;
            let output = required_path(
                args.next(),
                "index-content-segment requires an output segment path",
            )?;
            preflight_content_segment_index_volumes(&root, &output, "content segment index")?;
            let volume = detect_volume_id(&root)
                .ok()
                .or_else(|| parent_volume(&root));
            let (inaccessible_len, indexed) = run_volume_task_cancellable(
                volume,
                Priority::Visible,
                "content segment index",
                move |cancellation| {
                    let _access = retain_content_segment_index_access(
                        &root,
                        &output,
                        "content segment index",
                    )?;
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
            let volume_manifest = manifest_path.clone();
            let volume_output = output_archive.clone();
            let volume_segments = segments.clone();
            let worker = BackgroundContentIndexer::default();
            let outcome = run_scheduled_volume_task_cancellable_with_volume_and_payload_path(
                Priority::Background,
                "content maintenance",
                pressure,
                move || {
                    preflight_content_segments_volumes(
                        Some(&volume_manifest),
                        &volume_output,
                        &volume_segments,
                        "content maintenance",
                    )?;
                    Ok(detect_volume_id(&volume_manifest)
                        .ok()
                        .or_else(|| parent_volume(&volume_output)))
                },
                output_archive.clone(),
                move |cancellation| {
                    let _access = retain_content_segments_access(
                        Some(&manifest_path),
                        &output_archive,
                        &segments,
                        "content maintenance",
                    )?;
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
                let _access = preflight_access_scope(
                    write_probe_path(&spec_path)?,
                    AccessIntent::Write,
                    "background content index",
                )?;
            } else {
                preflight_content_job_volumes(
                    &spec,
                    &spec_path,
                    Some(journal.path()),
                    "background content index",
                )?;
                spec = spec.with_volume(detect_volume_id(&root)?);
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
    preflight_volume_access_scope(&root, AccessIntent::Index, "content search")?;
    let volume = detect_volume_id(&root).ok();
    run_volume_task_cancellable(
        volume,
        Priority::Visible,
        "content extraction search",
        move |cancellation| {
            let _access = preflight_access_scope(&root, AccessIntent::Index, "content search")?;
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
) -> Result<String> {
    preflight_volume_access_scope(&path, AccessIntent::Read, worker)?;
    let volume = detect_volume_id(&path)
        .ok()
        .or_else(|| parent_volume(&path));
    run_volume_task_cancellable(volume, Priority::Visible, worker, move |cancellation| {
        cancellation.check()?;
        let _access = preflight_access_scope(&path, AccessIntent::Read, worker)?;
        cancellation.check()?;
        let report = extractor.extract_path_report_checked(&path, || cancellation.check())?;
        let mut quarantine = ExtractionQuarantine::default();
        let decision = quarantine.record_report(&report);
        Ok(format!("{}\n{}\n", report.as_tsv(), decision.as_tsv()))
    })
}

fn run_extraction_cache(path: PathBuf) -> Result<String> {
    const WORKER: &str = "content extraction cache";
    preflight_volume_access_scope(&path, AccessIntent::Read, WORKER)?;
    let volume = detect_volume_id(&path)
        .ok()
        .or_else(|| parent_volume(&path));
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = preflight_access_scope(&path, AccessIntent::Read, WORKER)?;
        cancellation.check()?;
        let record = record_for_path(&path, None, false)?;
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
    preflight_content_segments_volumes(None, &output, &segments, WORKER)?;
    let output_probe = write_probe_path(&output)?.to_path_buf();
    let volume = path_volume(&output_probe);
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = retain_content_segments_access(None, &output, &segments, WORKER)?;
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
    preflight_content_segments_volumes(None, &output, &segments, WORKER)?;
    let output_probe = write_probe_path(&output)?.to_path_buf();
    let volume = path_volume(&output_probe);
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = retain_content_segments_access(None, &output, &segments, WORKER)?;
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
    preflight_content_segments_volumes(Some(&manifest_path), &output_archive, &segments, WORKER)?;
    let output_probe = write_probe_path(&output_archive)?.to_path_buf();
    let volume = path_volume(&manifest_path).or_else(|| path_volume(&output_probe));
    let worker = BackgroundContentIndexer::default();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = retain_content_segments_access(
            Some(&manifest_path),
            &output_archive,
            &segments,
            WORKER,
        )?;
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
    preflight_background_content_recovery_volumes(journal)?;
    let volume = parent_volume(journal.path())
        .or_else(|| runtime_progress_store().and_then(|store| parent_volume(store.path())))
        .or_else(|| parent_volume(&spec_path));
    let journal = JobJournal::new(journal.path().to_path_buf());
    run_volume_task_cancellable_without_progress(
        volume,
        Priority::Visible,
        WORKER,
        move |cancellation| {
            cancellation.check()?;
            let recoverable = recoverable_background_content_jobs(&journal)?;
            if recoverable.total == 0 {
                return Ok(None);
            }
            preflight_volume_access_scope(
                &spec_path,
                AccessIntent::Read,
                "resume background content index",
            )?;
            cancellation.check()?;
            let _access = preflight_access_scope(
                &spec_path,
                AccessIntent::Read,
                "resume background content index",
            )?;
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
    preflight_index_footprint_volumes(&spec, worker)?;
    let volume = detect_volume_id(&spec.records)
        .ok()
        .or_else(|| parent_volume(&spec.records));
    run_volume_task_cancellable(volume, Priority::Visible, worker, move |cancellation| {
        cancellation.check()?;
        let _access = retain_index_footprint_access(&spec, worker)?;
        let archive_paths = index_footprint_content_archive_paths(&spec, &cancellation)?;
        preflight_index_footprint_archive_volumes(&archive_paths, worker)?;
        cancellation.check()?;
        let _archive_access = retain_index_footprint_archive_access(&archive_paths, worker)?;
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

fn retain_index_footprint_access(
    spec: &IndexFootprintSpec,
    worker: &str,
) -> Result<Vec<ScopedAccessGuard>> {
    let mut guards = Vec::new();
    for (path, role) in unique_index_footprint_paths(spec) {
        guards.push(preflight_access_scope(
            path,
            AccessIntent::Read,
            &format!("{worker} {role}"),
        )?);
    }
    Ok(guards)
}

fn retain_index_footprint_archive_access(
    paths: &[PathBuf],
    worker: &str,
) -> Result<Vec<ScopedAccessGuard>> {
    let mut guards = Vec::new();
    for path in unique_path_refs(paths.iter().map(PathBuf::as_path)) {
        guards.push(preflight_access_scope(
            path,
            AccessIntent::Read,
            &format!("{worker} content archive"),
        )?);
    }
    Ok(guards)
}

fn preflight_index_footprint_volumes(spec: &IndexFootprintSpec, worker: &str) -> Result<()> {
    for (path, role) in unique_index_footprint_paths(spec) {
        preflight_volume_access_scope(path, AccessIntent::Read, &format!("{worker} {role}"))?;
    }
    Ok(())
}

fn preflight_index_footprint_archive_volumes(paths: &[PathBuf], worker: &str) -> Result<()> {
    for path in unique_path_refs(paths.iter().map(PathBuf::as_path)) {
        preflight_volume_access_scope(
            path,
            AccessIntent::Read,
            &format!("{worker} content archive"),
        )?;
    }
    Ok(())
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
}

fn recoverable_background_content_jobs(journal: &JobJournal) -> Result<RecoverableContentJobs> {
    let _journal_access = retain_optional_recovery_store_access(
        journal.path(),
        "background content recovery journal",
    )?;
    let mut ids = HashSet::new();
    let mut recoverable = RecoverableContentJobs::default();
    for job in journal.recoverable(RetryPolicy { max_attempts: 2 })? {
        if job.label == "background content index" && ids.insert(job.id) {
            recoverable.add_journal_job(job.reason, job.failure_class, job.next_delay_ms);
        }
    }
    if let Some(store) = runtime_progress_store() {
        let _progress_access = retain_optional_recovery_store_access(
            store.path(),
            "background content recovery progress",
        )?;
        for snapshot in store.restorable()? {
            if snapshot.label == "background content index" && ids.insert(snapshot.id) {
                recoverable.add_progress_job();
            }
        }
    }
    Ok(recoverable)
}

fn preflight_background_content_recovery_volumes(journal: &JobJournal) -> Result<()> {
    preflight_optional_recovery_store_volumes(
        journal.path(),
        "background content recovery journal",
    )?;
    if let Some(store) = runtime_progress_store() {
        preflight_optional_recovery_store_volumes(
            store.path(),
            "background content recovery progress",
        )?;
    }
    Ok(())
}

fn preflight_optional_recovery_store_volumes(path: &Path, worker: &str) -> Result<()> {
    let parent = crate::parent_or_cwd(path);
    preflight_volume_access_scope(parent, AccessIntent::Read, worker)?;
    if optional_recovery_store_exists(path, worker)? {
        preflight_volume_access_scope(path, AccessIntent::Read, worker)?;
    }
    Ok(())
}

fn retain_optional_recovery_store_access(
    path: &Path,
    worker: &str,
) -> Result<Vec<ScopedAccessGuard>> {
    let mut guards = Vec::with_capacity(2);
    let parent = crate::parent_or_cwd(path);
    guards.push(preflight_access_scope(parent, AccessIntent::Read, worker)?);
    if optional_recovery_store_exists(path, worker)? {
        guards.push(preflight_access_scope(path, AccessIntent::Read, worker)?);
    }
    Ok(guards)
}

fn optional_recovery_store_exists(path: &Path, worker: &str) -> Result<bool> {
    path.try_exists()
        .map_err(|err| GfmError::io(path, format!("{worker} existence unavailable: {err}")))
}

fn retain_extraction_quarantine_access(
    path: &Path,
    store: &Path,
    worker: &str,
) -> Result<Vec<ScopedAccessGuard>> {
    Ok(vec![
        preflight_access_scope(path, AccessIntent::Read, worker)?,
        preflight_access_scope(
            checked_write_probe_path(store)?,
            AccessIntent::Write,
            worker,
        )?,
    ])
}

fn preflight_extraction_quarantine_volumes(path: &Path, store: &Path, worker: &str) -> Result<()> {
    preflight_volume_access_scope(path, AccessIntent::Read, worker)?;
    preflight_volume_access_scope(
        checked_write_probe_path(store)?,
        AccessIntent::Write,
        worker,
    )
}

fn run_extraction_quarantine(
    path: PathBuf,
    store: PathBuf,
    kind: gfm_content::QuarantineFailureKind,
    attempts: u32,
) -> Result<Vec<String>> {
    const WORKER: &str = "extraction quarantine";
    preflight_extraction_quarantine_volumes(&path, &store, WORKER)?;
    let store_probe = checked_write_probe_path(&store)?.to_path_buf();
    let volume = detect_volume_id(&path)
        .ok()
        .or_else(|| parent_volume(&path))
        .or_else(|| parent_volume(&store_probe));
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = retain_extraction_quarantine_access(&path, &store, WORKER)?;
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

fn retain_foreground_content_index_access(
    root: &Path,
    records: &Path,
    content: &Path,
    worker: &str,
) -> Result<Vec<ScopedAccessGuard>> {
    Ok(vec![
        preflight_access_scope(root, AccessIntent::Index, worker)?,
        preflight_access_scope(write_probe_path(records)?, AccessIntent::Write, worker)?,
        preflight_access_scope(write_probe_path(content)?, AccessIntent::Write, worker)?,
    ])
}

fn preflight_foreground_content_index_volumes(
    root: &Path,
    records: &Path,
    content: &Path,
    worker: &str,
) -> Result<()> {
    preflight_volume_access_scope(root, AccessIntent::Index, worker)?;
    preflight_volume_access_scope(write_probe_path(records)?, AccessIntent::Write, worker)?;
    preflight_volume_access_scope(write_probe_path(content)?, AccessIntent::Write, worker)
}

fn retain_content_segment_index_access(
    root: &Path,
    output: &Path,
    worker: &str,
) -> Result<Vec<ScopedAccessGuard>> {
    Ok(vec![
        preflight_access_scope(root, AccessIntent::Index, worker)?,
        preflight_access_scope(write_probe_path(output)?, AccessIntent::Write, worker)?,
    ])
}

fn preflight_content_segment_index_volumes(root: &Path, output: &Path, worker: &str) -> Result<()> {
    preflight_volume_access_scope(root, AccessIntent::Index, worker)?;
    preflight_volume_access_scope(write_probe_path(output)?, AccessIntent::Write, worker)
}

fn retain_content_segments_access(
    manifest_path: Option<&Path>,
    output_archive: &Path,
    segments: &[PathBuf],
    worker: &str,
) -> Result<Vec<ScopedAccessGuard>> {
    let mut guards = Vec::with_capacity(segments.len() + 1 + usize::from(manifest_path.is_some()));
    if let Some(manifest_path) = manifest_path {
        guards.push(preflight_access_scope(
            manifest_path,
            AccessIntent::Read,
            worker,
        )?);
    }
    guards.push(preflight_access_scope(
        write_probe_path(output_archive)?,
        AccessIntent::Write,
        worker,
    )?);
    for segment in segments {
        guards.push(preflight_access_scope(segment, AccessIntent::Read, worker)?);
    }
    Ok(guards)
}

fn preflight_content_segments_volumes(
    manifest_path: Option<&Path>,
    output_archive: &Path,
    segments: &[PathBuf],
    worker: &str,
) -> Result<()> {
    if let Some(manifest_path) = manifest_path {
        preflight_volume_access_scope(manifest_path, AccessIntent::Read, worker)?;
    }
    preflight_volume_access_scope(
        write_probe_path(output_archive)?,
        AccessIntent::Write,
        worker,
    )?;
    for segment in segments {
        preflight_volume_access_scope(segment, AccessIntent::Read, worker)?;
    }
    Ok(())
}

fn retain_content_job_access(
    spec: &ContentIndexJobSpec,
    spec_path: &Path,
    worker: &str,
) -> Result<Vec<ScopedAccessGuard>> {
    preflight_content_job_volumes(spec, spec_path, None, worker)?;
    Ok(vec![
        preflight_access_scope(&spec.root, AccessIntent::Index, worker)?,
        preflight_access_scope(
            write_probe_path(&spec.segment_dir)?,
            AccessIntent::Write,
            worker,
        )?,
        preflight_access_scope(
            write_probe_path(&spec.records_path)?,
            AccessIntent::Write,
            worker,
        )?,
        preflight_access_scope(
            write_probe_path(&spec.content_path)?,
            AccessIntent::Write,
            worker,
        )?,
        preflight_access_scope(write_probe_path(spec_path)?, AccessIntent::Write, worker)?,
        preflight_access_scope(
            write_probe_path(&default_extraction_quarantine_path())?,
            AccessIntent::Write,
            worker,
        )?,
    ])
}

fn preflight_content_job_volumes(
    spec: &ContentIndexJobSpec,
    spec_path: &Path,
    journal_path: Option<&Path>,
    worker: &str,
) -> Result<()> {
    preflight_volume_access_scope(&spec.root, AccessIntent::Index, worker)?;
    preflight_volume_access_scope(
        write_probe_path(&spec.segment_dir)?,
        AccessIntent::Write,
        worker,
    )?;
    preflight_volume_access_scope(
        write_probe_path(&spec.records_path)?,
        AccessIntent::Write,
        worker,
    )?;
    preflight_volume_access_scope(
        write_probe_path(&spec.content_path)?,
        AccessIntent::Write,
        worker,
    )?;
    preflight_volume_access_scope(write_probe_path(spec_path)?, AccessIntent::Write, worker)?;
    preflight_volume_access_scope(
        write_probe_path(&default_extraction_quarantine_path())?,
        AccessIntent::Write,
        worker,
    )?;
    if let Some(journal_path) = journal_path {
        preflight_volume_access_scope(
            write_probe_path(journal_path)?,
            AccessIntent::Write,
            worker,
        )?;
    }
    Ok(())
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

fn content_input_file_exists(path: &Path, worker: &str) -> Result<bool> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(metadata.is_file()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(GfmError::io(
            path,
            format!("{worker} previous index metadata unavailable: {err}"),
        )),
    }
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
    preflight_content_job_volumes(spec, spec_path, Some(journal.path()), label)?;
    let volume = spec
        .volume
        .or_else(|| detect_volume_id(&spec.root).ok())
        .or_else(|| parent_volume(&spec.records_path))
        .ok_or_else(|| {
            GfmError::Format(format!(
                "could not determine content index volume for {}",
                spec.root.display()
            ))
        })?;
    let job_spec = spec.clone();
    let job_spec_path = spec_path.to_path_buf();
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
            let job_spec_path = job_spec_path.clone();
            let job_result_tx = job_result_tx.clone();
            let runtime = runtime.clone();
            RetriableTask::new(scheduled, move |cancellation| {
                runtime.running()?;
                let _access = retain_content_job_access(
                    &job_spec,
                    &job_spec_path,
                    "background content index",
                )?;
                let snapshot =
                    Indexer::default().build_cancellable(&job_spec.root, &cancellation)?;
                let inaccessible = snapshot.inaccessible.len();
                runtime.resize(
                    snapshot.records.len().max(1) as u64,
                    format!("index:{}", job_spec.root.display()),
                )?;
                let previous_records = if content_input_file_exists(
                    &job_spec.records_path,
                    "background content index",
                )? && content_input_file_exists(
                    &job_spec.content_path,
                    "background content index",
                )? {
                    read_records_checked(&job_spec.records_path, || cancellation.check())?
                } else {
                    Vec::new()
                };
                snapshot.save_checked(&job_spec.records_path, || cancellation.check())?;
                let extractor = Extractor::with_budget_profile(extraction_budget_profile(
                    &job_spec.root,
                    pressure,
                ));
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
