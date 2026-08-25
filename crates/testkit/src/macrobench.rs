use gfm_content::Extractor;
use gfm_index::Indexer;
use gfm_telemetry::{PerformanceBudgets, ScenarioMetric};
use gfm_types::{GfmError, Result};
use std::collections::BTreeMap;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MacrobenchStage {
    IndexBuild,
    HotSearch,
    StreamSearch,
    ContentSearch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacrobenchMeasurement {
    pub scenario: MacrobenchScenario,
    pub stage: MacrobenchStage,
    pub duration: Duration,
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
            records: snapshot.records.len(),
            hits: 0,
        });

        let hot_start = Instant::now();
        let hot_hits = snapshot.search("needle", options.limit);
        let hot_duration = hot_start.elapsed();
        scenario_observations
            .entry(ScenarioMetric::FirstResult)
            .and_modify(|duration| *duration = (*duration).max(hot_duration))
            .or_insert(hot_duration);
        measurements.push(MacrobenchMeasurement {
            scenario,
            stage: MacrobenchStage::HotSearch,
            duration: hot_duration,
            records: snapshot.records.len(),
            hits: hot_hits.len(),
        });

        let stream_start = Instant::now();
        let stream_hits: usize = snapshot
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
