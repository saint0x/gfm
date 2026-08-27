use crate::access::{preflight_access_scope, preflight_volume_access_scope, ScopedAccessGuard};
use crate::extract::{
    extraction_budget_profile, read_extraction_quarantine,
    run_adaptive_extraction_worker_cancellable,
    run_quarantined_adaptive_extraction_worker_cancellable, ADAPTIVE_WORKER_TIMEOUT,
};
use crate::runtime::{
    default_content_job_path, default_extraction_quarantine_path, default_job_journal_path,
    run_scheduled_volume_task_cancellable, run_scheduled_volume_task_cancellable_with_volume,
    run_volume_task_cancellable, runtime_progress_store, RuntimeJobHandle,
};
use crate::{
    detect_volume_id, optional_path_arg, parent_volume, parse_battery_state, parse_io_pressure,
    parse_optional_scheduling_pressure, parse_quarantine_failure_kind,
    parse_required_scheduling_pressure, parse_thermal_state, parse_u32, parse_u64,
    parse_user_activity, required_path, required_string,
};
use gfm_content::{CachedExtractor, ExtractionFingerprint, ExtractionQuarantine, Extractor};
use gfm_fs::record_for_path;
use gfm_index::{
    BackgroundContentIndexer, CompactionPressure, ContentIndexJobSpec, ContentIndexReport,
    ContentMaintenanceOptions, ContentMaintenanceReport, ContentMergePolicy, IndexFootprintSpec,
    Indexer, QuarantineContentIndexRequest,
};
use gfm_jobs::{
    Cancellation, JobFairnessPolicy, JobJournal, JobPayloadKind, Priority, RetriableTask,
    RetryPolicy, Scheduler, SchedulingAction, SchedulingPressure, TaskStatus, WorkerPool,
};
use gfm_mac::AccessIntent;
use gfm_store::read_records;
use gfm_types::{GfmError, Result, SearchHit};
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub(crate) fn run(command: &str, args: &mut impl Iterator<Item = String>) -> Result<bool> {
    match command {
        "index-content" => {
            let root = required_path(args.next(), "index-content requires a root path")?;
            let records = required_path(args.next(), "index-content requires a records path")?;
            let content = required_path(args.next(), "index-content requires a content path")?;
            let _access =
                retain_foreground_content_index_access(&root, &records, &content, "content index")?;
            let snapshot = Indexer::default().build(root)?;
            let indexed = snapshot.save_with_content(records, content, &Extractor::default())?;
            eprintln!(
                "indexed {} records; content-indexed {} files; {} inaccessible",
                snapshot.records.len(),
                indexed,
                snapshot.inaccessible.len()
            );
        }
        "extract-report" => {
            let path = required_path(args.next(), "extract-report requires a path")?;
            let _access = preflight_access_scope(&path, AccessIntent::Read, "content extraction")?;
            let extractor = Extractor::default();
            let report = extractor.extract_path_report(&path)?;
            let mut quarantine = ExtractionQuarantine::default();
            let decision = quarantine.record_report(&report);
            println!("{}", report.as_tsv());
            println!("{}", decision.as_tsv());
        }
        "extract-report-adaptive" => {
            let path = required_path(args.next(), "extract-report-adaptive requires a path")?;
            let pressure = parse_required_scheduling_pressure(args, "extract report")?;
            let _access =
                preflight_access_scope(&path, AccessIntent::Read, "adaptive content extraction")?;
            let root = path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            let extractor =
                Extractor::with_budget_profile(extraction_budget_profile(&root, pressure));
            let report = extractor.extract_path_report(&path)?;
            let mut quarantine = ExtractionQuarantine::default();
            let decision = quarantine.record_report(&report);
            println!("{}", report.as_tsv());
            println!("{}", decision.as_tsv());
        }
        "extract-worker-adaptive" => {
            let path = required_path(args.next(), "extract-worker-adaptive requires a path")?;
            let pressure = parse_required_scheduling_pressure(args, "extract worker")?;
            let volume_path = path.clone();
            let outcome = run_scheduled_volume_task_cancellable_with_volume(
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
            let volume_path = path.clone();
            let volume_store = store.clone();
            let outcome = run_scheduled_volume_task_cancellable_with_volume(
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
                        write_probe_path(&volume_store),
                        AccessIntent::Write,
                        "quarantined adaptive extraction",
                    )?;
                    Ok(detect_volume_id(&volume_path)
                        .ok()
                        .or_else(|| parent_volume(&volume_path)))
                },
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
            let _access =
                preflight_access_scope(&path, AccessIntent::Read, "content extraction cache")?;
            let record = record_for_path(&path, None, false)?;
            let mut cached = CachedExtractor::default();
            println!("{}", cached.extract_record_report(&record)?.as_tsv());
            println!("{}", cached.extract_record_report(&record)?.as_tsv());
        }
        "extract-quarantine" => {
            let path = required_path(args.next(), "extract-quarantine requires a path")?;
            let store = required_path(
                args.next(),
                "extract-quarantine requires a quarantine store path",
            )?;
            let _access =
                retain_extraction_quarantine_access(&path, &store, "extraction quarantine")?;
            let kind = parse_quarantine_failure_kind(
                args.next().as_deref().unwrap_or("timeout"),
                "failure kind",
            )?;
            let attempts = args
                .next()
                .map(|value| parse_u32(&value, "attempts"))
                .transpose()?
                .unwrap_or(2);
            let fingerprint = ExtractionFingerprint::for_path(&path)?;
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
            quarantine.write(&store)?;
            let reloaded = ExtractionQuarantine::read(&store)?;
            println!("{}", decision.as_tsv());
            println!("{}", reloaded.before_extract(&path, &fingerprint).as_tsv());
        }
        "index-content-segment" => {
            let root = required_path(args.next(), "index-content-segment requires a root path")?;
            let output = required_path(
                args.next(),
                "index-content-segment requires an output segment path",
            )?;
            let _access =
                retain_content_segment_index_access(&root, &output, "content segment index")?;
            let snapshot = Indexer::default().build(root)?;
            let indexed =
                snapshot.save_content_segment(output, &Extractor::default(), Vec::new())?;
            eprintln!(
                "content-segmented {} files; {} inaccessible",
                indexed,
                snapshot.inaccessible.len()
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
            let _access =
                retain_content_segments_access(None, &output, &segments, "content compaction")?;
            let terms = Indexer::default().compact_content_segments(output, &segments)?;
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
            let _access = retain_content_segments_access(
                None,
                &output,
                &segments,
                "tiered content compaction",
            )?;
            let outcome = Indexer::default().compact_content_segments_with_policy(
                output,
                &segments,
                &ContentMergePolicy::default(),
            )?;
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
            let _access = retain_content_segments_access(
                Some(&manifest_path),
                &output_archive,
                &segments,
                "content maintenance",
            )?;
            let worker = BackgroundContentIndexer::default();
            let report = worker.maintain_segments(
                &manifest_path,
                &output_archive,
                &segments,
                &ContentMaintenanceOptions::default(),
            )?;
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
            let worker = BackgroundContentIndexer::default();
            let outcome =
                if pressure.decide(Priority::Background, 1, 1).action == SchedulingAction::Defer {
                    run_scheduled_volume_task_cancellable_with_volume(
                        Priority::Background,
                        "content maintenance",
                        pressure,
                        || Ok(None),
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
                    )?
                } else {
                    let _access = retain_content_segments_access(
                        Some(&manifest_path),
                        &output_archive,
                        &segments,
                        "content maintenance",
                    )?;
                    let volume = detect_volume_id(&manifest_path)
                        .ok()
                        .or_else(|| parent_volume(&output_archive));
                    run_scheduled_volume_task_cancellable(
                        volume,
                        Priority::Background,
                        "content maintenance",
                        pressure,
                        move |cancellation| {
                            worker.maintain_segments_cancellable(
                                &manifest_path,
                                &output_archive,
                                &segments,
                                &ContentMaintenanceOptions::default(),
                                &cancellation,
                            )
                        },
                    )?
                };
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
                    write_probe_path(&spec_path),
                    AccessIntent::Write,
                    "background content index",
                )?;
            } else {
                let _access = retain_content_job_launch_access(
                    &spec,
                    &spec_path,
                    &journal,
                    pressure,
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
            let recoverable = recoverable_background_content_jobs(&journal)?;
            if recoverable == 0 {
                eprintln!("no recoverable background content jobs");
            } else {
                let _access = preflight_access_scope(
                    &spec_path,
                    AccessIntent::Read,
                    "resume background content index",
                )?;
                let spec = ContentIndexJobSpec::read(&spec_path)?;
                let outcome =
                    run_content_job(&spec, &journal, SchedulingPressure::default(), &spec_path)?;
                if outcome.deferred {
                    eprintln!(
                        "resumed-background-content-deferred action={:?}; recoverable {}",
                        outcome.scheduling_action, recoverable
                    );
                } else {
                    let report = outcome.report.ok_or_else(|| {
                        GfmError::Format(
                            "resumed background content index ran without a report".to_string(),
                        )
                    })?;
                    eprintln!(
                        "resumed-background-content-indexed {} files; skipped {}; quarantined {}; unchanged {}; tombstoned {}; segments {}; terms {}; action={:?}; recoverable {}",
                        report.indexed,
                        report.skipped,
                        report.quarantined,
                        report.unchanged,
                        report.tombstoned,
                        report.segments.len(),
                        report.terms,
                        outcome.scheduling_action,
                        recoverable
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
                let recoverable = recoverable_background_content_jobs(&journal)?;
                if recoverable == 0 {
                    eprintln!("no recoverable background content jobs");
                } else {
                    let _access = preflight_access_scope(
                        &spec_path,
                        AccessIntent::Read,
                        "resume background content index",
                    )?;
                    let spec = ContentIndexJobSpec::read(&spec_path)?;
                    let outcome = run_content_job(&spec, &journal, pressure, &spec_path)?;
                    if outcome.deferred {
                        eprintln!(
                            "resumed-background-content-deferred action={:?}; recoverable {}",
                            outcome.scheduling_action, recoverable
                        );
                    } else {
                        let report = outcome.report.ok_or_else(|| {
                            GfmError::Format(
                                "resumed background content index ran without a report".to_string(),
                            )
                        })?;
                        eprintln!(
                            "resumed-background-content-indexed {} files; skipped {}; quarantined {}; unchanged {}; tombstoned {}; segments {}; terms {}; action={:?}; recoverable {}",
                            report.indexed,
                            report.skipped,
                            report.quarantined,
                            report.unchanged,
                            report.tombstoned,
                            report.segments.len(),
                            report.terms,
                            outcome.scheduling_action,
                            recoverable
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
            let report = gfm_index::inspect_index_footprint(&spec)?;
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
            let report = gfm_index::inspect_index_footprint(&spec)?;
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
    let _access = preflight_access_scope(&root, AccessIntent::Index, "content search")?;
    let volume = detect_volume_id(&root).ok();
    run_volume_task_cancellable(
        volume,
        Priority::Visible,
        "content extraction search",
        move |cancellation| {
            let snapshot = Indexer::default().build_cancellable(root, &cancellation)?;
            let mut live = snapshot.into_live();
            let indexed = live.index_content_cancellable(&extractor, &cancellation)?;
            let hits =
                live.search_with_snippets_cancellable(&query, 50, &extractor, 96, &cancellation)?;
            Ok((indexed, hits))
        },
    )
}

fn recoverable_background_content_jobs(journal: &JobJournal) -> Result<usize> {
    let _journal_access = retain_optional_recovery_store_access(
        journal.path(),
        "background content recovery journal",
    )?;
    let mut ids = journal
        .recoverable(RetryPolicy { max_attempts: 2 })?
        .into_iter()
        .filter(|job| job.label == "background content index")
        .map(|job| job.id)
        .collect::<HashSet<_>>();
    if let Some(store) = runtime_progress_store() {
        let _progress_access = retain_optional_recovery_store_access(
            store.path(),
            "background content recovery progress",
        )?;
        for snapshot in store.restorable()? {
            if snapshot.label == "background content index" {
                ids.insert(snapshot.id);
            }
        }
    }
    Ok(ids.len())
}

fn retain_optional_recovery_store_access(
    path: &Path,
    worker: &str,
) -> Result<Vec<ScopedAccessGuard>> {
    let mut guards = Vec::with_capacity(2);
    let parent = path.parent().unwrap_or(path);
    guards.push(preflight_access_scope(parent, AccessIntent::Read, worker)?);
    if path.exists() {
        guards.push(preflight_access_scope(path, AccessIntent::Read, worker)?);
    }
    Ok(guards)
}

fn retain_extraction_quarantine_access(
    path: &Path,
    store: &Path,
    worker: &str,
) -> Result<Vec<ScopedAccessGuard>> {
    Ok(vec![
        preflight_access_scope(path, AccessIntent::Read, worker)?,
        preflight_access_scope(write_probe_path(store), AccessIntent::Write, worker)?,
    ])
}

fn retain_foreground_content_index_access(
    root: &Path,
    records: &Path,
    content: &Path,
    worker: &str,
) -> Result<Vec<ScopedAccessGuard>> {
    Ok(vec![
        preflight_access_scope(root, AccessIntent::Index, worker)?,
        preflight_access_scope(write_probe_path(records), AccessIntent::Write, worker)?,
        preflight_access_scope(write_probe_path(content), AccessIntent::Write, worker)?,
    ])
}

fn retain_content_segment_index_access(
    root: &Path,
    output: &Path,
    worker: &str,
) -> Result<Vec<ScopedAccessGuard>> {
    Ok(vec![
        preflight_access_scope(root, AccessIntent::Index, worker)?,
        preflight_access_scope(write_probe_path(output), AccessIntent::Write, worker)?,
    ])
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
        write_probe_path(output_archive),
        AccessIntent::Write,
        worker,
    )?);
    for segment in segments {
        guards.push(preflight_access_scope(segment, AccessIntent::Read, worker)?);
    }
    Ok(guards)
}

fn retain_content_job_access(
    spec: &ContentIndexJobSpec,
    spec_path: &Path,
    worker: &str,
) -> Result<Vec<ScopedAccessGuard>> {
    Ok(vec![
        preflight_access_scope(&spec.root, AccessIntent::Index, worker)?,
        preflight_access_scope(
            write_probe_path(&spec.segment_dir),
            AccessIntent::Write,
            worker,
        )?,
        preflight_access_scope(
            write_probe_path(&spec.records_path),
            AccessIntent::Write,
            worker,
        )?,
        preflight_access_scope(
            write_probe_path(&spec.content_path),
            AccessIntent::Write,
            worker,
        )?,
        preflight_access_scope(write_probe_path(spec_path), AccessIntent::Write, worker)?,
        preflight_access_scope(
            write_probe_path(&default_extraction_quarantine_path()),
            AccessIntent::Write,
            worker,
        )?,
    ])
}

fn retain_content_job_launch_access(
    spec: &ContentIndexJobSpec,
    spec_path: &Path,
    journal: &JobJournal,
    pressure: SchedulingPressure,
    worker: &str,
) -> Result<Vec<ScopedAccessGuard>> {
    let mut guards = retain_content_job_access(spec, spec_path, worker)?;
    if pressure.decide(Priority::Background, 1, 1).action != SchedulingAction::Defer {
        guards.push(preflight_access_scope(
            write_probe_path(journal.path()),
            AccessIntent::Write,
            worker,
        )?);
    }
    Ok(guards)
}

fn write_probe_path(path: &Path) -> &Path {
    if path.is_dir() {
        return path;
    }
    path.parent().unwrap_or(path)
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
    let _access = retain_content_job_access(spec, spec_path, "background content index")?;
    let _journal_access =
        preflight_access_scope(write_probe_path(journal.path()), AccessIntent::Write, label)?;
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
    let extractor = Extractor::with_budget_profile(extraction_budget_profile(&spec.root, pressure));
    let worker = BackgroundContentIndexer::new(extractor, spec.options());
    let quarantine_store = default_extraction_quarantine_path();
    let extraction_quarantine = read_extraction_quarantine(&quarantine_store, 2)?;
    let content_report = Arc::new(Mutex::new(None));
    let content_report_task = Arc::clone(&content_report);
    let mut scheduler = Scheduler::new();
    let job = scheduler.schedule_on_volume(Priority::Background, label, volume);
    let runtime = RuntimeJobHandle::begin_with_payload_path(
        &job,
        JobPayloadKind::Indexing,
        label,
        spec_path,
        snapshot.records.len().max(1) as u64,
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
