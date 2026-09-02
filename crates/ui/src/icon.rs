use gfm_types::{FileId, FileKind, FileRecord};
use gpui::{div, prelude::*, px, rgb, IntoElement, Styled};
use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::VirtualWindow;

const DEFAULT_ICON_SIZE: u16 = 64;
const DEFAULT_CELL_WIDTH: u16 = 112;
const DEFAULT_CELL_HEIGHT: u16 = 104;
const DEFAULT_LABEL_LINE_HEIGHT: u16 = 17;
const DEFAULT_COLUMNS: u16 = 6;
const DEFAULT_VIEWPORT_ROWS: u16 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconSortMode {
    FinderName,
    ModifiedNewest,
    KindThenName,
}

impl IconSortMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FinderName => "finder-name",
            Self::ModifiedNewest => "modified-newest",
            Self::KindThenName => "kind-then-name",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconViewOptions {
    pub sort: IconSortMode,
    pub icon_size_px: u16,
    pub cell_width_px: u16,
    pub cell_height_px: u16,
    pub label_line_height_px: u16,
    pub columns: u16,
    pub viewport_rows: u16,
    pub scroll_row: u16,
    pub show_hidden: bool,
    pub desktop_mode: bool,
    pub selected: BTreeSet<FileId>,
}

impl Default for IconViewOptions {
    fn default() -> Self {
        Self {
            sort: IconSortMode::FinderName,
            icon_size_px: DEFAULT_ICON_SIZE,
            cell_width_px: DEFAULT_CELL_WIDTH,
            cell_height_px: DEFAULT_CELL_HEIGHT,
            label_line_height_px: DEFAULT_LABEL_LINE_HEIGHT,
            columns: DEFAULT_COLUMNS,
            viewport_rows: DEFAULT_VIEWPORT_ROWS,
            scroll_row: 0,
            show_hidden: false,
            desktop_mode: false,
            selected: BTreeSet::new(),
        }
    }
}

impl IconViewOptions {
    pub fn with_columns(mut self, columns: u16) -> Self {
        self.columns = columns.max(1);
        self
    }

    pub fn with_viewport_rows(mut self, rows: u16) -> Self {
        self.viewport_rows = rows.max(1);
        self
    }

    pub fn with_scroll_row(mut self, row: u16) -> Self {
        self.scroll_row = row;
        self
    }

