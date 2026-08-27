use crate::{parse_u32_arg, parse_usize_arg, required_path};
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
use std::path::PathBuf;

pub(crate) fn run(command: &str, args: &mut impl Iterator<Item = String>) -> Result<bool> {
    match command {
        "macrobench" => {
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
        "macrobench-fixture" => {
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
        "parity-fixture" => {
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
        "pixel-diff" => {
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
            let masks = args
                .next()
                .map(|path| read_governed_mask_file(path, size))
                .transpose()?
                .unwrap_or_default();
            let options = PixelDiffOptions::strict(size).with_governed_masks(masks);
            let report = diff_rgba_files(expected, actual, &options)?;
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
            let report = run_search_typing_benchmark(&options)?;
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
            let report = run_search_typing_session_benchmark(&options)?;
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
