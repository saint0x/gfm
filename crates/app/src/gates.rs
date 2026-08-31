use crate::{
    access::{
        preflight_access_scope_checked_with_volume_report,
        preflight_volume_access_scope_with_report, ScopedAccessGuard,
    },
    parse_u32_arg, parse_usize_arg, required_path,
    runtime::run_volume_task_cancellable,
};
use gfm_jobs::Priority;
use gfm_mac::{AccessIntent, VolumeDiscoveryReport};
use gfm_testkit::{
    diff_rgba_files, evaluate_pixel_threshold, materialize_macrobench_fixture_report,
    materialize_parity_fixture, read_governed_mask_file, read_mask_file, run_large_sidecar_gate,
    run_macrobench, run_parity_gate_manifest, run_regression_gate, run_search_typing_benchmark,
    run_search_typing_session_benchmark, write_parity_review_bundle_manifest, ColorProfile,
    DisplayScale, LargeSidecarGateOptions, MacOsParityProfile, MacrobenchOptions, MacrobenchScale,
    MacrobenchStage, ParityAppearance, ParityFixtureOptions, ParityFixtureScale, ParitySurface,
    PixelDiffOptions, PixelDriftThreshold, PixelSize, RegressionGateOptions,
    SearchTypingBenchmarkOptions,
};
use gfm_types::{GfmError, Result};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub(crate) fn run(command: &str, args: &mut impl Iterator<Item = String>) -> Result<bool> {
    match command {
        "macrobench" => {
            let options = macrobench_options(args.next(), args.next(), "macrobench")?;
            let workspace = options.workspace.clone();
            let report = run_workspace_write_task(
                &workspace,
                "macrobench workspace",
                move |cancellation| {
                    cancellation.check()?;
                    let report = run_macrobench(&options)?;
                    cancellation.check()?;
                    Ok(report)
                },
            )?;
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
        "macrobench-fixture" => {
            let (root, scale) =
                macrobench_fixture_options(args.next(), args.next(), "macrobench-fixture")?;
            let workspace = root.clone();
            let report = run_workspace_write_task(
                &workspace,
                "macrobench fixture workspace",
                move |cancellation| {
                    cancellation.check()?;
                    let report = materialize_macrobench_fixture_report(root, scale)?;
                    cancellation.check()?;
                    Ok(report)
                },
            )?;
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
        "parity-fixture" => {
            let options = parity_fixture_options(args.next(), args.next(), "parity-fixture")?;
            let workspace = options.workspace.clone();
            let report = run_workspace_write_task(
                &workspace,
                "parity fixture workspace",
                move |cancellation| {
                    cancellation.check()?;
                    let report = materialize_parity_fixture(&options)?;
                    cancellation.check()?;
                    Ok(report)
                },
            )?;
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
        "pixel-diff" => {
            let expected = required_path(args.next(), "pixel-diff requires an expected RGBA path")?;
            let actual = required_path(args.next(), "pixel-diff requires an actual RGBA path")?;
            let width = parse_u32_arg(args.next(), "pixel-diff requires a width")?;
            let height = parse_u32_arg(args.next(), "pixel-diff requires a height")?;
            let size = PixelSize::new(width, height);
            let mask_path = args.next().map(PathBuf::from);
            let access_reports =
                pixel_diff_access_reports(&expected, &actual, mask_path.as_deref());
            access_reports.preflight_volumes()?;
            let volume = access_reports.first_volume();
            let report = run_volume_task_cancellable(
                volume,
                Priority::Visible,
                "pixel diff",
                move |cancellation| {
                    cancellation.check()?;
                    let _access = access_reports.access_checked(|| cancellation.check())?;
                    cancellation.check()?;
                    let masks = mask_path
                        .as_ref()
                        .map(|path| read_mask_file(path, size))
                        .transpose()?
                        .unwrap_or_default();
                    let options = PixelDiffOptions::strict(size).with_masks(masks);
                    diff_rgba_files(expected, actual, &options)
                },
            )?;
            println!(
                "pixel-diff\t{}x{}\ttotal={}\tmismatched={}\tunmasked={}\tmasked={}\tmax-channel-delta={}\tpassed={}",
                report.size.width,
                report.size.height,
                report.total_pixels,
                report.mismatched_pixels,
                report.unmasked_mismatches,
                report.masked_mismatches,
                report.max_channel_delta,
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
        "pixel-threshold-check" => {
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
            let mask_path = args.next().map(PathBuf::from);
            let access_reports =
                pixel_diff_access_reports(&expected, &actual, mask_path.as_deref());
            access_reports.preflight_volumes()?;
            let volume = access_reports.first_volume();
            let report = run_volume_task_cancellable(
                volume,
                Priority::Visible,
                "pixel threshold",
                move |cancellation| {
                    cancellation.check()?;
                    let _access = access_reports.access_checked(|| cancellation.check())?;
                    cancellation.check()?;
                    let masks = mask_path
                        .as_ref()
                        .map(|path| read_governed_mask_file(path, size))
                        .transpose()?
                        .unwrap_or_default();
                    let options = PixelDiffOptions::strict(size).with_governed_masks(masks);
                    diff_rgba_files(expected, actual, &options)
                },
            )?;
            let threshold = PixelDriftThreshold::finder_strict(surface);
            let evaluation = evaluate_pixel_threshold(&report, threshold);
            println!(
                "{}\tpassed={}\tmismatched={}\tunmasked={}\tmasked={}\tmax-channel-delta={}",
                threshold.as_tsv(),
                evaluation.passed,
                report.mismatched_pixels,
                report.unmasked_mismatches,
                report.masked_mismatches,
                report.max_channel_delta
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
        "parity-gate" => {
            let manifest = required_path(args.next(), "parity-gate requires a manifest path")?;
            let manifest_access =
                GateAccessReport::new(manifest.clone(), AccessIntent::Read, "parity gate");
            manifest_access.preflight_volume()?;
            let volume = manifest_access.volume();
            let manifest_for_worker = manifest.clone();
            let report = run_volume_task_cancellable(
                volume,
                Priority::Visible,
                "parity gate",
                move |cancellation| {
                    cancellation.check()?;
                    let _access = manifest_access.access_checked(|| cancellation.check())?;
                    cancellation.check()?;
                    run_parity_gate_manifest(&manifest_for_worker)
                },
            )?;
            println!(
                "parity-gate\tmanifest={}\tentries={}\tviolations={}\tpassed={}",
                manifest.display(),
                report.entries.len(),
                report.violations(),
                report.passed()
            );
            for entry in &report.entries {
                println!(
                    "{}\tpassed={}\tmismatched={}\tunmasked={}\tmasked={}\tmax-channel-delta={}\texpected={}\tactual={}",
                    entry.evaluation.threshold.as_tsv(),
                    entry.evaluation.passed,
                    entry.diff.mismatched_pixels,
                    entry.diff.unmasked_mismatches,
                    entry.diff.masked_mismatches,
                    entry.diff.max_channel_delta,
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
        "parity-review" => {
            let manifest = required_path(args.next(), "parity-review requires a manifest path")?;
            let output_dir =
                required_path(args.next(), "parity-review requires an output directory")?;
            let access_reports = parity_review_access_reports(&manifest, &output_dir)?;
            access_reports.preflight_volumes()?;
            let volume = access_reports.first_volume();
            let manifest_for_worker = manifest.clone();
            let output_dir_for_worker = output_dir.clone();
            let bundle = run_volume_task_cancellable(
                volume,
                Priority::Visible,
                "parity review",
                move |cancellation| {
                    cancellation.check()?;
                    let _access = access_reports.access_checked(|| cancellation.check())?;
                    cancellation.check()?;
                    write_parity_review_bundle_manifest(
                        &manifest_for_worker,
                        &output_dir_for_worker,
                    )
                },
            )?;
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
            println!("regions\t{}", bundle.region_summary_path.display());
            println!(
                "mask-justifications\t{}",
                bundle.mask_justification_path.display()
            );
            println!("visual-diffs\t{}", bundle.visual_diff_dir.display());
            println!("source-artifacts\t{}", bundle.source_artifact_dir.display());
            println!("bundle\t{}", bundle.bundle_manifest_path.display());
            if !bundle.report.passed() {
                return Err(GfmError::Format(format!(
                    "parity review captured {} violation(s)",
                    bundle.report.violations()
                )));
            }
        }
        "parity-profile" => {
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
        "regression-gate" => {
            let options = macrobench_options(args.next(), args.next(), "regression-gate")?;
            let workspace = options.workspace.clone();
            let run = run_workspace_write_task(
                &workspace,
                "regression gate workspace",
                move |cancellation| {
                    cancellation.check()?;
                    let run = run_regression_gate(&options, RegressionGateOptions::default())?;
                    cancellation.check()?;
                    Ok(run)
                },
            )?;
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
                return Err(GfmError::Format(format!(
                    "regression gate failed with {} violation(s)",
                    run.gate.violations.len()
                )));
            }
        }
        "large-sidecar-gate" => {
            let workspace =
                required_path(args.next(), "large-sidecar-gate requires a workspace path")?;
            let records = parse_usize_arg(
                args.next(),
                "large-sidecar-gate requires a synthetic record count",
            )?;
            let worker_workspace = workspace.clone();
            let report = run_workspace_write_task(
                &workspace,
                "large sidecar gate workspace",
                move |cancellation| {
                    cancellation.check()?;
                    let report = run_large_sidecar_gate(&LargeSidecarGateOptions::new(
                        worker_workspace,
                        records,
                    ))?;
                    cancellation.check()?;
                    Ok(report)
                },
            )?;
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
        "search-typing-benchmark" => {
            let workspace = required_path(
                args.next(),
                "search-typing-benchmark requires a workspace path",
            )?;
            let records = parse_usize_arg(
                args.next(),
                "search-typing-benchmark requires a synthetic record count",
            )?;
            let mut options = SearchTypingBenchmarkOptions::new(workspace, records);
            if let Some(repetitions) = args.next() {
                options.repetitions = repetitions.parse().map_err(|_| {
                    GfmError::Format(format!(
                        "search-typing-benchmark repetitions must be an unsigned integer, got `{repetitions}`"
                    ))
                })?;
            }
            if let Some(query) = args.next() {
                options.query = query;
            }
            let workspace = options.workspace.clone();
            let report = run_workspace_write_task(
                &workspace,
                "search typing benchmark workspace",
                move |cancellation| {
                    cancellation.check()?;
                    let report = run_search_typing_benchmark(&options)?;
                    cancellation.check()?;
                    Ok(report)
                },
            )?;
            println!(
                "search-typing-benchmark\tfixture={}\thistory={}\trecords={}\tprobe-records={}\trepetitions={}\tqueries={}\tsamples={}\thits={}\tp50-ns={}\tp95-ns={}\tp99-ns={}\tmax-ns={}\tprefix-candidates={}\tsubstring-candidates={}\tfuzzy-verified={}\tprefix-cache-hits={}\tsubstring-cache-hits={}\tfuzzy-cache-hits={}\tviolations={}\tpassed={}",
                report.fixture_root.display(),
                report.history_path.display(),
                report.records,
                report.probe_records,
                report.repetitions,
                report.queries.len(),
                report.samples,
                report.hits,
                report.p50.as_nanos(),
                report.p95.as_nanos(),
                report.p99.as_nanos(),
                report.max.as_nanos(),
                report.lookup.prefix_candidate_ids,
                report.lookup.substring_candidate_ids,
                report.lookup.fuzzy_verified_candidates,
                report.lookup.prefix_cache_hits,
                report.lookup.substring_cache_hits,
                report.lookup.fuzzy_cache_hits,
                report.violations.len(),
                report.passed
            );
            for violation in &report.violations {
                eprintln!("search-typing-violation\t{violation:?}");
            }
            if !report.passed {
                return Err(GfmError::Format(
                    "search typing benchmark gate failed".to_string(),
                ));
            }
        }
        "search-typing-session-benchmark" => {
            let workspace = required_path(
                args.next(),
                "search-typing-session-benchmark requires a workspace path",
            )?;
            let records = parse_usize_arg(
                args.next(),
                "search-typing-session-benchmark requires a synthetic record count",
            )?;
            let mut options = SearchTypingBenchmarkOptions::new(workspace, records);
            if let Some(repetitions) = args.next() {
                options.repetitions = repetitions.parse().map_err(|_| {
                    GfmError::Format(format!(
                        "search-typing-session-benchmark repetitions must be an unsigned integer, got `{repetitions}`"
                    ))
                })?;
            }
            if let Some(query) = args.next() {
                options.query = query;
            }
            let workspace = options.workspace.clone();
            let report = run_workspace_write_task(
                &workspace,
                "search typing session benchmark workspace",
                move |cancellation| {
                    cancellation.check()?;
                    let report = run_search_typing_session_benchmark(&options)?;
                    cancellation.check()?;
                    Ok(report)
                },
            )?;
            println!(
                "search-typing-session-benchmark\tfixture={}\thistory={}\trecords={}\tindexed-records={}\tindexed-prefixes={}\tindexed-substring-grams={}\tindexed-fuzzy-keys={}\trepetitions={}\tqueries={}\tsamples={}\thits={}\tp50-ns={}\tp95-ns={}\tp99-ns={}\tmax-ns={}\tprefix-candidates={}\tsubstring-candidates={}\tfuzzy-verified={}\tprefix-cache-hits={}\tsubstring-cache-hits={}\tfuzzy-cache-hits={}\tcontent-cache-hits={}\tcontent-cache-misses={}\trecord-cache-hits={}\trecord-cache-misses={}\tresult-cache-hits={}\tresult-cache-misses={}\tviolations={}\tpassed={}",
                report.fixture_root.display(),
                report.history_path.display(),
                report.records,
                report.indexed_records,
                report.indexed_prefixes,
                report.indexed_substring_grams,
                report.indexed_fuzzy_keys,
                report.repetitions,
                report.queries.len(),
                report.samples,
                report.hits,
                report.p50.as_nanos(),
                report.p95.as_nanos(),
                report.p99.as_nanos(),
                report.max.as_nanos(),
                report.lookup.prefix_candidate_ids,
                report.lookup.substring_candidate_ids,
                report.lookup.fuzzy_verified_candidates,
                report.lookup.prefix_cache_hits,
                report.lookup.substring_cache_hits,
                report.lookup.fuzzy_cache_hits,
                report.content_cache_hits,
                report.content_cache_misses,
                report.record_cache_hits,
                report.record_cache_misses,
                report.result_cache_hits,
                report.result_cache_misses,
                report.violations.len(),
                report.passed
            );
            for violation in &report.violations {
                eprintln!("search-typing-session-violation\t{violation:?}");
            }
            if !report.passed {
                return Err(GfmError::Format(
                    "search typing session benchmark gate failed".to_string(),
                ));
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}

#[derive(Clone)]
struct GateAccessReport {
    path: PathBuf,
    intent: AccessIntent,
    worker: String,
    volume_report: VolumeDiscoveryReport,
}

impl GateAccessReport {
    fn new(path: PathBuf, intent: AccessIntent, worker: impl Into<String>) -> Self {
        Self::new_checked(path, intent, worker, || Ok(()))
            .expect("uncancellable gate access report cannot cancel")
    }

    fn new_checked(
        path: PathBuf,
        intent: AccessIntent,
        worker: impl Into<String>,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        check_control()?;
        let volume_report =
            VolumeDiscoveryReport::for_containing_path_checked(&path, &mut check_control)?;
        check_control()?;
        Ok(Self {
            path,
            intent,
            worker: worker.into(),
            volume_report,
        })
    }

    fn preflight_volume(&self) -> Result<()> {
        preflight_volume_access_scope_with_report(
            &self.path,
            self.intent,
            &self.worker,
            &self.volume_report,
        )
    }

    fn access_checked(
        &self,
        check_control: impl FnMut() -> Result<()>,
    ) -> Result<ScopedAccessGuard> {
        preflight_access_scope_checked_with_volume_report(
            &self.path,
            self.intent,
            &self.worker,
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
struct GateAccessReports {
    entries: Vec<GateAccessReport>,
}

impl GateAccessReports {
    fn new(entries: Vec<GateAccessReport>) -> Self {
        Self { entries }
    }

    fn preflight_volumes(&self) -> Result<()> {
        for entry in &self.entries {
            entry.preflight_volume()?;
        }
        Ok(())
    }

    fn access_checked(
        &self,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Vec<ScopedAccessGuard>> {
        let mut guards = Vec::with_capacity(self.entries.len());
        for entry in &self.entries {
            check_control()?;
            guards.push(entry.access_checked(&mut check_control)?);
        }
        check_control()?;
        Ok(guards)
    }

    fn first_volume(&self) -> Option<gfm_types::VolumeId> {
        self.entries.iter().find_map(GateAccessReport::volume)
    }
}

fn pixel_diff_access_reports(
    expected: &Path,
    actual: &Path,
    mask: Option<&Path>,
) -> GateAccessReports {
    pixel_diff_access_reports_checked(expected, actual, mask, || Ok(()))
        .expect("uncancellable pixel diff access reports cannot cancel")
}

fn pixel_diff_access_reports_checked(
    expected: &Path,
    actual: &Path,
    mask: Option<&Path>,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<GateAccessReports> {
    let mut entries = vec![
        GateAccessReport::new_checked(
            expected.to_path_buf(),
            AccessIntent::Read,
            "pixel expected",
            &mut check_control,
        )?,
        GateAccessReport::new_checked(
            actual.to_path_buf(),
            AccessIntent::Read,
            "pixel actual",
            &mut check_control,
        )?,
    ];
    if let Some(mask) = mask {
        check_control()?;
        entries.push(GateAccessReport::new_checked(
            mask.to_path_buf(),
            AccessIntent::Read,
            "pixel mask",
            &mut check_control,
        )?);
    }
    check_control()?;
    Ok(GateAccessReports::new(entries))
}

fn parity_review_access_reports(manifest: &Path, output_dir: &Path) -> Result<GateAccessReports> {
    parity_review_access_reports_checked(manifest, output_dir, || Ok(()))
}

fn parity_review_access_reports_checked(
    manifest: &Path,
    output_dir: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<GateAccessReports> {
    check_control()?;
    let output_probe = write_probe_path(output_dir)?.to_path_buf();
    check_control()?;
    Ok(GateAccessReports::new(vec![
        GateAccessReport::new_checked(
            manifest.to_path_buf(),
            AccessIntent::Read,
            "parity review manifest",
            &mut check_control,
        )?,
        GateAccessReport::new_checked(
            output_probe,
            AccessIntent::Write,
            "parity review output",
            &mut check_control,
        )?,
    ]))
}

fn workspace_write_access_report(workspace: &Path, worker: &str) -> Result<GateAccessReport> {
    workspace_write_access_report_checked(workspace, worker, || Ok(()))
}

fn workspace_write_access_report_checked(
    workspace: &Path,
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<GateAccessReport> {
    check_control()?;
    let workspace = write_probe_path(workspace)?.to_path_buf();
    check_control()?;
    GateAccessReport::new_checked(workspace, AccessIntent::Write, worker, &mut check_control)
}
fn run_workspace_write_task<T>(
    workspace: &Path,
    worker: &'static str,
    work: impl FnOnce(gfm_jobs::Cancellation) -> Result<T> + Send + 'static,
) -> Result<T>
where
    T: Send + 'static,
{
    let workspace = workspace.to_path_buf();
    let access_report = workspace_write_access_report(&workspace, worker)?;
    access_report.preflight_volume()?;
    let volume = access_report.volume();
    run_volume_task_cancellable(volume, Priority::Visible, worker, move |cancellation| {
        cancellation.check()?;
        let _access = access_report.access_checked(|| cancellation.check())?;
        cancellation.check()?;
        work(cancellation)
    })
}

#[cfg(test)]
fn retain_pixel_diff_access_checked(
    expected: &Path,
    actual: &Path,
    mask: Option<&Path>,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    check_control()?;
    pixel_diff_access_reports_checked(expected, actual, mask, &mut check_control)?
        .access_checked(check_control)
}

#[cfg(test)]
fn retain_parity_review_access_checked(
    manifest: &Path,
    output_dir: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    check_control()?;
    parity_review_access_reports_checked(manifest, output_dir, &mut check_control)?
        .access_checked(check_control)
}

#[cfg(test)]
fn retain_workspace_write_access_checked(
    path: &Path,
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<ScopedAccessGuard> {
    check_control()?;
    workspace_write_access_report_checked(path, worker, &mut check_control)?
        .access_checked(check_control)
}

fn write_probe_path(path: &Path) -> Result<&Path> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(path),
        Ok(_) => Ok(crate::parent_or_cwd(path)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(crate::parent_or_cwd(path)),
        Err(err) => Err(GfmError::io(
            path,
            format!("gate write path metadata unavailable: {err}"),
        )),
    }
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
            return Err(GfmError::Format(format!(
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
            return Err(GfmError::Format(format!(
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn pixel_diff_access_checked_honors_pre_cancelled_control() {
        let root = unique_temp_dir("gfm-gates-pixel-diff-access-pre-cancel");
        let expected = root.join("expected.rgba");
        let actual = root.join("actual.rgba");

        let result =
            retain_pixel_diff_access_checked(&expected, &actual, None, || Err(GfmError::Cancelled));

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parity_review_access_checked_can_cancel_before_output_probe() {
        let root = unique_temp_dir("gfm-gates-parity-review-access-cancel");
        let manifest = root.join("manifest.tsv");
        let output = root.join("review");
        let mut checks = 0usize;

        let result = retain_parity_review_access_checked(&manifest, &output, || {
            checks += 1;
            if checks >= 2 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(checks >= 2);
        assert!(!output.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_write_access_checked_honors_pre_cancelled_control() {
        let root = unique_temp_dir("gfm-gates-workspace-write-access-pre-cancel");
        let workspace = root.join("fixture");

        let result = retain_workspace_write_access_checked(&workspace, "fixture workspace", || {
            Err(GfmError::Cancelled)
        });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!workspace.exists());
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
