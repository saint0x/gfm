use gfm_config::ConfigStore;
use gfm_content::Extractor;
use gfm_diagnostics::{
    export_operator_trace, inspect_storage, rebuild_index, select_parity_baseline, RebuildSpec,
    StorageInspection,
};
use gfm_fs::read_directory;
use gfm_index::{
    BackgroundContentIndexer, ContentIndexJobSpec, ContentIndexReport, Indexer, SearchStreamStage,
};
use gfm_jobs::{
    JobJournal, Priority, RecoveryReason, RetriableTask, RetryPolicy, Scheduler, TaskStatus,
    WorkerPool,
};
use gfm_mac::{
    current_host_profile, current_permission_onboarding, FileEventStream, SupportMatrix, WatchRoot,
};
use gfm_ops::{ConflictPolicy, Operation, OperationContext, Operator};
use gfm_preview::{
    decide_invalidation, decide_preview_security, security_input_for_path,
    PreviewInvalidationEvent, PreviewKind, PreviewRequestKey, PreviewScheduler,
    PreviewSchedulingPolicy, PreviewSecurityPolicy, PreviewTask, Rect, Viewport,
};
use gfm_store::ContentArchive;
use gfm_testkit::{
    diff_rgba_files, evaluate_pixel_threshold, materialize_parity_fixture, read_mask_file,
    run_macrobench, run_parity_gate_manifest, run_regression_gate,
    write_parity_review_bundle_manifest, ColorProfile, DisplayScale, MacOsParityProfile,
    MacrobenchOptions, MacrobenchScale, MacrobenchStage, ParityAppearance, ParityFixtureOptions,
    ParityFixtureScale, ParitySurface, PixelDiffOptions, PixelDriftThreshold, PixelSize,
    RegressionGateOptions,
};
use gfm_types::{FileId, FileKind, GfmError, Result, SearchHit, VolumeId};
use gfm_ui::{
    AppLaunchSpec, ColumnSource, ColumnViewContract, ColumnViewOptions, ContextMenuContract,
    ContextMenuInput, ContextSurface, DialogContract, DialogSurface, GalleryViewContract,
    GalleryViewOptions, IconViewContract, IconViewOptions, ListViewContract, ListViewOptions,
    MenuContract, SearchResultsBatch, SearchResultsContract, SearchResultsOptions,
    SearchResultsStage, SidebarContract, TitlebarContract, ToolbarContract,
    WindowLifecycleContract, WindowSessionContract, WindowSessionStore,
};
use std::env;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

mod packaging;

