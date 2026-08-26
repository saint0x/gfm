use extract::{
    extraction_budget_profile, read_extraction_quarantine, run_adaptive_extraction_worker,
    run_adaptive_extraction_worker_cancellable, run_quarantined_adaptive_extraction_worker,
    ADAPTIVE_WORKER_TIMEOUT,
};
use gfm_config::ConfigStore;
use gfm_content::{
    CachedExtractor, ExtractionFingerprint, ExtractionQuarantine, Extractor, QuarantineFailureKind,
};
use gfm_diagnostics::{
    export_operator_trace, inspect_storage, plan_index_recovery, rebuild_index, recover_index,
    select_parity_baseline, PersistentIndexRecoverySpec, RebuildSpec, StorageInspection,
};
use gfm_fs::{read_directory, record_for_path};
use gfm_index::{
    query_sidecar_imports, BackgroundContentIndexer, BatteryState, CompactionPressure,
    ContentArchiveCleanupPolicy, ContentArchiveManifest, ContentArchiveManifestEntry,
    ContentIndexJobSpec, ContentIndexReport, ContentMaintenanceOptions, ContentMaintenanceReport,
    ContentMergePolicy, ContentMergeTier, EventBackpressureQueue, EventPriority, FseventsCursor,
    FseventsCursorHealth, IndexFootprintSpec, IndexMountState, IndexVolumeClass,
    IndexVolumeDescriptor, IndexVolumeState, Indexer, IoPressure, LiveIndex,
    PersistentIndexRecovery, QuarantineContentIndexRequest, SearchArchiveLookup,
    SearchLookupBudget, SearchRecordColumns, SearchStreamStage, ThermalState, UserActivity,
};
use gfm_jobs::{
    Cancellation, Job, JobBatteryState, JobClass, JobFairnessPlanner, JobFairnessPolicy,
    JobIoPressure, JobJournal, JobPayloadCatalog, JobPayloadKind, JobPayloadRecord,
    JobProgressSnapshot, JobProgressState, JobProgressStore, JobThermalState, JobUserActivity,
    Priority, RecoveryReason, RetriableTask, RetryPolicy, Scheduler, SchedulingAction,
    SchedulingPressure, Task, TaskStatus, VolumeConcurrencyPolicy, WorkerPool,
};
use gfm_mac::{
    current_host_profile, current_permission_onboarding, FileEventStream, MountState,
    SupportMatrix, VolumeDescriptor, VolumeKind, WatchRoot,
};
use gfm_store::{
    dictionary_term_report_from_records, inspect_archive_schema, metadata_postings_from_records,
    migrate_content_archive, migrate_metadata_archive, migrate_record_archive,
    plan_archive_rebuilds, plan_columns_archive_rebuild, plan_content_archive_migration,
    plan_content_manifest_promotion_recovery, plan_content_manifest_recovery,
    plan_derived_sidecar_rebuild, plan_metadata_archive_migration, plan_record_archive_migration,
    promote_content_archive_manifest, read_records, rebuild_columns_archive,
    rebuild_derived_sidecar, recover_content_manifest, recover_content_manifest_promotion,
    write_dictionary, write_metadata_postings, write_record_columns, ArchiveRebuildInputs,
    ArchiveSchemaKind, ContentArchive, ContentArchiveHealth, MetadataField, MmapContentArchive,
    MmapContentSet, MmapDictionary, MmapFuzzyArchive, MmapMetadataArchive, MmapPrefixArchive,
    MmapRecordArchive, MmapRecordColumns, MmapSubstringArchive,
};
use gfm_store::{
    fuzzy_postings_from_records, plan_sidecar_recovery, prefix_postings_from_records,
    recover_sidecars, sidecar_kind_name, substring_postings_from_records, write_fuzzy_postings,
    write_prefix_postings, write_substring_postings, SidecarHealth, SidecarKind, SidecarPaths,
    SidecarRecovery,
};
use gfm_testkit::{
    diff_rgba_files, evaluate_pixel_threshold, materialize_macrobench_fixture_report,
    materialize_parity_fixture, read_mask_file, run_large_sidecar_gate, run_macrobench,
    run_parity_gate_manifest, run_regression_gate, write_parity_review_bundle_manifest,
    ColorProfile, DisplayScale, LargeSidecarGateOptions, MacOsParityProfile, MacrobenchOptions,
    MacrobenchScale, MacrobenchStage, ParityAppearance, ParityFixtureOptions, ParityFixtureScale,
    ParitySurface, PixelDiffOptions, PixelDriftThreshold, PixelSize, RegressionGateOptions,
};
use gfm_types::{
    FileEvent, FileEventKind, FileId, FileKind, GfmError, Result, SearchHit, VolumeId,
};
use std::env;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

mod extract;
mod interface;
mod operation;
mod packaging;
mod platform;

