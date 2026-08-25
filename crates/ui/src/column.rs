use gfm_types::{FileId, FileKind, FileRecord};
use gpui::{div, prelude::*, px, rgb, IntoElement, Styled};
use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::VirtualWindow;

const DEFAULT_COLUMN_WIDTH: u16 = 220;
const DEFAULT_MIN_COLUMN_WIDTH: u16 = 160;
const DEFAULT_ROW_HEIGHT: u16 = 24;
const DEFAULT_VIEWPORT_ROWS: u16 = 24;
const DEFAULT_PREVIEW_WIDTH: u16 = 280;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnSortMode {
    FinderName,
    ModifiedNewest,
    KindThenName,
}

impl ColumnSortMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FinderName => "finder-name",
            Self::ModifiedNewest => "modified-newest",
            Self::KindThenName => "kind-then-name",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnViewOptions {
    pub sort: ColumnSortMode,
    pub column_width_px: u16,
    pub min_column_width_px: u16,
    pub row_height_px: u16,
    pub viewport_rows: u16,
    pub show_hidden: bool,
    pub preview_width_px: u16,
    pub selected: BTreeSet<FileId>,
    pub scroll_rows: Vec<u32>,
}

impl Default for ColumnViewOptions {
    fn default() -> Self {
        Self {
            sort: ColumnSortMode::FinderName,
            column_width_px: DEFAULT_COLUMN_WIDTH,
            min_column_width_px: DEFAULT_MIN_COLUMN_WIDTH,
            row_height_px: DEFAULT_ROW_HEIGHT,
            viewport_rows: DEFAULT_VIEWPORT_ROWS,
            show_hidden: false,
            preview_width_px: DEFAULT_PREVIEW_WIDTH,
            selected: BTreeSet::new(),
            scroll_rows: Vec::new(),
        }
    }
}

impl ColumnViewOptions {
    pub fn with_viewport_rows(mut self, rows: u16) -> Self {
        self.viewport_rows = rows.max(1);
        self
    }

    pub fn with_selected(mut self, selected: impl IntoIterator<Item = FileId>) -> Self {
        self.selected = selected.into_iter().collect();
        self
    }