fn main() {
    if let Err(err) = run() {
        eprintln!("gfm: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("app") => {
            let spec = match args.next() {
                Some(path) => AppLaunchSpec::new(path),
                None => AppLaunchSpec::default(),
            };
            gfm_ui::run_native(spec)?;
        }
        Some("ui-contract") => {
            let spec = match args.next() {
                Some(path) => AppLaunchSpec::new(path),
                None => AppLaunchSpec::default(),
            };
            println!("{}", WindowLifecycleContract::from_spec(&spec)?.as_tsv());
        }
        Some("ui-menu-contract") => {
            println!("{}", MenuContract::finder_default().as_tsv());
        }
        Some("ui-context-menu-contract") => {
            let surface = args
                .next()
                .unwrap_or_else(|| "file".to_string())
                .parse::<ContextSurface>()
                .map_err(GfmError::Format)?;
            let selection_count = args
                .next()
                .map(|value| parse_u16(&value, "selection-count"))
                .transpose()?
                .unwrap_or(match surface {
                    ContextSurface::Empty => 0,
                    _ => 1,
                });
            let writable = args
                .next()
                .map(|value| parse_bool(&value, "writable"))
                .transpose()?
                .unwrap_or(true);
            let ejectable = args
                .next()
                .map(|value| parse_bool(&value, "ejectable"))
                .transpose()?
                .unwrap_or(surface == ContextSurface::Volume);
            let has_clipboard_items = args
                .next()
                .map(|value| parse_bool(&value, "has-clipboard-items"))
                .transpose()?
                .unwrap_or(true);

            let input = ContextMenuInput::new(surface)
                .with_selection_count(selection_count)
                .with_writable(writable)
                .with_ejectable(ejectable)
                .with_clipboard_items(has_clipboard_items);
            println!("{}", ContextMenuContract::finder_default(input).as_tsv());
        }
        Some("ui-dialog-contract") => {
            let surface = args
                .next()
                .unwrap_or_else(|| "alert".to_string())
                .parse::<DialogSurface>()
                .map_err(GfmError::Format)?;
            println!("{}", DialogContract::finder_default(surface).as_tsv());
        }
        Some("ui-titlebar-contract") => {
            let spec = match args.next() {
                Some(path) => AppLaunchSpec::new(path),
                None => AppLaunchSpec::default(),
            };
            println!("{}", TitlebarContract::from_spec(&spec)?.as_tsv());
        }
        Some("ui-session-contract") => {
            let spec = match args.next() {
                Some(path) => AppLaunchSpec::new(path),
                None => AppLaunchSpec::default(),
            };
            let store = args
                .next()
                .map(WindowSessionStore::new)
                .unwrap_or_else(WindowSessionStore::platform_default);
            println!(
                "{}",
                WindowSessionContract::from_spec(&spec, &store, 0).as_tsv()
            );
        }
        Some("ui-toolbar-contract") => {
            let path = match args.next() {
                Some(path) => PathBuf::from(path),
                None => env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
            };
            println!("{}", ToolbarContract::finder_default(path).as_tsv());
        }
        Some("ui-sidebar-contract") => {
            let path = match args.next() {
                Some(path) => PathBuf::from(path),
                None => env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
            };
            println!("{}", SidebarContract::discover(path).as_tsv());
        }
        Some("ui-icon-view-contract") => {
            let path = required_path(
                args.next(),
                "ui-icon-view-contract requires a directory path",
            )?;
            let columns = args
                .next()
                .map(|value| parse_u16(&value, "columns"))
                .transpose()?
                .unwrap_or(6);
            let viewport_rows = args
                .next()
                .map(|value| parse_u16(&value, "viewport-rows"))
                .transpose()?
                .unwrap_or(4);
            let scroll_row = args
                .next()
                .map(|value| parse_u16(&value, "scroll-row"))
                .transpose()?
                .unwrap_or(0);
            let page = read_directory(&path)?;
            let options = IconViewOptions::default()
                .with_columns(columns)
                .with_viewport_rows(viewport_rows)
                .with_scroll_row(scroll_row);
            println!(
                "{}",
                IconViewContract::from_records(&page.entries, options).as_tsv()
            );
        }
        Some("ui-list-view-contract") => {
            let path = required_path(
                args.next(),
                "ui-list-view-contract requires a directory path",
            )?;
            let viewport_rows = args
                .next()
                .map(|value| parse_u16(&value, "viewport-rows"))
                .transpose()?
                .unwrap_or(24);
            let scroll_row = args
                .next()
                .map(|value| parse_u32(&value, "scroll-row"))
                .transpose()?
                .unwrap_or(0);
            let page = read_directory(&path)?;
            let options = ListViewOptions::default()
                .with_viewport_rows(viewport_rows)
                .with_scroll_row(scroll_row);
            println!(
                "{}",
                ListViewContract::from_records(&page.entries, options).as_tsv()
            );
        }
        Some("ui-column-view-contract") => {
            let path = required_path(
                args.next(),
                "ui-column-view-contract requires a directory path",
            )?;
            let viewport_rows = args
                .next()
                .map(|value| parse_u16(&value, "viewport-rows"))
                .transpose()?
                .unwrap_or(24);
            let scroll_row = args
                .next()
                .map(|value| parse_u32(&value, "scroll-row"))
                .transpose()?
                .unwrap_or(0);
            let selected_name = args.next();
            let page = read_directory(&path)?;
            let selected_record = selected_name
                .as_deref()
                .and_then(|name| page.entries.iter().find(|record| record.name == name));
            let mut sources = vec![ColumnSource::new(path.clone(), page.entries.clone())
                .with_scroll_row(scroll_row)
                .with_selected(selected_record.map(|record| record.id))];
            if let Some(record) =
                selected_record.filter(|record| record.kind == FileKind::Directory)
            {
                let child_page = read_directory(&record.path)?;
                sources.push(ColumnSource::new(record.path.clone(), child_page.entries));
            }
            let options = ColumnViewOptions::default().with_viewport_rows(viewport_rows);
            println!(
                "{}",
                ColumnViewContract::from_sources(sources, options).as_tsv()
            );
        }
        Some("ui-gallery-view-contract") => {
            let path = required_path(
                args.next(),
                "ui-gallery-view-contract requires a directory path",
            )?;
            let viewport_items = args
                .next()
                .map(|value| parse_u16(&value, "viewport-items"))
                .transpose()?
                .unwrap_or(8);
            let scroll_item = args
                .next()
                .map(|value| parse_u32(&value, "scroll-item"))
                .transpose()?
                .unwrap_or(0);
            let selected_name = args.next();
            let page = read_directory(&path)?;
            let selected = selected_name
                .as_deref()
                .and_then(|name| page.entries.iter().find(|record| record.name == name))
                .map(|record| record.id);
            let options = GalleryViewOptions::default()
                .with_viewport_items(viewport_items)
                .with_scroll_item(scroll_item)
                .with_selected(selected);
            println!(
                "{}",
                GalleryViewContract::from_records(&page.entries, options).as_tsv()
            );
        }
        Some("ui-search-results-contract") => {
            let root = required_path(
                args.next(),
                "ui-search-results-contract requires a root path",
            )?;
            let query = args.next().ok_or_else(|| {
                gfm_types::GfmError::Format(
                    "ui-search-results-contract requires a query string".to_string(),
                )
            })?;
            let viewport_rows = args
                .next()
                .map(|value| parse_u16(&value, "viewport-rows"))
                .transpose()?
                .unwrap_or(24);
            let scroll_row = args
                .next()
                .map(|value| parse_u32(&value, "scroll-row"))
                .transpose()?
                .unwrap_or(0);
            let snapshot = Indexer::default().build(root)?;
            let batches = snapshot
                .stream_search(&query, 50)?
                .into_iter()
                .map(|batch| SearchResultsBatch::new(search_results_stage(batch.stage), batch.hits))
                .collect();
            let options = SearchResultsOptions::new(query)
                .with_viewport_rows(viewport_rows)
                .with_scroll_row(scroll_row);
            println!(
                "{}",
                SearchResultsContract::from_batches(batches, options).as_tsv()
            );
        }
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
            let journal = JobJournal::new(default_job_journal_path());
            let spec = ContentIndexJobSpec::new(root, segment_dir, records, content);
            spec.write(default_content_job_path())?;
            let (report, inaccessible) = run_content_job(&spec, &journal)?;
            eprintln!(
                "background-content-indexed {} files; skipped {}; segments {}; terms {}; journal {}; {} inaccessible",
                report.indexed,
                report.skipped,
                report.segments.len(),
                report.terms,
                journal.path().display(),
                inaccessible
            );
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
                let (report, _) = run_content_job(&spec, &journal)?;
                eprintln!(
                    "resumed-background-content-indexed {} files; skipped {}; segments {}; terms {}; recoverable {}",
                    report.indexed,
                    report.skipped,
                    report.segments.len(),
                    report.terms,
                    recoverable.len()
                );
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
            let snapshot = Indexer::default().build(root)?;
            let mut live = snapshot.into_live();
            let indexed = live.index_content(&Extractor::default())?;
            eprintln!("content-indexed {indexed} files");
            for hit in live.search_with_snippets(&query, 50, &Extractor::default(), 96)? {
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
            let live = Indexer::default().load_live_with_content(records, content)?;
            for hit in live.search_with_snippets(&query, 50, &Extractor::default(), 96)? {
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
            let report = rebuild_index(&spec)?;
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
        Some("preview-check") => {
            let path = required_path(args.next(), "preview-check requires a path")?;
            let kind = parse_preview_kind(args.next())?;
            let input = security_input_for_path(&path, kind);
            let decision = decide_preview_security(&PreviewSecurityPolicy::default(), &input);
            let invalidation = decide_invalidation(PreviewInvalidationEvent {
                content_changed: true,
                ..PreviewInvalidationEvent::default()
            });
            println!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                kind.as_str(),
                input.trust.as_str(),
                input.is_executable,
                input.is_remote,
                decision.as_str(),
                invalidation.invalidate_disk,
                path.display()
            );
        }
        Some("preview-schedule") => {
            let mut scheduler = PreviewScheduler::new(PreviewSchedulingPolicy {
                max_visible: 8,
                max_prefetch: 8,
                cancel_offscreen: true,
            })?;
            let viewport = Viewport::new(Rect::new(0, 0, 100, 100), 64);
            for decision in scheduler.schedule(
                viewport,
                vec![
                    preview_task(1, 0, 0),
                    preview_task(2, 0, 130),
                    preview_task(3, 0, 260),
                ],
            ) {
                println!(
                    "{}\t{}",
                    decision.as_str(),
                    preview_decision_priority(&decision)
                );
            }
        }
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
                "fixture\t{}\tfiles\t{}\tindex-bytes\t{}\tpassed\t{}",
                run.macrobench.fixture_root.display(),
                run.macrobench.files_materialized,
                run.index_size_bytes,
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
        Some("watch-once") => {
            let root = required_path(args.next(), "watch-once requires a root path")?;
            let stream = FileEventStream::watch(&[WatchRoot::tree(root)])?;
            let event = stream.recv()?;
            println!("{}\t{}", event_marker(&event.kind), event.path.display());
        }
        Some("copy") => {
            let from = required_path(args.next(), "copy requires a source path")?;
            let to = required_path(args.next(), "copy requires a destination path")?;
            execute_operation(Operation::Copy { from, to }, ConflictPolicy::Fail)?;
        }
        Some("move") => {
            let from = required_path(args.next(), "move requires a source path")?;
            let to = required_path(args.next(), "move requires a destination path")?;
            execute_operation(Operation::Move { from, to }, ConflictPolicy::Fail)?;
        }
        Some("rename") => {
            let from = required_path(args.next(), "rename requires a source path")?;
            let to = required_path(args.next(), "rename requires a destination path")?;
            execute_operation(Operation::Rename { from, to }, ConflictPolicy::Fail)?;
        }
        Some("delete") => {
            let path = required_path(args.next(), "delete requires a path")?;
            execute_operation(Operation::Delete { path }, ConflictPolicy::Fail)?;
        }
        Some("trash") => {
            let path = required_path(args.next(), "trash requires a path")?;
            execute_operation(Operation::Trash { path }, ConflictPolicy::Fail)?;
        }
        _ => print_usage(),
    }
    Ok(())
}

fn required_path(value: Option<String>, message: &str) -> Result<PathBuf> {
    value
        .map(PathBuf::from)
        .ok_or_else(|| GfmError::Format(message.to_string()))
}

fn parse_u16(value: &str, name: &str) -> Result<u16> {
    value
        .parse()
        .map_err(|_| GfmError::Format(format!("{name} must be an unsigned 16-bit integer")))
}

fn parse_u32(value: &str, name: &str) -> Result<u32> {
    value
        .parse()
        .map_err(|_| GfmError::Format(format!("{name} must be an unsigned 32-bit integer")))
}

fn parse_u32_arg(value: Option<String>, message: &str) -> Result<u32> {
    let value = value.ok_or_else(|| GfmError::Format(message.to_string()))?;
    value
        .parse()
        .map_err(|_| GfmError::Format(format!("{message}; got `{value}`")))
}

fn parse_bool(value: &str, name: &str) -> Result<bool> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(GfmError::Format(format!("{name} must be true or false"))),
    }
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

