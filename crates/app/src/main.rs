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
use gfm_mac::{FileEventStream, WatchRoot};
use gfm_ops::{ConflictPolicy, Operation, OperationContext, Operator};
use gfm_packaging::{
    build_app_bundle, notarize_app_bundle, register_launch_services, AppBundleSpec,
    NotarizationCredentials, NotarizationSpec, SigningIdentity,
};
use gfm_store::ContentArchive;
use gfm_testkit::{
    run_macrobench, run_regression_gate, MacrobenchOptions, MacrobenchScale, MacrobenchStage,
    RegressionGateOptions,
};
use gfm_types::{FileKind, Result, SearchHit};
use std::env;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn main() {
    if let Err(err) = run() {
        eprintln!("gfm: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
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
        Some("bundle-app") => {
            let executable = required_path(args.next(), "bundle-app requires an executable path")?;
            let icon = required_path(args.next(), "bundle-app requires an icon path")?;
            let output_dir = required_path(args.next(), "bundle-app requires an output directory")?;
            let mut spec = AppBundleSpec::new(executable, icon, output_dir);
            spec.signing_identity = match args.next().as_deref() {
                Some("--unsigned") => SigningIdentity::Unsigned,
                Some("--ad-hoc") | None => SigningIdentity::AdHoc,
                Some(identity) => SigningIdentity::DeveloperId(identity.to_string()),
            };
            let bundle = build_app_bundle(&spec)?;
            println!(
                "{}\t{}\t{}",
                bundle.app_path.display(),
                bundle.executable_path.display(),
                bundle.signed
            );
        }
        Some("register-app") => {
            let app_path = required_path(args.next(), "register-app requires an .app path")?;
            register_launch_services(&app_path)?;
            println!("{}", app_path.display());
        }
        Some("notarize-app") => {
            let app_path = required_path(args.next(), "notarize-app requires an .app path")?;
            let output_dir =
                required_path(args.next(), "notarize-app requires an output directory")?;
            let credentials = notarization_credentials(&mut args)?;
            let ticket =
                notarize_app_bundle(&NotarizationSpec::new(app_path, output_dir, credentials))?;
            println!(
                "{}\t{}\t{:?}\t{}",
                ticket.submission_id,
                ticket.archive_path.display(),
                ticket.status,
                ticket.stapled
            );
        }
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
        .ok_or_else(|| gfm_types::GfmError::Format(message.to_string()))
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

fn notarization_credentials(
    args: &mut impl Iterator<Item = String>,
) -> Result<NotarizationCredentials> {
    match args.next().as_deref() {
        Some("--keychain-profile") => args
            .next()
            .map(NotarizationCredentials::KeychainProfile)
            .ok_or_else(|| {
                gfm_types::GfmError::Format(
                    "notarize-app --keychain-profile requires a profile name".to_string(),
                )
            }),
        Some("--apple-id") => {
            let apple_id = args.next().ok_or_else(|| {
                gfm_types::GfmError::Format(
                    "notarize-app --apple-id requires an Apple ID".to_string(),
                )
            })?;
            let Some(team_flag) = args.next() else {
                return Err(gfm_types::GfmError::Format(
                    "notarize-app --apple-id requires --team-id".to_string(),
                ));
            };
            if team_flag != "--team-id" {
                return Err(gfm_types::GfmError::Format(format!(
                    "expected --team-id after --apple-id, got {team_flag}"
                )));
            }
            let team_id = args.next().ok_or_else(|| {
                gfm_types::GfmError::Format("notarize-app --team-id requires a team ID".to_string())
            })?;
            let Some(password_flag) = args.next() else {
                return Err(gfm_types::GfmError::Format(
                    "notarize-app --apple-id requires --password".to_string(),
                ));
            };
            if password_flag != "--password" {
                return Err(gfm_types::GfmError::Format(format!(
                    "expected --password after --team-id, got {password_flag}"
                )));
            }
            let password = args.next().ok_or_else(|| {
                gfm_types::GfmError::Format(
                    "notarize-app --password requires an app-specific password".to_string(),
                )
            })?;
            Ok(NotarizationCredentials::AppleId {
                apple_id,
                team_id,
                password,
            })
        }
        Some("--api-key") => {
            let key_path = required_path(
                args.next(),
                "notarize-app --api-key requires a .p8 key path",
            )?;
            let Some(key_id_flag) = args.next() else {
                return Err(gfm_types::GfmError::Format(
                    "notarize-app --api-key requires --key-id".to_string(),
                ));
            };
            if key_id_flag != "--key-id" {
                return Err(gfm_types::GfmError::Format(format!(
                    "expected --key-id after --api-key, got {key_id_flag}"
                )));
            }
            let key_id = args.next().ok_or_else(|| {
                gfm_types::GfmError::Format("notarize-app --key-id requires a key ID".to_string())
            })?;
            let Some(issuer_flag) = args.next() else {
                return Err(gfm_types::GfmError::Format(
                    "notarize-app --api-key requires --issuer".to_string(),
                ));
            };
            if issuer_flag != "--issuer" {
                return Err(gfm_types::GfmError::Format(format!(
                    "expected --issuer after --key-id, got {issuer_flag}"
                )));
            }
            let issuer_id = args.next().ok_or_else(|| {
                gfm_types::GfmError::Format(
                    "notarize-app --issuer requires an issuer ID".to_string(),
                )
            })?;
            Ok(NotarizationCredentials::ApiKey {
                key_id,
                issuer_id,
                key_path,
            })
        }
        Some(other) => Err(gfm_types::GfmError::Format(format!(
            "unknown notarize-app credential flag {other}"
        ))),
        None => Err(gfm_types::GfmError::Format(
            "notarize-app requires --keychain-profile, --apple-id, or --api-key credentials"
                .to_string(),
        )),
    }
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

fn macrobench_stage(stage: MacrobenchStage) -> &'static str {
    match stage {
        MacrobenchStage::IndexBuild => "index-build",
        MacrobenchStage::HotSearch => "hot-search",
        MacrobenchStage::StreamSearch => "stream-search",
        MacrobenchStage::ContentSearch => "content-search",
    }
}

fn print_usage() {
    println!(
        "gfm commands:
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
  gfm macrobench <workspace> [smoke|standard]
  gfm regression-gate <workspace> [smoke|standard]
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
