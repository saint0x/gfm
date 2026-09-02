use gfm_types::{FileId, FileKind, FileRecord};
use gpui::{div, prelude::*, px, rgb, IntoElement, Styled};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use crate::VirtualWindow;

const DEFAULT_ROW_HEIGHT: u16 = 24;
const DEFAULT_VIEWPORT_ROWS: u16 = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrashSortMode {
    DeletedNewest,
    FinderName,
    OriginalLocation,
}

impl TrashSortMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DeletedNewest => "deleted-newest",
            Self::FinderName => "finder-name",
            Self::OriginalLocation => "original-location",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashEntryMetadata {
    pub original_path: Option<PathBuf>,
    pub deleted_at: Option<String>,
    pub can_restore: bool,
    pub can_delete_permanently: bool,
    pub permission_issue: Option<String>,
}

impl TrashEntryMetadata {
    pub fn restorable(original_path: impl Into<PathBuf>, deleted_at: impl Into<String>) -> Self {
        Self {
            original_path: Some(original_path.into()),
            deleted_at: Some(deleted_at.into()),
            can_restore: true,
            can_delete_permanently: true,
            permission_issue: None,
        }
    }

    pub fn blocked(message: impl Into<String>) -> Self {
        Self {
            original_path: None,
            deleted_at: None,
            can_restore: false,
            can_delete_permanently: false,
            permission_issue: Some(message.into()),
        }
    }
}