fn execute_operation(operation: Operation, conflict: ConflictPolicy) -> Result<()> {
    let journal = default_journal_path();
    let operator = Operator::new(OperationContext::new(journal).with_conflict(conflict));
    let entry = operator.execute(operation)?;
    println!("{}\t{}", entry.id, operation_status(entry.status));
    Ok(())
}

fn run_content_job(
    spec: &ContentIndexJobSpec,
    journal: &JobJournal,
) -> Result<(ContentIndexReport, usize)> {
    let snapshot = Indexer::default().build(&spec.root)?;
    let inaccessible = snapshot.inaccessible.len();
    snapshot.save(&spec.records_path)?;
    let worker = BackgroundContentIndexer::new(Extractor::default(), spec.options());
    let content_report = Arc::new(Mutex::new(None));
    let content_report_task = Arc::clone(&content_report);
    let mut scheduler = Scheduler::new();
    let job = scheduler.schedule(Priority::Background, "background content index");
    let tasks: Vec<_> = scheduler
        .drain_ready()
        .into_iter()
        .map(|scheduled| {
            let snapshot = snapshot.clone();
            let segment_dir = spec.segment_dir.clone();
            let content = spec.content_path.clone();
            let worker = worker.clone();
            let content_report_task = Arc::clone(&content_report_task);
            RetriableTask::new(scheduled, move |cancellation| {
                let report = worker.run_and_compact(
                    &snapshot,
                    segment_dir.clone(),
                    content.clone(),
                    &cancellation,
                )?;
                *content_report_task
                    .lock()
                    .expect("content index report lock poisoned") = Some(report);
                Ok(())
            })
        })
        .collect();
    let worker_report =
        WorkerPool::new(1).run_retriable(tasks, journal, RetryPolicy { max_attempts: 2 });
    let outcome = worker_report
        .outcomes
        .iter()
        .find(|outcome| outcome.id == job.id)
        .ok_or_else(|| {
            gfm_types::GfmError::Format("background content index job did not run".to_string())
        })?;
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
    Ok((report, inaccessible))
}