    pub fn with_scroll_rows(mut self, rows: impl IntoIterator<Item = u32>) -> Self {
        self.scroll_rows = rows.into_iter().collect();
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnSource {
    pub path: PathBuf,
    pub records: Vec<FileRecord>,
    pub selected: Option<FileId>,
    pub scroll_row: u32,
}

impl ColumnSource {
    pub fn new(path: impl Into<PathBuf>, records: Vec<FileRecord>) -> Self {
        Self {
            path: path.into(),
            records,
            selected: None,
            scroll_row: 0,
        }
    }

    pub fn with_selected(mut self, selected: Option<FileId>) -> Self {
        self.selected = selected;
        self
    }

    pub fn with_scroll_row(mut self, scroll_row: u32) -> Self {
        self.scroll_row = scroll_row;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnViewContract {
    pub sort: ColumnSortMode,
    pub column_width_px: u16,
    pub min_column_width_px: u16,
    pub row_height_px: u16,
    pub viewport_rows: u16,
    pub preview_width_px: u16,
    pub hidden_filtered: usize,
    pub keyboard_flow: ColumnKeyboardFlow,
    pub columns: Vec<ColumnSpec>,
    pub preview: Option<PreviewColumnSpec>,
}

impl ColumnViewContract {
    pub fn from_sources(sources: Vec<ColumnSource>, options: ColumnViewOptions) -> Self {
        let mut hidden_filtered = 0;
        let mut selected_record = None;
        let mut columns = Vec::with_capacity(sources.len());

        for (index, source) in sources.into_iter().enumerate() {
            let scroll_row = options
                .scroll_rows
                .get(index)
                .copied()
                .unwrap_or(source.scroll_row);
            let column =
                ColumnSpec::from_source(index, source, &options, scroll_row, &mut hidden_filtered);
            if let Some(row) = column.rows.iter().find(|row| row.selected) {
                selected_record = Some(row.preview_record());
            }
            columns.push(column);
        }

        let preview = selected_record.map(|record| {
            PreviewColumnSpec::from_record(
                columns.len(),
                options.preview_width_px.max(DEFAULT_MIN_COLUMN_WIDTH),
                record,
            )
        });

        Self {
            sort: options.sort,
            column_width_px: options.column_width_px.max(options.min_column_width_px),
            min_column_width_px: options.min_column_width_px.max(1),
            row_height_px: options.row_height_px.max(1),
            viewport_rows: options.viewport_rows.max(1),
            preview_width_px: options.preview_width_px.max(DEFAULT_MIN_COLUMN_WIDTH),
            hidden_filtered,
            keyboard_flow: ColumnKeyboardFlow::finder_default(),
            columns,
            preview,
        }
    }

    pub fn as_tsv(&self) -> String {
        let mut lines = Vec::with_capacity(
            1 + self
                .columns
                .iter()
                .map(|column| column.rows.len() + 1)
                .sum::<usize>(),
        );
        lines.push(format!(
            "column-view\tsort={}\tcolumn-width={}px\tmin-column-width={}px\trow-height={}px\tviewport-rows={}\tcolumns={}\tpreview={}\thidden-filtered={}\tkeyboard={}",
            self.sort.as_str(),
            self.column_width_px,
            self.min_column_width_px,
            self.row_height_px,
            self.viewport_rows,
            self.columns.len(),
            self.preview.is_some(),
            self.hidden_filtered,
            self.keyboard_flow.as_str()
        ));
        for column in &self.columns {
            lines.push(column.as_tsv());
            lines.extend(column.rows.iter().map(ColumnRowSpec::as_tsv));
        }
        if let Some(preview) = &self.preview {
            lines.push(preview.as_tsv());
        }
        lines.join("\n")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnSpec {
    pub index: usize,
    pub path: PathBuf,
    pub x_px: u32,
    pub width_px: u16,
    pub scroll_row: u32,
    pub total_rows: usize,
    pub visible_start: usize,
    pub visible_end: usize,
    pub rows: Vec<ColumnRowSpec>,
}

impl ColumnSpec {
    fn from_source(
        index: usize,
        source: ColumnSource,
        options: &ColumnViewOptions,
        scroll_row: u32,
        hidden_filtered: &mut usize,
    ) -> Self {
        let original_len = source.records.len();
        let mut records = source
            .records
            .into_iter()
            .filter(|record| options.show_hidden || !record.hidden)
            .collect::<Vec<_>>();
        *hidden_filtered += original_len.saturating_sub(records.len());
        sort_records(&mut records, options.sort);

        let window = VirtualWindow::rows(records.len(), scroll_row, options.viewport_rows);
        let visible_start = window.start;
        let visible_end = window.end;
        let selected = source
            .selected
            .into_iter()
            .chain(options.selected.iter().copied())
            .collect::<BTreeSet<_>>();
        let width_px = options.column_width_px.max(options.min_column_width_px);
        let rows = records[visible_start..visible_end]
            .iter()
            .enumerate()
            .map(|(offset, record)| {
                let row_index = visible_start + offset;
                ColumnRowSpec::from_record(
                    record,
                    index,
                    row_index,
                    options.row_height_px.max(1),
                    selected.contains(&record.id),
                )
            })
            .collect();

        Self {
            index,
            path: source.path,
            x_px: (index as u32).saturating_mul(u32::from(width_px)),
            width_px,
            scroll_row,
            total_rows: records.len(),
            visible_start,
            visible_end,
            rows,
        }
    }

    fn as_tsv(&self) -> String {
        format!(
            "column\t{}\t{}\t{}px\twidth={}px\tscroll-row={}\ttotal={}\tvisible={}..{}",
            self.index,
            self.path.display(),
            self.x_px,
            self.width_px,
            self.scroll_row,
            self.total_rows,
            self.visible_start,
            self.visible_end
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnRowSpec {
    pub id: FileId,
    pub column: usize,
    pub row: usize,
    pub y_px: u32,
    pub name: String,
    pub path: PathBuf,
    pub kind: FileKind,
    pub size: u64,
    pub selected: bool,
    pub expandable: bool,
    pub previewable: bool,
    pub branch_loaded: bool,
}

impl ColumnRowSpec {
    fn from_record(
        record: &FileRecord,
        column: usize,
        row: usize,
        row_height_px: u16,
        selected: bool,
    ) -> Self {
        Self {
            id: record.id,
            column,
            row,
            y_px: (row as u32).saturating_mul(u32::from(row_height_px)),
            name: record.name.clone(),
            path: record.path.clone(),
            kind: record.kind,
            size: record.len,
            selected,
            expandable: record.kind == FileKind::Directory,
            previewable: record.kind != FileKind::Directory,
            branch_loaded: selected && record.kind == FileKind::Directory,
        }
    }

    fn preview_record(&self) -> PreviewRecord {
        PreviewRecord {
            id: self.id,
            name: self.name.clone(),
            path: self.path.clone(),
            kind: self.kind,
            size: self.size,
        }
    }

    fn as_tsv(&self) -> String {
        format!(
            "row\t{}\t{}\t{}\t{}\t{}\t{}px\t{}\t{}\tselected={}\texpandable={}\tpreviewable={}\tbranch-loaded={}",
            self.column,
            self.row,
            self.id.volume.0,
            self.id.node,
            kind_tsv(self.kind),
            self.y_px,
            self.name,
            self.path.display(),
            self.selected,
            self.expandable,
            self.previewable,
            self.branch_loaded
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreviewRecord {
    id: FileId,
    name: String,
    path: PathBuf,
    kind: FileKind,
    size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewColumnSpec {
    pub index: usize,
    pub x_px: u32,
    pub width_px: u16,
    pub id: FileId,
    pub name: String,
    pub path: PathBuf,
    pub kind: FileKind,
    pub size: u64,
    pub role: PreviewRole,
}

impl PreviewColumnSpec {
    fn from_record(index: usize, width_px: u16, record: PreviewRecord) -> Self {
        Self {
            index,
            x_px: (index as u32).saturating_mul(u32::from(DEFAULT_COLUMN_WIDTH)),
            width_px,
            id: record.id,
            name: record.name,
            path: record.path,
            kind: record.kind,
            size: record.size,
            role: PreviewRole::for_kind(record.kind),
        }
    }

    fn as_tsv(&self) -> String {
        format!(
            "preview\t{}\t{}\t{}px\twidth={}px\t{}\t{}\t{}\t{}\tsize={}",
            self.index,
            self.id.volume.0,
            self.x_px,
            self.width_px,
            self.id.node,
            kind_tsv(self.kind),
            self.role.as_str(),
            self.name,
            self.size
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewRole {
    FolderSummary,
    FilePreview,
    AliasPreview,
    GenericPreview,
}

impl PreviewRole {
    fn for_kind(kind: FileKind) -> Self {
        match kind {
            FileKind::Directory => Self::FolderSummary,
            FileKind::File => Self::FilePreview,
            FileKind::Symlink => Self::AliasPreview,
            FileKind::Other => Self::GenericPreview,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FolderSummary => "folder-summary",
            Self::FilePreview => "file-preview",
            Self::AliasPreview => "alias-preview",
            Self::GenericPreview => "generic-preview",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnKeyboardFlow {
    FinderLeftRightColumnNavigation,
}

impl ColumnKeyboardFlow {
    const fn finder_default() -> Self {
        Self::FinderLeftRightColumnNavigation
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FinderLeftRightColumnNavigation => "finder-left-right-column-navigation",
        }
    }
}

pub fn render(contract: &ColumnViewContract) -> impl IntoElement {
    let mut view = div()
        .id("gfm-column-view")
        .flex()
        .flex_row()
        .bg(rgb(0x1e1e1e))
        .text_color(rgb(0xd8d8d8));

    for column in &contract.columns {
        view = view.child(render_column(contract, column));
    }
    if let Some(preview) = &contract.preview {
        view = view.child(render_preview(preview));
    }
    view
}

fn render_column(contract: &ColumnViewContract, column: &ColumnSpec) -> impl IntoElement {
    let mut element = div()
        .id(("gfm-column", column.index))
        .flex()
        .flex_col()
        .w(px(f32::from(column.width_px)))
        .border_r_1()
        .border_color(rgb(0x333333));

    for row in &column.rows {
        element = element.child(render_row(contract, row));
    }
    element
}

fn render_row(contract: &ColumnViewContract, row: &ColumnRowSpec) -> impl IntoElement {
    let background = if row.selected {
        rgb(0x4a6ea9)
    } else {
        rgb(0x1e1e1e)
    };
    let disclosure = if row.expandable { ">" } else { "" };
    div()
        .id((
            "gfm-column-row",
            row.column.saturating_mul(1_000_000).saturating_add(row.row),
        ))
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .h(px(f32::from(contract.row_height_px)))
        .px(px(8.0))
        .bg(background)
        .text_sm()
        .child(row.name.clone())
        .child(disclosure)
}

fn render_preview(preview: &PreviewColumnSpec) -> impl IntoElement {
    div()
        .id("gfm-column-preview")
        .flex()
        .flex_col()
        .w(px(f32::from(preview.width_px)))
        .p(px(16.0))
        .bg(rgb(0x202020))
        .child(
            div()
                .w(px(72.0))
                .h(px(72.0))
                .rounded(px(6.0))
                .bg(preview_color(preview.role)),
        )
        .child(div().mt(px(10.0)).text_sm().child(preview.name.clone()))
        .child(
            div()
                .mt(px(4.0))
                .text_xs()
                .text_color(rgb(0xa8a8a8))
                .child(preview.role.as_str()),
        )
}

fn sort_records(records: &mut [FileRecord], sort: ColumnSortMode) {
    records.sort_by(|left, right| match sort {
        ColumnSortMode::FinderName => finder_name_key(left).cmp(&finder_name_key(right)),
        ColumnSortMode::KindThenName => (kind_name(left), finder_name_key(left))
            .cmp(&(kind_name(right), finder_name_key(right))),
        ColumnSortMode::ModifiedNewest => right
            .modified
            .cmp(&left.modified)
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

fn kind_tsv(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Directory => "dir",
        FileKind::File => "file",
        FileKind::Symlink => "link",
        FileKind::Other => "other",
    }
}

fn preview_color(role: PreviewRole) -> gpui::Rgba {
    match role {
        PreviewRole::FolderSummary => rgb(0x39a7dc),
        PreviewRole::FilePreview => rgb(0xf2f2f2),
        PreviewRole::AliasPreview => rgb(0xcfcfcf),
        PreviewRole::GenericPreview => rgb(0xa0a0a0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn column_view_tracks_selection_branch_and_preview() {
        let folder = record(2, "Folder", FileKind::Directory);
        let folder_id = folder.id;
        let contract = ColumnViewContract::from_sources(
            vec![
                ColumnSource::new(
                    "/tmp",
                    vec![
                        record(1, "zeta.txt", FileKind::File),
                        folder.clone(),
                        hidden(record(3, ".hidden", FileKind::File)),
                    ],
                )
                .with_selected(Some(folder_id)),
                ColumnSource::new(
                    "/tmp/Folder",
                    vec![
                        record(4, "Child.txt", FileKind::File),
                        record(5, "Sub", FileKind::Directory),
                    ],
                ),
            ],
            ColumnViewOptions::default(),
        );

        assert_eq!(contract.columns.len(), 2);
        assert_eq!(contract.hidden_filtered, 1);
        assert_eq!(contract.columns[0].rows[0].name, "Folder");
        assert!(contract.columns[0].rows[0].selected);
        assert!(contract.columns[0].rows[0].branch_loaded);
        assert_eq!(contract.columns[1].rows[0].name, "Sub");
        assert_eq!(
            contract.preview.as_ref().map(|preview| preview.role),
            Some(PreviewRole::FolderSummary)
        );
    }

    #[test]
    fn column_view_virtualizes_each_column_independently() {
        let records = (0..10)
            .map(|index| record(index, &format!("File {index}.txt"), FileKind::File))
            .collect::<Vec<_>>();
        let contract = ColumnViewContract::from_sources(
            vec![ColumnSource::new("/tmp", records).with_scroll_row(4)],
            ColumnViewOptions::default().with_viewport_rows(3),
        );

        assert_eq!(contract.columns[0].total_rows, 10);
        assert_eq!(contract.columns[0].visible_start, 4);
        assert_eq!(contract.columns[0].visible_end, 7);
        assert_eq!(contract.columns[0].rows.len(), 3);
        assert_eq!(contract.columns[0].rows[0].row, 4);
        assert_eq!(contract.columns[0].rows[0].y_px, 96);
    }

    #[test]
    fn column_view_output_is_stable_for_cli_and_fozzy() {
        let selected = FileId::new(gfm_types::VolumeId(1), 2);
        let contract = ColumnViewContract::from_sources(
            vec![ColumnSource::new(
                "/tmp",
                vec![
                    record(1, "Folder", FileKind::Directory),
                    record(2, "Note.txt", FileKind::File),
                ],
            )
            .with_selected(Some(selected))],
            ColumnViewOptions::default().with_viewport_rows(2),
        );
        let tsv = contract.as_tsv();

        assert!(tsv.starts_with(
            "column-view\tsort=finder-name\tcolumn-width=220px\tmin-column-width=160px"
        ));
        assert!(tsv.contains("column\t0\t/tmp\t0px\twidth=220px"));
        assert!(tsv.contains("row\t0\t0\t1\t1\tdir\t0px\tFolder"));
        assert!(tsv.contains("row\t0\t1\t1\t2\tfile\t24px\tNote.txt"));
        assert!(tsv.contains("preview\t1\t1\t220px\twidth=280px\t2\tfile\tfile-preview\tNote.txt"));
    }

    fn hidden(mut record: FileRecord) -> FileRecord {
        record.hidden = true;
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
