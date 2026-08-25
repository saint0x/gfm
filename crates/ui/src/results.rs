use gfm_types::{FileId, FileKind, MatchReason, SearchHit};
use gpui::{div, prelude::*, px, rgb, IntoElement, Styled};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

const DEFAULT_ROW_HEIGHT: u16 = 24;
const DEFAULT_VIEWPORT_ROWS: u16 = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchResultsStage {
    Hot,
    Deep,
    Settled,
}

impl SearchResultsStage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hot => "hot",
            Self::Deep => "deep",
            Self::Settled => "settled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchResultsScope {
    ThisMac,
    CurrentFolder,
    Recents,
}

impl SearchResultsScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ThisMac => "this-mac",
            Self::CurrentFolder => "current-folder",
            Self::Recents => "recents",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchResultsGrouping {
    None,
    Kind,
    Parent,
    Reason,
}

impl SearchResultsGrouping {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Kind => "kind",
            Self::Parent => "parent",
            Self::Reason => "reason",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResultsOptions {
    pub query: String,
    pub scope: SearchResultsScope,
    pub grouping: SearchResultsGrouping,
    pub row_height_px: u16,
    pub viewport_rows: u16,
    pub scroll_row: u32,
    pub progressive: bool,
    pub selected: BTreeSet<FileId>,
}

impl SearchResultsOptions {
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            ..Self::default()
        }
    }

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
}