fn default_journal_path() -> PathBuf {
    std::env::var_os("GFM_OPS_JOURNAL")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("gfm-ops.journal"))
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

fn operation_status(status: gfm_ops::OperationStatus) -> &'static str {
    match status {
        gfm_ops::OperationStatus::Started => "started",
        gfm_ops::OperationStatus::Completed => "completed",
        gfm_ops::OperationStatus::Failed => "failed",
    }
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

fn stream_stage(stage: SearchStreamStage) -> &'static str {
    match stage {
        SearchStreamStage::Hot => "hot",
        SearchStreamStage::Deep => "deep",
    }
}

fn search_results_stage(stage: SearchStreamStage) -> SearchResultsStage {
    match stage {
        SearchStreamStage::Hot => SearchResultsStage::Hot,
        SearchStreamStage::Deep => SearchResultsStage::Deep,
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

fn parse_preview_kind(value: Option<String>) -> Result<PreviewKind> {
    match value.as_deref() {
        Some("icon") | None => Ok(PreviewKind::Icon),
        Some("thumbnail") => Ok(PreviewKind::Thumbnail),
        Some("quick-look") => Ok(PreviewKind::QuickLook),
        Some("text") => Ok(PreviewKind::Text),
        Some(other) => Err(gfm_types::GfmError::Format(format!(
            "preview kind must be icon, thumbnail, quick-look, or text; got `{other}`"
        ))),
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

fn preview_task(node: u64, x: i32, y: i32) -> PreviewTask {
    PreviewTask::new(
        PreviewRequestKey::new(
            FileId::new(VolumeId(1), node),
            PathBuf::from(format!("{node}.preview")),
            PreviewKind::Thumbnail,
        ),
        Rect::new(x, y, 32, 32),
    )
}

fn preview_decision_priority(decision: &gfm_preview::PreviewTaskDecision) -> &'static str {
    match decision {
        gfm_preview::PreviewTaskDecision::Scheduled { priority, .. }
        | gfm_preview::PreviewTaskDecision::Coalesced { priority, .. } => priority.as_str(),
        gfm_preview::PreviewTaskDecision::Cancelled { reason, .. } => reason,
    }
}

fn print_usage() {
    println!(
        "gfm commands:
  gfm app [path]
  gfm ui-contract [path]
  gfm ui-menu-contract
  gfm ui-context-menu-contract [file|folder|volume|sidebar|empty|selection|search-result|trash] [selection-count] [writable] [ejectable] [has-clipboard-items]
  gfm ui-dialog-contract [alert|rename|popover|disclosure|progress|conflict|permission]
  gfm ui-titlebar-contract [path]
  gfm ui-session-contract [path] [window-session.tsv]
  gfm ui-toolbar-contract [path]
  gfm ui-sidebar-contract [path]
  gfm ui-icon-view-contract <path> [columns] [viewport-rows] [scroll-row]
  gfm list [path]
  gfm index <root> <output.gfmidx>
  gfm index-content <root> <records.gfmidx> <content.gfmcontent>
  gfm index-content-segment <root> <output.gfmseg>
  gfm compact-content <output.gfmcontent> <segments.gfmseg...>
  gfm index-content-background <root> <segment-dir> <records.gfmidx> <content.gfmcontent>
  gfm resume-content-background [content.job] [jobs.journal]
  gfm search <root> <query>
  gfm search-stream <root> <query>
  gfm search-content <root> <query>
  gfm search-index <index.gfmidx> <query>
  gfm search-content-index <records.gfmidx> <content.gfmcontent> <query>
  gfm content-ids <content.gfmcontent> <term>
  gfm config-path
  gfm config-init [config.toml]
  gfm config-check [config.toml]
  gfm config-dump [config.toml]
  gfm diagnostics-index-rebuild <root> <records.gfmidx> [content.gfmcontent]
  gfm diagnostics-trace-export <trace.json>
  gfm diagnostics-parity-baseline <config.toml> <baseline-root> <macos-build>
  gfm diagnostics-storage-inspect <records.gfmidx|content.gfmcontent>
  gfm support-check
  gfm permission-onboarding
  gfm preview-check <path> [icon|thumbnail|quick-look|text]
  gfm preview-schedule
  gfm macrobench <workspace> [smoke|standard]
  gfm parity-fixture <workspace> [smoke|standard]
  gfm pixel-diff <expected.rgba> <actual.rgba> <width> <height> [mask.tsv]
  gfm pixel-threshold-check <layout|text|icon|selection|focus|hover|toolbar|thumbnail|preview> <expected.rgba> <actual.rgba> <width> <height> [mask.tsv]
  gfm parity-gate <manifest.tsv>
  gfm parity-review <manifest.tsv> <output-dir>
  gfm parity-profile <macos-build> [system|light|dark] [1x|2x|3x] [srgb|display-p3]
  gfm regression-gate <workspace> [smoke|standard]
  gfm release-policy
  gfm release-validate <GFM.app> [--allow-unsigned] [--skip-notarization] [--skip-gatekeeper]
  gfm bundle-app <executable> <GFM.icns> <output-dir> [--ad-hoc|--unsigned|developer-id]
  gfm register-app <GFM.app>
  gfm notarize-app <GFM.app> <output-dir> --keychain-profile <profile>
  gfm notarize-app <GFM.app> <output-dir> --apple-id <email> --team-id <team> --password <password>
  gfm notarize-app <GFM.app> <output-dir> --api-key <AuthKey.p8> --key-id <key> --issuer <issuer>
  gfm jobs-recover [jobs.journal]
  gfm watch-once <root>
  gfm copy <source> <destination>
  gfm move <source> <destination>
  gfm rename <source> <destination>
  gfm delete <path>
  gfm trash <path>"
    );
}
