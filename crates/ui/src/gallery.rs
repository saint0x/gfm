use gfm_types::{FileId, FileKind, FileRecord};
use gpui::{div, prelude::*, px, rgb, IntoElement, Styled};
use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::VirtualWindow;
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_PREVIEW_WIDTH: u16 = 720;
const DEFAULT_PREVIEW_HEIGHT: u16 = 420;
const DEFAULT_FILMSTRIP_ITEM_WIDTH: u16 = 112;
const DEFAULT_FILMSTRIP_ITEM_HEIGHT: u16 = 96;
const DEFAULT_VIEWPORT_ITEMS: u16 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GallerySortMode {
    FinderName,
    ModifiedNewest,
    KindThenName,
}

impl GallerySortMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FinderName => "finder-name",
            Self::ModifiedNewest => "modified-newest",
            Self::KindThenName => "kind-then-name",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GalleryViewOptions {
    pub sort: GallerySortMode,
    pub preview_width_px: u16,
    pub preview_height_px: u16,
    pub filmstrip_item_width_px: u16,
    pub filmstrip_item_height_px: u16,
    pub viewport_items: u16,
    pub scroll_item: u32,
    pub show_hidden: bool,
    pub selected: Option<FileId>,
}

impl Default for GalleryViewOptions {
    fn default() -> Self {
        Self {
            sort: GallerySortMode::FinderName,
            preview_width_px: DEFAULT_PREVIEW_WIDTH,
            preview_height_px: DEFAULT_PREVIEW_HEIGHT,
            filmstrip_item_width_px: DEFAULT_FILMSTRIP_ITEM_WIDTH,
            filmstrip_item_height_px: DEFAULT_FILMSTRIP_ITEM_HEIGHT,
            viewport_items: DEFAULT_VIEWPORT_ITEMS,
            scroll_item: 0,
            show_hidden: false,
            selected: None,
        }
    }
}

impl GalleryViewOptions {
    pub fn with_viewport_items(mut self, items: u16) -> Self {
        self.viewport_items = items.max(1);
        self
    }

    pub fn with_scroll_item(mut self, item: u32) -> Self {
        self.scroll_item = item;
        self
    }

