use gfm_config::ConfigStore;
use gfm_content::QuarantineFailureKind;
use gfm_fs::read_directory;
use gfm_index::{
    BatteryState, EventBackpressureQueue, EventPriority, FseventsCursor, FseventsCursorHealth,
    IndexMountState, IndexVolumeClass, IndexVolumeDescriptor, IndexVolumeState, Indexer,
    IoPressure, LiveIndex, ThermalState, UserActivity,
};
use gfm_jobs::{
    JobBatteryState, JobIoPressure, JobThermalState, JobUserActivity, Priority, SchedulingPressure,
};
use gfm_mac::{
    current_host_profile, current_permission_onboarding, FileEventStream, MountState,
    SupportMatrix, VolumeDescriptor, VolumeKind, WatchRoot,
};
use gfm_testkit::{
    diff_rgba_files, evaluate_pixel_threshold, materialize_macrobench_fixture_report,
    materialize_parity_fixture, read_mask_file, run_large_sidecar_gate, run_macrobench,
    run_parity_gate_manifest, run_regression_gate, write_parity_review_bundle_manifest,
    ColorProfile, DisplayScale, LargeSidecarGateOptions, MacOsParityProfile, MacrobenchOptions,
    MacrobenchScale, MacrobenchStage, ParityAppearance, ParityFixtureOptions, ParityFixtureScale,
    ParitySurface, PixelDiffOptions, PixelDriftThreshold, PixelSize, RegressionGateOptions,
};
use gfm_types::{FileEvent, FileEventKind, FileKind, GfmError, Result, VolumeId};
use std::env;
use std::io::Write;
use std::path::{Path, PathBuf};

mod archive;
mod content;
mod diagnostics;
mod extract;
mod interface;
mod jobs;
mod manifest;
mod operation;
mod packaging;
mod platform;
mod runtime;
mod search;

use runtime::run_volume_task;

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
        Some(command) if search::run(command, &mut args)? => {}
        Some(command) if archive::run(command, &mut args)? => {}
        Some(command) if content::run(command, &mut args)? => {}
        Some(command) if manifest::run(command, &mut args)? => {}
        Some(command) if diagnostics::run(command, &mut args)? => {}
        Some(command) if jobs::run(command, &mut args)? => {}
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

pub(crate) fn optional_path_arg(value: Option<String>, message: &str) -> Result<Option<PathBuf>> {
    let value = value.ok_or_else(|| GfmError::Format(message.to_string()))?;
    Ok((value != "-").then(|| PathBuf::from(value)))
}

pub(crate) fn required_string(value: Option<String>, message: &str) -> Result<String> {
    value.ok_or_else(|| GfmError::Format(message.to_string()))
}

pub(crate) fn parse_optional_scheduling_pressure(
    args: &mut impl Iterator<Item = String>,
) -> Result<SchedulingPressure> {
    let Some(io) = args.next() else {
        return Ok(SchedulingPressure::default());
    };
    parse_scheduling_pressure_tail(io, args, "adaptive scheduling")
}

pub(crate) fn parse_required_scheduling_pressure(
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

pub(crate) fn parse_io_pressure(value: String) -> Result<IoPressure> {
    match value.as_str() {
        "nominal" => Ok(IoPressure::Nominal),
        "elevated" => Ok(IoPressure::Elevated),
        "saturated" => Ok(IoPressure::Saturated),
        _ => Err(GfmError::Format(format!(
            "invalid io pressure `{value}`; expected nominal, elevated, or saturated"
        ))),
    }
}

pub(crate) fn parse_thermal_state(value: String) -> Result<ThermalState> {
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

pub(crate) fn parse_battery_state(value: String) -> Result<BatteryState> {
    match value.as_str() {
        "ac" => Ok(BatteryState::AcPower),
        "battery" => Ok(BatteryState::Battery),
        "low" => Ok(BatteryState::LowPower),
        _ => Err(GfmError::Format(format!(
            "invalid battery state `{value}`; expected ac, battery, or low"
        ))),
    }
}

pub(crate) fn parse_user_activity(value: String) -> Result<UserActivity> {
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

pub(crate) fn parse_quarantine_failure_kind(
    value: &str,
    name: &str,
) -> Result<QuarantineFailureKind> {
    QuarantineFailureKind::parse(value)
        .ok_or_else(|| GfmError::Format(format!("invalid {name}: {value}")))
}

pub(crate) fn parse_u32(value: &str, name: &str) -> Result<u32> {
    value
        .parse()
        .map_err(|_| GfmError::Format(format!("{name} must be an unsigned 32-bit integer")))
}

pub(crate) fn parse_u64(value: &str, name: &str) -> Result<u64> {
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

pub(crate) fn parse_u64_arg(value: Option<String>, message: &str) -> Result<u64> {
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

pub(crate) fn parse_usize_arg(value: Option<String>, message: &str) -> Result<usize> {
    let value = value.ok_or_else(|| GfmError::Format(message.to_string()))?;
    parse_usize(&value, message)
}

fn parse_usize(value: &str, message: &str) -> Result<usize> {
    value
        .parse()
        .map_err(|_| GfmError::Format(format!("{message}; got `{value}`")))
}

pub(crate) fn config_store(value: Option<String>) -> Result<ConfigStore> {
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

fn marker(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Directory => "dir",
        FileKind::File => "file",
        FileKind::Symlink => "link",
        FileKind::Other => "other",
    }
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