impl Default for TrashEntryMetadata {
    fn default() -> Self {
        Self {
            original_path: None,
            deleted_at: None,
            can_restore: false,
            can_delete_permanently: true,
            permission_issue: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashViewOptions {
    pub sort: TrashSortMode,
    pub row_height_px: u16,
    pub viewport_rows: u16,
    pub scroll_row: u32,
    pub selected: BTreeSet<FileId>,
    pub metadata_by_name: BTreeMap<String, TrashEntryMetadata>,
    pub empty_trash_enabled: bool,
}

impl Default for TrashViewOptions {
    fn default() -> Self {
        Self {
            sort: TrashSortMode::DeletedNewest,
            row_height_px: DEFAULT_ROW_HEIGHT,
            viewport_rows: DEFAULT_VIEWPORT_ROWS,
            scroll_row: 0,
            selected: BTreeSet::new(),
            metadata_by_name: BTreeMap::new(),
            empty_trash_enabled: true,
        }
    }
}

impl TrashViewOptions {
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

    pub fn with_metadata(
        mut self,
        metadata: impl IntoIterator<Item = (String, TrashEntryMetadata)>,
    ) -> Self {
        self.metadata_by_name = metadata.into_iter().collect();
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashViewContract {
    pub sort: TrashSortMode,
    pub row_height_px: u16,
    pub viewport_rows: u16,
    pub scroll_row: u32,
    pub total_rows: usize,
    pub visible_start: usize,
    pub visible_end: usize,
    pub empty_trash: TrashCommandSpec,
    pub rows: Vec<TrashRowSpec>,
}

impl TrashViewContract {
    pub fn from_records(records: &[FileRecord], options: TrashViewOptions) -> Self {
        let mut rows = records
            .iter()
            .map(|record| {
                let metadata = options
                    .metadata_by_name
                    .get(&record.name)
                    .cloned()
                    .unwrap_or_default();
                TrashRowSpec::from_record(
                    record,
                    metadata,
                    options.selected.contains(&record.id),
                    options.row_height_px.max(1),
                    0,
                )
            })
            .collect::<Vec<_>>();
        sort_rows(&mut rows, options.sort);
        for (index, row) in rows.iter_mut().enumerate() {
            row.index = index;
            row.y_px = (index as u32).saturating_mul(u32::from(options.row_height_px.max(1)));
        }

        let viewport_rows = options.viewport_rows.max(1);
        let window = VirtualWindow::rows(rows.len(), options.scroll_row, viewport_rows);
        let visible_start = window.start;
        let visible_end = window.end;
        let visible_rows = rows[visible_start..visible_end].to_vec();
        let has_permission_block = rows.iter().any(|row| row.permission_issue.is_some());
        let empty_enabled =
            options.empty_trash_enabled && !rows.is_empty() && !has_permission_block;

        Self {
            sort: options.sort,
            row_height_px: options.row_height_px.max(1),
            viewport_rows,
            scroll_row: options.scroll_row,
            total_rows: rows.len(),
            visible_start,
            visible_end,
            empty_trash: TrashCommandSpec::new(
                "empty-trash",
                "Empty Trash",
                empty_enabled,
                true,
                has_permission_block.then(|| "permission-blocked".to_string()),
            ),
            rows: visible_rows,
        }
    }

    pub fn as_tsv(&self) -> String {
        let mut lines = Vec::with_capacity(self.rows.len() + 2);
        lines.push(format!(
            "trash-view\tsort={}\trow-height={}px\tviewport-rows={}\tscroll-row={}\ttotal={}\tvisible={}..{}",
            self.sort.as_str(),
            self.row_height_px,
            self.viewport_rows,
            self.scroll_row,
            self.total_rows,
            self.visible_start,
            self.visible_end
        ));
        lines.push(self.empty_trash.as_tsv());
        lines.extend(self.rows.iter().map(TrashRowSpec::as_tsv));
        lines.join("\n")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashCommandSpec {
    pub id: String,
    pub title: String,
    pub enabled: bool,
    pub destructive: bool,
    pub disabled_reason: Option<String>,
}

impl TrashCommandSpec {
    fn new(
        id: &str,
        title: &str,
        enabled: bool,
        destructive: bool,
        disabled_reason: Option<String>,
    ) -> Self {
        Self {
            id: id.to_string(),
            title: title.to_string(),
            enabled,
            destructive,
            disabled_reason,
        }
    }

    fn as_tsv(&self) -> String {
        format!(
            "command\t{}\t{}\tenabled={}\tdestructive={}\tdisabled-reason={}",
            escape_field(&self.id),
            escape_field(&self.title),
            self.enabled,
            self.destructive,
            escape_field(&self.disabled_reason.clone().unwrap_or_default())
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashRowSpec {
    pub id: FileId,
    pub index: usize,
    pub y_px: u32,
    pub name: String,
    pub path: PathBuf,
    pub kind: FileKind,
    pub size: u64,
    pub original_path: Option<PathBuf>,
    pub deleted_at: Option<String>,
    pub selected: bool,
    pub restore: TrashCommandSpec,
    pub delete_permanently: TrashCommandSpec,
    pub permission_issue: Option<String>,
}

impl TrashRowSpec {
    fn from_record(
        record: &FileRecord,
        metadata: TrashEntryMetadata,
        selected: bool,
        row_height_px: u16,
        index: usize,
    ) -> Self {
        let restore_enabled = metadata.can_restore && metadata.original_path.is_some();
        let delete_enabled = metadata.can_delete_permanently && metadata.permission_issue.is_none();
        Self {
            id: record.id,
            index,
            y_px: (index as u32).saturating_mul(u32::from(row_height_px)),
            name: record.name.clone(),
            path: record.path.clone(),
            kind: record.kind,
            size: record.len,
            original_path: metadata.original_path,
            deleted_at: metadata.deleted_at,
            selected,
            restore: TrashCommandSpec::new(
                "restore",
                "Put Back",
                restore_enabled,
                false,
                (!restore_enabled).then(|| "missing-restore-location".to_string()),
            ),
            delete_permanently: TrashCommandSpec::new(
                "delete-immediately",
                "Delete Immediately",
                delete_enabled,
                true,
                metadata.permission_issue.clone(),
            ),
            permission_issue: metadata.permission_issue,
        }
    }

    fn as_tsv(&self) -> String {
        format!(
            "row\t{}\t{}\t{}\t{}\t{}px\t{}\t{}\tsize={}\toriginal={}\tdeleted-at={}\tselected={}\trestore={}\tdelete={}\tpermission={}",
            self.index,
            self.id.volume.0,
            self.id.node,
            kind_tsv(self.kind),
            self.y_px,
            escape_field(&self.name),
            escape_path_field(&self.path),
            self.size,
            self.original_path
                .as_ref()
                .map(|path| escape_path_field(path))
                .unwrap_or_default(),
            escape_field(&self.deleted_at.clone().unwrap_or_default()),
            self.selected,
            self.restore.enabled,
            self.delete_permanently.enabled,
            escape_field(&self.permission_issue.clone().unwrap_or_default())
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

pub fn render(contract: &TrashViewContract) -> impl IntoElement {
    let mut view = div()
        .id("gfm-trash-view")
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

fn render_header(contract: &TrashViewContract) -> impl IntoElement {
    div()
        .id("gfm-trash-header")
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .h(px(f32::from(contract.row_height_px)))
        .px(px(8.0))
        .bg(rgb(0x252525))
        .text_xs()
        .child(format!("{} items", contract.total_rows))
        .child(contract.empty_trash.title.clone())
}

fn render_row(contract: &TrashViewContract, row: &TrashRowSpec) -> impl IntoElement {
    div()
        .id(("gfm-trash-row", row.index))
        .flex()
        .flex_row()
        .items_center()
        .h(px(f32::from(contract.row_height_px)))
        .px(px(8.0))
        .bg(if row.selected {
            rgb(0x4a6ea9)
        } else {
            rgb(0x1e1e1e)
        })
        .text_sm()
        .child(div().w(px(220.0)).child(row.name.clone()))
        .child(div().w(px(120.0)).child(kind_tsv(row.kind)))
        .child(
            div().flex_1().child(
                row.original_path
                    .as_ref()
                    .map_or_else(String::new, |path| path.display().to_string()),
            ),
        )
}

fn sort_rows(rows: &mut [TrashRowSpec], sort: TrashSortMode) {
    rows.sort_by(|left, right| match sort {
        TrashSortMode::DeletedNewest => right
            .deleted_at
            .cmp(&left.deleted_at)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase())),
        TrashSortMode::FinderName => left.name.to_lowercase().cmp(&right.name.to_lowercase()),
        TrashSortMode::OriginalLocation => left
            .original_path
            .cmp(&right.original_path)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase())),
    });
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
    use gfm_types::{FileRecord, VolumeId};
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn trash_view_tracks_restore_delete_and_permission_state() {
        let selected = FileId::new(VolumeId(1), 2);
        let contract = TrashViewContract::from_records(
            &[
                record(1, "Note.txt", FileKind::File),
                record(2, "Locked.txt", FileKind::File),
                record(3, "Folder", FileKind::Directory),
            ],
            TrashViewOptions::default()
                .with_selected([selected])
                .with_metadata([
                    (
                        "Note.txt".to_string(),
                        TrashEntryMetadata::restorable("/Users/me/Documents/Note.txt", "200"),
                    ),
                    (
                        "Locked.txt".to_string(),
                        TrashEntryMetadata::blocked("full-disk-access-required"),
                    ),
                    (
                        "Folder".to_string(),
                        TrashEntryMetadata::restorable("/Users/me/Desktop/Folder", "100"),
                    ),
                ]),
        );

        assert_eq!(contract.total_rows, 3);
        assert!(!contract.empty_trash.enabled);
        assert_eq!(
            contract.empty_trash.disabled_reason.as_deref(),
            Some("permission-blocked")
        );
        assert_eq!(contract.rows[0].name, "Note.txt");
        assert!(contract.rows[0].restore.enabled);
        assert!(contract.rows[0].delete_permanently.enabled);
        assert_eq!(contract.rows[1].name, "Folder");
        assert_eq!(contract.rows[2].name, "Locked.txt");
        assert!(contract.rows[2].selected);
        assert!(!contract.rows[2].restore.enabled);
        assert!(!contract.rows[2].delete_permanently.enabled);
    }

    #[test]
    fn trash_view_virtualizes_rows() {
        let records = (0..10)
            .map(|index| record(index, &format!("File {index}.txt"), FileKind::File))
            .collect::<Vec<_>>();
        let contract = TrashViewContract::from_records(
            &records,
            TrashViewOptions::default()
                .with_viewport_rows(3)
                .with_scroll_row(4),
        );

        assert_eq!(contract.total_rows, 10);
        assert_eq!(contract.visible_start, 4);
        assert_eq!(contract.visible_end, 7);
        assert_eq!(contract.rows.len(), 3);
        assert_eq!(contract.rows[0].index, 4);
        assert_eq!(contract.rows[0].y_px, 96);
    }

    #[test]
    fn trash_view_output_is_stable_for_cli_and_fozzy() {
        let contract = TrashViewContract::from_records(
            &[record(1, "Note.txt", FileKind::File)],
            TrashViewOptions::default().with_metadata([(
                "Note.txt".to_string(),
                TrashEntryMetadata::restorable("/Users/me/Documents/Note.txt", "100"),
            )]),
        );
        let tsv = contract.as_tsv();

        assert!(
            tsv.starts_with("trash-view\tsort=deleted-newest\trow-height=24px\tviewport-rows=24")
        );
        assert!(tsv.contains("command\tempty-trash\tEmpty Trash\tenabled=true\tdestructive=true"));
        assert!(tsv.contains("row\t0\t1\t1\tfile\t0px\tNote.txt"));
        assert!(tsv.contains("original=/Users/me/Documents/Note.txt\tdeleted-at=100"));
    }

    #[test]
    fn trash_view_tsv_escapes_control_characters_in_text_fields() {
        let contract = TrashViewContract::from_records(
            &[record(1, "Reports\tQ3\nDraft\rTrash.txt", FileKind::File)],
            TrashViewOptions::default().with_metadata([(
                "Reports\tQ3\nDraft\rTrash.txt".to_string(),
                TrashEntryMetadata {
                    original_path: Some(PathBuf::from(
                        "/Users/me/Documents/Reports\tQ3\nDraft\rTrash.txt",
                    )),
                    deleted_at: Some("100\t200\n300".to_string()),
                    can_restore: true,
                    can_delete_permanently: false,
                    permission_issue: Some("Full Disk Access\trequired\nbefore delete".to_string()),
                },
            )]),
        );
        let tsv = contract.as_tsv();
        let row = tsv.lines().find(|line| line.starts_with("row\t")).unwrap();

        assert_eq!(tsv.lines().count(), 3, "{tsv}");
        assert!(
            row.contains(
                "Reports\\tQ3\\nDraft\\rTrash.txt\t/tmp/.Trash/Reports\\tQ3\\nDraft\\rTrash.txt\t"
            ),
            "{tsv}"
        );
        assert!(
            row.contains("original=/Users/me/Documents/Reports\\tQ3\\nDraft\\rTrash.txt\t"),
            "{tsv}"
        );
        assert!(row.contains("deleted-at=100\\t200\\n300\t"), "{tsv}");
        assert!(
            row.contains("permission=Full Disk Access\\trequired\\nbefore delete"),
            "{tsv}"
        );
        assert_eq!(row.split('\t').count(), 15, "{tsv}");
    }

    fn record(node: u64, name: &str, kind: FileKind) -> FileRecord {
        FileRecord {
            id: FileId::new(VolumeId(1), node),
            parent: None,
            path: PathBuf::from("/tmp/.Trash").join(name),
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
