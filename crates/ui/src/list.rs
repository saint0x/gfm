use gfm_types::{FileId, FileKind, FileRecord};
use gpui::{div, prelude::*, px, rgb, IntoElement, Styled};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::VirtualWindow;

const DEFAULT_ROW_HEIGHT: u16 = 22;
const DEFAULT_VIEWPORT_ROWS: u16 = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListSortMode {
    FinderName,
    ModifiedNewest,
    SizeLargest,
    KindThenName,
}

impl ListSortMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FinderName => "finder-name",
            Self::ModifiedNewest => "modified-newest",
            Self::SizeLargest => "size-largest",
            Self::KindThenName => "kind-then-name",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ListColumnKind {
    Name,
    DateModified,
    Size,
    Kind,
    Tags,
}

impl ListColumnKind {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::DateModified => "date-modified",
            Self::Size => "size",
            Self::Kind => "kind",
            Self::Tags => "tags",
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::DateModified => "Date Modified",
            Self::Size => "Size",
            Self::Kind => "Kind",
            Self::Tags => "Tags",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListColumnSpec {
    pub kind: ListColumnKind,
    pub title: String,
    pub width_px: u16,
    pub min_width_px: u16,
    pub resizable: bool,
    pub visible: bool,
}

impl ListColumnSpec {
    pub fn new(kind: ListColumnKind, width_px: u16, min_width_px: u16) -> Self {
        Self {
            kind,
            title: kind.title().to_string(),
            width_px,
            min_width_px,
            resizable: true,
            visible: true,
        }
    }