impl Default for SearchResultsOptions {
    fn default() -> Self {
        Self {
            query: String::new(),
            scope: SearchResultsScope::ThisMac,
            grouping: SearchResultsGrouping::Kind,
            row_height_px: DEFAULT_ROW_HEIGHT,
            viewport_rows: DEFAULT_VIEWPORT_ROWS,
            scroll_row: 0,
            progressive: true,
            selected: BTreeSet::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResultsBatch {
    pub stage: SearchResultsStage,
    pub hits: Vec<SearchHit>,
}

impl SearchResultsBatch {
    pub fn new(stage: SearchResultsStage, hits: Vec<SearchHit>) -> Self {
        Self { stage, hits }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResultsContract {
    pub query: String,
    pub scope: SearchResultsScope,
    pub grouping: SearchResultsGrouping,
    pub row_height_px: u16,
    pub viewport_rows: u16,
    pub scroll_row: u32,
    pub progressive: bool,
    pub stage: SearchResultsStage,
    pub total_rows: usize,
    pub visible_start: usize,
    pub visible_end: usize,
    pub groups: Vec<SearchResultsGroupSpec>,
    pub rows: Vec<SearchResultRowSpec>,
}

impl SearchResultsContract {
    pub fn from_batches(batches: Vec<SearchResultsBatch>, options: SearchResultsOptions) -> Self {
        let stage = batches
            .last()
            .map(|batch| batch.stage)
            .unwrap_or(SearchResultsStage::Settled);
        let mut merged = BTreeMap::<FileId, SearchHit>::new();
        let mut stages = BTreeMap::<FileId, SearchResultsStage>::new();
        for batch in batches {
            for hit in batch.hits {
                stages
                    .entry(hit.record.id)
                    .and_modify(|existing| {
                        if *existing != batch.stage {
                            *existing = SearchResultsStage::Settled;
                        }
                    })
                    .or_insert(batch.stage);
                merged
                    .entry(hit.record.id)
                    .and_modify(|existing| {
                        if hit.score > existing.score {
                            *existing = hit.clone();
                        }
                    })
                    .or_insert(hit);
            }
        }

        let mut hits = merged.into_values().collect::<Vec<_>>();
        hits.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| {
                    left.record
                        .name
                        .to_lowercase()
                        .cmp(&right.record.name.to_lowercase())
                })
                .then_with(|| left.record.path.cmp(&right.record.path))
        });

        let visible_start = (options.scroll_row as usize).min(hits.len());
        let visible_end = visible_start
            .saturating_add(usize::from(options.viewport_rows.max(1)))
            .min(hits.len());
        let rows = hits[visible_start..visible_end]
            .iter()
            .enumerate()
            .map(|(offset, hit)| {
                let index = visible_start + offset;
                SearchResultRowSpec::from_hit(
                    hit,
                    index,
                    options.row_height_px.max(1),
                    *stages
                        .get(&hit.record.id)
                        .unwrap_or(&SearchResultsStage::Settled),
                    options.selected.contains(&hit.record.id),
                    options.grouping,
                )
            })
            .collect::<Vec<_>>();
        let groups = group_rows(&rows, options.grouping);

        Self {
            query: options.query,
            scope: options.scope,
            grouping: options.grouping,
            row_height_px: options.row_height_px.max(1),
            viewport_rows: options.viewport_rows.max(1),
            scroll_row: options.scroll_row,
            progressive: options.progressive,
            stage,
            total_rows: hits.len(),
            visible_start,
            visible_end,
            groups,
            rows,
        }
    }

    pub fn as_tsv(&self) -> String {
        let mut lines = Vec::with_capacity(1 + self.groups.len() + self.rows.len());
        lines.push(format!(
            "search-results\tquery={}\tscope={}\tgrouping={}\trow-height={}px\tviewport-rows={}\tscroll-row={}\tstage={}\tprogressive={}\ttotal={}\tvisible={}..{}",
            self.query,
            self.scope.as_str(),
            self.grouping.as_str(),
            self.row_height_px,
            self.viewport_rows,
            self.scroll_row,
            self.stage.as_str(),
            self.progressive,
            self.total_rows,
            self.visible_start,
            self.visible_end
        ));
        lines.extend(self.groups.iter().map(SearchResultsGroupSpec::as_tsv));
        lines.extend(self.rows.iter().map(SearchResultRowSpec::as_tsv));
        lines.join("\n")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResultsGroupSpec {
    pub key: String,
    pub title: String,
    pub count: usize,
    pub first_row: usize,
}

impl SearchResultsGroupSpec {
    fn as_tsv(&self) -> String {
        format!(
            "group\t{}\t{}\tcount={}\tfirst-row={}",
            self.key, self.title, self.count, self.first_row
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResultRowSpec {
    pub id: FileId,
    pub index: usize,
    pub y_px: u32,
    pub name: String,
    pub path: PathBuf,
    pub parent: String,
    pub kind: FileKind,
    pub size: u64,
    pub score: i64,
    pub reason: MatchReason,
    pub stage: SearchResultsStage,
    pub group_key: String,
    pub selected: bool,
    pub snippet: Option<String>,
}

impl SearchResultRowSpec {
    fn from_hit(
        hit: &SearchHit,
        index: usize,
        row_height_px: u16,
        stage: SearchResultsStage,
        selected: bool,
        grouping: SearchResultsGrouping,
    ) -> Self {
        Self {
            id: hit.record.id,
            index,
            y_px: (index as u32).saturating_mul(u32::from(row_height_px)),
            name: hit.record.name.clone(),
            path: hit.record.path.clone(),
            parent: hit
                .record
                .path
                .parent()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            kind: hit.record.kind,
            size: hit.record.len,
            score: hit.score,
            reason: hit.reason.clone(),
            stage,
            group_key: group_key(hit, grouping),
            selected,
            snippet: hit.snippet.as_ref().map(|snippet| snippet.text.clone()),
        }
    }

    fn as_tsv(&self) -> String {
        format!(
            "row\t{}\t{}\t{}\t{}\t{}px\t{}\t{}\tparent={}\tsize={}\tscore={}\treason={}\tstage={}\tgroup={}\tselected={}\tsnippet={}",
            self.index,
            self.id.volume.0,
            self.id.node,
            kind_tsv(self.kind),
            self.y_px,
            self.name,
            self.path.display(),
            self.parent,
            self.size,
            self.score,
            reason_tsv(&self.reason),
            self.stage.as_str(),
            self.group_key,
            self.selected,
            self.snippet.clone().unwrap_or_default()
        )
    }
}

pub fn render(contract: &SearchResultsContract) -> impl IntoElement {
    let mut view = div()
        .id("gfm-search-results-view")
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

fn render_header(contract: &SearchResultsContract) -> impl IntoElement {
    div()
        .id("gfm-search-results-header")
        .flex()
        .flex_row()
        .items_center()
        .h(px(f32::from(contract.row_height_px)))
        .px(px(8.0))
        .bg(rgb(0x252525))
        .text_xs()
        .child(format!(
            "{} results in {}",
            contract.total_rows,
            contract.scope.as_str()
        ))
}

fn render_row(contract: &SearchResultsContract, row: &SearchResultRowSpec) -> impl IntoElement {
    div()
        .id(("gfm-search-result-row", row.index))
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
        .child(div().w(px(110.0)).child(kind_tsv(row.kind)))
        .child(div().w(px(72.0)).child(row.score.to_string()))
        .child(div().flex_1().child(row.parent.clone()))
}

fn group_rows(
    rows: &[SearchResultRowSpec],
    grouping: SearchResultsGrouping,
) -> Vec<SearchResultsGroupSpec> {
    if grouping == SearchResultsGrouping::None {
        return Vec::new();
    }
    let mut groups = BTreeMap::<String, SearchResultsGroupSpec>::new();
    for row in rows {
        groups
            .entry(row.group_key.clone())
            .and_modify(|group| group.count += 1)
            .or_insert_with(|| SearchResultsGroupSpec {
                key: row.group_key.clone(),
                title: group_title(&row.group_key, grouping),
                count: 1,
                first_row: row.index,
            });
    }
    groups.into_values().collect()
}

fn group_key(hit: &SearchHit, grouping: SearchResultsGrouping) -> String {
    match grouping {
        SearchResultsGrouping::None => "all".to_string(),
        SearchResultsGrouping::Kind => kind_tsv(hit.record.kind).to_string(),
        SearchResultsGrouping::Parent => hit
            .record
            .path
            .parent()
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
        SearchResultsGrouping::Reason => reason_tsv(&hit.reason).to_string(),
    }
}

fn group_title(key: &str, grouping: SearchResultsGrouping) -> String {
    match grouping {
        SearchResultsGrouping::None => "All".to_string(),
        SearchResultsGrouping::Kind => match key {
            "dir" => "Folders".to_string(),
            "file" => "Documents".to_string(),
            "link" => "Aliases".to_string(),
            _ => "Other".to_string(),
        },
        SearchResultsGrouping::Parent => key.to_string(),
        SearchResultsGrouping::Reason => key.to_string(),
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

fn reason_tsv(reason: &MatchReason) -> &'static str {
    match reason {
        MatchReason::ExactName => "exact-name",
        MatchReason::PrefixName => "prefix-name",
        MatchReason::SubstringName => "substring-name",
        MatchReason::Extension => "extension",
        MatchReason::PathComponent => "path-component",
        MatchReason::Tag => "tag",
        MatchReason::Content => "content",
        MatchReason::FuzzyName => "fuzzy-name",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfm_types::{FileRecord, SearchSnippet, SnippetHighlight};
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn search_results_merge_progressive_batches_and_group_visible_rows() {
        let selected = FileId::new(gfm_types::VolumeId(1), 3);
        let contract = SearchResultsContract::from_batches(
            vec![
                SearchResultsBatch::new(
                    SearchResultsStage::Hot,
                    vec![hit(
                        1,
                        "Folder",
                        FileKind::Directory,
                        70,
                        MatchReason::PrefixName,
                    )],
                ),
                SearchResultsBatch::new(
                    SearchResultsStage::Deep,
                    vec![
                        hit(2, "Note.txt", FileKind::File, 90, MatchReason::Content),
                        hit(3, "Project.md", FileKind::File, 80, MatchReason::Content),
                    ],
                ),
            ],
            SearchResultsOptions::new("project")
                .with_viewport_rows(3)
                .with_selected([selected]),
        );

        assert_eq!(contract.stage, SearchResultsStage::Deep);
        assert_eq!(contract.total_rows, 3);
        assert_eq!(contract.rows[0].name, "Note.txt");
        assert_eq!(contract.rows[0].stage, SearchResultsStage::Deep);
        assert_eq!(contract.rows[1].name, "Project.md");
        assert!(contract.rows[1].selected);
        assert_eq!(contract.groups.len(), 2);
        assert!(contract
            .groups
            .iter()
            .any(|group| group.key == "file" && group.count == 2));
    }

    #[test]
    fn search_results_virtualize_by_scroll_row() {
        let hits = (0..10)
            .map(|index| {
                hit(
                    index,
                    &format!("File {index}.txt"),
                    FileKind::File,
                    100 - index as i64,
                    MatchReason::SubstringName,
                )
            })
            .collect();
        let contract = SearchResultsContract::from_batches(
            vec![SearchResultsBatch::new(SearchResultsStage::Hot, hits)],
            SearchResultsOptions::new("file")
                .with_viewport_rows(3)
                .with_scroll_row(4),
        );

        assert_eq!(contract.total_rows, 10);
        assert_eq!(contract.visible_start, 4);
        assert_eq!(contract.visible_end, 7);
        assert_eq!(contract.rows[0].index, 4);
        assert_eq!(contract.rows[0].y_px, 96);
    }

    #[test]
    fn search_results_output_is_stable_for_cli_and_fozzy() {
        let contract = SearchResultsContract::from_batches(
            vec![SearchResultsBatch::new(
                SearchResultsStage::Hot,
                vec![hit(
                    1,
                    "PLAN.md",
                    FileKind::File,
                    88,
                    MatchReason::ExactName,
                )],
            )],
            SearchResultsOptions::new("PLAN").with_viewport_rows(1),
        );
        let tsv = contract.as_tsv();

        assert!(tsv.starts_with(
            "search-results\tquery=PLAN\tscope=this-mac\tgrouping=kind\trow-height=24px"
        ));
        assert!(tsv.contains("group\tfile\tDocuments\tcount=1\tfirst-row=0"));
        assert!(tsv.contains("row\t0\t1\t1\tfile\t0px\tPLAN.md"));
        assert!(tsv.contains("score=88\treason=exact-name\tstage=hot"));
    }

    fn hit(node: u64, name: &str, kind: FileKind, score: i64, reason: MatchReason) -> SearchHit {
        SearchHit {
            record: record(node, name, kind),
            score,
            reason,
            snippet: Some(SearchSnippet {
                text: format!("snippet for {name}"),
                highlights: vec![SnippetHighlight { start: 0, end: 7 }],
            }),
        }
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
