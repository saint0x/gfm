use gfm_content::Extractor;
use gfm_index::Indexer;
use gfm_mac::{current_host_profile, current_process_memory, CpuArchitecture, MacOsVersion};
use gfm_telemetry::{PerformanceBudgets, ScenarioMetric};
use gfm_types::{GfmError, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const FIXTURE_ROOT: &str = "gfm-macrobench-fixture";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MacrobenchScenario {
    Small,
    Medium,
    Huge,
    Developer,
    Documents,
    Media,
    ICloud,
    External,
    Network,
}

impl MacrobenchScenario {
    pub const ALL: [Self; 9] = [
        Self::Small,
        Self::Medium,
        Self::Huge,
        Self::Developer,
        Self::Documents,
        Self::Media,
        Self::ICloud,
        Self::External,
        Self::Network,
    ];

    pub const fn directory(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Huge => "huge",
            Self::Developer => "developer",
            Self::Documents => "documents",
            Self::Media => "media",
            Self::ICloud => "icloud",
            Self::External => "external",
            Self::Network => "network",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "small" => Some(Self::Small),
            "medium" => Some(Self::Medium),
            "huge" => Some(Self::Huge),
            "developer" => Some(Self::Developer),
            "documents" => Some(Self::Documents),
            "media" => Some(Self::Media),
            "icloud" => Some(Self::ICloud),
            "external" => Some(Self::External),
            "network" => Some(Self::Network),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MacrobenchScale {
    pub small_files: usize,
    pub medium_files: usize,
    pub huge_files: usize,
    pub developer_projects: usize,
    pub document_files: usize,
    pub media_files: usize,
    pub icloud_files: usize,
    pub external_files: usize,
    pub network_files: usize,
}

impl MacrobenchScale {
    pub const fn smoke() -> Self {
        Self {
            small_files: 8,
            medium_files: 24,
            huge_files: 96,
            developer_projects: 3,
            document_files: 12,
            media_files: 16,
            icloud_files: 12,
            external_files: 12,
            network_files: 12,
        }
    }

    pub const fn standard() -> Self {
        Self {
            small_files: 64,
            medium_files: 1_024,
            huge_files: 25_000,
            developer_projects: 64,
            document_files: 2_000,
            media_files: 2_000,
            icloud_files: 2_000,
            external_files: 2_000,
            network_files: 2_000,
        }
    }

    pub const fn million_files() -> Self {
        Self {
            small_files: 10_000,
            medium_files: 160_000,
            huge_files: 430_000,
            developer_projects: 50_000,
            document_files: 100_000,
            media_files: 80_000,
            icloud_files: 30_000,
            external_files: 25_000,
            network_files: 15_000,
        }
    }

    fn count_for(self, scenario: MacrobenchScenario) -> usize {
        match scenario {
            MacrobenchScenario::Small => self.small_files,
            MacrobenchScenario::Medium => self.medium_files,
            MacrobenchScenario::Huge => self.huge_files,
            MacrobenchScenario::Developer => self.developer_projects,
            MacrobenchScenario::Documents => self.document_files,
            MacrobenchScenario::Media => self.media_files,
            MacrobenchScenario::ICloud => self.icloud_files,
            MacrobenchScenario::External => self.external_files,
            MacrobenchScenario::Network => self.network_files,
        }
    }
}

impl Default for MacrobenchScale {
    fn default() -> Self {
        Self::standard()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacrobenchOptions {
    pub workspace: PathBuf,
    pub scale: MacrobenchScale,
    pub limit: usize,
}

impl MacrobenchOptions {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            scale: MacrobenchScale::default(),
            limit: 50,
        }
    }

    pub fn smoke(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            scale: MacrobenchScale::smoke(),
            limit: 25,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacrobenchReport {
    pub fixture_root: PathBuf,
    pub files_materialized: usize,
    pub measurements: Vec<MacrobenchMeasurement>,
    pub budget_violations: Vec<gfm_telemetry::BudgetViolation>,
}

impl MacrobenchReport {
    pub fn passed(&self) -> bool {
        self.budget_violations.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacrobenchArtifactReport {
    pub output_dir: PathBuf,
    pub summary_path: PathBuf,
    pub measurements_path: PathBuf,
    pub budget_violations_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacrobenchArtifactVerification {
    pub output_dir: PathBuf,
    pub macos_version: MacOsVersion,
    pub macos_build: String,
    pub cpu_architecture: CpuArchitecture,
    pub host_memory_bytes: u64,
    pub logical_cpus: u16,
    pub files_materialized: usize,
    pub measurements: usize,
    pub scenarios: usize,
    pub stages_per_scenario: usize,
    pub max_peak_resident_bytes: u64,
    pub budget_violations: usize,
    pub passed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MacrobenchStage {
    IndexBuild,
    HotSearch,
    StreamSearch,
    ContentSearch,
}

impl MacrobenchStage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IndexBuild => "index-build",
            Self::HotSearch => "hot-search",
            Self::StreamSearch => "stream-search",
            Self::ContentSearch => "content-search",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "index-build" => Some(Self::IndexBuild),
            "hot-search" => Some(Self::HotSearch),
            "stream-search" => Some(Self::StreamSearch),
            "content-search" => Some(Self::ContentSearch),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacrobenchMeasurement {
    pub scenario: MacrobenchScenario,
    pub stage: MacrobenchStage,
    pub duration: Duration,
    pub peak_resident_bytes: u64,
    pub records: usize,
    pub hits: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacrobenchFixtureReport {
    pub fixture_root: PathBuf,
    pub manifest_path: PathBuf,
    pub scenarios: Vec<MacrobenchFixtureScenarioReport>,
}

impl MacrobenchFixtureReport {
    pub fn files_materialized(&self) -> usize {
        self.scenarios.iter().map(|scenario| scenario.files).sum()
    }

    pub fn directories_materialized(&self) -> usize {
        self.scenarios
            .iter()
            .map(|scenario| scenario.directories)
            .sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacrobenchFixtureScenarioReport {
    pub scenario: MacrobenchScenario,
    pub root: PathBuf,
    pub files: usize,
    pub directories: usize,
}

pub fn run_macrobench(options: &MacrobenchOptions) -> Result<MacrobenchReport> {
    let fixture = materialize_macrobench_fixture_report(&options.workspace, options.scale)?;
    let files_materialized = fixture.files_materialized();
    let fixture_root = fixture.fixture_root;
    let mut measurements = Vec::new();
    let mut scenario_observations: BTreeMap<ScenarioMetric, Duration> = BTreeMap::new();

    for scenario in MacrobenchScenario::ALL {
        let root = fixture_root.join(scenario.directory());
        let build_start = Instant::now();
        let snapshot = Indexer::default().build(&root)?;
        let build_duration = build_start.elapsed();
        scenario_observations
            .entry(ScenarioMetric::DirectoryOpen)
            .and_modify(|duration| *duration = (*duration).max(build_duration))
            .or_insert(build_duration);
        measurements.push(MacrobenchMeasurement {
            scenario,
            stage: MacrobenchStage::IndexBuild,
            duration: build_duration,
            peak_resident_bytes: current_peak_resident_bytes()?,
            records: snapshot.records.len(),
            hits: 0,
        });

        let session = snapshot.query_session();
        let hot_start = Instant::now();
        let hot_hits = session.search("needle", options.limit);
        let hot_duration = hot_start.elapsed();
        scenario_observations
            .entry(ScenarioMetric::FirstResult)
            .and_modify(|duration| *duration = (*duration).max(hot_duration))
            .or_insert(hot_duration);
        measurements.push(MacrobenchMeasurement {
            scenario,
            stage: MacrobenchStage::HotSearch,
            duration: hot_duration,
            peak_resident_bytes: current_peak_resident_bytes()?,
            records: snapshot.records.len(),
            hits: hot_hits.len(),
        });

        let stream_start = Instant::now();
        let stream_hits: usize = session
            .stream_search("project", options.limit)?
            .into_iter()
            .map(|batch| batch.hits.len())
            .sum();
        let stream_duration = stream_start.elapsed();
        scenario_observations
            .entry(ScenarioMetric::FullResult)
            .and_modify(|duration| *duration = (*duration).max(stream_duration))
            .or_insert(stream_duration);
        measurements.push(MacrobenchMeasurement {
            scenario,
            stage: MacrobenchStage::StreamSearch,
            duration: stream_duration,
            peak_resident_bytes: current_peak_resident_bytes()?,
            records: snapshot.records.len(),
            hits: stream_hits,
        });

        let content_start = Instant::now();
        let content_hits = snapshot.search_with_content_snippets(
            "contentneedle",
            options.limit,
            &Extractor::default(),
            32,
        )?;
        let content_duration = content_start.elapsed();
        scenario_observations
            .entry(ScenarioMetric::VisibleThumbnailCompletion)
            .and_modify(|duration| *duration = (*duration).max(content_duration))
            .or_insert(content_duration);
        measurements.push(MacrobenchMeasurement {
            scenario,
            stage: MacrobenchStage::ContentSearch,
            duration: content_duration,
            peak_resident_bytes: current_peak_resident_bytes()?,
            records: snapshot.records.len(),
            hits: content_hits.len(),
        });
    }

    let budget_violations = PerformanceBudgets::default()
        .evaluate_scenarios(&scenario_observations)
        .violations;

    Ok(MacrobenchReport {
        fixture_root,
        files_materialized,
        measurements,
        budget_violations,
    })
}

pub fn run_macrobench_report(
    options: &MacrobenchOptions,
    output_dir: impl AsRef<Path>,
) -> Result<(MacrobenchReport, MacrobenchArtifactReport)> {
    let report = run_macrobench(options)?;
    let artifacts = write_macrobench_artifacts(&report, output_dir)?;
    Ok((report, artifacts))
}

pub fn write_macrobench_artifacts(
    report: &MacrobenchReport,
    output_dir: impl AsRef<Path>,
) -> Result<MacrobenchArtifactReport> {
    let output_dir = output_dir.as_ref().to_path_buf();
    fs::create_dir_all(&output_dir).map_err(|err| GfmError::io(&output_dir, err))?;
    let summary_path = output_dir.join("summary.tsv");
    let measurements_path = output_dir.join("measurements.tsv");
    let budget_violations_path = output_dir.join("budget-violations.tsv");
    write_macrobench_summary(report, &summary_path)?;
    write_macrobench_measurements(report, &measurements_path)?;
    write_macrobench_budget_violations(report, &budget_violations_path)?;
    Ok(MacrobenchArtifactReport {
        output_dir,
        summary_path,
        measurements_path,
        budget_violations_path,
    })
}

pub fn verify_macrobench_artifacts(
    output_dir: impl AsRef<Path>,
    min_files_materialized: usize,
) -> Result<MacrobenchArtifactVerification> {
    let output_dir = output_dir.as_ref();
    let summary = read_summary_tsv(&output_dir.join("summary.tsv"))?;
    let measurements = read_measurements_tsv(&output_dir.join("measurements.tsv"))?;
    let budget_violations = read_budget_violations_tsv(&output_dir.join("budget-violations.tsv"))?;

    let files_materialized = required_summary_usize(&summary, "files_materialized")?;
    let summary_measurements = required_summary_usize(&summary, "measurements")?;
    let summary_budget_violations = required_summary_usize(&summary, "budget_violations")?;
    let passed = required_summary_bool(&summary, "passed")?;
    let macos_version = MacOsVersion::parse(required_summary_field(&summary, "macos_version")?)?;
    let macos_build = required_summary_field(&summary, "macos_build")?.to_string();
    let cpu_architecture =
        CpuArchitecture::parse(required_summary_field(&summary, "cpu_architecture")?);
    let host_memory_bytes = required_summary_u64(&summary, "host_memory_bytes")?;
    let logical_cpus = required_summary_u16(&summary, "logical_cpus")?;

    if macos_build.trim().is_empty() {
        return Err(GfmError::Format(
            "macrobench report missing macOS build provenance".to_string(),
        ));
    }
    if cpu_architecture == CpuArchitecture::Unsupported {
        return Err(GfmError::Format(
            "macrobench report captured on unsupported CPU architecture".to_string(),
        ));
    }
    if host_memory_bytes == 0 || logical_cpus == 0 {
        return Err(GfmError::Format(
            "macrobench report missing usable host hardware provenance".to_string(),
        ));
    }
    if files_materialized < min_files_materialized {
        return Err(GfmError::Format(format!(
            "macrobench report materialized {files_materialized} files below required floor {min_files_materialized}"
        )));
    }
    if summary_measurements != measurements.len() {
        return Err(GfmError::Format(format!(
            "macrobench report summary measurement count {summary_measurements} does not match measurements.tsv count {}",
            measurements.len()
        )));
    }
    if summary_measurements != MacrobenchScenario::ALL.len() * 4 {
        return Err(GfmError::Format(format!(
            "macrobench report expected {} measurements, found {summary_measurements}",
            MacrobenchScenario::ALL.len() * 4
        )));
    }
    if summary_budget_violations != budget_violations {
        return Err(GfmError::Format(format!(
            "macrobench report summary budget violation count {summary_budget_violations} does not match budget-violations.tsv count {budget_violations}"
        )));
    }
    if summary_budget_violations != 0 || !passed {
        return Err(GfmError::Format(format!(
            "macrobench report retained {summary_budget_violations} budget violations and passed={passed}"
        )));
    }

    let mut seen = BTreeSet::new();
    let mut records_by_scenario: BTreeMap<MacrobenchScenario, usize> = BTreeMap::new();
    let mut max_peak_resident_bytes = 0;
    for measurement in &measurements {
        if !seen.insert((measurement.scenario, measurement.stage)) {
            return Err(GfmError::Format(format!(
                "macrobench report contains duplicate measurement for {} {}",
                measurement.scenario.directory(),
                measurement.stage.as_str()
            )));
        }
        if measurement.duration.is_zero() {
            return Err(GfmError::Format(format!(
                "macrobench report contains zero duration for {} {}",
                measurement.scenario.directory(),
                measurement.stage.as_str()
            )));
        }
        if measurement.peak_resident_bytes == 0 {
            return Err(GfmError::Format(format!(
                "macrobench report contains zero peak resident memory for {} {}",
                measurement.scenario.directory(),
                measurement.stage.as_str()
            )));
        }
        max_peak_resident_bytes = max_peak_resident_bytes.max(measurement.peak_resident_bytes);
        if measurement.records == 0 {
            return Err(GfmError::Format(format!(
                "macrobench report contains zero records for {} {}",
                measurement.scenario.directory(),
                measurement.stage.as_str()
            )));
        }
        records_by_scenario
            .entry(measurement.scenario)
            .and_modify(|records| {
                if *records != measurement.records {
                    *records = 0;
                }
            })
            .or_insert(measurement.records);
    }

    for scenario in MacrobenchScenario::ALL {
        for stage in [
            MacrobenchStage::IndexBuild,
            MacrobenchStage::HotSearch,
            MacrobenchStage::StreamSearch,
            MacrobenchStage::ContentSearch,
        ] {
            if !seen.contains(&(scenario, stage)) {
                return Err(GfmError::Format(format!(
                    "macrobench report missing measurement for {} {}",
                    scenario.directory(),
                    stage.as_str()
                )));
            }
        }
        if records_by_scenario.get(&scenario) == Some(&0) {
            return Err(GfmError::Format(format!(
                "macrobench report records are inconsistent across stages for {}",
                scenario.directory()
            )));
        }
    }

    Ok(MacrobenchArtifactVerification {
        output_dir: output_dir.to_path_buf(),
        macos_version,
        macos_build,
        cpu_architecture,
        host_memory_bytes,
        logical_cpus,
        files_materialized,
        measurements: measurements.len(),
        scenarios: MacrobenchScenario::ALL.len(),
        stages_per_scenario: 4,
        max_peak_resident_bytes,
        budget_violations,
        passed,
    })
}

pub fn materialize_macrobench_fixture(
    workspace: impl AsRef<Path>,
    scale: MacrobenchScale,
) -> Result<PathBuf> {
    Ok(materialize_macrobench_fixture_report(workspace, scale)?.fixture_root)
}

pub fn materialize_macrobench_fixture_report(
    workspace: impl AsRef<Path>,
    scale: MacrobenchScale,
) -> Result<MacrobenchFixtureReport> {
    let workspace = workspace.as_ref();
    fs::create_dir_all(workspace).map_err(|err| GfmError::io(workspace, err))?;
    let fixture_root = workspace.join(FIXTURE_ROOT);
    if fixture_root.exists() {
        fs::remove_dir_all(&fixture_root).map_err(|err| GfmError::io(&fixture_root, err))?;
    }
    fs::create_dir_all(&fixture_root).map_err(|err| GfmError::io(&fixture_root, err))?;
    let mut scenarios = Vec::with_capacity(MacrobenchScenario::ALL.len());
    for scenario in MacrobenchScenario::ALL {
        scenarios.push(materialize_scenario(
            &fixture_root,
            scenario,
            scale.count_for(scenario),
        )?);
    }
    let manifest_path = fixture_root.join("manifest.tsv");
    write_fixture_manifest(&manifest_path, &scenarios)?;
    Ok(MacrobenchFixtureReport {
        fixture_root,
        manifest_path,
        scenarios,
    })
}

fn materialize_scenario(
    fixture_root: &Path,
    scenario: MacrobenchScenario,
    count: usize,
) -> Result<MacrobenchFixtureScenarioReport> {
    let root = fixture_root.join(scenario.directory());
    fs::create_dir_all(&root).map_err(|err| GfmError::io(&root, err))?;
    let (files, directories) = match scenario {
        MacrobenchScenario::Developer => materialize_developer(&root, count),
        MacrobenchScenario::Documents => materialize_documents(&root, count),
        MacrobenchScenario::Media => materialize_media(&root, count),
        MacrobenchScenario::ICloud => materialize_flat(&root, count, "icloud", "icloud-state"),
        MacrobenchScenario::External => {
            materialize_flat(&root, count, "external", "external-volume")
        }
        MacrobenchScenario::Network => materialize_flat(&root, count, "network", "network-volume"),
        MacrobenchScenario::Small => materialize_flat(&root, count, "small", "small-tree"),
        MacrobenchScenario::Medium => materialize_nested(&root, count, 32, "medium"),
        MacrobenchScenario::Huge => materialize_nested(&root, count, 256, "huge"),
    }?;
    Ok(MacrobenchFixtureScenarioReport {
        scenario,
        root,
        files,
        directories,
    })
}

fn materialize_flat(
    root: &Path,
    count: usize,
    prefix: &str,
    marker: &str,
) -> Result<(usize, usize)> {
    for index in 0..count {
        let path = root.join(format!("{prefix}-{index:06}.md"));
        write_file(
            &path,
            &format!(
                "{marker} project needle contentneedle file {index} deterministic macrobench data\n"
            ),
        )?;
    }
    Ok((count, 0))
}

fn materialize_nested(
    root: &Path,
    count: usize,
    fanout: usize,
    prefix: &str,
) -> Result<(usize, usize)> {
    let fanout = fanout.max(1);
    let mut directories = 0;
    for index in 0..count {
        let shard = root.join(format!("shard-{:04}", index / fanout));
        if index % fanout == 0 {
            fs::create_dir_all(&shard).map_err(|err| GfmError::io(&shard, err))?;
            directories += 1;
        }
        let path = shard.join(format!("{prefix}-{index:08}.txt"));
        write_file(
            &path,
            &format!("project needle contentneedle nested {prefix} file {index}\n"),
        )?;
    }
    Ok((count, directories))
}

fn materialize_developer(root: &Path, projects: usize) -> Result<(usize, usize)> {
    for project in 0..projects {
        let src = root.join(format!("project-{project:04}")).join("src");
        fs::create_dir_all(&src).map_err(|err| GfmError::io(&src, err))?;
        write_file(
            &src.join("main.rs"),
            &format!("fn main() {{ println!(\"project needle {project}\"); }}\n"),
        )?;
        write_file(
            &src.join("content.rs"),
            &format!("pub const CONTENT: &str = \"contentneedle developer {project}\";\n"),
        )?;
        write_file(
            &root
                .join(format!("project-{project:04}"))
                .join("Cargo.toml"),
            "[package]\nname = \"macrobench-project\"\nversion = \"0.0.0\"\n",
        )?;
    }
    Ok((projects * 3, projects * 2))
}

fn materialize_documents(root: &Path, count: usize) -> Result<(usize, usize)> {
    let mut directories = 0;
    for index in 0..count {
        let year = 2020 + (index % 7);
        let folder = root.join(format!("year-{year}"));
        if index < 7 {
            fs::create_dir_all(&folder).map_err(|err| GfmError::io(&folder, err))?;
            directories += 1;
        }
        let path = folder.join(format!("Briefing Project {index:08}.md"));
        write_file(
            &path,
            &format!("documents project needle contentneedle briefing text {index}\n"),
        )?;
    }
    Ok((count, directories))
}

fn materialize_media(root: &Path, count: usize) -> Result<(usize, usize)> {
    let mut directories = 0;
    for index in 0..count {
        let album = root.join(format!("album-{:04}", index / 64));
        if index % 64 == 0 {
            fs::create_dir_all(&album).map_err(|err| GfmError::io(&album, err))?;
            directories += 1;
        }
        write_file(
            &album.join(format!("image-{index:06}.jpg.meta.md")),
            &format!("media project needle contentneedle width height asset {index}\n"),
        )?;
    }
    Ok((count, directories))
}

fn write_macrobench_summary(report: &MacrobenchReport, path: &Path) -> Result<()> {
    let mut file = fs::File::create(path).map_err(|err| GfmError::io(path, err))?;
    let host = current_host_profile()?;
    writeln!(file, "key\tvalue").map_err(|err| GfmError::io(path, err))?;
    writeln!(
        file,
        "fixture_root\t{}",
        escape_tsv_field(&report.fixture_root.display().to_string())
    )
    .map_err(|err| GfmError::io(path, err))?;
    writeln!(
        file,
        "macos_version\t{}.{}.{}",
        host.macos_version.major, host.macos_version.minor, host.macos_version.patch
    )
    .map_err(|err| GfmError::io(path, err))?;
    writeln!(file, "macos_build\t{}", escape_tsv_field(&host.build))
        .map_err(|err| GfmError::io(path, err))?;
    writeln!(
        file,
        "cpu_architecture\t{}",
        host.hardware.architecture.as_str()
    )
    .map_err(|err| GfmError::io(path, err))?;
    writeln!(file, "host_memory_bytes\t{}", host.hardware.memory_bytes)
        .map_err(|err| GfmError::io(path, err))?;
    writeln!(file, "logical_cpus\t{}", host.hardware.logical_cpus)
        .map_err(|err| GfmError::io(path, err))?;
    writeln!(file, "files_materialized\t{}", report.files_materialized)
        .map_err(|err| GfmError::io(path, err))?;
    writeln!(file, "measurements\t{}", report.measurements.len())
        .map_err(|err| GfmError::io(path, err))?;
    writeln!(
        file,
        "budget_violations\t{}",
        report.budget_violations.len()
    )
    .map_err(|err| GfmError::io(path, err))?;
    writeln!(file, "passed\t{}", report.passed()).map_err(|err| GfmError::io(path, err))
}

fn write_macrobench_measurements(report: &MacrobenchReport, path: &Path) -> Result<()> {
    let mut file = fs::File::create(path).map_err(|err| GfmError::io(path, err))?;
    writeln!(
        file,
        "scenario\tstage\tduration_ns\tpeak_resident_bytes\trecords\thits"
    )
    .map_err(|err| GfmError::io(path, err))?;
    for measurement in &report.measurements {
        writeln!(
            file,
            "{}\t{}\t{}\t{}\t{}\t{}",
            measurement.scenario.directory(),
            measurement.stage.as_str(),
            measurement.duration.as_nanos(),
            measurement.peak_resident_bytes,
            measurement.records,
            measurement.hits
        )
        .map_err(|err| GfmError::io(path, err))?;
    }
    Ok(())
}

fn write_macrobench_budget_violations(report: &MacrobenchReport, path: &Path) -> Result<()> {
    let mut file = fs::File::create(path).map_err(|err| GfmError::io(path, err))?;
    writeln!(file, "violation").map_err(|err| GfmError::io(path, err))?;
    for violation in &report.budget_violations {
        writeln!(file, "{}", escape_tsv_field(&format!("{violation:?}")))
            .map_err(|err| GfmError::io(path, err))?;
    }
    Ok(())
}

fn read_summary_tsv(path: &Path) -> Result<BTreeMap<String, String>> {
    let content = fs::read_to_string(path).map_err(|err| GfmError::io(path, err))?;
    let mut lines = content.lines();
    if lines.next() != Some("key\tvalue") {
        return Err(GfmError::Format(format!(
            "{}: invalid macrobench summary header",
            path.display()
        )));
    }
    let mut summary = BTreeMap::new();
    for (line_index, line) in lines.enumerate() {
        let line_number = line_index + 2;
        let columns = split_tsv_line(path, line_number, line, 2)?;
        if summary
            .insert(columns[0].to_string(), columns[1].to_string())
            .is_some()
        {
            return Err(GfmError::Format(format!(
                "{}:{line_number}: duplicate summary key {}",
                path.display(),
                columns[0]
            )));
        }
    }
    for key in [
        "fixture_root",
        "macos_version",
        "macos_build",
        "cpu_architecture",
        "host_memory_bytes",
        "logical_cpus",
        "files_materialized",
        "measurements",
        "budget_violations",
        "passed",
    ] {
        if !summary.contains_key(key) {
            return Err(GfmError::Format(format!(
                "{}: missing summary key {key}",
                path.display()
            )));
        }
    }
    Ok(summary)
}

fn read_measurements_tsv(path: &Path) -> Result<Vec<MacrobenchMeasurement>> {
    let content = fs::read_to_string(path).map_err(|err| GfmError::io(path, err))?;
    let mut lines = content.lines();
    if lines.next() != Some("scenario\tstage\tduration_ns\tpeak_resident_bytes\trecords\thits") {
        return Err(GfmError::Format(format!(
            "{}: invalid macrobench measurements header",
            path.display()
        )));
    }
    let mut measurements = Vec::new();
    for (line_index, line) in lines.enumerate() {
        let line_number = line_index + 2;
        let columns = split_tsv_line(path, line_number, line, 6)?;
        let scenario = MacrobenchScenario::parse(columns[0]).ok_or_else(|| {
            GfmError::Format(format!(
                "{}:{line_number}: unknown macrobench scenario {}",
                path.display(),
                columns[0]
            ))
        })?;
        let stage = MacrobenchStage::parse(columns[1]).ok_or_else(|| {
            GfmError::Format(format!(
                "{}:{line_number}: unknown macrobench stage {}",
                path.display(),
                columns[1]
            ))
        })?;
        let duration = Duration::from_nanos(parse_u64_field(
            path,
            line_number,
            "duration_ns",
            columns[2],
        )?);
        let peak_resident_bytes =
            parse_u64_field(path, line_number, "peak_resident_bytes", columns[3])?;
        let records = parse_usize_field(path, line_number, "records", columns[4])?;
        let hits = parse_usize_field(path, line_number, "hits", columns[5])?;
        measurements.push(MacrobenchMeasurement {
            scenario,
            stage,
            duration,
            peak_resident_bytes,
            records,
            hits,
        });
    }
    Ok(measurements)
}

fn read_budget_violations_tsv(path: &Path) -> Result<usize> {
    let content = fs::read_to_string(path).map_err(|err| GfmError::io(path, err))?;
    let mut lines = content.lines();
    if lines.next() != Some("violation") {
        return Err(GfmError::Format(format!(
            "{}: invalid macrobench budget violation header",
            path.display()
        )));
    }
    Ok(lines.filter(|line| !line.trim().is_empty()).count())
}

fn required_summary_usize(summary: &BTreeMap<String, String>, key: &str) -> Result<usize> {
    required_summary_field(summary, key)?
        .parse::<usize>()
        .map_err(|err| GfmError::Format(format!("invalid summary {key}: {err}")))
}

fn required_summary_u64(summary: &BTreeMap<String, String>, key: &str) -> Result<u64> {
    required_summary_field(summary, key)?
        .parse::<u64>()
        .map_err(|err| GfmError::Format(format!("invalid summary {key}: {err}")))
}

fn required_summary_u16(summary: &BTreeMap<String, String>, key: &str) -> Result<u16> {
    required_summary_field(summary, key)?
        .parse::<u16>()
        .map_err(|err| GfmError::Format(format!("invalid summary {key}: {err}")))
}

fn required_summary_bool(summary: &BTreeMap<String, String>, key: &str) -> Result<bool> {
    required_summary_field(summary, key)?
        .parse::<bool>()
        .map_err(|err| GfmError::Format(format!("invalid summary {key}: {err}")))
}

fn required_summary_field<'a>(summary: &'a BTreeMap<String, String>, key: &str) -> Result<&'a str> {
    summary
        .get(key)
        .map(String::as_str)
        .ok_or_else(|| GfmError::Format(format!("missing summary key {key}")))
}

fn split_tsv_line<'a>(
    path: &Path,
    line_number: usize,
    line: &'a str,
    expected_columns: usize,
) -> Result<Vec<&'a str>> {
    let columns: Vec<&str> = line.split('\t').collect();
    if columns.len() != expected_columns {
        return Err(GfmError::Format(format!(
            "{}:{line_number}: expected {expected_columns} TSV columns, found {}",
            path.display(),
            columns.len()
        )));
    }
    Ok(columns)
}

fn parse_u64_field(path: &Path, line_number: usize, field: &str, value: &str) -> Result<u64> {
    value.parse::<u64>().map_err(|err| {
        GfmError::Format(format!(
            "{}:{line_number}: invalid {field} value {value}: {err}",
            path.display()
        ))
    })
}

fn parse_usize_field(path: &Path, line_number: usize, field: &str, value: &str) -> Result<usize> {
    value.parse::<usize>().map_err(|err| {
        GfmError::Format(format!(
            "{}:{line_number}: invalid {field} value {value}: {err}",
            path.display()
        ))
    })
}

fn escape_tsv_field(value: &str) -> String {
    value.replace('\\', "\\\\").replace(['\t', '\n', '\r'], " ")
}

fn write_fixture_manifest(
    path: &Path,
    scenarios: &[MacrobenchFixtureScenarioReport],
) -> Result<()> {
    let mut file = fs::File::create(path).map_err(|err| GfmError::io(path, err))?;
    writeln!(file, "scenario\troot\tfiles\tdirectories").map_err(|err| GfmError::io(path, err))?;
    for scenario in scenarios {
        writeln!(
            file,
            "{}\t{}\t{}\t{}",
            scenario.scenario.directory(),
            scenario.root.display(),
            scenario.files,
            scenario.directories
        )
        .map_err(|err| GfmError::io(path, err))?;
    }
    Ok(())
}

fn write_file(path: &Path, contents: &str) -> Result<()> {
    let mut file = fs::File::create(path).map_err(|err| GfmError::io(path, err))?;
    file.write_all(contents.as_bytes())
        .map_err(|err| GfmError::io(path, err))
}

fn current_peak_resident_bytes() -> Result<u64> {
    Ok(current_process_memory()?.peak_resident_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn materializes_all_macrobench_scenarios() {
        let root = unique_temp_dir("gfm-testkit-materialize");
        let fixture = materialize_macrobench_fixture(&root, MacrobenchScale::smoke()).unwrap();

        for scenario in MacrobenchScenario::ALL {
            assert!(fixture.join(scenario.directory()).exists());
        }
        assert!(fixture.join("manifest.tsv").exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn macrobench_fixture_report_counts_real_files_and_directories() {
        let root = unique_temp_dir("gfm-testkit-fixture-report");
        let report =
            materialize_macrobench_fixture_report(&root, MacrobenchScale::smoke()).unwrap();

        assert_eq!(report.scenarios.len(), MacrobenchScenario::ALL.len());
        assert_eq!(report.files_materialized(), 201);
        assert!(report.directories_materialized() > 0);
        assert!(report
            .scenarios
            .iter()
            .any(|scenario| scenario.scenario == MacrobenchScenario::Documents));
        assert!(fs::read_to_string(&report.manifest_path)
            .unwrap()
            .contains("documents\t"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn runs_smoke_macrobench_against_real_index_paths() {
        let root = unique_temp_dir("gfm-testkit-macrobench");
        let report = run_macrobench(&MacrobenchOptions::smoke(&root)).unwrap();

        assert_eq!(report.files_materialized, 201);
        assert_eq!(report.measurements.len(), MacrobenchScenario::ALL.len() * 4);
        assert!(report
            .measurements
            .iter()
            .all(|measurement| measurement.records > 0));
        assert!(report
            .measurements
            .iter()
            .any(|measurement| measurement.hits > 0));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn writes_retained_macrobench_telemetry_artifacts() {
        let root = unique_temp_dir("gfm-testkit-macrobench-artifacts");
        let output = root.join("telemetry");
        let (report, artifacts) =
            run_macrobench_report(&MacrobenchOptions::smoke(&root), &output).unwrap();

        assert_eq!(artifacts.output_dir, output);
        assert_eq!(report.files_materialized, 201);
        let summary = fs::read_to_string(&artifacts.summary_path).unwrap();
        let measurements = fs::read_to_string(&artifacts.measurements_path).unwrap();
        let violations = fs::read_to_string(&artifacts.budget_violations_path).unwrap();
        assert!(summary.contains("macos_version\t"), "{summary}");
        assert!(summary.contains("macos_build\t"), "{summary}");
        assert!(summary.contains("cpu_architecture\t"), "{summary}");
        assert!(summary.contains("host_memory_bytes\t"), "{summary}");
        assert!(summary.contains("logical_cpus\t"), "{summary}");
        assert!(summary.contains("files_materialized\t201"), "{summary}");
        assert!(summary.contains("measurements\t36"), "{summary}");
        assert!(measurements
            .starts_with("scenario\tstage\tduration_ns\tpeak_resident_bytes\trecords\thits\n"));
        assert!(
            measurements.contains("small\tindex-build\t"),
            "{measurements}"
        );
        assert!(
            measurements.contains("network\tcontent-search\t"),
            "{measurements}"
        );
        assert_eq!(violations.lines().next(), Some("violation"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn verifies_retained_macrobench_telemetry_artifacts() {
        let root = unique_temp_dir("gfm-testkit-macrobench-verify");
        let output = root.join("telemetry");
        run_macrobench_report(&MacrobenchOptions::smoke(&root), &output).unwrap();

        let verification = verify_macrobench_artifacts(&output, 201).unwrap();

        assert_eq!(verification.output_dir, output);
        assert!(!verification.macos_build.is_empty());
        assert!(verification.host_memory_bytes > 0);
        assert!(verification.logical_cpus > 0);
        assert_eq!(verification.files_materialized, 201);
        assert_eq!(verification.measurements, MacrobenchScenario::ALL.len() * 4);
        assert_eq!(verification.scenarios, MacrobenchScenario::ALL.len());
        assert_eq!(verification.stages_per_scenario, 4);
        assert!(verification.max_peak_resident_bytes > 0);
        assert_eq!(verification.budget_violations, 0);
        assert!(verification.passed);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_incomplete_or_failing_macrobench_telemetry_artifacts() {
        let root = unique_temp_dir("gfm-testkit-macrobench-rejects");
        let output = root.join("telemetry");
        run_macrobench_report(&MacrobenchOptions::smoke(&root), &output).unwrap();

        let too_small = verify_macrobench_artifacts(&output, 202).unwrap_err();
        assert!(
            too_small.to_string().contains("below required floor 202"),
            "{too_small}"
        );

        fs::write(
            output.join("budget-violations.tsv"),
            "violation\nDirectoryOpen over budget\n",
        )
        .unwrap();
        let violations = verify_macrobench_artifacts(&output, 201).unwrap_err();
        assert!(
            violations
                .to_string()
                .contains("summary budget violation count 0 does not match"),
            "{violations}"
        );

        fs::write(output.join("budget-violations.tsv"), "violation\n").unwrap();
        let summary = fs::read_to_string(output.join("summary.tsv")).unwrap();
        let without_host = summary
            .lines()
            .filter(|line| {
                !line.starts_with("macos_version\t")
                    && !line.starts_with("macos_build\t")
                    && !line.starts_with("cpu_architecture\t")
                    && !line.starts_with("host_memory_bytes\t")
                    && !line.starts_with("logical_cpus\t")
            })
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(output.join("summary.tsv"), format!("{without_host}\n")).unwrap();
        let missing_provenance = verify_macrobench_artifacts(&output, 201).unwrap_err();
        assert!(
            missing_provenance
                .to_string()
                .contains("missing summary key macos_version"),
            "{missing_provenance}"
        );

        run_macrobench_report(&MacrobenchOptions::smoke(&root), &output).unwrap();
        let measurements = fs::read_to_string(output.join("measurements.tsv")).unwrap();
        let incomplete = measurements
            .lines()
            .filter(|line| *line != "network\tcontent-search\t0\t0\t0")
            .filter(|line| !line.starts_with("network\tcontent-search\t"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(output.join("measurements.tsv"), format!("{incomplete}\n")).unwrap();
        let missing = verify_macrobench_artifacts(&output, 201).unwrap_err();
        assert!(
            missing
                .to_string()
                .contains("summary measurement count 36 does not match"),
            "{missing}"
        );

        fs::remove_dir_all(root).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