fn main() {
    if let Err(err) = run() {
        eprintln!("gfm: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some(command) if interface::run(command, &mut args)? => {}
        Some("list") => {
            let path = args
                .next()
                .map(PathBuf::from)
                .unwrap_or(env::current_dir().unwrap());
            let page = read_directory(path)?;
            for record in page.entries {
                println!(
                    "{}\t{}\t{}",
                    marker(record.kind),
                    record.len,
                    record.path.display()
                );
            }
            for issue in page.inaccessible {
                eprintln!("inaccessible\t{}\t{}", issue.path.display(), issue.reason);
            }
        }
        Some("index") => {
            let root = required_path(args.next(), "index requires a root path")?;
            let output = required_path(args.next(), "index requires an output path")?;
            let snapshot = Indexer::default().build(root)?;
            snapshot.save(output)?;
            eprintln!(
                "indexed {} records; {} inaccessible",
                snapshot.records.len(),
                snapshot.inaccessible.len()
            );
        }
        Some("index-state") => {
            let root = required_path(args.next(), "index-state requires a root path")?;
            let records = required_path(args.next(), "index-state requires a records path")?;
            let state = required_path(args.next(), "index-state requires a state path")?;
            let state = Indexer::default().build_persistent(root, records, state)?;
            println!("{}", state.as_tsv());
        }
        Some("index-state-inspect") => {
            let state = required_path(
                args.next(),
                "index-state-inspect requires an index state path",
            )?;
            println!("{}", IndexVolumeState::read(state)?.as_tsv());
        }
        Some("scan-progress") => {
            let root = required_path(args.next(), "scan-progress requires a root path")?;
            let records = required_path(args.next(), "scan-progress requires a records path")?;
            let progress = required_path(
                args.next(),
                "scan-progress requires a progress checkpoint path",
            )?;
            let checkpoint = Indexer::default().build_with_progress(root, records, progress)?;
            println!("{}", checkpoint.as_tsv());
        }
        Some("scan-progress-inspect") => {
            let progress = required_path(
                args.next(),
                "scan-progress-inspect requires a progress checkpoint path",
            )?;
            println!("{}", Indexer::default().scan_progress(progress)?.as_tsv());
        }
        Some("fair-scan") => {
            let root = required_path(args.next(), "fair-scan requires a root path")?;
            let visible_burst =
                parse_usize_arg(args.next(), "fair-scan requires a visible burst size")?;
            let visible_roots = args.map(PathBuf::from).collect::<Vec<_>>();
            let report = Indexer::default().build_fair(root, &visible_roots, visible_burst)?;
            println!("{}", report.as_tsv());
        }
        Some("rename-correlation") => {
            let from = required_path(args.next(), "rename-correlation requires a source path")?;
            let to = required_path(
                args.next(),
                "rename-correlation requires a destination path",
            )?;
            let root = from
                .parent()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            let snapshot = Indexer::default().build(root)?;
            std::fs::rename(&from, &to).map_err(|err| GfmError::io(&from, err))?;
            let mut live = LiveIndex::from_records(snapshot.records);
            let report = live.apply_rename(&from, &to)?;
            println!("{}", report.as_tsv());
        }
        Some("metadata-update") => {
            let path = required_path(args.next(), "metadata-update requires a path")?;
            let root = path
                .parent()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            let snapshot = Indexer::default().build(root)?;
            if let Some(append) = args.next() {
                let mut file = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .map_err(|err| GfmError::io(&path, err))?;
                file.write_all(append.as_bytes())
                    .map_err(|err| GfmError::io(&path, err))?;
            }
            let mut live = LiveIndex::from_records(snapshot.records);
            let report = live.apply_metadata_update(&path)?;
            println!("{}", report.as_tsv());
        }
        Some("event-backpressure") => {
            let capacity = parse_usize_arg(args.next(), "event-backpressure requires a capacity")?;
            let visible_burst = parse_usize_arg(
                args.next(),
                "event-backpressure requires a visible burst size",
            )?;
            let background = parse_usize_arg(
                args.next(),
                "event-backpressure requires a background event count",
            )?;
            let visible = args
                .next()
                .map(|value| parse_usize(&value, "visible event count"))
                .transpose()?
                .unwrap_or(1);
            let mut queue = EventBackpressureQueue::new(capacity, visible_burst);
            for index in 0..background {
                queue.enqueue(
                    EventPriority::Background,
                    FileEvent::new(
                        format!("/tmp/gfm-background-{index}.md"),
                        FileEventKind::Modify,
                    ),
                );
            }
            for index in 0..visible {
                queue.enqueue(
                    EventPriority::Visible,
                    FileEvent::new(
                        format!("/tmp/gfm-visible-{index}.md"),
                        FileEventKind::Modify,
                    ),
                );
            }
            println!("{}", queue.snapshot().as_tsv());
        }
        Some("fsevents-cursor-checkpoint") => {
            let state = required_path(
                args.next(),
                "fsevents-cursor-checkpoint requires an index state path",
            )?;
            let cursor = required_path(
                args.next(),
                "fsevents-cursor-checkpoint requires a cursor path",
            )?;
            let event_id = parse_u64_arg(
                args.next(),
                "fsevents-cursor-checkpoint requires a last event id",
            )?;
            let health = args
                .next()
                .map(|value| FseventsCursorHealth::parse(&value))
                .transpose()?
                .unwrap_or(FseventsCursorHealth::Clean);
            let cursor =
                Indexer::default().checkpoint_fsevents_cursor(state, cursor, event_id, health)?;
            println!("{}", cursor.as_tsv());
        }
        Some("fsevents-cursor-inspect") => {
            let cursor = required_path(
                args.next(),
                "fsevents-cursor-inspect requires a cursor path",
            )?;
            println!("{}", FseventsCursor::read(cursor)?.as_tsv());
        }
        Some("fsevents-cursor-resume") => {
            let state = required_path(
                args.next(),
                "fsevents-cursor-resume requires an index state path",
            )?;
            let cursor =
                required_path(args.next(), "fsevents-cursor-resume requires a cursor path")?;
            println!(
                "{}",
                Indexer::default()
                    .fsevents_resume_plan(state, cursor)?
                    .as_tsv()
            );
        }
        Some("fsevents-repair-schedule") => {
            let state = required_path(
                args.next(),
                "fsevents-repair-schedule requires an index state path",
            )?;
            let cursor = required_path(
                args.next(),
                "fsevents-repair-schedule requires a cursor path",
            )?;
            let event_ids = args.next().ok_or_else(|| {
                GfmError::Format(
                    "fsevents-repair-schedule requires observed event ids or `-`".to_string(),
                )
            })?;
            let observed_event_ids = parse_event_ids(&event_ids)?;
            let reason = args
                .next()
                .and_then(|value| (value != "-").then_some(value));
            let dropped_roots: Vec<PathBuf> = args.map(PathBuf::from).collect();
            println!(
                "{}",
                Indexer::default()
                    .repair_schedule(
                        state,
                        cursor,
                        &observed_event_ids,
                        &dropped_roots,
                        reason.as_deref(),
                    )?
                    .as_tsv()
            );
        }
        Some("index-content") => {
            let root = required_path(args.next(), "index-content requires a root path")?;
            let records = required_path(args.next(), "index-content requires a records path")?;
            let content = required_path(args.next(), "index-content requires a content path")?;
            let snapshot = Indexer::default().build(root)?;
            let indexed = snapshot.save_with_content(records, content, &Extractor::default())?;
            eprintln!(
                "indexed {} records; content-indexed {} files; {} inaccessible",
                snapshot.records.len(),
                indexed,
                snapshot.inaccessible.len()
            );
        }
        Some("extract-report") => {
            let path = required_path(args.next(), "extract-report requires a path")?;
            let extractor = Extractor::default();
            let report = extractor.extract_path_report(&path)?;
            let mut quarantine = ExtractionQuarantine::default();
            let decision = quarantine.record_report(&report);
            println!("{}", report.as_tsv());
            println!("{}", decision.as_tsv());
        }
        Some("extract-report-adaptive") => {
            let path = required_path(args.next(), "extract-report-adaptive requires a path")?;
            let pressure = parse_required_scheduling_pressure(&mut args, "extract report")?;
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
        Some("extract-worker-adaptive") => {
            let path = required_path(args.next(), "extract-worker-adaptive requires a path")?;
            let pressure = parse_required_scheduling_pressure(&mut args, "extract worker")?;
            let volume = detect_volume_id(&path)
                .ok()
                .or_else(|| parent_volume(&path));
            let report = run_volume_task(
                volume,
                Priority::Background,
                "adaptive extraction",
                move || run_adaptive_extraction_worker(&path, pressure),
            )?;
            print!("{}", report);
        }
        Some("extract-worker-cancel-adaptive") => {
            let path = required_path(
                args.next(),
                "extract-worker-cancel-adaptive requires a path",
            )?;
            let pressure = parse_required_scheduling_pressure(&mut args, "extract worker")?;
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
        Some("extract-worker-quarantine-adaptive") => {
            let path = required_path(
                args.next(),
                "extract-worker-quarantine-adaptive requires a path",
            )?;
            let store = required_path(
                args.next(),
                "extract-worker-quarantine-adaptive requires a quarantine store path",
            )?;
            let pressure = parse_required_scheduling_pressure(&mut args, "extract worker")?;
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
            let volume = detect_volume_id(&path)
                .ok()
                .or_else(|| parent_volume(&path));
            let output = run_volume_task(
                volume,
                Priority::Background,
                "quarantined adaptive extraction",
                move || {
                    run_quarantined_adaptive_extraction_worker(
                        &path, &store, pressure, timeout, threshold,
                    )
                },
            )?;
            print!("{output}");
        }
        Some("extract-cache") => {
            let path = required_path(args.next(), "extract-cache requires a path")?;
            let record = record_for_path(&path, None, false)?;
            let mut cached = CachedExtractor::default();
            println!("{}", cached.extract_record_report(&record)?.as_tsv());
            println!("{}", cached.extract_record_report(&record)?.as_tsv());
        }
        Some("extract-quarantine") => {
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
        Some("index-content-segment") => {
            let root = required_path(args.next(), "index-content-segment requires a root path")?;
            let output = required_path(
                args.next(),
                "index-content-segment requires an output segment path",
            )?;
            let snapshot = Indexer::default().build(root)?;
            let indexed =
                snapshot.save_content_segment(output, &Extractor::default(), Vec::new())?;
            eprintln!(
                "content-segmented {} files; {} inaccessible",
                indexed,
                snapshot.inaccessible.len()
            );
        }
        Some("compact-content") => {
            let output = required_path(args.next(), "compact-content requires an output path")?;
            let segments: Vec<PathBuf> = args.map(PathBuf::from).collect();
            if segments.is_empty() {
                return Err(gfm_types::GfmError::Format(
                    "compact-content requires at least one segment path".to_string(),
                ));
            }
            let terms = Indexer::default().compact_content_segments(output, &segments)?;
            eprintln!("compacted {terms} content terms");
        }
        Some("compact-content-tiered") => {
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
        Some("content-manifest-write") => {
            let output = required_path(
                args.next(),
                "content-manifest-write requires a manifest path",
            )?;
            let archives = args
                .map(|spec| parse_content_manifest_archive_spec(&spec))
                .collect::<Result<Vec<_>>>()?;
            let manifest = ContentArchiveManifest::new(archives)?;
            manifest.write(&output)?;
            eprintln!("content-manifest\tarchives={}", manifest.archives.len());
        }
        Some("content-manifest-inspect") => {
            let manifest_path = required_path(
                args.next(),
                "content-manifest-inspect requires a manifest path",
            )?;
            let manifest = ContentArchiveManifest::read(&manifest_path)?;
            let paths = manifest.resolved_archive_paths(&manifest_path);
            let set = MmapContentSet::open(&paths)?;
            println!(
                "content-manifest\tarchives={}\tterms={}\tbytes={}",
                set.archive_count(),
                set.indexed_terms(),
                set.mapped_len()
            );
            for (entry, path) in manifest.archives.iter().zip(paths) {
                println!(
                    "archive\t{}\t{}\t{}",
                    content_tier_name(entry.tier),
                    entry.path.display(),
                    path.display()
                );
            }
        }
        Some("content-manifest-recovery-plan") => {
            let manifest_path = required_path(
                args.next(),
                "content-manifest-recovery-plan requires a manifest path",
            )?;
            let discovered = args
                .map(|spec| parse_content_manifest_archive_spec(&spec))
                .collect::<Result<Vec<_>>>()?;
            let plan = plan_content_manifest_recovery(&manifest_path, &discovered);
            println!("{}", plan.as_tsv());
            print_content_archive_health("invalid", &plan.invalid_archives);
        }
        Some("content-manifest-recover") => {
            let manifest_path = required_path(
                args.next(),
                "content-manifest-recover requires a manifest path",
            )?;
            let quarantine = required_path(
                args.next(),
                "content-manifest-recover requires a quarantine directory",
            )?;
            let discovered = args
                .map(|spec| parse_content_manifest_archive_spec(&spec))
                .collect::<Result<Vec<_>>>()?;
            let report = recover_content_manifest(&manifest_path, &discovered, &quarantine)?;
            println!("{}", report.before.as_tsv());
            println!(
                "content-manifest-recovery\twrote-manifest={}\tquarantined-manifest={}",
                report.wrote_manifest,
                report
                    .quarantined_manifest_path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "-".to_string())
            );
            println!("{}", report.after.as_tsv());
            print_content_archive_health("invalid-before", &report.before.invalid_archives);
        }
        Some("content-manifest-promote") => {
            let manifest_path = required_path(
                args.next(),
                "content-manifest-promote requires a manifest path",
            )?;
            let new_archive = args.next().ok_or_else(|| {
                GfmError::Format(
                    "content-manifest-promote requires a hot:path, warm:path, or cold:path archive"
                        .to_string(),
                )
            })?;
            let new_archive = parse_content_manifest_archive_spec(&new_archive)?;
            let retired_paths = args.map(PathBuf::from).collect::<Vec<_>>();
            let promotion =
                promote_content_archive_manifest(&manifest_path, new_archive, &retired_paths)?;
            eprintln!(
                "content-manifest-promoted\tarchives={}\tretired={}\tmissing-retirements={}",
                promotion.manifest.archives.len(),
                promotion.retired_archives.len(),
                promotion.missing_retirements.len()
            );
            for path in promotion.retired_archives {
                println!("retire\t{}", path.display());
            }
            for path in promotion.missing_retirements {
                println!("missing-retirement\t{}", path.display());
            }
        }
        Some("content-manifest-promotion-recovery-plan") => {
            let manifest_path = required_path(
                args.next(),
                "content-manifest-promotion-recovery-plan requires a manifest path",
            )?;
            println!(
                "{}",
                plan_content_manifest_promotion_recovery(manifest_path).as_tsv()
            );
        }
        Some("content-manifest-promotion-recover") => {
            let manifest_path = required_path(
                args.next(),
                "content-manifest-promotion-recover requires a manifest path",
            )?;
            let recovery = recover_content_manifest_promotion(manifest_path)?;
            println!("{}", recovery.before.as_tsv());
            println!(
                "content-manifest-promotion-recovery\tcompleted-promotion={}\tremoved-journal={}",
                recovery.completed_promotion, recovery.removed_journal
            );
            println!("{}", recovery.after.as_tsv());
        }
        Some("content-manifest-cleanup") => {
            let manifest_path = required_path(
                args.next(),
                "content-manifest-cleanup requires a manifest path",
            )?;
            let candidates = args.map(PathBuf::from).collect::<Vec<_>>();
            if candidates.is_empty() {
                return Err(GfmError::Format(
                    "content-manifest-cleanup requires at least one candidate archive".to_string(),
                ));
            }
            let manifest = ContentArchiveManifest::read(&manifest_path)?;
            let report = manifest.cleanup_inactive_archives(&manifest_path, &candidates)?;
            eprintln!(
                "content-manifest-cleanup\tremoved={}\tactive={}\tmissing={}",
                report.removed_archives.len(),
                report.active_archives.len(),
                report.missing_archives.len()
            );
            for path in report.removed_archives {
                println!("removed\t{}", path.display());
            }
            for path in report.active_archives {
                println!("active\t{}", path.display());
            }
            for path in report.missing_archives {
                println!("missing\t{}", path.display());
            }
        }
        Some("content-cleanup-plan") => {
            let manifest_path =
                required_path(args.next(), "content-cleanup-plan requires a manifest path")?;
            let min_retired_archives = parse_usize_arg(
                args.next(),
                "content-cleanup-plan requires min-retired-archives",
            )?;
            let min_retired_bytes = parse_u64_arg(
                args.next(),
                "content-cleanup-plan requires min-retired-bytes",
            )?;
            let max_cleanup_archives = parse_usize_arg(
                args.next(),
                "content-cleanup-plan requires max-cleanup-archives",
            )?;
            let candidates = args.map(PathBuf::from).collect::<Vec<_>>();
            let manifest = ContentArchiveManifest::read(&manifest_path)?;
            let plan = manifest.plan_inactive_archive_cleanup(
                &manifest_path,
                &candidates,
                &ContentArchiveCleanupPolicy {
                    min_retired_archives,
                    min_retired_bytes,
                    max_cleanup_archives,
                },
            )?;
            eprintln!(
                "content-cleanup-plan\taction={:?}\tcleanup={}\tdeferred={}\tactive={}\tmissing={}\tactive-bytes={}\tcleanup-bytes={}\tdeferred-bytes={}",
                plan.action,
                plan.cleanup_archives.len(),
                plan.deferred_archives.len(),
                plan.active_archives.len(),
                plan.missing_archives.len(),
                plan.active_bytes,
                plan.cleanup_bytes,
                plan.deferred_bytes
            );
            for path in plan.cleanup_archives {
                println!("cleanup\t{}", path.display());
            }
            for path in plan.deferred_archives {
                println!("defer\t{}", path.display());
            }
            for path in plan.active_archives {
                println!("active\t{}", path.display());
            }
            for path in plan.missing_archives {
                println!("missing\t{}", path.display());
            }
        }
        Some("content-maintain-segments") => {
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
            let worker = BackgroundContentIndexer::default();
            let report = worker.maintain_segments(
                &manifest_path,
                &output_archive,
                &segments,
                &ContentMaintenanceOptions::default(),
            )?;
            print_content_maintenance_report(report);
        }
        Some("content-maintain-segments-adaptive") => {
            let manifest_path = required_path(
                args.next(),
                "content-maintain-segments-adaptive requires a manifest path",
            )?;
            let output_archive = required_path(
                args.next(),
                "content-maintain-segments-adaptive requires an output archive path",
            )?;
            let pressure = parse_required_scheduling_pressure(&mut args, "content maintenance")?;
            let segments = args.map(PathBuf::from).collect::<Vec<_>>();
            if segments.is_empty() {
                return Err(GfmError::Format(
                    "content-maintain-segments-adaptive requires at least one segment".to_string(),
                ));
            }
            let volume = detect_volume_id(&manifest_path)
                .ok()
                .or_else(|| parent_volume(&output_archive));
            let worker = BackgroundContentIndexer::default();
            let outcome = run_scheduled_volume_task(
                volume,
                Priority::Background,
                "content maintenance",
                pressure,
                move || {
                    worker.maintain_segments(
                        &manifest_path,
                        &output_archive,
                        &segments,
                        &ContentMaintenanceOptions::default(),
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
        Some("index-content-background") => {
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
            let pressure = parse_optional_scheduling_pressure(&mut args)?;
            let journal = JobJournal::new(default_job_journal_path());
            let spec = ContentIndexJobSpec::new(&root, segment_dir, records, content)
                .with_volume(detect_volume_id(&root)?);
            spec.write(default_content_job_path())?;
            let outcome = run_content_job(&spec, &journal, pressure)?;
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
        Some("resume-content-background") => {
            let spec_path = args
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(default_content_job_path);
            let journal = args
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(default_job_journal_path);
            let journal = JobJournal::new(journal);
            let recoverable = journal.recoverable(RetryPolicy { max_attempts: 2 })?;
            if recoverable.is_empty() {
                eprintln!("no recoverable background content jobs");
            } else {
                let spec = ContentIndexJobSpec::read(spec_path)?;
                let outcome = run_content_job(&spec, &journal, SchedulingPressure::default())?;
                if outcome.deferred {
                    eprintln!(
                        "resumed-background-content-deferred action={:?}; recoverable {}",
                        outcome.scheduling_action,
                        recoverable.len()
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
                        recoverable.len()
                    );
                }
            }
        }
        Some("resume-content-background-adaptive") => {
            let spec_path = required_path(
                args.next(),
                "resume-content-background-adaptive requires a content job spec path",
            )?;
            let journal_path = required_path(
                args.next(),
                "resume-content-background-adaptive requires a job journal path",
            )?;
            let pressure = parse_required_scheduling_pressure(&mut args, "resume content job")?;
            let journal = JobJournal::new(journal_path);
            let recoverable = journal.recoverable(RetryPolicy { max_attempts: 2 })?;
            if recoverable.is_empty() {
                eprintln!("no recoverable background content jobs");
            } else {
                let spec = ContentIndexJobSpec::read(spec_path)?;
                let outcome = run_content_job(&spec, &journal, pressure)?;
                if outcome.deferred {
                    eprintln!(
                        "resumed-background-content-deferred action={:?}; recoverable {}",
                        outcome.scheduling_action,
                        recoverable.len()
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
                        recoverable.len()
                    );
                }
            }
        }
        Some("search") => {
            let root = required_path(args.next(), "search requires a root path")?;
            let query = args.next().ok_or_else(|| {
                gfm_types::GfmError::Format("search requires a query string".to_string())
            })?;
            let snapshot = Indexer::default().build(root)?;
            for hit in snapshot.search(&query, 50) {
                print_hit(&hit);
            }
        }
        Some("search-stream") => {
            let root = required_path(args.next(), "search-stream requires a root path")?;
            let query = args.next().ok_or_else(|| {
                gfm_types::GfmError::Format("search-stream requires a query string".to_string())
            })?;
            let snapshot = Indexer::default().build(root)?;
            for batch in snapshot.stream_search(&query, 50)? {
                println!("batch\t{}", stream_stage(batch.stage));
                for hit in batch.hits {
                    print_hit(&hit);
                }
            }
        }
        Some("search-content") => {
            let root = required_path(args.next(), "search-content requires a root path")?;
            let query = args.next().ok_or_else(|| {
                gfm_types::GfmError::Format("search-content requires a query string".to_string())
            })?;
            let (indexed, hits) = run_content_search(root, query, Extractor::default())?;
            eprintln!("content-indexed {indexed} files");
            for hit in hits {
                print_hit(&hit);
            }
        }
        Some("search-content-adaptive") => {
            let root = required_path(args.next(), "search-content-adaptive requires a root path")?;
            let query = args.next().ok_or_else(|| {
                gfm_types::GfmError::Format(
                    "search-content-adaptive requires a query string".to_string(),
                )
            })?;
            let pressure = parse_required_scheduling_pressure(&mut args, "content search")?;
            let extractor =
                Extractor::with_budget_profile(extraction_budget_profile(&root, pressure));
            let (indexed, hits) = run_content_search(root, query, extractor)?;
            eprintln!("content-indexed {indexed} files");
            for hit in hits {
                print_hit(&hit);
            }
        }
        Some("search-index") => {
            let index_path = required_path(args.next(), "search-index requires an index path")?;
            let query = args.next().ok_or_else(|| {
                gfm_types::GfmError::Format("search-index requires a query string".to_string())
            })?;
            let snapshot = Indexer::default().load(index_path)?;
            for hit in snapshot.search(&query, 50) {
                print_hit(&hit);
            }
        }
        Some("search-index-mmap") => {
            let index_path =
                required_path(args.next(), "search-index-mmap requires an index path")?;
            let query = args.next().ok_or_else(|| {
                gfm_types::GfmError::Format("search-index-mmap requires a query string".to_string())
            })?;
            let live = LiveIndex::from_records(MmapRecordArchive::open(index_path)?.records()?);
            for hit in live.search(&query, 50) {
                print_hit(&hit);
            }
        }
        Some("search-index-columns") => {
            let records =
                required_path(args.next(), "search-index-columns requires a records path")?;
            let columns =
                required_path(args.next(), "search-index-columns requires a columns path")?;
            let query = args.next().ok_or_else(|| {
                gfm_types::GfmError::Format(
                    "search-index-columns requires a query string".to_string(),
                )
            })?;
            let records = MmapRecordArchive::open(records)?;
            let columns = MmapRecordColumns::open(columns)?;
            let mut search_columns = Vec::with_capacity(columns.len());
            for index in 0..columns.len() {
                let column = columns.column(index)?;
                search_columns.push(SearchRecordColumns {
                    id: column.id,
                    name: column.name,
                    path: column.path,
                    extension: column.extension,
                    tags: column.tags,
                    comment: column.comment,
                });
            }
            let (live, applied) =
                LiveIndex::from_records_with_columns(records.records()?, search_columns);
            eprintln!("columns-indexed {applied}");
            for hit in live.search(&query, 50) {
                print_hit(&hit);
            }
        }
        Some("search-index-sidecars") => {
            let records =
                required_path(args.next(), "search-index-sidecars requires a records path")?;
            let columns =
                required_path(args.next(), "search-index-sidecars requires a columns path")?;
            let metadata = required_path(
                args.next(),
                "search-index-sidecars requires a metadata path",
            )?;
            let prefixes = required_path(
                args.next(),
                "search-index-sidecars requires a prefixes path",
            )?;
            let substrings = required_path(
                args.next(),
                "search-index-sidecars requires a substrings path",
            )?;
            let fuzzy = required_path(args.next(), "search-index-sidecars requires a fuzzy path")?;
            let content =
                required_path(args.next(), "search-index-sidecars requires a content path")?;
            let query = args.next().ok_or_else(|| {
                gfm_types::GfmError::Format(
                    "search-index-sidecars requires a query string".to_string(),
                )
            })?;
            let records = MmapRecordArchive::open(records)?;
            let columns = MmapRecordColumns::open(columns)?;
            let metadata = MmapMetadataArchive::open(metadata)?;
            let substrings_archive = MmapSubstringArchive::open(&substrings)?;
            let lookup = SearchArchiveLookup::open(prefixes, substrings, fuzzy)?;
            let content = MmapContentArchive::open(content)?;
            let budget = SearchLookupBudget::default();
            let import = query_sidecar_imports(
                &metadata,
                &lookup,
                &substrings_archive,
                &content,
                &query,
                budget,
            )?;
            let (live, hydration) =
                LiveIndex::from_mmap_records_with_sidecar_import(&records, &columns, import)?;
            eprintln!(
                "columns-indexed {} records-loaded {} records-missing {} candidate-ids {} full-hydration {} metadata-keys {} prefix-keys {} substring-keys {} fuzzy-keys {} prefix-archive-keys {} substring-archive-keys {} fuzzy-archive-keys {} content-keys {} metadata-budget {} substring-budget {} content-budget {}",
                hydration.columns_applied,
                hydration.records_loaded,
                hydration.records_missing,
                hydration.import.candidate_ids,
                hydration.import.requires_full_record_hydration,
                hydration.metadata_keys,
                hydration.prefix_keys,
                hydration.substring_keys,
                hydration.fuzzy_keys,
                lookup.indexed_prefixes(),
                lookup.indexed_substring_grams(),
                lookup.indexed_fuzzy_keys(),
                hydration.content_keys,
                budget.max_metadata_ids_per_term,
                budget.max_substring_ids_per_gram,
                budget.max_content_ids_per_term
            );
            let report = live.search_with_lookup_budget(&query, 50, &lookup, budget)?;
            for hit in report.hits {
                print_hit(&hit);
            }
        }
        Some("search-index-sidecars-budget") => {
            let records = required_path(
                args.next(),
                "search-index-sidecars-budget requires a records path",
            )?;
            let columns = required_path(
                args.next(),
                "search-index-sidecars-budget requires a columns path",
            )?;
            let metadata = required_path(
                args.next(),
                "search-index-sidecars-budget requires a metadata path",
            )?;
            let prefixes = required_path(
                args.next(),
                "search-index-sidecars-budget requires a prefixes path",
            )?;
            let substrings = required_path(
                args.next(),
                "search-index-sidecars-budget requires a substrings path",
            )?;
            let fuzzy = required_path(
                args.next(),
                "search-index-sidecars-budget requires a fuzzy path",
            )?;
            let content = required_path(
                args.next(),
                "search-index-sidecars-budget requires a content path",
            )?;
            let max_prefix_ids = parse_usize_arg(
                args.next(),
                "search-index-sidecars-budget requires max-prefix-ids",
            )?;
            let max_substring_grams = parse_usize_arg(
                args.next(),
                "search-index-sidecars-budget requires max-substring-grams",
            )?;
            let max_substring_ids = parse_usize_arg(
                args.next(),
                "search-index-sidecars-budget requires max-substring-ids",
            )?;
            let max_fuzzy_keys = parse_usize_arg(
                args.next(),
                "search-index-sidecars-budget requires max-fuzzy-keys",
            )?;
            let max_fuzzy_terms = parse_usize_arg(
                args.next(),
                "search-index-sidecars-budget requires max-fuzzy-terms",
            )?;
            let max_fuzzy_candidates = parse_usize_arg(
                args.next(),
                "search-index-sidecars-budget requires max-fuzzy-candidates",
            )?;
            let max_content_ids = parse_usize_arg(
                args.next(),
                "search-index-sidecars-budget requires max-content-ids",
            )?;
            let query = args.next().ok_or_else(|| {
                gfm_types::GfmError::Format(
                    "search-index-sidecars-budget requires a query string".to_string(),
                )
            })?;
            let records = MmapRecordArchive::open(records)?;
            let columns = MmapRecordColumns::open(columns)?;
            let metadata = MmapMetadataArchive::open(metadata)?;
            let substrings_archive = MmapSubstringArchive::open(&substrings)?;
            let lookup = SearchArchiveLookup::open(prefixes, substrings, fuzzy)?;
            let content = MmapContentArchive::open(content)?;
            let budget = SearchLookupBudget {
                max_prefix_ids_per_term: max_prefix_ids,
                min_archive_prefix_chars: SearchLookupBudget::default().min_archive_prefix_chars,
                max_substring_grams_per_term: max_substring_grams,
                max_substring_ids_per_gram: max_substring_ids,
                max_fuzzy_keys_per_term: max_fuzzy_keys,
                max_fuzzy_terms_per_key: max_fuzzy_terms,
                max_fuzzy_candidates_per_term: max_fuzzy_candidates,
                max_metadata_ids_per_term: max_content_ids,
                max_content_ids_per_term: max_content_ids,
            };
            let import = query_sidecar_imports(
                &metadata,
                &lookup,
                &substrings_archive,
                &content,
                &query,
                budget,
            )?;
            let (live, hydration) =
                LiveIndex::from_mmap_records_with_sidecar_import(&records, &columns, import)?;
            let report = live.search_with_lookup_budget(&query, 50, &lookup, budget)?;
            eprintln!(
                "sidecar-budget\tcolumns-indexed={}\trecords-loaded={}\trecords-missing={}\tcandidate-ids={}\tfull-hydration={}\tmetadata-keys={}\tprefix-keys={}\tsubstring-keys={}\tfuzzy-keys={}\tcontent-keys={}\tmetadata-budget={max_content_ids}\tcontent-budget={max_content_ids}\tprefix-archive-keys={}\tsubstring-archive-keys={}\tfuzzy-archive-keys={}\tprefix-terms={}\tprefix-lookup-requests={}\tprefix-lookup-ids={}\tprefix-candidate-ids={}\tprefix-cache-hits={}\tprefix-cache-misses={}\tprefix-cutoff-terms={}\tprefix-truncated-terms={}\tsubstring-terms={}\tsubstring-grams={}\tsubstring-lookup-requests={}\tsubstring-lookup-ids={}\tsubstring-candidate-ids={}\tsubstring-cache-hits={}\tsubstring-cache-misses={}\tsubstring-cutoff-terms={}\tsubstring-term-truncated-grams={}\tsubstring-truncated-grams={}\tfuzzy-terms={}\tfuzzy-keys-read={}\tfuzzy-lookup-requests={}\tfuzzy-lookup-terms={}\tfuzzy-candidate-terms={}\tfuzzy-verified-candidates={}\tfuzzy-cache-hits={}\tfuzzy-cache-misses={}\tfuzzy-key-truncated-terms={}\tfuzzy-term-truncated-keys={}\tfuzzy-candidate-truncated-terms={}",
                hydration.columns_applied,
                hydration.records_loaded,
                hydration.records_missing,
                hydration.import.candidate_ids,
                hydration.import.requires_full_record_hydration,
                hydration.metadata_keys,
                hydration.prefix_keys,
                hydration.substring_keys,
                hydration.fuzzy_keys,
                hydration.content_keys,
                lookup.indexed_prefixes(),
                lookup.indexed_substring_grams(),
                lookup.indexed_fuzzy_keys(),
                report.lookup.prefix_terms,
                report.lookup.prefix_lookup_requests,
                report.lookup.prefix_lookup_ids,
                report.lookup.prefix_candidate_ids,
                report.lookup.prefix_cache_hits,
                report.lookup.prefix_cache_misses,
                report.lookup.prefix_cutoff_terms,
                report.lookup.prefix_truncated_terms,
                report.lookup.substring_terms,
                report.lookup.substring_grams,
                report.lookup.substring_lookup_requests,
                report.lookup.substring_lookup_ids,
                report.lookup.substring_candidate_ids,
                report.lookup.substring_cache_hits,
                report.lookup.substring_cache_misses,
                report.lookup.substring_cutoff_terms,
                report.lookup.substring_term_truncated_grams,
                report.lookup.substring_truncated_grams,
                report.lookup.fuzzy_terms,
                report.lookup.fuzzy_keys,
                report.lookup.fuzzy_lookup_requests,
                report.lookup.fuzzy_lookup_terms,
                report.lookup.fuzzy_candidate_terms,
                report.lookup.fuzzy_verified_candidates,
                report.lookup.fuzzy_cache_hits,
                report.lookup.fuzzy_cache_misses,
                report.lookup.fuzzy_key_truncated_terms,
                report.lookup.fuzzy_term_truncated_keys,
                report.lookup.fuzzy_candidate_truncated_terms
            );
            for hit in report.hits {
                print_hit(&hit);
            }
        }
        Some("index-footprint") => {
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
        Some("index-compaction-plan") => {
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
        Some("records-verify") => {
            let records = required_path(args.next(), "records-verify requires a records path")?;
            let archive = MmapRecordArchive::open(records)?;
            println!(
                "records-verify\trecords={}\tbytes={}\tchecksum={}",
                archive.len(),
                archive.mapped_len(),
                if archive.is_checksummed() {
                    "verified"
                } else {
                    "legacy"
                }
            );
        }
        Some("archive-schema") => {
            let kind = args
                .next()
                .and_then(|kind| ArchiveSchemaKind::parse(&kind))
                .ok_or_else(|| {
                    GfmError::Format(
                        "archive-schema requires records, columns, metadata, prefixes, substrings, fuzzy, dictionary, content, or content-manifest".to_string(),
                    )
                })?;
            let path = required_path(args.next(), "archive-schema requires an archive path")?;
            println!("{}", inspect_archive_schema(kind, path).as_tsv());
        }
        Some("archive-rebuild-plan") => {
            let records =
                required_path(args.next(), "archive-rebuild-plan requires a records path")?;
            let columns =
                required_path(args.next(), "archive-rebuild-plan requires a columns path")?;
            let metadata =
                required_path(args.next(), "archive-rebuild-plan requires a metadata path")?;
            let prefixes =
                required_path(args.next(), "archive-rebuild-plan requires a prefixes path")?;
            let substrings = required_path(
                args.next(),
                "archive-rebuild-plan requires a substrings path",
            )?;
            let fuzzy = required_path(args.next(), "archive-rebuild-plan requires a fuzzy path")?;
            let dictionary = required_path(
                args.next(),
                "archive-rebuild-plan requires a dictionary path",
            )?;
            let content =
                required_path(args.next(), "archive-rebuild-plan requires a content path")?;
            let manifest = required_path(
                args.next(),
                "archive-rebuild-plan requires a content manifest path",
            )?;
            let discovered_archives = args
                .map(|spec| parse_content_manifest_archive_spec(&spec))
                .collect::<Result<Vec<_>>>()?;
            let inputs = ArchiveRebuildInputs {
                records_path: records,
                columns_path: columns,
                metadata_path: metadata,
                prefixes_path: prefixes,
                substrings_path: substrings,
                fuzzy_path: fuzzy,
                dictionary_path: dictionary,
                content_path: content,
                manifest_path: manifest,
                discovered_content_archives: discovered_archives,
            };
            for line in plan_archive_rebuilds(&inputs).as_tsv_lines() {
                println!("{line}");
            }
        }
        Some("records-migration-plan") => {
            let records = required_path(
                args.next(),
                "records-migration-plan requires a records path",
            )?;
            println!("{}", plan_record_archive_migration(records).as_tsv());
        }
        Some("records-migrate") => {
            let records = required_path(args.next(), "records-migrate requires a records path")?;
            let backup_dir =
                required_path(args.next(), "records-migrate requires a backup directory")?;
            let migration = migrate_record_archive(records, backup_dir)?;
            println!("{}", migration.as_tsv());
        }
        Some("content-migration-plan") => {
            let content = required_path(
                args.next(),
                "content-migration-plan requires a content path",
            )?;
            println!("{}", plan_content_archive_migration(content).as_tsv());
        }
        Some("content-migrate") => {
            let content = required_path(args.next(), "content-migrate requires a content path")?;
            let backup_dir =
                required_path(args.next(), "content-migrate requires a backup directory")?;
            let migration = migrate_content_archive(content, backup_dir)?;
            println!("{}", migration.as_tsv());
        }
        Some("metadata-migration-plan") => {
            let metadata = required_path(
                args.next(),
                "metadata-migration-plan requires a metadata path",
            )?;
            println!("{}", plan_metadata_archive_migration(metadata).as_tsv());
        }
        Some("metadata-migrate") => {
            let metadata = required_path(args.next(), "metadata-migrate requires a metadata path")?;
            let backup_dir =
                required_path(args.next(), "metadata-migrate requires a backup directory")?;
            let migration = migrate_metadata_archive(metadata, backup_dir)?;
            println!("{}", migration.as_tsv());
        }
        Some("columns-rebuild-plan") => {
            let records =
                required_path(args.next(), "columns-rebuild-plan requires a records path")?;
            let columns =
                required_path(args.next(), "columns-rebuild-plan requires a columns path")?;
            println!(
                "{}",
                plan_columns_archive_rebuild(records, columns).as_tsv()
            );
        }
        Some("columns-rebuild") => {
            let records = required_path(args.next(), "columns-rebuild requires a records path")?;
            let columns = required_path(args.next(), "columns-rebuild requires a columns path")?;
            let backup_dir =
                required_path(args.next(), "columns-rebuild requires a backup directory")?;
            let rebuild = rebuild_columns_archive(records, columns, backup_dir)?;
            println!("{}", rebuild.as_tsv());
        }
        Some("derived-sidecar-rebuild-plan") => {
            let records = required_path(
                args.next(),
                "derived-sidecar-rebuild-plan requires a records path",
            )?;
            let kind = parse_sidecar_kind(args.next(), "derived-sidecar-rebuild-plan")?;
            let sidecar = required_path(
                args.next(),
                "derived-sidecar-rebuild-plan requires a sidecar path",
            )?;
            println!(
                "{}",
                plan_derived_sidecar_rebuild(records, kind, sidecar).as_tsv()
            );
        }
        Some("derived-sidecar-rebuild") => {
            let records = required_path(
                args.next(),
                "derived-sidecar-rebuild requires a records path",
            )?;
            let kind = parse_sidecar_kind(args.next(), "derived-sidecar-rebuild")?;
            let sidecar = required_path(
                args.next(),
                "derived-sidecar-rebuild requires a sidecar path",
            )?;
            let backup_dir = required_path(
                args.next(),
                "derived-sidecar-rebuild requires a backup directory",
            )?;
            let rebuild = rebuild_derived_sidecar(records, kind, sidecar, backup_dir)?;
            println!("{}", rebuild.as_tsv());
        }
        Some("index-columns") => {
            let records = required_path(args.next(), "index-columns requires a records path")?;
            let output =
                required_path(args.next(), "index-columns requires an output columns path")?;
            let archive = MmapRecordArchive::open(records)?;
            let records = archive.records()?;
            write_record_columns(output, &records)?;
            eprintln!("columns-indexed {} records", records.len());
        }
        Some("columns-verify") => {
            let columns = required_path(args.next(), "columns-verify requires a columns path")?;
            let archive = MmapRecordColumns::open(columns)?;
            println!(
                "columns-verify\trecords={}\tbytes={}\tchecksum={}",
                archive.len(),
                archive.mapped_len(),
                if archive.is_checksummed() {
                    "verified"
                } else {
                    "legacy"
                }
            );
        }
        Some("columns-lookup") => {
            let columns = required_path(args.next(), "columns-lookup requires a columns path")?;
            let volume = parse_u64_arg(args.next(), "columns-lookup requires a volume id")?;
            let node = parse_u64_arg(args.next(), "columns-lookup requires a node id")?;
            let archive = MmapRecordColumns::open(columns)?;
            match archive.find(FileId::new(VolumeId(volume), node))? {
                Some(column) => println!(
                    "columns\tfound\tid={}:{}\tname={}\text={}\ttags={}\tcomment={}\tpath={}",
                    column.id.volume.0,
                    column.id.node,
                    column.name,
                    column.extension.as_deref().unwrap_or(""),
                    column.tags.join(","),
                    column.comment.as_deref().unwrap_or(""),
                    column.path
                ),
                None => println!("columns\tmissing\tid={volume}:{node}"),
            }
        }
        Some("index-metadata") => {
            let records = required_path(args.next(), "index-metadata requires a records path")?;
            let output = required_path(
                args.next(),
                "index-metadata requires an output metadata path",
            )?;
            let archive = MmapRecordArchive::open(records)?;
            let postings = metadata_postings_from_records(&archive.records()?);
            write_metadata_postings(output, &postings)?;
            eprintln!("metadata-indexed {} terms", postings.len());
        }
        Some("index-dictionary") => {
            let records = required_path(args.next(), "index-dictionary requires a records path")?;
            let output = required_path(
                args.next(),
                "index-dictionary requires an output dictionary path",
            )?;
            let archive = MmapRecordArchive::open(records)?;
            let report = dictionary_term_report_from_records(&archive.records()?);
            write_dictionary(output, &report.terms)?;
            eprintln!(
                "dictionary-indexed\tterms={}\tpaths={}\tpath-prefixes={}\textensions={}\ttags={}\tkinds={}\tmetadata-keys={}\tcomment-tokens={}",
                report.terms.len(),
                report.paths,
                report.path_prefixes,
                report.extensions,
                report.tags,
                report.kinds,
                report.metadata_keys,
                report.comment_tokens
            );
        }
        Some("index-prefixes") => {
            let records = required_path(args.next(), "index-prefixes requires a records path")?;
            let output =
                required_path(args.next(), "index-prefixes requires an output prefix path")?;
            let archive = MmapRecordArchive::open(records)?;
            let postings = prefix_postings_from_records(&archive.records()?);
            write_prefix_postings(output, &postings)?;
            eprintln!("prefixes-indexed {} prefixes", postings.len());
        }
        Some("index-substrings") => {
            let records = required_path(args.next(), "index-substrings requires a records path")?;
            let output = required_path(
                args.next(),
                "index-substrings requires an output substring path",
            )?;
            let archive = MmapRecordArchive::open(records)?;
            let postings = substring_postings_from_records(&archive.records()?);
            write_substring_postings(output, &postings)?;
            eprintln!("substrings-indexed {} grams", postings.len());
        }
        Some("index-fuzzy") => {
            let records = required_path(args.next(), "index-fuzzy requires a records path")?;
            let output = required_path(args.next(), "index-fuzzy requires an output fuzzy path")?;
            let archive = MmapRecordArchive::open(records)?;
            let postings = fuzzy_postings_from_records(&archive.records()?);
            write_fuzzy_postings(output, &postings)?;
            eprintln!("fuzzy-indexed {} keys", postings.len());
        }
        Some("sidecar-recovery-plan") => {
            let records =
                required_path(args.next(), "sidecar-recovery-plan requires a records path")?;
            let sidecars = parse_sidecar_paths(&mut args, "sidecar-recovery-plan")?;
            let plan = plan_sidecar_recovery(&records, &sidecars);
            println!("{}", plan.as_tsv());
            print_sidecar_health("invalid", &plan.invalid_sidecars);
        }
        Some("sidecar-recover") => {
            let records = required_path(args.next(), "sidecar-recover requires a records path")?;
            let quarantine = required_path(
                args.next(),
                "sidecar-recover requires a quarantine directory",
            )?;
            let sidecars = parse_sidecar_paths(&mut args, "sidecar-recover")?;
            let volume = detect_volume_id(&records)
                .ok()
                .or_else(|| parent_volume(&records));
            let report = run_volume_task(volume, Priority::Visible, "sidecar repair", move || {
                recover_sidecars(&records, &sidecars, &quarantine)
            })?;
            print_sidecar_recovery_report(report);
        }
        Some("sidecar-recover-adaptive") => {
            let records = required_path(
                args.next(),
                "sidecar-recover-adaptive requires a records path",
            )?;
            let quarantine = required_path(
                args.next(),
                "sidecar-recover-adaptive requires a quarantine directory",
            )?;
            let pressure = parse_required_scheduling_pressure(&mut args, "sidecar repair")?;
            let sidecars = parse_sidecar_paths(&mut args, "sidecar-recover-adaptive")?;
            let volume = detect_volume_id(&records)
                .ok()
                .or_else(|| parent_volume(&records));
            let outcome = run_scheduled_volume_task(
                volume,
                Priority::Background,
                "sidecar repair",
                pressure,
                move || recover_sidecars(&records, &sidecars, &quarantine),
            )?;
            if outcome.deferred {
                eprintln!(
                    "sidecar-recovery-deferred\taction={:?}",
                    outcome.scheduling_action
                );
            } else {
                let report = outcome.result.ok_or_else(|| {
                    GfmError::Format("sidecar repair ran without a report".to_string())
                })?;
                eprintln!("sidecar-recovery-action\t{:?}", outcome.scheduling_action);
                print_sidecar_recovery_report(report);
            }
        }
        Some("fuzzy-terms-mmap") => {
            let fuzzy = required_path(args.next(), "fuzzy-terms-mmap requires a fuzzy path")?;
            let key = args
                .next()
                .ok_or_else(|| GfmError::Format("fuzzy-terms-mmap requires a key".to_string()))?;
            let archive = MmapFuzzyArchive::open(fuzzy)?;
            for term in archive.terms_for(&key)? {
                println!("{term}");
            }
        }
        Some("fuzzy-verify") => {
            let fuzzy = required_path(args.next(), "fuzzy-verify requires a fuzzy path")?;
            let archive = MmapFuzzyArchive::open(fuzzy)?;
            println!(
                "fuzzy-verify\tkeys={}\tbytes={}\tchecksum={}",
                archive.indexed_keys(),
                archive.mapped_len(),
                archive.is_checksummed()
            );
        }
        Some("prefix-ids-mmap") => {
            let prefixes = required_path(args.next(), "prefix-ids-mmap requires a prefix path")?;
            let prefix = args
                .next()
                .ok_or_else(|| GfmError::Format("prefix-ids-mmap requires a prefix".to_string()))?;
            let archive = MmapPrefixArchive::open(prefixes)?;
            for id in archive.ids_for(&prefix)? {
                println!("{}\t{}", id.volume.0, id.node);
            }
        }
        Some("prefix-id-block-mmap") => {
            let prefixes =
                required_path(args.next(), "prefix-id-block-mmap requires a prefix path")?;
            let prefix = args.next().ok_or_else(|| {
                GfmError::Format("prefix-id-block-mmap requires a prefix".to_string())
            })?;
            let block_index = args
                .next()
                .ok_or_else(|| {
                    GfmError::Format("prefix-id-block-mmap requires a block index".to_string())
                })?
                .parse::<usize>()
                .map_err(|err| GfmError::Format(format!("invalid prefix block index: {err}")))?;
            let archive = MmapPrefixArchive::open(prefixes)?;
            for id in archive.id_block_for(&prefix, block_index)? {
                println!("{}\t{}", id.volume.0, id.node);
            }
        }
        Some("prefix-verify") => {
            let prefixes = required_path(args.next(), "prefix-verify requires a prefix path")?;
            let archive = MmapPrefixArchive::open(prefixes)?;
            println!(
                "prefix-verify\tprefixes={}\tbytes={}\tchecksum={}",
                archive.indexed_prefixes(),
                archive.mapped_len(),
                archive.is_checksummed()
            );
        }
        Some("substring-ids-mmap") => {
            let substrings =
                required_path(args.next(), "substring-ids-mmap requires a substring path")?;
            let gram = args.next().ok_or_else(|| {
                GfmError::Format("substring-ids-mmap requires a trigram".to_string())
            })?;
            let archive = MmapSubstringArchive::open(substrings)?;
            for id in archive.ids_for(&gram)? {
                println!("{}\t{}", id.volume.0, id.node);
            }
        }
        Some("substring-id-block-mmap") => {
            let substrings = required_path(
                args.next(),
                "substring-id-block-mmap requires a substring path",
            )?;
            let gram = args.next().ok_or_else(|| {
                GfmError::Format("substring-id-block-mmap requires a trigram".to_string())
            })?;
            let block_index = args
                .next()
                .ok_or_else(|| {
                    GfmError::Format("substring-id-block-mmap requires a block index".to_string())
                })?
                .parse::<usize>()
                .map_err(|err| GfmError::Format(format!("invalid substring block index: {err}")))?;
            let archive = MmapSubstringArchive::open(substrings)?;
            for id in archive.id_block_for(&gram, block_index)? {
                println!("{}\t{}", id.volume.0, id.node);
            }
        }
        Some("substring-verify") => {
            let substrings =
                required_path(args.next(), "substring-verify requires a substring path")?;
            let archive = MmapSubstringArchive::open(substrings)?;
            println!(
                "substring-verify\tgrams={}\tbytes={}\tchecksum={}",
                archive.indexed_grams(),
                archive.mapped_len(),
                archive.is_checksummed()
            );
        }
        Some("dictionary-lookup") => {
            let dictionary =
                required_path(args.next(), "dictionary-lookup requires a dictionary path")?;
            let term = args
                .next()
                .ok_or_else(|| GfmError::Format("dictionary-lookup requires a term".to_string()))?;
            let archive = MmapDictionary::open(dictionary)?;
            match archive.find(&term)? {
                Some(index) => println!("dictionary\tfound\tindex={index}\tterm={term}"),
                None => println!("dictionary\tmissing\tterm={term}"),
            }
        }
        Some("dictionary-verify") => {
            let dictionary =
                required_path(args.next(), "dictionary-verify requires a dictionary path")?;
            let archive = MmapDictionary::open(dictionary)?;
            println!(
                "dictionary-verify\tterms={}\tbytes={}\tchecksum={}",
                archive.len(),
                archive.mapped_len(),
                if archive.is_checksummed() {
                    "verified"
                } else {
                    "legacy"
                }
            );
        }
        Some("metadata-ids-mmap") => {
            let metadata =
                required_path(args.next(), "metadata-ids-mmap requires a metadata path")?;
            let field = parse_metadata_field(
                args.next().as_deref().ok_or_else(|| {
                    GfmError::Format("metadata-ids-mmap requires a field".to_string())
                })?,
                "metadata field",
            )?;
            let term = args
                .next()
                .ok_or_else(|| GfmError::Format("metadata-ids-mmap requires a term".to_string()))?;
            let archive = MmapMetadataArchive::open(metadata)?;
            for id in archive.ids_for(field, &term)? {
                println!("{}\t{}", id.volume.0, id.node);
            }
        }
        Some("metadata-id-block-mmap") => {
            let metadata = required_path(
                args.next(),
                "metadata-id-block-mmap requires a metadata path",
            )?;
            let field = parse_metadata_field(
                args.next().as_deref().ok_or_else(|| {
                    GfmError::Format("metadata-id-block-mmap requires a field".to_string())
                })?,
                "metadata field",
            )?;
            let term = args.next().ok_or_else(|| {
                GfmError::Format("metadata-id-block-mmap requires a term".to_string())
            })?;
            let block_index = args
                .next()
                .ok_or_else(|| {
                    GfmError::Format("metadata-id-block-mmap requires a block index".to_string())
                })?
                .parse::<usize>()
                .map_err(|err| GfmError::Format(format!("invalid metadata block index: {err}")))?;
            let archive = MmapMetadataArchive::open(metadata)?;
            for id in archive.id_block_for(field, &term, block_index)? {
                println!("{}\t{}", id.volume.0, id.node);
            }
        }
        Some("metadata-verify") => {
            let metadata = required_path(args.next(), "metadata-verify requires a metadata path")?;
            let archive = MmapMetadataArchive::open(metadata)?;
            println!(
                "metadata-verify\tterms={}\tbytes={}\tchecksum={}",
                archive.indexed_terms(),
                archive.mapped_len(),
                if archive.is_checksummed() {
                    "verified"
                } else {
                    "legacy"
                }
            );
        }
        Some("search-content-index") => {
            let records =
                required_path(args.next(), "search-content-index requires a records path")?;
            let content =
                required_path(args.next(), "search-content-index requires a content path")?;
            let query = args.next().ok_or_else(|| {
                gfm_types::GfmError::Format(
                    "search-content-index requires a query string".to_string(),
                )
            })?;
            let (live, report) =
                Indexer::default().load_live_with_content_for_query(records, content, &query)?;
            eprintln!(
                "content-keys {} records-loaded {} records-missing {} candidate-ids {} full-hydration {}",
                report.content_keys,
                report.records_loaded,
                report.records_missing,
                report.candidate_ids,
                report.full_hydration
            );
            for hit in live.search_with_snippets(&query, 50, &Extractor::default(), 96)? {
                print_hit(&hit);
            }
        }
        Some("search-content-index-adaptive") => {
            let records = required_path(
                args.next(),
                "search-content-index-adaptive requires a records path",
            )?;
            let content = required_path(
                args.next(),
                "search-content-index-adaptive requires a content path",
            )?;
            let query = args.next().ok_or_else(|| {
                gfm_types::GfmError::Format(
                    "search-content-index-adaptive requires a query string".to_string(),
                )
            })?;
            let pressure = parse_required_scheduling_pressure(&mut args, "content index search")?;
            let root = records
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            let extractor =
                Extractor::with_budget_profile(extraction_budget_profile(&root, pressure));
            let (live, report) =
                Indexer::default().load_live_with_content_for_query(records, content, &query)?;
            eprintln!(
                "content-keys {} records-loaded {} records-missing {} candidate-ids {} full-hydration {}",
                report.content_keys,
                report.records_loaded,
                report.records_missing,
                report.candidate_ids,
                report.full_hydration
            );
            for hit in live.search_with_snippets(&query, 50, &extractor, 96)? {
                print_hit(&hit);
            }
        }
        Some("search-content-index-set") => {
            let records = required_path(
                args.next(),
                "search-content-index-set requires a records path",
            )?;
            let query = args.next().ok_or_else(|| {
                gfm_types::GfmError::Format(
                    "search-content-index-set requires a query string".to_string(),
                )
            })?;
            let content_paths: Vec<PathBuf> = args.map(PathBuf::from).collect();
            if content_paths.is_empty() {
                return Err(gfm_types::GfmError::Format(
                    "search-content-index-set requires at least one content archive".to_string(),
                ));
            }
            let (live, report) =
                Indexer::default().load_live_with_content_set(records, &content_paths, &query)?;
            eprintln!(
                "content-archives {} content-keys {} records-loaded {} records-missing {} candidate-ids {} full-hydration {}",
                content_paths.len(),
                report.content_keys,
                report.records_loaded,
                report.records_missing,
                report.candidate_ids,
                report.full_hydration
            );
            for hit in live.search(&query, 50) {
                print_hit(&hit);
            }
        }
        Some("search-content-index-manifest") => {
            let records = required_path(
                args.next(),
                "search-content-index-manifest requires a records path",
            )?;
            let manifest = required_path(
                args.next(),
                "search-content-index-manifest requires a manifest path",
            )?;
            let query = args.next().ok_or_else(|| {
                gfm_types::GfmError::Format(
                    "search-content-index-manifest requires a query string".to_string(),
                )
            })?;
            let (live, report) =
                Indexer::default().load_live_with_content_manifest(records, manifest, &query)?;
            eprintln!(
                "content-manifest-keys {} records-loaded {} records-missing {} candidate-ids {} full-hydration {}",
                report.content_keys,
                report.records_loaded,
                report.records_missing,
                report.candidate_ids,
                report.full_hydration
            );
            for hit in live.search(&query, 50) {
                print_hit(&hit);
            }
        }
        Some("content-ids") => {
            let content = required_path(args.next(), "content-ids requires a content path")?;
            let term = args.next().ok_or_else(|| {
                gfm_types::GfmError::Format("content-ids requires a term".to_string())
            })?;
            let mut archive = ContentArchive::open(content)?;
            for id in archive.ids_for_term(&term)? {
                println!("{}\t{}", id.volume.0, id.node);
            }
        }
        Some("content-ids-mmap") => {
            let content = required_path(args.next(), "content-ids-mmap requires a content path")?;
            let term = args.next().ok_or_else(|| {
                gfm_types::GfmError::Format("content-ids-mmap requires a term".to_string())
            })?;
            let archive = MmapContentArchive::open(content)?;
            for id in archive.ids_for_term(&term)? {
                println!("{}\t{}", id.volume.0, id.node);
            }
        }
        Some("content-ids-mmap-set") => {
            let term = args.next().ok_or_else(|| {
                gfm_types::GfmError::Format("content-ids-mmap-set requires a term".to_string())
            })?;
            let content_paths: Vec<PathBuf> = args.map(PathBuf::from).collect();
            if content_paths.is_empty() {
                return Err(gfm_types::GfmError::Format(
                    "content-ids-mmap-set requires at least one content archive".to_string(),
                ));
            }
            let archive = MmapContentSet::open(&content_paths)?;
            for id in archive.ids_for_term(&term)? {
                println!("{}\t{}", id.volume.0, id.node);
            }
        }
        Some("content-ids-mmap-manifest") => {
            let manifest = required_path(
                args.next(),
                "content-ids-mmap-manifest requires a manifest path",
            )?;
            let term = args.next().ok_or_else(|| {
                gfm_types::GfmError::Format("content-ids-mmap-manifest requires a term".to_string())
            })?;
            let archive = MmapContentSet::open_manifest(manifest)?;
            for id in archive.ids_for_term(&term)? {
                println!("{}\t{}", id.volume.0, id.node);
            }
        }
        Some("content-id-block-mmap") => {
            let content =
                required_path(args.next(), "content-id-block-mmap requires a content path")?;
            let term = args.next().ok_or_else(|| {
                gfm_types::GfmError::Format("content-id-block-mmap requires a term".to_string())
            })?;
            let block_index = args
                .next()
                .ok_or_else(|| {
                    GfmError::Format("content-id-block-mmap requires a block index".to_string())
                })?
                .parse::<usize>()
                .map_err(|err| GfmError::Format(format!("invalid content block index: {err}")))?;
            let archive = MmapContentArchive::open(content)?;
            for id in archive.id_block_for_term(&term, block_index)? {
                println!("{}\t{}", id.volume.0, id.node);
            }
        }
        Some("content-verify") => {
            let content = required_path(args.next(), "content-verify requires a content path")?;
            let archive = MmapContentArchive::open(content)?;
            println!(
                "content-verify\tterms={}\tbytes={}\tchecksum={}",
                archive.indexed_terms(),
                archive.mapped_len(),
                if archive.is_checksummed() {
                    "verified"
                } else {
                    "legacy"
                }
            );
        }
        Some("config-path") => {
            println!("{}", ConfigStore::platform_default()?.path().display());
        }
        Some("config-init") => {
            let store = config_store(args.next())?;
            let config = store.load_or_create_default()?;
            println!("{}\t{}", config.schema_version, store.path().display());
        }
        Some("config-check") => {
            let store = config_store(args.next())?;
            let config = store.load()?;
            config.validate()?;
            println!("{}\t{}", config.schema_version, store.path().display());
        }
        Some("config-dump") => {
            let store = config_store(args.next())?;
            let config = store.load_or_create_default()?;
            print!("{}", config.to_toml()?);
        }
        Some("diagnostics-index-rebuild") => {
            let root = required_path(
                args.next(),
                "diagnostics-index-rebuild requires a root path",
            )?;
            let records = required_path(
                args.next(),
                "diagnostics-index-rebuild requires a records path",
            )?;
            let spec = match args.next() {
                Some(content) => RebuildSpec::with_content(root, records, PathBuf::from(content)),
                None => RebuildSpec::records(root, records),
            };
            let volume = detect_volume_id(&spec.root)
                .ok()
                .or_else(|| parent_volume(&spec.records_path));
            let report = run_volume_task(volume, Priority::Visible, "index rebuild", move || {
                rebuild_index(&spec)
            })?;
            print_index_rebuild_report(report);
        }
        Some("diagnostics-index-rebuild-adaptive") => {
            let root = required_path(
                args.next(),
                "diagnostics-index-rebuild-adaptive requires a root path",
            )?;
            let records = required_path(
                args.next(),
                "diagnostics-index-rebuild-adaptive requires a records path",
            )?;
            let pressure = parse_required_scheduling_pressure(&mut args, "index rebuild")?;
            let spec = match args.next() {
                Some(content) => RebuildSpec::with_content(root, records, PathBuf::from(content)),
                None => RebuildSpec::records(root, records),
            };
            let volume = detect_volume_id(&spec.root)
                .ok()
                .or_else(|| parent_volume(&spec.records_path));
            let outcome = run_scheduled_volume_task(
                volume,
                Priority::Background,
                "index rebuild",
                pressure,
                move || rebuild_index(&spec),
            )?;
            if outcome.deferred {
                eprintln!(
                    "index-rebuild-deferred\taction={:?}",
                    outcome.scheduling_action
                );
            } else {
                let report = outcome.result.ok_or_else(|| {
                    GfmError::Format("index rebuild ran without a report".to_string())
                })?;
                eprintln!("index-rebuild-action\t{:?}", outcome.scheduling_action);
                print_index_rebuild_report(report);
            }
        }
        Some("diagnostics-index-recovery-plan") => {
            let root = required_path(
                args.next(),
                "diagnostics-index-recovery-plan requires a root path",
            )?;
            let records = required_path(
                args.next(),
                "diagnostics-index-recovery-plan requires a records path",
            )?;
            let state = required_path(
                args.next(),
                "diagnostics-index-recovery-plan requires a state path",
            )?;
            let quarantine = args
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(|| records.with_extension("quarantine"));
            let spec = PersistentIndexRecoverySpec::new(root, records, state, quarantine);
            println!("{}", plan_index_recovery(&spec).as_tsv());
        }
        Some("diagnostics-index-recover") => {
            let root = required_path(
                args.next(),
                "diagnostics-index-recover requires a root path",
            )?;
            let records = required_path(
                args.next(),
                "diagnostics-index-recover requires a records path",
            )?;
            let state = required_path(
                args.next(),
                "diagnostics-index-recover requires a state path",
            )?;
            let quarantine = args
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(|| records.with_extension("quarantine"));
            let spec = PersistentIndexRecoverySpec::new(root, records, state, quarantine);
            let volume = detect_volume_id(&spec.root)
                .ok()
                .or_else(|| parent_volume(&spec.records_path));
            let report = run_volume_task(
                volume,
                Priority::Visible,
                "persistent index repair",
                move || recover_index(&spec),
            )?;
            print_persistent_index_recovery_report(report);
        }
        Some("diagnostics-index-recover-adaptive") => {
            let root = required_path(
                args.next(),
                "diagnostics-index-recover-adaptive requires a root path",
            )?;
            let records = required_path(
                args.next(),
                "diagnostics-index-recover-adaptive requires a records path",
            )?;
            let state = required_path(
                args.next(),
                "diagnostics-index-recover-adaptive requires a state path",
            )?;
            let pressure =
                parse_required_scheduling_pressure(&mut args, "persistent index repair")?;
            let quarantine = args
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(|| records.with_extension("quarantine"));
            let spec = PersistentIndexRecoverySpec::new(root, records, state, quarantine);
            let volume = detect_volume_id(&spec.root)
                .ok()
                .or_else(|| parent_volume(&spec.records_path));
            let outcome = run_scheduled_volume_task(
                volume,
                Priority::Background,
                "persistent index repair",
                pressure,
                move || recover_index(&spec),
            )?;
            if outcome.deferred {
                eprintln!(
                    "persistent-index-recovery-deferred\taction={:?}",
                    outcome.scheduling_action
                );
            } else {
                let report = outcome.result.ok_or_else(|| {
                    GfmError::Format("persistent index repair ran without a report".to_string())
                })?;
                eprintln!(
                    "persistent-index-recovery-action\t{:?}",
                    outcome.scheduling_action
                );
                print_persistent_index_recovery_report(report);
            }
        }
        Some("diagnostics-trace-export") => {
            let output = required_path(
                args.next(),
                "diagnostics-trace-export requires an output path",
            )?;
            let report = export_operator_trace(output)?;
            println!("{}\t{}", report.path.display(), report.bytes_written);
        }
        Some("diagnostics-parity-baseline") => {
            let store = config_store(args.next())?;
            let baseline = required_path(
                args.next(),
                "diagnostics-parity-baseline requires a baseline root",
            )?;
            let macos_build = args.next().ok_or_else(|| {
                gfm_types::GfmError::Format(
                    "diagnostics-parity-baseline requires a macOS build".to_string(),
                )
            })?;
            let report = select_parity_baseline(&store, baseline, macos_build)?;
            println!(
                "{}\t{}\t{}",
                report.config_path.display(),
                report.baseline_root.display(),
                report.macos_build
            );
        }
        Some("diagnostics-storage-inspect") => {
            let storage = required_path(
                args.next(),
                "diagnostics-storage-inspect requires a storage path",
            )?;
            match inspect_storage(storage)? {
                StorageInspection::Records(report) => println!(
                    "records\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    report.path.display(),
                    report.bytes,
                    report.records,
                    report.files,
                    report.directories,
                    report.symlinks,
                    report.hidden,
                    report.tagged
                ),
                StorageInspection::Content(report) => println!(
                    "content\t{}\t{}\t{}",
                    report.path.display(),
                    report.bytes,
                    report.terms
                ),
            }
        }
        Some("support-check") => {
            let matrix = SupportMatrix::default();
            let host = current_host_profile()?;
            let evaluation = matrix.evaluate(&host);
            println!(
                "{}\t{}.{}.{}\t{}\t{}\t{}\t{}",
                evaluation.tier.as_str(),
                host.macos_version.major,
                host.macos_version.minor,
                host.macos_version.patch,
                host.build,
                host.hardware.architecture.as_str(),
                host.hardware.memory_bytes,
                host.hardware.logical_cpus
            );
            for reason in evaluation.reasons {
                eprintln!("unsupported\t{reason}");
            }
        }
        Some("permission-onboarding") => {
            let plan = current_permission_onboarding()?;
            println!(
                "{}\t{}\t{}",
                plan.action.as_str(),
                plan.policy.prompt_mode.as_str(),
                plan.finder_parity_default
            );
            for item in plan.readiness {
                println!(
                    "{}\t{}\t{}\t{}",
                    item.scope.as_str(),
                    item.state.as_str(),
                    item.path.display(),
                    escape_output_field(&item.reason)
                );
            }
        }
        Some(command) if platform::run(command, &mut args)? => {}
        Some("macrobench") => {
            let options = macrobench_options(args.next(), args.next(), "macrobench")?;
            let report = run_macrobench(&options)?;
            println!(
                "fixture\t{}\tfiles\t{}\tpassed\t{}",
                report.fixture_root.display(),
                report.files_materialized,
                report.passed()
            );
            for measurement in report.measurements {
                println!(
                    "{}\t{}\t{}\t{}\t{}",
                    measurement.scenario.directory(),
                    macrobench_stage(measurement.stage),
                    measurement.duration.as_nanos(),
                    measurement.records,
                    measurement.hits
                );
            }
            for violation in report.budget_violations {
                eprintln!("budget-violation\t{violation:?}");
            }
        }
        Some("macrobench-fixture") => {
            let (root, scale) =
                macrobench_fixture_options(args.next(), args.next(), "macrobench-fixture")?;
            let report = materialize_macrobench_fixture_report(root, scale)?;
            println!(
                "fixture\t{}\tmanifest\t{}\tfiles\t{}\tdirectories\t{}\tscenarios\t{}",
                report.fixture_root.display(),
                report.manifest_path.display(),
                report.files_materialized(),
                report.directories_materialized(),
                report.scenarios.len()
            );
            for scenario in report.scenarios {
                println!(
                    "{}\t{}\t{}\t{}",
                    scenario.scenario.directory(),
                    scenario.root.display(),
                    scenario.files,
                    scenario.directories
                );
            }
        }
        Some("parity-fixture") => {
            let options = parity_fixture_options(args.next(), args.next(), "parity-fixture")?;
            let report = materialize_parity_fixture(&options)?;
            println!(
                "fixture\t{}\tmanifest\t{}\tfiles\t{}\tscenarios\t{}",
                report.fixture_root.display(),
                report.manifest_path.display(),
                report.files_materialized(),
                report.scenarios.len()
            );
            for scenario in report.scenarios {
                println!(
                    "{}\t{}\t{}\t{}\t{}",
                    scenario.scenario.directory(),
                    scenario.scenario.finder_view(),
                    scenario.root.display(),
                    scenario.files,
                    scenario.directories
                );
            }
        }
        Some("pixel-diff") => {
            let expected = required_path(args.next(), "pixel-diff requires an expected RGBA path")?;
            let actual = required_path(args.next(), "pixel-diff requires an actual RGBA path")?;
            let width = parse_u32_arg(args.next(), "pixel-diff requires a width")?;
            let height = parse_u32_arg(args.next(), "pixel-diff requires a height")?;
            let size = PixelSize::new(width, height);
            let masks = args
                .next()
                .map(|path| read_mask_file(path, size))
                .transpose()?
                .unwrap_or_default();
            let options = PixelDiffOptions::strict(size).with_masks(masks);
            let report = diff_rgba_files(expected, actual, &options)?;
            println!(
                "pixel-diff\t{}x{}\ttotal={}\tmismatched={}\tunmasked={}\tmasked={}\tpassed={}",
                report.size.width,
                report.size.height,
                report.total_pixels,
                report.mismatched_pixels,
                report.unmasked_mismatches,
                report.masked_mismatches,
                report.passed()
            );
            if let Some(mismatch) = report.first_unmasked_mismatch {
                println!(
                    "first-unmasked\t{}\t{}\t{:02x}{:02x}{:02x}{:02x}\t{:02x}{:02x}{:02x}{:02x}",
                    mismatch.x,
                    mismatch.y,
                    mismatch.expected[0],
                    mismatch.expected[1],
                    mismatch.expected[2],
                    mismatch.expected[3],
                    mismatch.actual[0],
                    mismatch.actual[1],
                    mismatch.actual[2],
                    mismatch.actual[3]
                );
            }
            if !report.passed() {
                return Err(GfmError::Format(format!(
                    "pixel diff failed with {} unmasked mismatch(es)",
                    report.unmasked_mismatches
                )));
            }
        }
        Some("pixel-threshold-check") => {
            let surface = args
                .next()
                .ok_or_else(|| {
                    GfmError::Format("pixel-threshold-check requires a surface".to_string())
                })?
                .parse::<ParitySurface>()
                .map_err(GfmError::Format)?;
            let expected = required_path(
                args.next(),
                "pixel-threshold-check requires an expected RGBA path",
            )?;
            let actual = required_path(
                args.next(),
                "pixel-threshold-check requires an actual RGBA path",
            )?;
            let width = parse_u32_arg(args.next(), "pixel-threshold-check requires a width")?;
            let height = parse_u32_arg(args.next(), "pixel-threshold-check requires a height")?;
            let size = PixelSize::new(width, height);
            let masks = args
                .next()
                .map(|path| read_mask_file(path, size))
                .transpose()?
                .unwrap_or_default();
            let options = PixelDiffOptions::strict(size).with_masks(masks);
            let report = diff_rgba_files(expected, actual, &options)?;
            let threshold = PixelDriftThreshold::finder_strict(surface);
            let evaluation = evaluate_pixel_threshold(&report, threshold);
            println!(
                "{}\tpassed={}\tmismatched={}\tunmasked={}\tmasked={}",
                threshold.as_tsv(),
                evaluation.passed,
                report.mismatched_pixels,
                report.unmasked_mismatches,
                report.masked_mismatches
            );
            for violation in &evaluation.violations {
                println!("{}", violation.as_tsv());
            }
            if !evaluation.passed {
                return Err(GfmError::Format(format!(
                    "pixel threshold failed for {} with {} violation(s)",
                    surface.as_str(),
                    evaluation.violations.len()
                )));
            }
        }
        Some("parity-gate") => {
            let manifest = required_path(args.next(), "parity-gate requires a manifest path")?;
            let report = run_parity_gate_manifest(&manifest)?;
            println!(
                "parity-gate\tmanifest={}\tentries={}\tviolations={}\tpassed={}",
                manifest.display(),
                report.entries.len(),
                report.violations(),
                report.passed()
            );
            for entry in &report.entries {
                println!(
                    "{}\tpassed={}\tmismatched={}\tunmasked={}\tmasked={}\texpected={}\tactual={}",
                    entry.evaluation.threshold.as_tsv(),
                    entry.evaluation.passed,
                    entry.diff.mismatched_pixels,
                    entry.diff.unmasked_mismatches,
                    entry.diff.masked_mismatches,
                    entry.input.expected_path.display(),
                    entry.input.actual_path.display()
                );
                for violation in &entry.evaluation.violations {
                    println!("{}\t{}", entry.input.surface.as_str(), violation.as_tsv());
                }
            }
            if !report.passed() {
                return Err(GfmError::Format(format!(
                    "parity gate failed with {} violation(s)",
                    report.violations()
                )));
            }
        }
        Some("parity-review") => {
            let manifest = required_path(args.next(), "parity-review requires a manifest path")?;
            let output_dir =
                required_path(args.next(), "parity-review requires an output directory")?;
            let bundle = write_parity_review_bundle_manifest(&manifest, &output_dir)?;
            println!(
                "parity-review\tmanifest={}\toutput={}\tentries={}\tviolations={}\tpassed={}",
                manifest.display(),
                output_dir.display(),
                bundle.report.entries.len(),
                bundle.report.violations(),
                bundle.report.passed()
            );
            println!("review\t{}", bundle.review_path.display());
            println!("entries\t{}", bundle.entries_path.display());
            println!("violations\t{}", bundle.violations_path.display());
            println!("first-unmasked\t{}", bundle.first_mismatch_path.display());
            println!("bundle\t{}", bundle.bundle_manifest_path.display());
            if !bundle.report.passed() {
                return Err(GfmError::Format(format!(
                    "parity review captured {} violation(s)",
                    bundle.report.violations()
                )));
            }
        }
        Some("parity-profile") => {
            let macos_build = args.next().ok_or_else(|| {
                GfmError::Format("parity-profile requires a macOS build".to_string())
            })?;
            let appearance = parse_parity_appearance(args.next())?;
            let scale = parse_display_scale(args.next())?;
            let color_profile = parse_color_profile(args.next())?;
            let profile =
                MacOsParityProfile::finder_default(macos_build, appearance, scale, color_profile)?;
            println!("{}", profile.as_tsv());
        }
        Some("regression-gate") => {
            let options = macrobench_options(args.next(), args.next(), "regression-gate")?;
            let run = run_regression_gate(&options, RegressionGateOptions::default())?;
            println!(
                "fixture\t{}\tfiles\t{}\tindex-bytes\t{}\tsidecar-prefix-candidates\t{}\tsidecar-substring-candidates\t{}\tsidecar-fuzzy-verified\t{}\tsidecar-prefix-cache-hits\t{}\tsidecar-substring-cache-hits\t{}\tsidecar-fuzzy-cache-hits\t{}\tsidecar-prefix-cutoffs\t{}\tsidecar-prefix-truncated\t{}\tsidecar-substring-cutoffs\t{}\tsidecar-substring-truncated\t{}\tsidecar-fuzzy-truncated\t{}\tpassed\t{}",
                run.macrobench.fixture_root.display(),
                run.macrobench.files_materialized,
                run.index_size_bytes,
                run.sidecar_lookup.prefix_candidate_ids,
                run.sidecar_lookup.substring_candidate_ids,
                run.sidecar_lookup.fuzzy_verified_candidates,
                run.sidecar_lookup.prefix_cache_hits,
                run.sidecar_lookup.substring_cache_hits,
                run.sidecar_lookup.fuzzy_cache_hits,
                run.sidecar_lookup.prefix_cutoff_terms,
                run.sidecar_lookup.prefix_truncated_terms,
                run.sidecar_lookup.substring_cutoff_terms,
                run.sidecar_lookup.substring_term_truncated_grams
                    + run.sidecar_lookup.substring_truncated_grams,
                run.sidecar_lookup.fuzzy_term_truncated_keys
                    + run.sidecar_lookup.fuzzy_key_truncated_terms
                    + run.sidecar_lookup.fuzzy_candidate_truncated_terms,
                run.passed()
            );
            for violation in &run.gate.violations {
                eprintln!("regression-violation\t{violation:?}");
            }
            if !run.passed() {
                return Err(gfm_types::GfmError::Format(format!(
                    "regression gate failed with {} violation(s)",
                    run.gate.violations.len()
                )));
            }
        }
        Some("large-sidecar-gate") => {
            let workspace =
                required_path(args.next(), "large-sidecar-gate requires a workspace path")?;
            let records = parse_usize_arg(
                args.next(),
                "large-sidecar-gate requires a synthetic record count",
            )?;
            let report = run_large_sidecar_gate(&LargeSidecarGateOptions::new(workspace, records))?;
            println!(
                "large-sidecar-gate\tfixture={}\tthresholds={}\thistory={}\tprofile={}\tmin-ci-records={}\trecords={}\tprobe-records={}\tprefix-keys={}\tsubstring-keys={}\tfuzzy-keys={}\tprefix-bytes={}\tsubstring-bytes={}\tfuzzy-bytes={}\tprefix-candidates={}\tsubstring-candidates={}\tfuzzy-verified={}\tprefix-cache-hits={}\tsubstring-cache-hits={}\tfuzzy-cache-hits={}\tprefix-cutoffs={}\tprefix-truncated={}\tsubstring-cutoffs={}\tsubstring-truncated={}\tfuzzy-truncated={}\tviolations={}\tpassed={}",
                report.fixture_root.display(),
                report.thresholds_path.display(),
                report.history_path.display(),
                report.thresholds.profile,
                report.thresholds.min_required_ci_records,
                report.records,
                report.probe_records,
                report.prefix_keys,
                report.substring_keys,
                report.fuzzy_keys,
                report.prefix_bytes,
                report.substring_bytes,
                report.fuzzy_bytes,
                report.lookup.prefix_candidate_ids,
                report.lookup.substring_candidate_ids,
                report.lookup.fuzzy_verified_candidates,
                report.lookup.prefix_cache_hits,
                report.lookup.substring_cache_hits,
                report.lookup.fuzzy_cache_hits,
                report.lookup.prefix_cutoff_terms,
                report.lookup.prefix_truncated_terms,
                report.lookup.substring_cutoff_terms,
                report.lookup.substring_term_truncated_grams + report.lookup.substring_truncated_grams,
                report.lookup.fuzzy_term_truncated_keys
                    + report.lookup.fuzzy_key_truncated_terms
                    + report.lookup.fuzzy_candidate_truncated_terms,
                report.violations.len(),
                report.passed
            );
            for violation in &report.violations {
                eprintln!("large-sidecar-violation\t{violation:?}");
            }
            if !report.passed {
                return Err(GfmError::Format(
                    "large sidecar lookup gate failed".to_string(),
                ));
            }
        }
        Some("release-policy") => packaging::release_policy()?,
        Some("release-validate") => packaging::release_validate(&mut args)?,
        Some("bundle-app") => packaging::bundle_app(&mut args)?,
        Some("register-app") => packaging::register_app(&mut args)?,
        Some("notarize-app") => packaging::notarize_app(&mut args)?,
        Some("jobs-recover") => {
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
        Some("jobs-retry-plan") => {
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
        Some("jobs-payload-catalog") => {
            let path = required_path(args.next(), "jobs-payload-catalog requires a catalog path")?;
            let catalog = JobPayloadCatalog::new(&path);
            let records = sample_payload_catalog_records();
            catalog.write_all(&records)?;
            for record in catalog.read()? {
                println!("{}", record.as_tsv());
            }
        }
        Some("jobs-fairness-plan") => {
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
        Some("jobs-progress-snapshot") => {
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
        Some("jobs-cancel-tree") => {
            for line in sample_cancellation_tree_report() {
                println!("{line}");
            }
        }
        Some("jobs-runtime-retry-probe") => {
            let state = required_path(
                args.next(),
                "jobs-runtime-retry-probe requires an attempt state path",
            )?;
            let pressure = parse_optional_scheduling_pressure(&mut args)?;
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
        Some(command) if operation::run(command, &mut args)? => {}
        Some("watch-once") => {
            let root = required_path(args.next(), "watch-once requires a root path")?;
            let stream = FileEventStream::watch(&[WatchRoot::tree(root)])?;
            let event = stream.recv()?;
            println!("{}\t{}", event_marker(&event.kind), event.path.display());
        }
        _ => print_usage(),
    }
    Ok(())
}

pub(crate) fn required_path(value: Option<String>, message: &str) -> Result<PathBuf> {
    value
        .map(PathBuf::from)
        .ok_or_else(|| GfmError::Format(message.to_string()))
}

fn optional_path_arg(value: Option<String>, message: &str) -> Result<Option<PathBuf>> {
    let value = value.ok_or_else(|| GfmError::Format(message.to_string()))?;
    Ok((value != "-").then(|| PathBuf::from(value)))
}

fn parse_sidecar_paths(
    args: &mut impl Iterator<Item = String>,
    command: &str,
) -> Result<SidecarPaths> {
    Ok(SidecarPaths {
        columns: optional_path_arg(
            args.next(),
            &format!("{command} requires a columns path or -"),
        )?,
        metadata: optional_path_arg(
            args.next(),
            &format!("{command} requires a metadata path or -"),
        )?,
        prefixes: optional_path_arg(
            args.next(),
            &format!("{command} requires a prefixes path or -"),
        )?,
        substrings: optional_path_arg(
            args.next(),
            &format!("{command} requires a substrings path or -"),
        )?,
        fuzzy: optional_path_arg(
            args.next(),
            &format!("{command} requires a fuzzy path or -"),
        )?,
        dictionary: optional_path_arg(
            args.next(),
            &format!("{command} requires a dictionary path or -"),
        )?,
    })
}

fn parse_sidecar_kind(value: Option<String>, command: &str) -> Result<SidecarKind> {
    let value = value.ok_or_else(|| {
        GfmError::Format(format!(
            "{command} requires columns, metadata, prefixes, substrings, fuzzy, or dictionary"
        ))
    })?;
    match value.as_str() {
        "columns" => Ok(SidecarKind::Columns),
        "metadata" => Ok(SidecarKind::Metadata),
        "prefixes" | "prefix" => Ok(SidecarKind::Prefixes),
        "substrings" | "substring" => Ok(SidecarKind::Substrings),
        "fuzzy" => Ok(SidecarKind::Fuzzy),
        "dictionary" => Ok(SidecarKind::Dictionary),
        _ => Err(GfmError::Format(format!(
            "{command} requires columns, metadata, prefixes, substrings, fuzzy, or dictionary"
        ))),
    }
}

fn required_string(value: Option<String>, message: &str) -> Result<String> {
    value.ok_or_else(|| GfmError::Format(message.to_string()))
}

fn parse_optional_scheduling_pressure(
    args: &mut impl Iterator<Item = String>,
) -> Result<SchedulingPressure> {
    let Some(io) = args.next() else {
        return Ok(SchedulingPressure::default());
    };
    parse_scheduling_pressure_tail(io, args, "adaptive scheduling")
}

fn parse_required_scheduling_pressure(
    args: &mut impl Iterator<Item = String>,
    context: &str,
) -> Result<SchedulingPressure> {
    let io = required_string(
        args.next(),
        &format!("{context} requires adaptive scheduling io pressure"),
    )?;
    parse_scheduling_pressure_tail(io, args, context)
}

fn parse_scheduling_pressure_tail(
    io: String,
    args: &mut impl Iterator<Item = String>,
    context: &str,
) -> Result<SchedulingPressure> {
    let thermal = required_string(
        args.next(),
        &format!("{context} requires adaptive scheduling thermal state"),
    )?;
    let battery = required_string(
        args.next(),
        &format!("{context} requires adaptive scheduling battery state"),
    )?;
    let user_activity = required_string(
        args.next(),
        &format!("{context} requires adaptive scheduling user activity"),
    )?;
    Ok(SchedulingPressure {
        io: match parse_io_pressure(io)? {
            IoPressure::Nominal => JobIoPressure::Nominal,
            IoPressure::Elevated => JobIoPressure::Elevated,
            IoPressure::Saturated => JobIoPressure::Saturated,
        },
        thermal: match parse_thermal_state(thermal)? {
            ThermalState::Nominal => JobThermalState::Nominal,
            ThermalState::Fair => JobThermalState::Fair,
            ThermalState::Serious => JobThermalState::Serious,
            ThermalState::Critical => JobThermalState::Critical,
        },
        battery: match parse_battery_state(battery)? {
            BatteryState::AcPower => JobBatteryState::AcPower,
            BatteryState::Battery => JobBatteryState::Battery,
            BatteryState::LowPower => JobBatteryState::LowPower,
        },
        user_activity: match parse_user_activity(user_activity)? {
            UserActivity::Idle => JobUserActivity::Idle,
            UserActivity::Active => JobUserActivity::Active,
        },
    })
}

fn parse_io_pressure(value: String) -> Result<IoPressure> {
    match value.as_str() {
        "nominal" => Ok(IoPressure::Nominal),
        "elevated" => Ok(IoPressure::Elevated),
        "saturated" => Ok(IoPressure::Saturated),
        _ => Err(GfmError::Format(format!(
            "invalid io pressure `{value}`; expected nominal, elevated, or saturated"
        ))),
    }
}

fn parse_thermal_state(value: String) -> Result<ThermalState> {
    match value.as_str() {
        "nominal" => Ok(ThermalState::Nominal),
        "fair" => Ok(ThermalState::Fair),
        "serious" => Ok(ThermalState::Serious),
        "critical" => Ok(ThermalState::Critical),
        _ => Err(GfmError::Format(format!(
            "invalid thermal state `{value}`; expected nominal, fair, serious, or critical"
        ))),
    }
}

fn parse_battery_state(value: String) -> Result<BatteryState> {
    match value.as_str() {
        "ac" => Ok(BatteryState::AcPower),
        "battery" => Ok(BatteryState::Battery),
        "low" => Ok(BatteryState::LowPower),
        _ => Err(GfmError::Format(format!(
            "invalid battery state `{value}`; expected ac, battery, or low"
        ))),
    }
}

fn parse_user_activity(value: String) -> Result<UserActivity> {
    match value.as_str() {
        "idle" => Ok(UserActivity::Idle),
        "active" => Ok(UserActivity::Active),
        _ => Err(GfmError::Format(format!(
            "invalid user activity `{value}`; expected idle or active"
        ))),
    }
}

pub(crate) fn index_volume_descriptor(volume: &VolumeDescriptor) -> IndexVolumeDescriptor {
    IndexVolumeDescriptor::new(
        volume.label.clone(),
        volume.path.clone(),
        index_volume_class(volume.kind),
        index_mount_state(volume.mount_state),
    )
}

fn index_volume_class(kind: VolumeKind) -> IndexVolumeClass {
    match kind {
        VolumeKind::System => IndexVolumeClass::System,
        VolumeKind::Internal => IndexVolumeClass::Internal,
        VolumeKind::External | VolumeKind::Removable | VolumeKind::DiskImage => {
            IndexVolumeClass::External
        }
        VolumeKind::Network => IndexVolumeClass::Network,
        VolumeKind::Unknown => IndexVolumeClass::Unknown,
    }
}

fn index_mount_state(state: MountState) -> IndexMountState {
    match state {
        MountState::Mounted => IndexMountState::Mounted,
        MountState::Unmounted => IndexMountState::Unmounted,
        MountState::Stale => IndexMountState::Stale,
    }
}

fn parse_quarantine_failure_kind(value: &str, name: &str) -> Result<QuarantineFailureKind> {
    QuarantineFailureKind::parse(value)
        .ok_or_else(|| GfmError::Format(format!("invalid {name}: {value}")))
}

fn parse_metadata_field(value: &str, name: &str) -> Result<MetadataField> {
    MetadataField::parse(value).ok_or_else(|| GfmError::Format(format!("invalid {name}: {value}")))
}

fn parse_u32(value: &str, name: &str) -> Result<u32> {
    value
        .parse()
        .map_err(|_| GfmError::Format(format!("{name} must be an unsigned 32-bit integer")))
}

fn parse_u64(value: &str, name: &str) -> Result<u64> {
    value
        .parse()
        .map_err(|_| GfmError::Format(format!("{name} must be an unsigned 64-bit integer")))
}

fn parse_u32_arg(value: Option<String>, message: &str) -> Result<u32> {
    let value = value.ok_or_else(|| GfmError::Format(message.to_string()))?;
    value
        .parse()
        .map_err(|_| GfmError::Format(format!("{message}; got `{value}`")))
}

fn parse_u64_arg(value: Option<String>, message: &str) -> Result<u64> {
    let value = value.ok_or_else(|| GfmError::Format(message.to_string()))?;
    value
        .parse()
        .map_err(|_| GfmError::Format(format!("{message}; got `{value}`")))
}

fn parse_event_ids(value: &str) -> Result<Vec<u64>> {
    if value == "-" || value.trim().is_empty() {
        return Ok(Vec::new());
    }
    value
        .split(',')
        .map(|part| {
            part.parse().map_err(|_| {
                GfmError::Format(format!("observed event id `{part}` must be unsigned"))
            })
        })
        .collect()
}

fn parse_usize_arg(value: Option<String>, message: &str) -> Result<usize> {
    let value = value.ok_or_else(|| GfmError::Format(message.to_string()))?;
    parse_usize(&value, message)
}

fn parse_usize(value: &str, message: &str) -> Result<usize> {
    value
        .parse()
        .map_err(|_| GfmError::Format(format!("{message}; got `{value}`")))
}

fn config_store(value: Option<String>) -> Result<ConfigStore> {
    value
        .map(|path| Ok(ConfigStore::new(path)))
        .unwrap_or_else(ConfigStore::platform_default)
}

fn macrobench_options(
    root: Option<String>,
    scale: Option<String>,
    command: &str,
) -> Result<MacrobenchOptions> {
    let root = required_path(root, &format!("{command} requires a workspace path"))?;
    let mut options = MacrobenchOptions::smoke(root);
    match scale.as_deref() {
        Some("standard") => {
            options.scale = MacrobenchScale::standard();
            options.limit = 50;
        }
        Some("smoke") | None => {}
        Some(other) => {
            return Err(gfm_types::GfmError::Format(format!(
                "{command} scale must be `smoke` or `standard`, got `{other}`"
            )));
        }
    }
    Ok(options)
}

fn macrobench_fixture_options(
    root: Option<String>,
    scale: Option<String>,
    command: &str,
) -> Result<(PathBuf, MacrobenchScale)> {
    let root = required_path(root, &format!("{command} requires a workspace path"))?;
    let scale = match scale.as_deref() {
        Some("standard") => MacrobenchScale::standard(),
        Some("million") => MacrobenchScale::million_files(),
        Some("smoke") | None => MacrobenchScale::smoke(),
        Some(other) => {
            return Err(gfm_types::GfmError::Format(format!(
                "{command} scale must be `smoke`, `standard`, or `million`, got `{other}`"
            )));
        }
    };
    Ok((root, scale))
}

fn parity_fixture_options(
    root: Option<String>,
    scale: Option<String>,
    command: &str,
) -> Result<ParityFixtureOptions> {
    let root = required_path(root, &format!("{command} requires a workspace path"))?;
    let scale = match scale.as_deref() {
        Some("standard") => ParityFixtureScale::standard(),
        Some("smoke") | None => ParityFixtureScale::smoke(),
        Some(other) => {
            return Err(GfmError::Format(format!(
                "{command} scale must be `smoke` or `standard`, got `{other}`"
            )));
        }
    };
    Ok(ParityFixtureOptions {
        workspace: root,
        scale,
    })
}

pub(crate) fn parent_volume(path: &Path) -> Option<VolumeId> {
    path.parent()
        .and_then(|parent| detect_volume_id(parent).ok())
}

pub(crate) fn run_preview_contract<T>(
    volume: Option<VolumeId>,
    label: &'static str,
    build: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T>
where
    T: Send + 'static,
{
    run_volume_task(volume, Priority::Visible, label, build)
}

fn run_volume_task<T>(
    volume: Option<VolumeId>,
    priority: Priority,
    label: &'static str,
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T>
where
    T: Send + 'static,
{
    let result_slot = Arc::new(Mutex::new(None));
    let result_slot_task = Arc::clone(&result_slot);
    let mut scheduler = Scheduler::new();
    let job = if let Some(volume) = volume {
        scheduler.schedule_on_volume(priority, label, volume)
    } else {
        scheduler.schedule(priority, label)
    };
    let runtime = RuntimeJobHandle::begin(
        &job,
        payload_kind_for_label(label),
        label,
        1,
        format!("{}:{label}", priority.as_str()),
    )?;
    let runtime_task = runtime.clone();
    let task = Task::new(job.clone(), move |_| {
        runtime_task.running()?;
        let result = work()?;
        *result_slot_task
            .lock()
            .expect("volume task result lock poisoned") = Some(result);
        Ok(())
    });
    let report = WorkerPool::new(1).run_isolated(vec![task], VolumeConcurrencyPolicy::new(1));
    let outcome = report
        .outcomes
        .iter()
        .find(|outcome| outcome.id == job.id)
        .ok_or_else(|| GfmError::Format(format!("{label} job did not run")))?;
    runtime.finish(&outcome.status)?;
    match &outcome.status {
        TaskStatus::Completed => {}
        TaskStatus::Started => {
            return Err(GfmError::Format(format!("{label} job is still running")))
        }
        TaskStatus::Cancelled => return Err(GfmError::Cancelled),
        TaskStatus::Failed(message) => {
            return Err(GfmError::Format(format!("{label} job failed: {message}")))
        }
    }
    let result = result_slot
        .lock()
        .expect("volume task result lock poisoned")
        .take()
        .ok_or_else(|| GfmError::Format(format!("{label} job completed without a result")))?;
    Ok(result)
}

#[derive(Clone)]
struct RuntimeJobHandle {
    progress_store: Option<JobProgressStore>,
    snapshot: JobProgressSnapshot,
}

impl RuntimeJobHandle {
    fn begin(
        job: &Job,
        kind: JobPayloadKind,
        label: &str,
        total_units: u64,
        summary: String,
    ) -> Result<Self> {
        if let Some(catalog) = runtime_payload_catalog() {
            catalog.append(&JobPayloadRecord::new(
                job.id,
                kind,
                label,
                runtime_payload_path(kind, label),
                job.volume,
                summary.clone(),
            ))?;
        }
        let snapshot = JobProgressSnapshot::new(
            job.id,
            job.class,
            job.priority,
            label,
            job.volume,
            total_units.max(1),
        )
        .with_progress(JobProgressState::Planned, 0, summary, job_timestamp_ms());
        let progress_store = runtime_progress_store();
        if let Some(store) = &progress_store {
            store.upsert(snapshot.clone())?;
        }
        Ok(Self {
            progress_store,
            snapshot,
        })
    }

    fn running(&self) -> Result<()> {
        if let Some(store) = &self.progress_store {
            store.upsert(self.snapshot.clone().with_progress(
                JobProgressState::Running,
                0,
                self.snapshot.detail.clone(),
                job_timestamp_ms(),
            ))?;
        }
        Ok(())
    }

    fn finish(&self, status: &TaskStatus) -> Result<()> {
        if let Some(store) = &self.progress_store {
            let (completed_units, detail) = match status {
                TaskStatus::Started => (0, "still-running".to_string()),
                TaskStatus::Completed => (self.snapshot.total_units, "completed".to_string()),
                TaskStatus::Cancelled => (0, "cancelled".to_string()),
                TaskStatus::Failed(message) => (0, message.clone()),
            };
            store.upsert(self.snapshot.clone().with_progress(
                JobProgressState::from(status),
                completed_units,
                detail,
                job_timestamp_ms(),
            ))?;
        }
        Ok(())
    }
}

fn runtime_payload_catalog() -> Option<JobPayloadCatalog> {
    env::var_os("GFM_JOB_PAYLOAD_CATALOG").map(JobPayloadCatalog::new)
}

fn runtime_progress_store() -> Option<JobProgressStore> {
    env::var_os("GFM_JOB_PROGRESS_STORE").map(JobProgressStore::new)
}

fn runtime_payload_path(kind: JobPayloadKind, label: &str) -> PathBuf {
    PathBuf::from("runtime")
        .join(kind.as_str())
        .join(format!("{}.gfmjob", label_slug(label)))
}

fn label_slug(label: &str) -> String {
    let mut slug = String::with_capacity(label.len());
    let mut last_dash = false;
    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "job".to_string()
    } else {
        slug.to_string()
    }
}

fn payload_kind_for_label(label: &str) -> JobPayloadKind {
    let label = label.to_ascii_lowercase();
    if label.contains("thumbnail") {
        JobPayloadKind::Thumbnail
    } else if label.contains("quicklook") || label.contains("preview") {
        JobPayloadKind::Preview
    } else if label.contains("repair") || label.contains("recover") {
        JobPayloadKind::Repair
    } else if label.contains("extract") {
        JobPayloadKind::Extraction
    } else if label.contains("index") || label.contains("content") {
        JobPayloadKind::Indexing
    } else {
        JobPayloadKind::Operation
    }
}

fn job_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScheduledTaskOutcome<T> {
    result: Option<T>,
    scheduling_action: SchedulingAction,
    deferred: bool,
}

fn run_scheduled_volume_task<T>(
    volume: Option<VolumeId>,
    priority: Priority,
    label: &'static str,
    pressure: SchedulingPressure,
    work: impl Fn() -> Result<T> + Send + Sync + 'static,
) -> Result<ScheduledTaskOutcome<T>>
where
    T: Send + 'static,
{
    let scheduling = pressure.decide(priority, 1, 1);
    if scheduling.action == SchedulingAction::Defer {
        return Ok(ScheduledTaskOutcome {
            result: None,
            scheduling_action: scheduling.action,
            deferred: true,
        });
    }

    let result_slot = Arc::new(Mutex::new(None));
    let result_slot_task = Arc::clone(&result_slot);
    let mut scheduler = Scheduler::new();
    let job = if let Some(volume) = volume {
        scheduler.schedule_on_volume(priority, label, volume)
    } else {
        scheduler.schedule(priority, label)
    };
    let runtime = RuntimeJobHandle::begin(
        &job,
        payload_kind_for_label(label),
        label,
        1,
        format!("{}:{label}:adaptive", priority.as_str()),
    )?;
    let runtime_task = runtime.clone();
    let task = RetriableTask::new(job.clone(), move |_| {
        runtime_task.running()?;
        let result = work()?;
        *result_slot_task
            .lock()
            .expect("scheduled task result lock poisoned") = Some(result);
        Ok(())
    });
    let journal = JobJournal::new(default_job_journal_path());
    let report = WorkerPool::new(scheduling.worker_threads).run_retriable_isolated(
        vec![task],
        &journal,
        RetryPolicy { max_attempts: 2 },
        scheduling.volume_policy,
    );
    let outcome = report
        .outcomes
        .iter()
        .find(|outcome| outcome.id == job.id)
        .ok_or_else(|| GfmError::Format(format!("{label} job did not run")))?;
    runtime.finish(&outcome.status)?;
    match &outcome.status {
        TaskStatus::Completed => {}
        TaskStatus::Started => {
            return Err(GfmError::Format(format!("{label} job is still running")))
        }
        TaskStatus::Cancelled => return Err(GfmError::Cancelled),
        TaskStatus::Failed(message) => {
            return Err(GfmError::Format(format!("{label} job failed: {message}")))
        }
    }
    let result = result_slot
        .lock()
        .expect("scheduled task result lock poisoned")
        .take()
        .ok_or_else(|| GfmError::Format(format!("{label} job completed without a result")))?;
    Ok(ScheduledTaskOutcome {
        result: Some(result),
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

fn print_sidecar_recovery_report(report: SidecarRecovery) {
    println!("{}", report.before.as_tsv());
    println!(
        "sidecar-recovery\trebuilt={}\tquarantined={}",
        report.rebuilt_sidecars.len(),
        report.quarantined_sidecars.len()
    );
    println!("{}", report.after.as_tsv());
    print_sidecar_health("invalid-before", &report.before.invalid_sidecars);
    for path in report.quarantined_sidecars {
        println!("quarantined\t{}", path.display());
    }
}

fn print_index_rebuild_report(report: gfm_diagnostics::RebuildReport) {
    println!(
        "{}\t{}\t{}\t{}\t{}",
        report.root.display(),
        report.records_path.display(),
        report
            .content_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "-".to_string()),
        report.records,
        report.content_indexed
    );
    if report.inaccessible != 0 {
        eprintln!("inaccessible\t{}", report.inaccessible);
    }
}

fn print_persistent_index_recovery_report(report: PersistentIndexRecovery) {
    println!("{}", report.before.as_tsv());
    println!(
        "persistent-index-recovery\trebuilt-records={}\trebuilt-state={}\tquarantined-records={}",
        report.rebuilt_records,
        report.rebuilt_state,
        report
            .quarantined_records_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "-".to_string())
    );
    println!("{}", report.after.as_tsv());
}

fn run_content_search(
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
struct ContentJobOutcome {
    report: Option<ContentIndexReport>,
    inaccessible: usize,
    scheduling_action: SchedulingAction,
    deferred: bool,
}

fn run_content_job(
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
            gfm_types::GfmError::Format(format!(
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
        .ok_or_else(|| {
            gfm_types::GfmError::Format("background content index job did not run".to_string())
        })?;
    runtime.finish(&outcome.status)?;
    match &outcome.status {
        TaskStatus::Completed => {}
        TaskStatus::Started => {
            return Err(gfm_types::GfmError::Format(
                "background content index is still running".to_string(),
            ))
        }
        TaskStatus::Cancelled => return Err(gfm_types::GfmError::Cancelled),
        TaskStatus::Failed(message) => {
            return Err(gfm_types::GfmError::Format(format!(
                "background content index failed: {message}"
            )))
        }
    }
    let report = content_report
        .lock()
        .expect("content index report lock poisoned")
        .clone()
        .ok_or_else(|| {
            gfm_types::GfmError::Format(
                "background content index completed without a report".to_string(),
            )
        })?;
    Ok(ContentJobOutcome {
        report: Some(report),
        inaccessible,
        scheduling_action: scheduling.action,
        deferred: false,
    })
}

pub(crate) fn detect_volume_id(path: &Path) -> Result<VolumeId> {
    volume_id_from_metadata(&std::fs::metadata(path).map_err(|err| GfmError::io(path, err))?)
}

#[cfg(unix)]
fn volume_id_from_metadata(metadata: &std::fs::Metadata) -> Result<VolumeId> {
    use std::os::unix::fs::MetadataExt;

    Ok(VolumeId(metadata.dev()))
}

#[cfg(not(unix))]
fn volume_id_from_metadata(_metadata: &std::fs::Metadata) -> Result<VolumeId> {
    Ok(VolumeId(0))
}

fn default_journal_path() -> PathBuf {
    std::env::var_os("GFM_OPS_JOURNAL")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("gfm-ops.journal"))
}

fn default_trash_metadata_path() -> PathBuf {
    std::env::var_os("GFM_TRASH_METADATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("gfm-trash.tsv"))
}

fn default_job_journal_path() -> PathBuf {
    std::env::var_os("GFM_JOB_JOURNAL")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("gfm-jobs.journal"))
}

fn default_content_job_path() -> PathBuf {
    std::env::var_os("GFM_CONTENT_JOB")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("gfm-content.job"))
}

fn default_extraction_quarantine_path() -> PathBuf {
    std::env::var_os("GFM_EXTRACTION_QUARANTINE")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("gfm-extraction.gfmquarantine"))
}

fn marker(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Directory => "dir",
        FileKind::File => "file",
        FileKind::Symlink => "link",
        FileKind::Other => "other",
    }
}

fn print_hit(hit: &SearchHit) {
    print!(
        "{}\t{}\t{}\t{}",
        hit.score,
        marker(hit.record.kind),
        hit.record.len,
        hit.record.path.display()
    );
    if let Some(snippet) = &hit.snippet {
        print!("\t{}", escape_output_field(&highlight_snippet(snippet)));
    }
    println!();
}

fn highlight_snippet(snippet: &gfm_types::SearchSnippet) -> String {
    let Some(highlight) = snippet.highlights.first() else {
        return snippet.text.clone();
    };
    if highlight.start > highlight.end
        || highlight.end > snippet.text.len()
        || !snippet.text.is_char_boundary(highlight.start)
        || !snippet.text.is_char_boundary(highlight.end)
    {
        return snippet.text.clone();
    }
    format!(
        "{}[[{}]]{}",
        &snippet.text[..highlight.start],
        &snippet.text[highlight.start..highlight.end],
        &snippet.text[highlight.end..]
    )
}

fn escape_output_field(input: &str) -> String {
    input
        .chars()
        .map(|ch| match ch {
            '\t' | '\n' | '\r' => ' ',
            other => other,
        })
        .collect()
}

fn event_marker(kind: &gfm_types::FileEventKind) -> &'static str {
    match kind {
        gfm_types::FileEventKind::Create => "create",
        gfm_types::FileEventKind::Modify => "modify",
        gfm_types::FileEventKind::Remove => "remove",
        gfm_types::FileEventKind::Rename { .. } => "rename",
        gfm_types::FileEventKind::Rescan => "rescan",
        gfm_types::FileEventKind::Other => "other",
    }
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

    JobFairnessPlanner::new(
        JobFairnessPolicy::new()
            .with_quota(JobClass::Foreground, 1)
            .with_quota(JobClass::Visible, 1)
            .with_quota(JobClass::Background, 1)
            .with_quota(JobClass::Maintenance, 1)
            .with_quota(JobClass::Repair, 1),
    )
    .plan(scheduler.drain_ready())
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

fn stream_stage(stage: SearchStreamStage) -> &'static str {
    match stage {
        SearchStreamStage::Hot => "hot",
        SearchStreamStage::Deep => "deep",
    }
}

fn macrobench_stage(stage: MacrobenchStage) -> &'static str {
    match stage {
        MacrobenchStage::IndexBuild => "index-build",
        MacrobenchStage::HotSearch => "hot-search",
        MacrobenchStage::StreamSearch => "stream-search",
        MacrobenchStage::ContentSearch => "content-search",
    }
}

fn parse_parity_appearance(value: Option<String>) -> Result<ParityAppearance> {
    value
        .unwrap_or_else(|| "system".to_string())
        .parse::<ParityAppearance>()
        .map_err(GfmError::Format)
}

fn parse_display_scale(value: Option<String>) -> Result<DisplayScale> {
    value
        .unwrap_or_else(|| "2x".to_string())
        .parse::<DisplayScale>()
        .map_err(GfmError::Format)
}

fn parse_color_profile(value: Option<String>) -> Result<ColorProfile> {
    value
        .unwrap_or_else(|| "srgb".to_string())
        .parse::<ColorProfile>()
        .map_err(GfmError::Format)
}

fn parse_content_manifest_archive_spec(value: &str) -> Result<ContentArchiveManifestEntry> {
    let (tier, path) = value.split_once(':').ok_or_else(|| {
        GfmError::Format(format!(
            "content manifest archive `{value}` must be formatted as hot:path, warm:path, or cold:path"
        ))
    })?;
    if path.is_empty() {
        return Err(GfmError::Format(format!(
            "content manifest archive `{value}` has an empty path"
        )));
    }
    Ok(ContentArchiveManifestEntry {
        tier: parse_content_tier(tier)?,
        path: PathBuf::from(path),
    })
}

fn parse_content_tier(value: &str) -> Result<ContentMergeTier> {
    match value {
        "hot" => Ok(ContentMergeTier::Hot),
        "warm" => Ok(ContentMergeTier::Warm),
        "cold" => Ok(ContentMergeTier::Cold),
        other => Err(GfmError::Format(format!(
            "content archive tier must be hot, warm, or cold; got `{other}`"
        ))),
    }
}

fn content_tier_name(tier: ContentMergeTier) -> &'static str {
    match tier {
        ContentMergeTier::Hot => "hot",
        ContentMergeTier::Warm => "warm",
        ContentMergeTier::Cold => "cold",
    }
}

fn print_content_archive_health(label: &str, archives: &[ContentArchiveHealth]) {
    for archive in archives {
        println!(
            "{}\t{}\t{}\t{}",
            label,
            content_tier_name(archive.entry.tier),
            archive.resolved_path.display(),
            archive.detail.as_deref().unwrap_or("-")
        );
    }
}

fn print_sidecar_health(label: &str, sidecars: &[SidecarHealth]) {
    for sidecar in sidecars {
        println!(
            "{}\t{}\t{}\t{}",
            label,
            sidecar_kind_name(sidecar.kind),
            sidecar.path.display(),
            sidecar.detail.as_deref().unwrap_or("-")
        );
    }
}

fn print_usage() {
    println!(
        "gfm commands:
  gfm app [path]
  gfm ui-contract [path]
  gfm ui-menu-contract
  gfm ui-context-menu-contract [file|folder|volume|sidebar|empty|selection|search-result|trash] [selection-count] [writable] [ejectable] [has-clipboard-items]
  gfm ui-dialog-contract [alert|rename|popover|disclosure|progress|conflict|permission] [running|paused] [true|false]
  gfm ui-titlebar-contract [path]
  gfm ui-session-contract [path] [window-session.tsv]
  gfm ui-toolbar-contract [path]
  gfm ui-sidebar-contract [path]
  gfm ui-icon-view-contract <path> [columns] [viewport-rows] [scroll-row]
  gfm ui-virtualization-contract <icon-grid|list-rows|column-rows|gallery-filmstrip|search-results|trash-rows> <total> <viewport> <scroll> [columns]
  gfm package-traversal <root> [opaque|traverse]
  gfm finder-metadata <path>
  gfm list [path]
  gfm index <root> <output.gfmidx>
  gfm index-state <root> <records.gfmidx> <state.gfmstate>
  gfm index-state-inspect <state.gfmstate>
  gfm scan-progress <root> <records.gfmidx> <progress.gfmprogress>
  gfm scan-progress-inspect <progress.gfmprogress>
  gfm fair-scan <root> <visible-burst> [visible-root...]
  gfm rename-correlation <source> <destination>
  gfm metadata-update <path> [append-text]
  gfm event-backpressure <capacity> <visible-burst> <background-events> [visible-events]
  gfm fsevents-cursor-checkpoint <state.gfmstate> <cursor.gfmcursor> <last-event-id> [clean|repair-required]
  gfm fsevents-cursor-inspect <cursor.gfmcursor>
  gfm fsevents-cursor-resume <state.gfmstate> <cursor.gfmcursor>
  gfm fsevents-repair-schedule <state.gfmstate> <cursor.gfmcursor> <observed-event-ids|-> [reason|-] [dropped-roots...]
  gfm index-content <root> <records.gfmidx> <content.gfmcontent>
  gfm extract-report <path>
  gfm extract-report-adaptive <path> <nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active>
  gfm extract-worker-adaptive <path> <nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active>
  gfm extract-worker-cancel-adaptive <path> <nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active>
  gfm extract-worker-quarantine-adaptive <path> <store.gfmquarantine> <nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active> [timeout-ms] [failure-threshold]
  gfm extract-cache <path>
  gfm extract-quarantine <path> <store.gfmquarantine> [corrupt|encrypted|crash|timeout] [attempts]
  gfm index-content-segment <root> <output.gfmseg>
  gfm compact-content <output.gfmcontent> <segments.gfmseg...>
  gfm compact-content-tiered <output.gfmcontent> <segments.gfmseg...>
  gfm content-manifest-write <manifest.gfmmanifest> <hot|warm|cold:path...>
  gfm content-manifest-inspect <manifest.gfmmanifest>
  gfm content-manifest-recovery-plan <manifest.gfmmanifest> [hot|warm|cold:path...]
  gfm content-manifest-recover <manifest.gfmmanifest> <quarantine-dir> [hot|warm|cold:path...]
  gfm content-manifest-promote <manifest.gfmmanifest> <hot|warm|cold:path> [retired-archive...]
  gfm content-manifest-promotion-recovery-plan <manifest.gfmmanifest>
  gfm content-manifest-promotion-recover <manifest.gfmmanifest>
  gfm content-manifest-cleanup <manifest.gfmmanifest> <candidate-archive...>
  gfm content-cleanup-plan <manifest.gfmmanifest> <min-retired-archives> <min-retired-bytes> <max-cleanup-archives> <candidate-archive...>
  gfm content-maintain-segments <manifest.gfmmanifest> <output.gfmcontent> <segments.gfmseg...>
  gfm content-maintain-segments-adaptive <manifest.gfmmanifest> <output.gfmcontent> <nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active> <segments.gfmseg...>
  gfm index-content-background <root> <segment-dir> <records.gfmidx> <content.gfmcontent> [<nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active>]
  gfm resume-content-background [content.job] [jobs.journal]
  gfm resume-content-background-adaptive <content.job> <jobs.journal> <nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active>
  gfm search <root> <query>
  gfm search-stream <root> <query>
  gfm search-content <root> <query>
  gfm search-content-adaptive <root> <query> <nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active>
  gfm search-index <index.gfmidx> <query>
  gfm search-index-mmap <index.gfmidx> <query>
  gfm search-index-columns <index.gfmidx> <columns.gfmcols> <query>
  gfm search-index-sidecars <index.gfmidx> <columns.gfmcols> <metadata.gfmmeta> <prefixes.gfmprefix> <substrings.gfmsubstr> <fuzzy.gfmfuzzy> <content.gfmcontent> <query>
  gfm search-index-sidecars-budget <index.gfmidx> <columns.gfmcols> <metadata.gfmmeta> <prefixes.gfmprefix> <substrings.gfmsubstr> <fuzzy.gfmfuzzy> <content.gfmcontent> <max-prefix-ids> <max-substring-grams> <max-substring-ids> <max-fuzzy-keys> <max-fuzzy-terms> <max-fuzzy-candidates> <max-content-ids> <query>
  gfm index-footprint <index.gfmidx> <columns.gfmcols|-> <metadata.gfmmeta|-> <prefixes.gfmprefix|-> <substrings.gfmsubstr|-> <fuzzy.gfmfuzzy|-> <content-manifest.gfmmanifest|-> [segments.gfmseg...]
  gfm index-compaction-plan <index.gfmidx> <content-manifest.gfmmanifest|-> <nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active> [segments.gfmseg...]
  gfm archive-schema <records|columns|metadata|prefixes|substrings|fuzzy|dictionary|content|content-manifest> <archive-path>
  gfm archive-rebuild-plan <records.gfmidx> <columns.gfmcols> <metadata.gfmmeta> <prefixes.gfmprefix> <substrings.gfmsubstr> <fuzzy.gfmfuzzy> <dictionary.gfmdict> <content.gfmcontent> <content-manifest.gfmmanifest> [hot|warm|cold:content.gfmcontent...]
  gfm records-migration-plan <records.gfmidx>
  gfm records-migrate <records.gfmidx> <backup-dir>
  gfm content-migration-plan <content.gfmcontent>
  gfm content-migrate <content.gfmcontent> <backup-dir>
  gfm metadata-migration-plan <metadata.gfmmeta>
  gfm metadata-migrate <metadata.gfmmeta> <backup-dir>
  gfm columns-rebuild-plan <records.gfmidx> <columns.gfmcols>
  gfm columns-rebuild <records.gfmidx> <columns.gfmcols> <backup-dir>
  gfm derived-sidecar-rebuild-plan <records.gfmidx> <columns|metadata|prefixes|substrings|fuzzy|dictionary> <sidecar-path>
  gfm derived-sidecar-rebuild <records.gfmidx> <columns|metadata|prefixes|substrings|fuzzy|dictionary> <sidecar-path> <backup-dir>
  gfm records-verify <index.gfmidx>
  gfm index-columns <records.gfmidx> <columns.gfmcols>
  gfm columns-verify <columns.gfmcols>
  gfm columns-lookup <columns.gfmcols> <volume-id> <node-id>
  gfm index-metadata <records.gfmidx> <metadata.gfmmeta>
  gfm index-dictionary <records.gfmidx> <dictionary.gfmdict>
  gfm index-prefixes <records.gfmidx> <prefixes.gfmprefix>
  gfm index-substrings <records.gfmidx> <substrings.gfmsubstr>
  gfm index-fuzzy <records.gfmidx> <fuzzy.gfmfuzzy>
  gfm sidecar-recovery-plan <records.gfmidx> <columns.gfmcols|-> <metadata.gfmmeta|-> <prefixes.gfmprefix|-> <substrings.gfmsubstr|-> <fuzzy.gfmfuzzy|-> <dictionary.gfmdict|->
  gfm sidecar-recover <records.gfmidx> <quarantine-dir> <columns.gfmcols|-> <metadata.gfmmeta|-> <prefixes.gfmprefix|-> <substrings.gfmsubstr|-> <fuzzy.gfmfuzzy|-> <dictionary.gfmdict|->
  gfm sidecar-recover-adaptive <records.gfmidx> <quarantine-dir> <nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active> <columns.gfmcols|-> <metadata.gfmmeta|-> <prefixes.gfmprefix|-> <substrings.gfmsubstr|-> <fuzzy.gfmfuzzy|-> <dictionary.gfmdict|->
  gfm fuzzy-terms-mmap <fuzzy.gfmfuzzy> <key>
  gfm fuzzy-verify <fuzzy.gfmfuzzy>
  gfm prefix-ids-mmap <prefixes.gfmprefix> <prefix>
  gfm prefix-id-block-mmap <prefixes.gfmprefix> <prefix> <block-index>
  gfm prefix-verify <prefixes.gfmprefix>
  gfm substring-ids-mmap <substrings.gfmsubstr> <trigram>
  gfm substring-id-block-mmap <substrings.gfmsubstr> <trigram> <block-index>
  gfm substring-verify <substrings.gfmsubstr>
  gfm dictionary-lookup <dictionary.gfmdict> <term>
  gfm dictionary-verify <dictionary.gfmdict>
  gfm metadata-ids-mmap <metadata.gfmmeta> <tag|comment> <term>
  gfm metadata-id-block-mmap <metadata.gfmmeta> <tag|comment> <term> <block-index>
  gfm metadata-verify <metadata.gfmmeta>
  gfm search-content-index <records.gfmidx> <content.gfmcontent> <query>
  gfm search-content-index-adaptive <records.gfmidx> <content.gfmcontent> <query> <nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active>
  gfm search-content-index-set <records.gfmidx> <query> <content.gfmcontent...>
  gfm search-content-index-manifest <records.gfmidx> <manifest.gfmmanifest> <query>
  gfm content-ids <content.gfmcontent> <term>
  gfm content-ids-mmap <content.gfmcontent> <term>
  gfm content-ids-mmap-set <term> <content.gfmcontent...>
  gfm content-ids-mmap-manifest <manifest.gfmmanifest> <term>
  gfm content-id-block-mmap <content.gfmcontent> <term> <block-index>
  gfm content-verify <content.gfmcontent>
  gfm config-path
  gfm config-init [config.toml]
  gfm config-check [config.toml]
  gfm config-dump [config.toml]
  gfm diagnostics-index-rebuild <root> <records.gfmidx> [content.gfmcontent]
  gfm diagnostics-index-rebuild-adaptive <root> <records.gfmidx> <nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active> [content.gfmcontent]
  gfm diagnostics-index-recovery-plan <root> <records.gfmidx> <state.gfmstate> [quarantine-dir]
  gfm diagnostics-index-recover <root> <records.gfmidx> <state.gfmstate> [quarantine-dir]
  gfm diagnostics-index-recover-adaptive <root> <records.gfmidx> <state.gfmstate> <nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active> [quarantine-dir]
  gfm diagnostics-trace-export <trace.json>
  gfm diagnostics-parity-baseline <config.toml> <baseline-root> <macos-build>
  gfm diagnostics-storage-inspect <records.gfmidx|content.gfmcontent>
  gfm support-check
  gfm permission-onboarding
  gfm security-scope <path> [read|write|index|preview|operate]
  gfm mac-bridges
  gfm native-icon <path>
  gfm fileprovider-state <path>
  gfm volume-discovery [paths...]
  gfm volume-index-policy <external:disabled|opt-in|enabled> <network:disabled|opt-in|enabled> [opt-in:path...] [paths...]
  gfm spotlight-reconcile <path> [spotlight-fixture.tsv]
  gfm preview-check <path> [icon|thumbnail|quick-look|text]
  gfm quicklook-session <path>
  gfm thumbnail-generation <path>
  gfm preview-schedule
  gfm macrobench <workspace> [smoke|standard]
  gfm macrobench-fixture <workspace> [smoke|standard|million]
  gfm parity-fixture <workspace> [smoke|standard]
  gfm pixel-diff <expected.rgba> <actual.rgba> <width> <height> [mask.tsv]
  gfm pixel-threshold-check <layout|text|icon|selection|focus|hover|toolbar|thumbnail|preview> <expected.rgba> <actual.rgba> <width> <height> [mask.tsv]
  gfm parity-gate <manifest.tsv>
  gfm parity-review <manifest.tsv> <output-dir>
  gfm parity-profile <macos-build> [system|light|dark] [1x|2x|3x] [srgb|display-p3]
  gfm regression-gate <workspace> [smoke|standard]
  gfm large-sidecar-gate <workspace> <synthetic-records>
  gfm release-policy
  gfm release-validate <GFM.app> [--allow-unsigned] [--skip-notarization] [--skip-gatekeeper]
  gfm bundle-app <executable> <GFM.icns> <output-dir> [--ad-hoc|--unsigned|developer-id]
  gfm register-app <GFM.app>
  gfm notarize-app <GFM.app> <output-dir> --keychain-profile <profile>
  gfm notarize-app <GFM.app> <output-dir> --apple-id <email> --team-id <team> --password <password>
  gfm notarize-app <GFM.app> <output-dir> --api-key <AuthKey.p8> --key-id <key> --issuer <issuer>
  gfm jobs-recover [jobs.journal]
  gfm jobs-retry-plan <max-attempts> <attempts> <failure-message...>
  gfm jobs-payload-catalog <catalog.gfmjobs>
  gfm jobs-fairness-plan
  gfm jobs-progress-snapshot <progress.gfmprogress>
  gfm jobs-cancel-tree
  gfm jobs-runtime-retry-probe <attempt-state> [<nominal|elevated|saturated> <nominal|fair|serious|critical> <ac|battery|low> <idle|active>]
  gfm ops-recover [ops.journal] [--retry-failed] [--max-attempts N]
  gfm watch-once <root>
  gfm copy <source> <destination> [--replace|--keep-both|--merge|--skip]
  gfm move <source> <destination> [--replace|--keep-both|--merge|--skip]
  gfm rename <source> <destination> [--replace|--keep-both|--merge|--skip]
  gfm delete <path>
  gfm trash <path>
  gfm empty-trash <trash-dir>
  gfm restore <trash-entry> [original-path] [--replace|--keep-both|--merge|--skip]"
    );
}