    pub fn with_selected(mut self, selected: impl IntoIterator<Item = FileId>) -> Self {
        self.selected = selected.into_iter().collect();
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconViewContract {
    pub sort: IconSortMode,
    pub icon_size_px: u16,
    pub cell_width_px: u16,
    pub cell_height_px: u16,
    pub label_line_height_px: u16,
    pub columns: u16,
    pub viewport_rows: u16,
    pub scroll_row: u16,
    pub total_items: usize,
    pub total_rows: u16,
    pub visible_start: usize,
    pub visible_end: usize,
    pub hidden_filtered: usize,
    pub desktop_mode: bool,
    pub cells: Vec<IconCellSpec>,
}

impl IconViewContract {
    pub fn from_records(records: &[FileRecord], options: IconViewOptions) -> Self {
        let original_len = records.len();
        let mut records = records
            .iter()
            .filter(|record| options.show_hidden || !record.hidden)
            .cloned()
            .collect::<Vec<_>>();
        let hidden_filtered = original_len.saturating_sub(records.len());
        sort_records(&mut records, options.sort);
        let columns = options.columns.max(1);
        let viewport_rows = options.viewport_rows.max(1);
        let total_rows = rows_for(records.len(), columns);
        let window = VirtualWindow::grid(records.len(), options.scroll_row, viewport_rows, columns);
        let visible_start = window.start;
        let visible_end = window.end;
        let cells = records[visible_start..visible_end]
            .iter()
            .enumerate()
            .map(|(offset, record)| {
                let index = visible_start + offset;
                IconCellSpec::from_record(
                    record,
                    index,
                    columns,
                    options.cell_width_px,
                    options.cell_height_px,
                    options.selected.contains(&record.id),
                )
            })
            .collect();

        Self {
            sort: options.sort,
            icon_size_px: options.icon_size_px,
            cell_width_px: options.cell_width_px,
            cell_height_px: options.cell_height_px,
            label_line_height_px: options.label_line_height_px,
            columns,
            viewport_rows,
            scroll_row: options.scroll_row,
            total_items: records.len(),
            total_rows,
            visible_start,
            visible_end,
            hidden_filtered,
            desktop_mode: options.desktop_mode,
            cells,
        }
    }

    pub fn as_tsv(&self) -> String {
        let mut lines = Vec::with_capacity(self.cells.len() + 1);
        lines.push(format!(
            "icon-view\tsort={}\ticon={}px\tcell={}x{}\tlabel-line={}px\tcolumns={}\tviewport-rows={}\tscroll-row={}\ttotal={}\trows={}\tvisible={}..{}\thidden-filtered={}\tdesktop={}",
            self.sort.as_str(),
            self.icon_size_px,
            self.cell_width_px,
            self.cell_height_px,
            self.label_line_height_px,
            self.columns,
            self.viewport_rows,
            self.scroll_row,
            self.total_items,
            self.total_rows,
            self.visible_start,
            self.visible_end,
            self.hidden_filtered,
            self.desktop_mode
        ));
        lines.extend(self.cells.iter().map(IconCellSpec::as_tsv));
        lines.join("\n")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconCellSpec {
    pub id: FileId,
    pub index: usize,
    pub row: u16,
    pub column: u16,
    pub x_px: u32,
    pub y_px: u32,
    pub name: String,
    pub path: PathBuf,
    pub kind: FileKind,
    pub icon_role: IconRole,
    pub label_lines: u8,
    pub selected: bool,
    pub badges: Vec<IconBadge>,
}

impl IconCellSpec {
    fn from_record(
        record: &FileRecord,
        index: usize,
        columns: u16,
        cell_width_px: u16,
        cell_height_px: u16,
        selected: bool,
    ) -> Self {
        let column = (index % usize::from(columns)) as u16;
        let row = (index / usize::from(columns)) as u16;
        let mut badges = Vec::new();
        if record.hidden {
            badges.push(IconBadge::Hidden);
        }
        if !record.tags.is_empty() {
            badges.push(IconBadge::Tagged);
        }
        if record.kind == FileKind::Symlink {
            badges.push(IconBadge::Alias);
        }
        if is_package(record) {
            badges.push(IconBadge::Package);
        }

        Self {
            id: record.id,
            index,
            row,
            column,
            x_px: u32::from(column) * u32::from(cell_width_px),
            y_px: u32::from(row) * u32::from(cell_height_px),
            name: record.name.clone(),
            path: record.path.clone(),
            kind: record.kind,
            icon_role: IconRole::for_record(record),
            label_lines: label_lines(&record.name),
            selected,
            badges,
        }
    }

    fn as_tsv(&self) -> String {
        format!(
            "cell\t{}\t{}\t{}\t{}\t{}x{}\t{}\t{}\t{}\t{}\tlines={}\tselected={}\tbadges={}",
            self.index,
            self.id.volume.0,
            self.id.node,
            kind_tsv(self.kind),
            self.x_px,
            self.y_px,
            escape_field(&self.name),
            escape_path_field(&self.path),
            self.icon_role.as_str(),
            self.column,
            self.label_lines,
            self.selected,
            self.badges
                .iter()
                .map(|badge| badge.as_str())
                .collect::<Vec<_>>()
                .join(",")
        )
    }
}

fn escape_path_field(path: &std::path::Path) -> String {
    escape_field(&path.to_string_lossy())
}

fn escape_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconRole {
    Folder,
    File,
    Symlink,
    Package,
    Application,
    Other,
}

impl IconRole {
    fn for_record(record: &FileRecord) -> Self {
        if is_application(record) {
            Self::Application
        } else if is_package(record) {
            Self::Package
        } else {
            match record.kind {
                FileKind::Directory => Self::Folder,
                FileKind::File => Self::File,
                FileKind::Symlink => Self::Symlink,
                FileKind::Other => Self::Other,
            }
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Folder => "folder",
            Self::File => "file",
            Self::Symlink => "symlink",
            Self::Package => "package",
            Self::Application => "application",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconBadge {
    Alias,
    Hidden,
    Package,
    Tagged,
}

impl IconBadge {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Alias => "alias",
            Self::Hidden => "hidden",
            Self::Package => "package",
            Self::Tagged => "tagged",
        }
    }
}

pub fn render(contract: &IconViewContract) -> impl IntoElement {
    let mut grid = div()
        .id("gfm-icon-view")
        .flex()
        .flex_row()
        .flex_wrap()
        .content_start()
        .items_start()
        .gap_0()
        .p(px(18.0))
        .bg(rgb(0x1e1e1e))
        .text_color(rgb(0xd8d8d8));

    for cell in &contract.cells {
        grid = grid.child(render_cell(contract, cell));
    }
    grid
}

fn render_cell(contract: &IconViewContract, cell: &IconCellSpec) -> impl IntoElement {
    let background = if cell.selected {
        rgb(0x4a6ea9)
    } else {
        rgb(0x1e1e1e)
    };
    div()
        .id(("gfm-icon-cell", cell.index))
        .flex()
        .flex_col()
        .items_center()
        .justify_start()
        .w(px(f32::from(contract.cell_width_px)))
        .h(px(f32::from(contract.cell_height_px)))
        .px(px(6.0))
        .py(px(4.0))
        .child(
            div()
                .w(px(f32::from(contract.icon_size_px)))
                .h(px(f32::from(contract.icon_size_px)))
                .rounded(px(6.0))
                .bg(icon_color(cell.icon_role)),
        )
        .child(
            div()
                .mt(px(4.0))
                .px(px(4.0))
                .rounded(px(4.0))
                .bg(background)
                .text_center()
                .text_sm()
                .line_height(px(f32::from(contract.label_line_height_px)))
                .child(cell.name.clone()),
        )
}

fn sort_records(records: &mut [FileRecord], sort: IconSortMode) {
    records.sort_by(|left, right| match sort {
        IconSortMode::FinderName => finder_name_key(left).cmp(&finder_name_key(right)),
        IconSortMode::KindThenName => (kind_group(left), finder_name_key(left))
            .cmp(&(kind_group(right), finder_name_key(right))),
        IconSortMode::ModifiedNewest => right
            .modified
            .cmp(&left.modified)
            .then_with(|| finder_name_key(left).cmp(&finder_name_key(right))),
    });
}

fn rows_for(items: usize, columns: u16) -> u16 {
    let columns = usize::from(columns.max(1));
    let rows = items.div_ceil(columns);
    rows.min(usize::from(u16::MAX)) as u16
}

fn finder_name_key(record: &FileRecord) -> (u8, String) {
    (kind_group(record), record.name.to_lowercase())
}

fn kind_group(record: &FileRecord) -> u8 {
    if record.kind == FileKind::Directory {
        0
    } else {
        1
    }
}

fn is_application(record: &FileRecord) -> bool {
    record.kind == FileKind::Directory && record.name.ends_with(".app")
}

fn is_package(record: &FileRecord) -> bool {
    record.kind == FileKind::Directory
        && matches!(
            record.extension(),
            Some("app" | "bundle" | "framework" | "photoslibrary")
        )
}

fn label_lines(name: &str) -> u8 {
    if name.chars().count() > 18 {
        2
    } else {
        1
    }
}

fn kind_tsv(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Directory => "dir",
        FileKind::File => "file",
        FileKind::Symlink => "link",
        FileKind::Other => "other",
    }
}

fn icon_color(role: IconRole) -> gpui::Rgba {
    match role {
        IconRole::Folder => rgb(0x39a7dc),
        IconRole::File => rgb(0xf2f2f2),
        IconRole::Symlink => rgb(0xcfcfcf),
        IconRole::Package | IconRole::Application => rgb(0x8ba8ff),
        IconRole::Other => rgb(0xa0a0a0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn icon_view_sorts_directories_first_and_tracks_selection_badges() {
        let selected = FileId::new(gfm_types::VolumeId(1), 3);
        let contract = IconViewContract::from_records(
            &[
                record(1, "zeta.txt", FileKind::File),
                record(2, "Folder", FileKind::Directory),
                tagged(record(3, "App.app", FileKind::Directory)),
                hidden(record(4, ".hidden", FileKind::File)),
                record(5, "Link", FileKind::Symlink),
            ],
            IconViewOptions::default()
                .with_columns(2)
                .with_viewport_rows(2)
                .with_selected([selected]),
        );

        assert_eq!(contract.total_items, 4);
        assert_eq!(contract.total_rows, 2);
        assert_eq!(contract.visible_start, 0);
        assert_eq!(contract.visible_end, 4);
        assert_eq!(contract.hidden_filtered, 1);
        assert_eq!(contract.cells[0].name, "App.app");
        assert_eq!(contract.cells[0].icon_role, IconRole::Application);
        assert_eq!(
            contract.cells[0].badges,
            vec![IconBadge::Tagged, IconBadge::Package]
        );
        assert!(contract.cells[0].selected);
        assert_eq!(contract.cells[1].name, "Folder");
        assert_eq!(contract.cells[2].name, "Link");
    }

    #[test]
    fn icon_view_virtualizes_by_scroll_row_and_columns() {
        let records = (0..10)
            .map(|index| record(index, &format!("File {index}.txt"), FileKind::File))
            .collect::<Vec<_>>();
        let contract = IconViewContract::from_records(
            &records,
            IconViewOptions::default()
                .with_columns(3)
                .with_viewport_rows(2)
                .with_scroll_row(1),
        );

        assert_eq!(contract.total_rows, 4);
        assert_eq!(contract.visible_start, 3);
        assert_eq!(contract.visible_end, 9);
        assert_eq!(contract.cells.len(), 6);
        assert_eq!(contract.cells[0].row, 1);
        assert_eq!(contract.cells[0].column, 0);
    }

    #[test]
    fn icon_view_output_is_stable_for_cli_and_fozzy() {
        let contract = IconViewContract::from_records(
            &[
                record(1, "Folder", FileKind::Directory),
                record(2, "Note.txt", FileKind::File),
            ],
            IconViewOptions::default()
                .with_columns(2)
                .with_viewport_rows(1),
        );
        let tsv = contract.as_tsv();

        assert!(tsv.starts_with(
            "icon-view\tsort=finder-name\ticon=64px\tcell=112x104\tlabel-line=17px\tcolumns=2"
        ));
        assert!(tsv.contains("cell\t0\t1\t1\tdir\t0x0\tFolder"));
        assert!(tsv.contains("cell\t1\t1\t2\tfile\t112x0\tNote.txt"));
    }

    #[test]
    fn icon_cell_tsv_escapes_control_characters_in_name_and_path() {
        let contract = IconViewContract::from_records(
            &[record(1, "Reports\tQ3\nDraft\rIcon.txt", FileKind::File)],
            IconViewOptions::default()
                .with_columns(1)
                .with_viewport_rows(1),
        );
        let tsv = contract.as_tsv();
        let cell = tsv.lines().nth(1).unwrap();

        assert!(
            cell.contains(
                "Reports\\tQ3\\nDraft\\rIcon.txt\t/tmp/Reports\\tQ3\\nDraft\\rIcon.txt\t"
            ),
            "{tsv}"
        );
        assert_eq!(cell.split('\t').count(), 13, "{tsv}");
    }

    fn hidden(mut record: FileRecord) -> FileRecord {
        record.hidden = true;
        record
    }

    fn tagged(mut record: FileRecord) -> FileRecord {
        record.tags = vec!["Red".to_string()];
        record
    }

    fn record(node: u64, name: &str, kind: FileKind) -> FileRecord {
        FileRecord {
            id: FileId::new(gfm_types::VolumeId(1), node),
            parent: None,
            path: PathBuf::from("/tmp").join(name),
            name: name.to_string(),
            kind,
            len: node * 10,
            mode: 0,
            owner: 0,
            group: 0,
            xattrs_digest: 0,
            created: Some(UNIX_EPOCH + Duration::from_secs(node)),
            modified: Some(UNIX_EPOCH + Duration::from_secs(node)),
            changed: Some(UNIX_EPOCH + Duration::from_secs(node)),
            hidden: false,
            tags: Vec::new(),
            finder_comment: None,
        }
    }
}
