use gfm_types::{GfmError, Result};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const FIXTURE_ROOT: &str = "gfm-parity-fixture";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ParityFixtureScenario {
    Icon,
    List,
    Column,
    Gallery,
    Sidebar,
    Toolbar,
    Search,
    Selection,
    Rename,
    Drag,
    Empty,
    Huge,
    ICloud,
    ExternalVolume,
    NetworkVolume,
    Trash,
    ConflictSheet,
}

impl ParityFixtureScenario {
    pub const ALL: [Self; 17] = [
        Self::Icon,
        Self::List,
        Self::Column,
        Self::Gallery,
        Self::Sidebar,
        Self::Toolbar,
        Self::Search,
        Self::Selection,
        Self::Rename,
        Self::Drag,
        Self::Empty,
        Self::Huge,
        Self::ICloud,
        Self::ExternalVolume,
        Self::NetworkVolume,
        Self::Trash,
        Self::ConflictSheet,
    ];

    pub const fn directory(self) -> &'static str {
        match self {
            Self::Icon => "icon",
            Self::List => "list",
            Self::Column => "column",
            Self::Gallery => "gallery",
            Self::Sidebar => "sidebar",
            Self::Toolbar => "toolbar",
            Self::Search => "search",
            Self::Selection => "selection",
            Self::Rename => "rename",
            Self::Drag => "drag",
            Self::Empty => "empty",
            Self::Huge => "huge",
            Self::ICloud => "icloud",
            Self::ExternalVolume => "external-volume",
            Self::NetworkVolume => "network-volume",
            Self::Trash => "trash",
            Self::ConflictSheet => "conflict-sheet",
        }
    }

    pub const fn finder_view(self) -> &'static str {
        match self {
            Self::Icon
            | Self::Toolbar
            | Self::Selection
            | Self::Rename
            | Self::Drag
            | Self::Empty
            | Self::ICloud
            | Self::ExternalVolume
            | Self::NetworkVolume
            | Self::Trash
            | Self::ConflictSheet => "icon",
            Self::List | Self::Huge => "list",
            Self::Column | Self::Sidebar => "column",
            Self::Gallery | Self::Search => "gallery",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParityFixtureScale {
    pub small_items: usize,
    pub medium_items: usize,
    pub huge_items: usize,
}

impl ParityFixtureScale {
    pub const fn smoke() -> Self {
        Self {
            small_items: 8,
            medium_items: 24,
            huge_items: 128,
        }
    }

    pub const fn standard() -> Self {
        Self {
            small_items: 32,
            medium_items: 256,
            huge_items: 50_000,
        }
    }
}

impl Default for ParityFixtureScale {
    fn default() -> Self {
        Self::standard()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParityFixtureOptions {
    pub workspace: PathBuf,
    pub scale: ParityFixtureScale,
}

impl ParityFixtureOptions {
    pub fn smoke(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            scale: ParityFixtureScale::smoke(),
        }
    }

    pub fn standard(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            scale: ParityFixtureScale::standard(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParityFixtureReport {
    pub fixture_root: PathBuf,
    pub scenarios: Vec<ParityFixtureScenarioReport>,
    pub manifest_path: PathBuf,
}

impl ParityFixtureReport {
    pub fn files_materialized(&self) -> usize {
        self.scenarios.iter().map(|scenario| scenario.files).sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParityFixtureScenarioReport {
    pub scenario: ParityFixtureScenario,
    pub root: PathBuf,
    pub files: usize,
    pub directories: usize,
}

pub fn materialize_parity_fixture(options: &ParityFixtureOptions) -> Result<ParityFixtureReport> {
    fs::create_dir_all(&options.workspace).map_err(|err| GfmError::io(&options.workspace, err))?;
    let fixture_root = options.workspace.join(FIXTURE_ROOT);
    if fixture_root.exists() {
        fs::remove_dir_all(&fixture_root).map_err(|err| GfmError::io(&fixture_root, err))?;
    }
    fs::create_dir_all(&fixture_root).map_err(|err| GfmError::io(&fixture_root, err))?;

    let mut scenarios = Vec::with_capacity(ParityFixtureScenario::ALL.len());
    for scenario in ParityFixtureScenario::ALL {
        scenarios.push(materialize_scenario(
            &fixture_root,
            scenario,
            options.scale,
        )?);
    }

    let manifest_path = fixture_root.join("manifest.tsv");
    write_manifest(&manifest_path, &scenarios)?;

    Ok(ParityFixtureReport {
        fixture_root,
        scenarios,
        manifest_path,
    })
}

fn materialize_scenario(
    fixture_root: &Path,
    scenario: ParityFixtureScenario,
    scale: ParityFixtureScale,
) -> Result<ParityFixtureScenarioReport> {
    let root = fixture_root.join(scenario.directory());
    fs::create_dir_all(&root).map_err(|err| GfmError::io(&root, err))?;

    let mut writer = ScenarioWriter::new(root.clone());
    match scenario {
        ParityFixtureScenario::Icon => icon_fixture(&mut writer, scale.small_items)?,
        ParityFixtureScenario::List => list_fixture(&mut writer, scale.medium_items)?,
        ParityFixtureScenario::Column => column_fixture(&mut writer)?,
        ParityFixtureScenario::Gallery => gallery_fixture(&mut writer, scale.small_items)?,
        ParityFixtureScenario::Sidebar => sidebar_fixture(&mut writer)?,
        ParityFixtureScenario::Toolbar => toolbar_fixture(&mut writer)?,
        ParityFixtureScenario::Search => search_fixture(&mut writer)?,
        ParityFixtureScenario::Selection => selection_fixture(&mut writer)?,
        ParityFixtureScenario::Rename => rename_fixture(&mut writer)?,
        ParityFixtureScenario::Drag => drag_fixture(&mut writer)?,
        ParityFixtureScenario::Empty => empty_fixture(&mut writer)?,
        ParityFixtureScenario::Huge => huge_fixture(&mut writer, scale.huge_items)?,
        ParityFixtureScenario::ICloud => icloud_fixture(&mut writer)?,
        ParityFixtureScenario::ExternalVolume => external_volume_fixture(&mut writer)?,
        ParityFixtureScenario::NetworkVolume => network_volume_fixture(&mut writer)?,
        ParityFixtureScenario::Trash => trash_fixture(&mut writer)?,
        ParityFixtureScenario::ConflictSheet => conflict_sheet_fixture(&mut writer)?,
    }

    Ok(ParityFixtureScenarioReport {
        scenario,
        root,
        files: writer.files,
        directories: writer.directories,
    })
}

fn icon_fixture(writer: &mut ScenarioWriter, count: usize) -> Result<()> {
    writer.dir("Folders/Projects")?;
    writer.dir("Folders/Archive")?;
    for index in 0..count {
        writer.file(
            &format!("Icon Item {index:02}.txt"),
            &format!("icon-view deterministic label wrapping item {index}\n"),
        )?;
    }
    writer.file(
        "Long filename for Finder truncation parity sample document.md",
        "long label\n",
    )
}

fn list_fixture(writer: &mut ScenarioWriter, count: usize) -> Result<()> {
    for index in 0..count {
        let extension = match index % 4 {
            0 => "md",
            1 => "txt",
            2 => "json",
            _ => "log",
        };
        writer.file(
            &format!("List Row {index:04}.{extension}"),
            &format!("list-view sort size date kind row {index}\n"),
        )?;
    }
    Ok(())
}

fn column_fixture(writer: &mut ScenarioWriter) -> Result<()> {
    writer.file("Root Note.md", "column root\n")?;
    writer.file("Projects/GFM/PLAN.md", "column project plan\n")?;
    writer.file("Projects/GFM/src/main.rs", "fn main() {}\n")?;
    writer.file("Projects/Archive/README.txt", "archive\n")?;
    writer.file("Media/Images/Hero.jpg.meta.md", "gallery image metadata\n")
}

fn gallery_fixture(writer: &mut ScenarioWriter, count: usize) -> Result<()> {
    for index in 0..count {
        writer.file(
            &format!("Image {index:02}.jpg.meta.md"),
            &format!("gallery preview width=1920 height=1080 index={index}\n"),
        )?;
    }
    writer.file("PDF Preview.pdf.txt", "pdf preview text stand-in\n")
}

fn sidebar_fixture(writer: &mut ScenarioWriter) -> Result<()> {
    writer.file("Desktop/Desktop Fixture.md", "desktop favorite\n")?;
    writer.file("Documents/Document Fixture.md", "documents favorite\n")?;
    writer.file(
        "Downloads/Download Fixture.zip.meta.md",
        "download favorite\n",
    )?;
    writer.file(
        "Applications/GFM Fixture.app/Contents/Info.plist",
        "bundle fixture\n",
    )?;
    writer.file("Tags/Red.tag", "red tag\n")?;
    writer.file("Tags/Blue.tag", "blue tag\n")
}

fn toolbar_fixture(writer: &mut ScenarioWriter) -> Result<()> {
    writer.file("Back Target.md", "back enabled\n")?;
    writer.file("Forward Target.md", "forward disabled baseline\n")?;
    writer.file("Search Target.txt", "toolbar search field target\n")
}

fn search_fixture(writer: &mut ScenarioWriter) -> Result<()> {
    writer.file("Needle Name.txt", "name result\n")?;
    writer.file("Content Only.md", "deep contentneedle result\n")?;
    writer.file(
        "Screenshots/Screenshot 2026-08-24 at 6.59.43 PM.png.meta.md",
        "screenshot result\n",
    )?;
    writer.file(
        "Applications/GFM.app/Contents/Info.plist",
        "application result\n",
    )
}

fn selection_fixture(writer: &mut ScenarioWriter) -> Result<()> {
    writer.file("Selected A.txt", "selected item a\n")?;
    writer.file("Selected B.txt", "selected item b\n")?;
    writer.file("Unselected.txt", "not selected\n")?;
    writer.file(".gfm-selection.tsv", "Selected A.txt\nSelected B.txt\n")
}

fn rename_fixture(writer: &mut ScenarioWriter) -> Result<()> {
    writer.file("Rename Target.txt", "rename target\n")?;
    writer.file(".gfm-rename-target", "Rename Target.txt\n")
}

fn drag_fixture(writer: &mut ScenarioWriter) -> Result<()> {
    writer.file("Drag Source.txt", "drag source\n")?;
    writer.file("Drag Also Selected.txt", "drag selected\n")?;
    writer.dir("Drop Destination")?;
    writer.file(".gfm-drag.tsv", "Drag Source.txt\nDrag Also Selected.txt\n")
}

fn empty_fixture(_writer: &mut ScenarioWriter) -> Result<()> {
    Ok(())
}

fn huge_fixture(writer: &mut ScenarioWriter, count: usize) -> Result<()> {
    for index in 0..count {
        let shard = format!("Shard {:04}", index / 1_000);
        writer.file(
            &format!("{shard}/Huge Row {index:08}.txt"),
            &format!("huge list virtualization row {index}\n"),
        )?;
    }
    Ok(())
}

fn icloud_fixture(writer: &mut ScenarioWriter) -> Result<()> {
    writer.file("Downloaded.icloud.md", "icloud downloaded\n")?;
    writer.file("Evicted.icloud-placeholder", "icloud evicted placeholder\n")?;
    writer.file("Conflict.icloud-conflict.md", "icloud conflict\n")
}

fn external_volume_fixture(writer: &mut ScenarioWriter) -> Result<()> {
    writer.file(".gfm-volume-kind", "external-removable\n")?;
    writer.file("External Project/README.md", "external volume project\n")?;
    writer.file("External Media/photo.jpg.meta.md", "external media\n")
}

fn network_volume_fixture(writer: &mut ScenarioWriter) -> Result<()> {
    writer.file(".gfm-volume-kind", "network-smb\n")?;
    writer.file("Shared/Team Plan.md", "network share\n")?;
    writer.file(
        "Offline/Unavailable.placeholder",
        "network unavailable placeholder\n",
    )
}

fn trash_fixture(writer: &mut ScenarioWriter) -> Result<()> {
    writer.file(
        ".gfm-trash-origin.tsv",
        "Deleted.txt\t/Users/example/Desktop/Deleted.txt\n",
    )?;
    writer.file("Deleted.txt", "trash item\n")?;
    writer.file("Folder In Trash/Nested.txt", "nested trash item\n")
}

fn conflict_sheet_fixture(writer: &mut ScenarioWriter) -> Result<()> {
    writer.file("Sources/New File.txt", "new file contents\n")?;
    writer.file("Targets/New File.txt", "existing file contents\n")?;
    writer.file("Sources/Project/README.md", "incoming project readme\n")?;
    writer.file("Targets/Project/README.md", "existing project readme\n")?;
    writer.file(
        ".gfm-operation-conflicts.tsv",
        "operation-conflict\toperation=copy\tsource=Sources/New File.txt\ttarget=Targets/New File.txt\texists=true\tkind=file\tpolicy=fail\tavailable=replace,keep-both,skip\tblocks-operation=true\treason=destination-conflict-requires-user-resolution\noperation-conflict\toperation=move\tsource=Sources/Project\ttarget=Targets/Project\texists=true\tkind=directory\tpolicy=fail\tavailable=replace,keep-both,merge,skip\tblocks-operation=true\treason=destination-conflict-requires-user-resolution\n",
    )?;
    writer.file(
        ".gfm-conflict-review.tsv",
        "operation\tsource\ttarget\tkind\tdefault-action\tcancel-action\tkeyboard\ncopy\tSources/New File.txt\tTargets/New File.txt\tfile\tkeep-both\tstop\tfinder-conflict-sheet-return-default-escape-cancel-tab-cycle\nmove\tSources/Project\tTargets/Project\tdirectory\tkeep-both\tstop\tfinder-conflict-sheet-return-default-escape-cancel-tab-cycle\n",
    )
}

fn write_manifest(path: &Path, scenarios: &[ParityFixtureScenarioReport]) -> Result<()> {
    let mut file = fs::File::create(path).map_err(|err| GfmError::io(path, err))?;
    writeln!(file, "scenario\troot\tfinder-view\tfiles\tdirectories")
        .map_err(|err| GfmError::io(path, err))?;
    for scenario in scenarios {
        writeln!(
            file,
            "{}\t{}\t{}\t{}\t{}",
            scenario.scenario.directory(),
            scenario.root.display(),
            scenario.scenario.finder_view(),
            scenario.files,
            scenario.directories
        )
        .map_err(|err| GfmError::io(path, err))?;
    }
    Ok(())
}

struct ScenarioWriter {
    root: PathBuf,
    files: usize,
    directories: usize,
}

impl ScenarioWriter {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            files: 0,
            directories: 1,
        }
    }

    fn dir(&mut self, relative: &str) -> Result<()> {
        let path = self.root.join(relative);
        fs::create_dir_all(&path).map_err(|err| GfmError::io(&path, err))?;
        self.directories += Path::new(relative).components().count();
        Ok(())
    }

    fn file(&mut self, relative: &str, contents: &str) -> Result<()> {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
                self.directories += relative_parent_depth(relative);
            }
        }
        let mut file = fs::File::create(&path).map_err(|err| GfmError::io(&path, err))?;
        file.write_all(contents.as_bytes())
            .map_err(|err| GfmError::io(&path, err))?;
        self.files += 1;
        Ok(())
    }
}

fn relative_parent_depth(relative: &str) -> usize {
    Path::new(relative)
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .map(|path| path.components().count())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn materializes_every_required_parity_scenario() {
        let root = unique_temp_dir("gfm-parity-fixture");
        let report =
            materialize_parity_fixture(&ParityFixtureOptions::smoke(&root)).expect("fixture");

        assert_eq!(report.scenarios.len(), ParityFixtureScenario::ALL.len());
        assert!(report.manifest_path.exists());
        assert!(report
            .fixture_root
            .join("icon")
            .join("Icon Item 00.txt")
            .exists());
        assert_eq!(
            fs::read_dir(report.fixture_root.join("empty"))
                .unwrap()
                .count(),
            0
        );
        assert!(report
            .fixture_root
            .join("trash")
            .join(".gfm-trash-origin.tsv")
            .exists());
        assert!(report
            .fixture_root
            .join("conflict-sheet")
            .join(".gfm-operation-conflicts.tsv")
            .exists());
        assert!(report.files_materialized() > ParityFixtureScenario::ALL.len());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn manifest_is_stable_and_capture_ready() {
        let root = unique_temp_dir("gfm-parity-manifest");
        let report =
            materialize_parity_fixture(&ParityFixtureOptions::smoke(&root)).expect("fixture");
        let manifest = fs::read_to_string(&report.manifest_path).unwrap();

        assert!(manifest.starts_with("scenario\troot\tfinder-view\tfiles\tdirectories\n"));
        assert!(manifest.contains("icon\t"));
        assert!(manifest.contains("\ticon\t"));
        assert!(manifest.contains("network-volume\t"));
        assert!(manifest.contains("trash\t"));
        assert!(manifest.contains("conflict-sheet\t"));

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