    fn as_tsv(&self) -> String {
        format!(
            "column\t{}\t{}\t{}px\tmin={}px\tresizable={}\tvisible={}",
            self.kind.id(),
            self.title,
            self.width_px,
            self.min_width_px,
            self.resizable,
            self.visible
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListViewOptions {
    pub sort: ListSortMode,
    pub row_height_px: u16,
    pub viewport_rows: u16,
    pub scroll_row: u32,
    pub show_hidden: bool,
    pub alternating_rows: bool,
    pub columns: Vec<ListColumnSpec>,
    pub selected: BTreeSet<FileId>,
    pub disclosed: BTreeSet<FileId>,
}

impl Default for ListViewOptions {
    fn default() -> Self {
        Self {
            sort: ListSortMode::FinderName,
            row_height_px: DEFAULT_ROW_HEIGHT,
            viewport_rows: DEFAULT_VIEWPORT_ROWS,
            scroll_row: 0,
            show_hidden: false,
            alternating_rows: true,
            columns: default_columns(),
            selected: BTreeSet::new(),
            disclosed: BTreeSet::new(),
        }
    }
}

impl ListViewOptions {
    pub fn with_viewport_rows(mut self, rows: u16) -> Self {
        self.viewport_rows = rows.max(1);
        self
    }

    pub fn with_scroll_row(mut self, row: u32) -> Self {
        self.scroll_row = row;
        self
    }

    pub fn with_selected(mut self, selected: impl IntoIterator<Item = FileId>) -> Self {
        self.selected = selected.into_iter().collect();
        self
    }

    pub fn with_disclosed(mut self, disclosed: impl IntoIterator<Item = FileId>) -> Self {
        self.disclosed = disclosed.into_iter().collect();
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListViewContract {
    pub sort: ListSortMode,
    pub row_height_px: u16,
    pub viewport_rows: u16,
    pub scroll_row: u32,
    pub total_rows: usize,
    pub visible_start: usize,
    pub visible_end: usize,
    pub hidden_filtered: usize,
    pub alternating_rows: bool,
    pub columns: Vec<ListColumnSpec>,
    pub rows: Vec<ListRowSpec>,
}

impl ListViewContract {
    pub fn from_records(records: &[FileRecord], options: ListViewOptions) -> Self {
        let original_len = records.len();
        let mut records = records
            .iter()
            .filter(|record| options.show_hidden || !record.hidden)
            .cloned()
            .collect::<Vec<_>>();
        let hidden_filtered = original_len.saturating_sub(records.len());
        sort_records(&mut records, options.sort);

        let viewport_rows = options.viewport_rows.max(1);
        let window = VirtualWindow::rows(records.len(), options.scroll_row, viewport_rows);
        let visible_start = window.start;
        let visible_end = window.end;
        let rows = records[visible_start..visible_end]
            .iter()
            .enumerate()
            .map(|(offset, record)| {
                let index = visible_start + offset;
                ListRowSpec::from_record(
                    record,
                    index,
                    options.row_height_px.max(1),
                    options.selected.contains(&record.id),
                    options.disclosed.contains(&record.id),
                    options.alternating_rows && index % 2 == 1,
                    &options.columns,
                )
            })
            .collect();

        Self {
            sort: options.sort,
            row_height_px: options.row_height_px.max(1),
            viewport_rows,
            scroll_row: options.scroll_row,
            total_rows: records.len(),
            visible_start,
            visible_end,
            hidden_filtered,
            alternating_rows: options.alternating_rows,
            columns: options.columns,
            rows,
        }
    }

    pub fn as_tsv(&self) -> String {
        let mut lines = Vec::with_capacity(self.columns.len() + self.rows.len() + 1);
        lines.push(format!(
            "list-view\tsort={}\trow-height={}px\tviewport-rows={}\tscroll-row={}\ttotal={}\tvisible={}..{}\thidden-filtered={}\talternating={}",
            self.sort.as_str(),
            self.row_height_px,
            self.viewport_rows,
            self.scroll_row,
            self.total_rows,
            self.visible_start,
            self.visible_end,
            self.hidden_filtered,
            self.alternating_rows
        ));
        lines.extend(self.columns.iter().map(ListColumnSpec::as_tsv));
        lines.extend(self.rows.iter().map(ListRowSpec::as_tsv));
        lines.join("\n")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListCellSpec {
    pub column: ListColumnKind,
    pub text: String,
    pub width_px: u16,
}

impl ListCellSpec {
    fn as_tsv(&self) -> String {
        format!("{}={}", self.column.id(), self.text)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListRowSpec {
    pub id: FileId,
    pub index: usize,
    pub y_px: u32,
    pub depth: u16,
    pub name: String,
    pub path: PathBuf,
    pub kind: FileKind,
    pub expandable: bool,
    pub disclosed: bool,
    pub selected: bool,
    pub alternating: bool,
    pub cells: Vec<ListCellSpec>,
}

impl ListRowSpec {
    fn from_record(
        record: &FileRecord,
        index: usize,
        row_height_px: u16,
        selected: bool,
        disclosed: bool,
        alternating: bool,
        columns: &[ListColumnSpec],
    ) -> Self {
        let cells = columns
            .iter()
            .filter(|column| column.visible)
            .map(|column| ListCellSpec {
                column: column.kind,
                text: cell_text(record, column.kind),
                width_px: column.width_px,
            })
            .collect();

        Self {
            id: record.id,
            index,
            y_px: (index as u32).saturating_mul(u32::from(row_height_px)),
            depth: 0,
            name: record.name.clone(),
            path: record.path.clone(),
            kind: record.kind,
            expandable: record.kind == FileKind::Directory,
            disclosed,
            selected,
            alternating,
            cells,
        }
    }

    fn as_tsv(&self) -> String {
        format!(
            "row\t{}\t{}\t{}\t{}\t{}px\tdepth={}\texpandable={}\tdisclosed={}\tselected={}\talternating={}\t{}\t{}",
            self.index,
            self.id.volume.0,
            self.id.node,
            kind_tsv(self.kind),
            self.y_px,
            self.depth,
            self.expandable,
            self.disclosed,
            self.selected,
            self.alternating,
            self.name,
            self.cells
                .iter()
                .map(ListCellSpec::as_tsv)
                .collect::<Vec<_>>()
                .join("\t")
        )
    }
}

pub fn render(contract: &ListViewContract) -> impl IntoElement {
    let mut view = div()
        .id("gfm-list-view")
        .flex()
        .flex_col()
        .bg(rgb(0x1e1e1e))
        .text_color(rgb(0xd8d8d8));

    view = view.child(render_header(contract));
    for row in &contract.rows {
        view = view.child(render_row(contract, row));
    }
    view
}

fn render_header(contract: &ListViewContract) -> impl IntoElement {
    let mut header = div()
        .id("gfm-list-header")
        .flex()
        .flex_row()
        .items_center()
        .h(px(f32::from(contract.row_height_px)))
        .bg(rgb(0x252525))
        .text_color(rgb(0xbdbdbd))
        .text_xs();

    for column in contract.columns.iter().filter(|column| column.visible) {
        header = header.child(
            div()
                .w(px(f32::from(column.width_px)))
                .px(px(6.0))
                .child(column.title.clone()),
        );
    }
    header
}

fn render_row(contract: &ListViewContract, row: &ListRowSpec) -> impl IntoElement {
    let background = if row.selected {
        rgb(0x4a6ea9)
    } else if row.alternating {
        rgb(0x222222)
    } else {
        rgb(0x1e1e1e)
    };
    let mut element = div()
        .id(("gfm-list-row", row.index))
        .flex()
        .flex_row()
        .items_center()
        .h(px(f32::from(contract.row_height_px)))
        .bg(background)
        .text_sm();

    for cell in &row.cells {
        element = element.child(
            div()
                .w(px(f32::from(cell.width_px)))
                .px(px(6.0))
                .child(cell.text.clone()),
        );
    }
    element
}

fn default_columns() -> Vec<ListColumnSpec> {
    vec![
        ListColumnSpec::new(ListColumnKind::Name, 260, 120),
        ListColumnSpec::new(ListColumnKind::DateModified, 160, 120),
        ListColumnSpec::new(ListColumnKind::Size, 92, 64),
        ListColumnSpec::new(ListColumnKind::Kind, 140, 88),
        ListColumnSpec::new(ListColumnKind::Tags, 120, 72),
    ]
}

fn sort_records(records: &mut [FileRecord], sort: ListSortMode) {
    records.sort_by(|left, right| match sort {
        ListSortMode::FinderName => finder_name_key(left).cmp(&finder_name_key(right)),
        ListSortMode::KindThenName => (kind_name(left), finder_name_key(left))
            .cmp(&(kind_name(right), finder_name_key(right))),
        ListSortMode::ModifiedNewest => right
            .modified
            .cmp(&left.modified)
            .then_with(|| finder_name_key(left).cmp(&finder_name_key(right))),
        ListSortMode::SizeLargest => right
            .len
            .cmp(&left.len)
            .then_with(|| finder_name_key(left).cmp(&finder_name_key(right))),
    });
}

fn finder_name_key(record: &FileRecord) -> (u8, String) {
    (directory_group(record), record.name.to_lowercase())
}

fn directory_group(record: &FileRecord) -> u8 {
    if record.kind == FileKind::Directory {
        0
    } else {
        1
    }
}

fn kind_name(record: &FileRecord) -> &'static str {
    match record.kind {
        FileKind::Directory => "folder",
        FileKind::File => "document",
        FileKind::Symlink => "alias",
        FileKind::Other => "other",
    }
}

fn cell_text(record: &FileRecord, column: ListColumnKind) -> String {
    match column {
        ListColumnKind::Name => record.name.clone(),
        ListColumnKind::DateModified => format_time(record.modified),
        ListColumnKind::Size => format_size(record),
        ListColumnKind::Kind => kind_name(record).to_string(),
        ListColumnKind::Tags => record.tags.join(","),
    }
}

fn format_time(time: Option<SystemTime>) -> String {
    time.and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn format_size(record: &FileRecord) -> String {
    if record.kind == FileKind::Directory {
        "--".to_string()
    } else {
        record.len.to_string()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn list_view_sorts_directories_first_and_tracks_row_state() {
        let folder = FileId::new(gfm_types::VolumeId(1), 2);
        let selected = FileId::new(gfm_types::VolumeId(1), 3);
        let contract = ListViewContract::from_records(
            &[
                record(1, "zeta.txt", FileKind::File),
                record(2, "Folder", FileKind::Directory),
                tagged(record(3, "App.app", FileKind::Directory)),
                hidden(record(4, ".hidden", FileKind::File)),
                record(5, "Link", FileKind::Symlink),
            ],
            ListViewOptions::default()
                .with_viewport_rows(4)
                .with_selected([selected])
                .with_disclosed([folder]),
        );

        assert_eq!(contract.total_rows, 4);
        assert_eq!(contract.visible_start, 0);
        assert_eq!(contract.visible_end, 4);
        assert_eq!(contract.hidden_filtered, 1);
        assert_eq!(contract.rows[0].name, "App.app");
        assert!(contract.rows[0].selected);
        assert!(contract.rows[0].expandable);
        assert!(!contract.rows[0].disclosed);
        assert_eq!(contract.rows[1].name, "Folder");
        assert!(contract.rows[1].disclosed);
        assert!(contract.rows[1].alternating);
        assert_eq!(contract.rows[2].name, "Link");
    }

    #[test]
    fn list_view_virtualizes_by_scroll_row() {
        let records = (0..10)
            .map(|index| record(index, &format!("File {index}.txt"), FileKind::File))
            .collect::<Vec<_>>();
        let contract = ListViewContract::from_records(
            &records,
            ListViewOptions::default()
                .with_viewport_rows(3)
                .with_scroll_row(4),
        );

        assert_eq!(contract.total_rows, 10);
        assert_eq!(contract.visible_start, 4);
        assert_eq!(contract.visible_end, 7);
        assert_eq!(contract.rows.len(), 3);
        assert_eq!(contract.rows[0].index, 4);
        assert_eq!(contract.rows[0].y_px, 88);
    }

    #[test]
    fn list_view_output_is_stable_for_cli_and_fozzy() {
        let contract = ListViewContract::from_records(
            &[
                record(1, "Folder", FileKind::Directory),
                record(2, "Note.txt", FileKind::File),
            ],
            ListViewOptions::default().with_viewport_rows(2),
        );
        let tsv = contract.as_tsv();

        assert!(tsv.starts_with("list-view\tsort=finder-name\trow-height=22px\tviewport-rows=2"));
        assert!(tsv.contains("column\tname\tName\t260px\tmin=120px"));
        assert!(tsv.contains("row\t0\t1\t1\tdir\t0px\tdepth=0"));
        assert!(tsv.contains("row\t1\t1\t2\tfile\t22px\tdepth=0"));
        assert!(tsv.contains("name=Note.txt"));
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
            created: Some(UNIX_EPOCH + Duration::from_secs(node)),
            modified: Some(UNIX_EPOCH + Duration::from_secs(node)),
            changed: Some(UNIX_EPOCH + Duration::from_secs(node)),
            hidden: false,
            tags: Vec::new(),
        }
    }
}