    pub fn with_selected(mut self, selected: Option<FileId>) -> Self {
        self.selected = selected;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GalleryViewContract {
    pub sort: GallerySortMode,
    pub preview_width_px: u16,
    pub preview_height_px: u16,
    pub filmstrip_item_width_px: u16,
    pub filmstrip_item_height_px: u16,
    pub viewport_items: u16,
    pub scroll_item: u32,
    pub total_items: usize,
    pub visible_start: usize,
    pub visible_end: usize,
    pub hidden_filtered: usize,
    pub keyboard_flow: GalleryKeyboardFlow,
    pub preview: Option<GalleryPreviewSpec>,
    pub metadata: Option<GalleryMetadataSpec>,
    pub quick_actions: Vec<GalleryQuickActionSpec>,
    pub filmstrip: Vec<GalleryFilmstripItemSpec>,
}

impl GalleryViewContract {
    pub fn from_records(records: &[FileRecord], options: GalleryViewOptions) -> Self {
        let original_len = records.len();
        let mut records = records
            .iter()
            .filter(|record| options.show_hidden || !record.hidden)
            .cloned()
            .collect::<Vec<_>>();
        let hidden_filtered = original_len.saturating_sub(records.len());
        sort_records(&mut records, options.sort);

        let selected_index = selected_index(&records, options.selected);
        let selected_record = selected_index.and_then(|index| records.get(index));
        let viewport_items = options.viewport_items.max(1);
        let window = VirtualWindow::items(records.len(), options.scroll_item, viewport_items);
        let visible_start = window.start;
        let visible_end = window.end;
        let selected_ids = selected_record
            .map(|record| BTreeSet::from([record.id]))
            .unwrap_or_default();
        let filmstrip = records[visible_start..visible_end]
            .iter()
            .enumerate()
            .map(|(offset, record)| {
                let index = visible_start + offset;
                GalleryFilmstripItemSpec::from_record(
                    record,
                    index,
                    options.filmstrip_item_width_px.max(1),
                    options.filmstrip_item_height_px.max(1),
                    selected_ids.contains(&record.id),
                )
            })
            .collect();
        let preview = selected_record.map(|record| {
            GalleryPreviewSpec::from_record(
                record,
                selected_index.unwrap_or_default(),
                options.preview_width_px.max(1),
                options.preview_height_px.max(1),
            )
        });
        let metadata = selected_record.map(GalleryMetadataSpec::from_record);
        let quick_actions = selected_record
            .map(quick_actions_for_record)
            .unwrap_or_default();

        Self {
            sort: options.sort,
            preview_width_px: options.preview_width_px.max(1),
            preview_height_px: options.preview_height_px.max(1),
            filmstrip_item_width_px: options.filmstrip_item_width_px.max(1),
            filmstrip_item_height_px: options.filmstrip_item_height_px.max(1),
            viewport_items,
            scroll_item: options.scroll_item,
            total_items: records.len(),
            visible_start,
            visible_end,
            hidden_filtered,
            keyboard_flow: GalleryKeyboardFlow::finder_default(),
            preview,
            metadata,
            quick_actions,
            filmstrip,
        }
    }

    pub fn as_tsv(&self) -> String {
        let mut lines = Vec::with_capacity(
            1 + usize::from(self.preview.is_some())
                + usize::from(self.metadata.is_some())
                + self.quick_actions.len()
                + self.filmstrip.len(),
        );
        lines.push(format!(
            "gallery-view\tsort={}\tpreview={}x{}\tfilmstrip-item={}x{}\tviewport-items={}\tscroll-item={}\ttotal={}\tvisible={}..{}\thidden-filtered={}\tkeyboard={}",
            self.sort.as_str(),
            self.preview_width_px,
            self.preview_height_px,
            self.filmstrip_item_width_px,
            self.filmstrip_item_height_px,
            self.viewport_items,
            self.scroll_item,
            self.total_items,
            self.visible_start,
            self.visible_end,
            self.hidden_filtered,
            self.keyboard_flow.as_str()
        ));
        if let Some(preview) = &self.preview {
            lines.push(preview.as_tsv());
        }
        if let Some(metadata) = &self.metadata {
            lines.push(metadata.as_tsv());
        }
        lines.extend(
            self.quick_actions
                .iter()
                .map(GalleryQuickActionSpec::as_tsv),
        );
        lines.extend(self.filmstrip.iter().map(GalleryFilmstripItemSpec::as_tsv));
        lines.join("\n")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GalleryPreviewSpec {
    pub id: FileId,
    pub index: usize,
    pub name: String,
    pub path: PathBuf,
    pub kind: FileKind,
    pub role: GalleryPreviewRole,
    pub width_px: u16,
    pub height_px: u16,
}

impl GalleryPreviewSpec {
    fn from_record(record: &FileRecord, index: usize, width_px: u16, height_px: u16) -> Self {
        Self {
            id: record.id,
            index,
            name: record.name.clone(),
            path: record.path.clone(),
            kind: record.kind,
            role: GalleryPreviewRole::for_record(record),
            width_px,
            height_px,
        }
    }

    fn as_tsv(&self) -> String {
        format!(
            "preview\t{}\t{}\t{}\t{}\t{}x{}\t{}\t{}",
            self.index,
            self.id.volume.0,
            self.id.node,
            kind_tsv(self.kind),
            self.width_px,
            self.height_px,
            self.role.as_str(),
            escape_field(&self.name)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GalleryPreviewRole {
    FolderSummary,
    ImagePreview,
    PdfPreview,
    DocumentPreview,
    GenericPreview,
}

impl GalleryPreviewRole {
    fn for_record(record: &FileRecord) -> Self {
        if record.kind == FileKind::Directory {
            Self::FolderSummary
        } else {
            match record
                .extension()
                .unwrap_or_default()
                .to_ascii_lowercase()
                .as_str()
            {
                "jpg" | "jpeg" | "png" | "gif" | "heic" | "tiff" | "webp" => Self::ImagePreview,
                "pdf" => Self::PdfPreview,
                "doc" | "docx" | "pages" | "rtf" | "txt" | "md" => Self::DocumentPreview,
                _ => Self::GenericPreview,
            }
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FolderSummary => "folder-summary",
            Self::ImagePreview => "image-preview",
            Self::PdfPreview => "pdf-preview",
            Self::DocumentPreview => "document-preview",
            Self::GenericPreview => "generic-preview",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GalleryMetadataSpec {
    pub id: FileId,
    pub name: String,
    pub kind: FileKind,
    pub size: u64,
    pub modified: String,
    pub tags: Vec<String>,
}

impl GalleryMetadataSpec {
    fn from_record(record: &FileRecord) -> Self {
        Self {
            id: record.id,
            name: record.name.clone(),
            kind: record.kind,
            size: record.len,
            modified: format_time(record.modified),
            tags: record.tags.clone(),
        }
    }

    fn as_tsv(&self) -> String {
        format!(
            "metadata\t{}\t{}\t{}\tsize={}\tmodified={}\ttags={}",
            self.id.volume.0,
            self.id.node,
            kind_tsv(self.kind),
            self.size,
            self.modified,
            escape_field(&self.tags.join(","))
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GalleryQuickActionSpec {
    pub id: String,
    pub title: String,
    pub enabled: bool,
}

impl GalleryQuickActionSpec {
    fn new(id: &str, title: &str, enabled: bool) -> Self {
        Self {
            id: id.to_string(),
            title: title.to_string(),
            enabled,
        }
    }

    fn as_tsv(&self) -> String {
        format!(
            "quick-action\t{}\t{}\tenabled={}",
            escape_field(&self.id),
            escape_field(&self.title),
            self.enabled
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GalleryFilmstripItemSpec {
    pub id: FileId,
    pub index: usize,
    pub x_px: u32,
    pub name: String,
    pub path: PathBuf,
    pub kind: FileKind,
    pub role: GalleryPreviewRole,
    pub selected: bool,
    pub width_px: u16,
    pub height_px: u16,
}

impl GalleryFilmstripItemSpec {
    fn from_record(
        record: &FileRecord,
        index: usize,
        width_px: u16,
        height_px: u16,
        selected: bool,
    ) -> Self {
        Self {
            id: record.id,
            index,
            x_px: (index as u32).saturating_mul(u32::from(width_px)),
            name: record.name.clone(),
            path: record.path.clone(),
            kind: record.kind,
            role: GalleryPreviewRole::for_record(record),
            selected,
            width_px,
            height_px,
        }
    }

    fn as_tsv(&self) -> String {
        format!(
            "filmstrip\t{}\t{}\t{}\t{}\t{}px\t{}x{}\t{}\t{}\tselected={}",
            self.index,
            self.id.volume.0,
            self.id.node,
            kind_tsv(self.kind),
            self.x_px,
            self.width_px,
            self.height_px,
            self.role.as_str(),
            escape_field(&self.name),
            self.selected
        )
    }
}

fn escape_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GalleryKeyboardFlow {
    FinderLeftRightFilmstripNavigation,
}

impl GalleryKeyboardFlow {
    const fn finder_default() -> Self {
        Self::FinderLeftRightFilmstripNavigation
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FinderLeftRightFilmstripNavigation => "finder-left-right-filmstrip-navigation",
        }
    }
}

pub fn render(contract: &GalleryViewContract) -> impl IntoElement {
    div()
        .id("gfm-gallery-view")
        .flex()
        .flex_col()
        .bg(rgb(0x1e1e1e))
        .text_color(rgb(0xd8d8d8))
        .child(render_preview_area(contract))
        .child(render_filmstrip(contract))
}

fn render_preview_area(contract: &GalleryViewContract) -> impl IntoElement {
    let mut area = div().flex().flex_row().flex_1().min_h(px(0.0));
    if let Some(preview) = &contract.preview {
        area = area.child(
            div()
                .id("gfm-gallery-preview")
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .w(px(f32::from(preview.width_px)))
                .h(px(f32::from(preview.height_px)))
                .child(
                    div()
                        .w(px(168.0))
                        .h(px(168.0))
                        .rounded(px(8.0))
                        .bg(preview_color(preview.role)),
                )
                .child(div().mt(px(12.0)).text_lg().child(preview.name.clone())),
        );
    }
    if let Some(metadata) = &contract.metadata {
        area = area.child(render_metadata(metadata, &contract.quick_actions));
    }
    area
}

fn render_metadata(
    metadata: &GalleryMetadataSpec,
    actions: &[GalleryQuickActionSpec],
) -> impl IntoElement {
    let mut panel = div()
        .id("gfm-gallery-metadata")
        .flex()
        .flex_col()
        .w(px(240.0))
        .p(px(14.0))
        .bg(rgb(0x202020))
        .text_sm()
        .child(metadata.name.clone())
        .child(format!("size {}", metadata.size))
        .child(format!("modified {}", metadata.modified));

    for action in actions {
        panel = panel.child(
            div()
                .mt(px(8.0))
                .px(px(8.0))
                .py(px(4.0))
                .rounded(px(5.0))
                .bg(if action.enabled {
                    rgb(0x303030)
                } else {
                    rgb(0x252525)
                })
                .child(action.title.clone()),
        );
    }
    panel
}

fn render_filmstrip(contract: &GalleryViewContract) -> impl IntoElement {
    let mut strip = div()
        .id("gfm-gallery-filmstrip")
        .flex()
        .flex_row()
        .h(px(f32::from(contract.filmstrip_item_height_px) + 24.0))
        .bg(rgb(0x1a1a1a));

    for item in &contract.filmstrip {
        strip = strip.child(render_filmstrip_item(item));
    }
    strip
}

fn render_filmstrip_item(item: &GalleryFilmstripItemSpec) -> impl IntoElement {
    div()
        .id(("gfm-gallery-filmstrip-item", item.index))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .w(px(f32::from(item.width_px)))
        .h(px(f32::from(item.height_px) + 24.0))
        .bg(if item.selected {
            rgb(0x4a6ea9)
        } else {
            rgb(0x1a1a1a)
        })
        .child(
            div()
                .w(px(54.0))
                .h(px(54.0))
                .rounded(px(5.0))
                .bg(preview_color(item.role)),
        )
        .child(div().mt(px(4.0)).text_xs().child(item.name.clone()))
}

fn quick_actions_for_record(record: &FileRecord) -> Vec<GalleryQuickActionSpec> {
    match GalleryPreviewRole::for_record(record) {
        GalleryPreviewRole::ImagePreview => vec![
            GalleryQuickActionSpec::new("rotate-left", "Rotate Left", true),
            GalleryQuickActionSpec::new("markup", "Markup", true),
        ],
        GalleryPreviewRole::PdfPreview | GalleryPreviewRole::DocumentPreview => vec![
            GalleryQuickActionSpec::new("markup", "Markup", true),
            GalleryQuickActionSpec::new("create-pdf", "Create PDF", true),
        ],
        GalleryPreviewRole::FolderSummary => vec![GalleryQuickActionSpec::new(
            "open-folder",
            "Open Folder",
            true,
        )],
        GalleryPreviewRole::GenericPreview => {
            vec![GalleryQuickActionSpec::new(
                "quick-look",
                "Quick Look",
                true,
            )]
        }
    }
}

fn selected_index(records: &[FileRecord], selected: Option<FileId>) -> Option<usize> {
    selected
        .and_then(|id| records.iter().position(|record| record.id == id))
        .or_else(|| (!records.is_empty()).then_some(0))
}

fn sort_records(records: &mut [FileRecord], sort: GallerySortMode) {
    records.sort_by(|left, right| match sort {
        GallerySortMode::FinderName => finder_name_key(left).cmp(&finder_name_key(right)),
        GallerySortMode::KindThenName => (kind_name(left), finder_name_key(left))
            .cmp(&(kind_name(right), finder_name_key(right))),
        GallerySortMode::ModifiedNewest => right
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

fn format_time(time: Option<SystemTime>) -> String {
    time.and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn preview_color(role: GalleryPreviewRole) -> gpui::Rgba {
    match role {
        GalleryPreviewRole::FolderSummary => rgb(0x39a7dc),
        GalleryPreviewRole::ImagePreview => rgb(0x4fa3ff),
        GalleryPreviewRole::PdfPreview => rgb(0xff6b6b),
        GalleryPreviewRole::DocumentPreview => rgb(0xf2f2f2),
        GalleryPreviewRole::GenericPreview => rgb(0xa0a0a0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn gallery_view_selects_preview_metadata_actions_and_filmstrip() {
        let selected = FileId::new(gfm_types::VolumeId(1), 3);
        let contract = GalleryViewContract::from_records(
            &[
                record(1, "Folder", FileKind::Directory),
                record(2, "Note.txt", FileKind::File),
                tagged(record(3, "Image.png", FileKind::File)),
                hidden(record(4, ".hidden", FileKind::File)),
            ],
            GalleryViewOptions::default()
                .with_viewport_items(4)
                .with_selected(Some(selected)),
        );

        assert_eq!(contract.total_items, 3);
        assert_eq!(contract.hidden_filtered, 1);
        assert_eq!(
            contract.preview.as_ref().map(|preview| preview.role),
            Some(GalleryPreviewRole::ImagePreview)
        );
        assert_eq!(
            contract
                .metadata
                .as_ref()
                .map(|metadata| metadata.tags.clone()),
            Some(vec!["Red".to_string()])
        );
        assert_eq!(contract.quick_actions[0].id, "rotate-left");
        assert!(contract
            .filmstrip
            .iter()
            .any(|item| item.name == "Image.png" && item.selected));
    }

    #[test]
    fn gallery_view_virtualizes_filmstrip_by_scroll_item() {
        let records = (0..10)
            .map(|index| record(index, &format!("File {index}.txt"), FileKind::File))
            .collect::<Vec<_>>();
        let contract = GalleryViewContract::from_records(
            &records,
            GalleryViewOptions::default()
                .with_viewport_items(3)
                .with_scroll_item(4),
        );

        assert_eq!(contract.total_items, 10);
        assert_eq!(contract.visible_start, 4);
        assert_eq!(contract.visible_end, 7);
        assert_eq!(contract.filmstrip.len(), 3);
        assert_eq!(contract.filmstrip[0].index, 4);
        assert_eq!(contract.filmstrip[0].x_px, 448);
    }

    #[test]
    fn gallery_view_output_is_stable_for_cli_and_fozzy() {
        let selected = FileId::new(gfm_types::VolumeId(1), 2);
        let contract = GalleryViewContract::from_records(
            &[
                record(1, "Folder", FileKind::Directory),
                record(2, "Note.pdf", FileKind::File),
            ],
            GalleryViewOptions::default()
                .with_viewport_items(2)
                .with_selected(Some(selected)),
        );
        let tsv = contract.as_tsv();

        assert!(tsv
            .starts_with("gallery-view\tsort=finder-name\tpreview=720x420\tfilmstrip-item=112x96"));
        assert!(tsv.contains("preview\t1\t1\t2\tfile\t720x420\tpdf-preview\tNote.pdf"));
        assert!(tsv.contains("quick-action\tmarkup\tMarkup\tenabled=true"));
        assert!(tsv.contains("filmstrip\t0\t1\t1\tdir\t0px\t112x96\tfolder-summary\tFolder"));
        assert!(tsv.contains(
            "filmstrip\t1\t1\t2\tfile\t112px\t112x96\tpdf-preview\tNote.pdf\tselected=true"
        ));
    }

    #[test]
    fn gallery_view_tsv_escapes_control_characters_in_text_fields() {
        let selected = FileId::new(gfm_types::VolumeId(1), 1);
        let contract = GalleryViewContract::from_records(
            &[tagged_control(record(
                1,
                "Reports\tQ3\nDraft\rGallery.pdf",
                FileKind::File,
            ))],
            GalleryViewOptions::default()
                .with_viewport_items(1)
                .with_selected(Some(selected)),
        );
        let tsv = contract.as_tsv();
        let mut lines = tsv.lines();
        let _header = lines.next().unwrap();
        let preview = lines.next().unwrap();
        let metadata = lines.next().unwrap();
        let first_action = lines.next().unwrap();
        let second_action = lines.next().unwrap();
        let filmstrip = lines.next().unwrap();

        assert_eq!(tsv.lines().count(), 6, "{tsv}");
        assert!(
            preview.contains("\tpdf-preview\tReports\\tQ3\\nDraft\\rGallery.pdf"),
            "{tsv}"
        );
        assert!(metadata.contains("\ttags=Red\\tTag\\n"), "{tsv}");
        assert_eq!(first_action.split('\t').count(), 4, "{tsv}");
        assert_eq!(second_action.split('\t').count(), 4, "{tsv}");
        assert!(
            filmstrip.contains("\tpdf-preview\tReports\\tQ3\\nDraft\\rGallery.pdf\tselected=true"),
            "{tsv}"
        );
        assert_eq!(preview.split('\t').count(), 8, "{tsv}");
        assert_eq!(metadata.split('\t').count(), 7, "{tsv}");
        assert_eq!(filmstrip.split('\t').count(), 10, "{tsv}");
    }

    fn hidden(mut record: FileRecord) -> FileRecord {
        record.hidden = true;
        record
    }

    fn tagged(mut record: FileRecord) -> FileRecord {
        record.tags = vec!["Red".to_string()];
        record
    }

    fn tagged_control(mut record: FileRecord) -> FileRecord {
        record.tags = vec!["Red\tTag\n".to_string()];
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
