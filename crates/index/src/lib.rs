use gfm_content::Extractor;
use gfm_fs::{scan_tree, ScanOptions};
use gfm_jobs::Cancellation;
pub use gfm_search::SearchStreamStage;
use gfm_search::{SearchQuery, SearchStreamBatch, ShardedSearchIndex};
use gfm_store::{
    compact_content_segments, read_content_postings, read_records, write_content_postings,
    write_content_segment, write_records,
};
use gfm_types::{
    ContentSegment, DirectoryPage, FileEvent, FileEventKind, FileId, FileRecord, GfmError, Result,
    ScanIssue, SearchHit,
};
use std::fs;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

mod state;

pub use state::{IndexVolumeState, INDEX_STATE_SCHEMA_VERSION};

#[derive(Debug, Clone)]
pub struct IndexSnapshot {
    pub root: PathBuf,
    pub records: Vec<FileRecord>,
    pub inaccessible: Vec<ScanIssue>,
}

impl IndexSnapshot {
    pub fn from_page(page: DirectoryPage) -> Self {
        Self {
            root: page.root,
            records: page.entries,
            inaccessible: page.inaccessible,
        }
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        let mut index = ShardedSearchIndex::new();
        for record in self.records.iter().cloned() {
            index.insert(record);
        }
        index.query(query, limit)
    }

    pub fn stream_search(&self, query: &str, limit: usize) -> Result<Vec<SearchStreamBatch>> {
        let mut index = ShardedSearchIndex::new();
        for record in self.records.iter().cloned() {
            index.insert(record);
        }
        index.stream(query, limit)
    }

    pub fn search_cancellable(
        &self,
        query: &str,
        limit: usize,
        cancellation: &Cancellation,
    ) -> Result<Vec<SearchHit>> {
        let mut index = ShardedSearchIndex::new();
        for record in self.records.iter().cloned() {
            cancellation.check()?;
            index.insert(record);
        }
        index.query_cancellable(query, limit, cancellation)
    }

    pub fn search_with_content(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let mut live = self.clone().into_live();
        live.index_content(&Extractor::default())?;
        Ok(live.search(query, limit))
    }

    pub fn search_with_content_snippets(
        &self,
        query: &str,
        limit: usize,
        extractor: &Extractor,
        context_bytes: usize,
    ) -> Result<Vec<SearchHit>> {
        let mut live = self.clone().into_live();
        live.index_content(extractor)?;
        live.search_with_snippets(query, limit, extractor, context_bytes)
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        write_records(path, &self.records)
    }

    pub fn volume_state(
        &self,
        records_path: impl Into<PathBuf>,
        previous: Option<&IndexVolumeState>,
    ) -> Result<IndexVolumeState> {
        IndexVolumeState::from_page(
            &DirectoryPage {
                root: self.root.clone(),
                entries: self.records.clone(),
                inaccessible: self.inaccessible.clone(),
            },
            records_path,
            previous,
        )
    }

    pub fn save_with_content(
        &self,
        records_path: impl AsRef<Path>,
        content_path: impl AsRef<Path>,
        extractor: &Extractor,
    ) -> Result<usize> {
        self.save(records_path)?;
        let mut live = self.clone().into_live();
        let indexed = live.index_content(extractor)?;
        live.save_content_postings(content_path)?;
        Ok(indexed)
    }

    pub fn save_content_segment(
        &self,
        segment_path: impl AsRef<Path>,
        extractor: &Extractor,
        tombstones: Vec<FileId>,
    ) -> Result<usize> {
        let mut live = self.clone().into_live();
        let indexed = live.index_content(extractor)?;
        let segment = ContentSegment {
            tombstones,
            postings: live.content_postings(),
        };
        write_content_segment(segment_path, &segment)?;
        Ok(indexed)
    }

    pub fn into_live(self) -> LiveIndex {
        LiveIndex::from_records(self.records)
    }
}

#[derive(Debug, Clone, Default)]
pub struct LiveIndex {
    index: ShardedSearchIndex,
}

impl LiveIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_records(records: Vec<FileRecord>) -> Self {
        let mut live = Self::new();
        for record in records {
            live.index.insert(record);
        }
        live
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        self.index.query(query, limit)
    }

    pub fn stream_search(&self, query: &str, limit: usize) -> Result<Vec<SearchStreamBatch>> {
        self.index.stream(query, limit)
    }

    pub fn search_cancellable(
        &self,
        query: &str,
        limit: usize,
        cancellation: &Cancellation,
    ) -> Result<Vec<SearchHit>> {
        self.index.query_cancellable(query, limit, cancellation)
    }

    pub fn search_with_snippets(
        &self,
        query: &str,
        limit: usize,
        extractor: &Extractor,
        context_bytes: usize,
    ) -> Result<Vec<SearchHit>> {
        let parsed = SearchQuery::parse(query);
        let mut hits = self.index.query_structured(&parsed, limit);
        for hit in &mut hits {
            if matches!(hit.reason, gfm_types::MatchReason::Content) {
                hit.snippet = extractor.snippet_for_record(
                    &hit.record,
                    &parsed.terms,
                    &parsed.phrases,
                    context_bytes,
                )?;
            }
        }
        Ok(hits)
    }

    pub fn index_content(&mut self, extractor: &Extractor) -> Result<usize> {
        let records: Vec<_> = self.index.records().cloned().collect();
        let mut indexed = 0;
        for record in records {
            if let Some(document) = extractor.extract_record(&record)? {
                self.index.insert_content(record.id, &document.text);
                indexed += 1;
            }
        }
        Ok(indexed)
    }

    pub fn save_content_postings(&self, path: impl AsRef<Path>) -> Result<()> {
        write_content_postings(path, &self.index.content_postings())
    }

    pub fn content_postings(&self) -> Vec<gfm_types::ContentPosting> {
        self.index.content_postings()
    }

    pub fn load_content_postings(&mut self, path: impl AsRef<Path>) -> Result<usize> {
        let postings = read_content_postings(path)?;
        let terms = postings.len();
        self.index.import_content_postings(&postings);
        Ok(terms)
    }

    pub fn apply_event(&mut self, event: &FileEvent) -> Result<UpdateOutcome> {
        match &event.kind {
            FileEventKind::Create | FileEventKind::Modify | FileEventKind::Other => {
                self.upsert_path(&event.path)
            }
            FileEventKind::Remove => {
                let removed = self.index.remove_subtree(&event.path).len();
                Ok(UpdateOutcome::Removed { records: removed })
            }
            FileEventKind::Rename { from, to } => {
                let removed = self.index.remove_subtree(from).len();
                match self.upsert_path(to) {
                    Ok(UpdateOutcome::Upserted) => Ok(UpdateOutcome::Renamed {
                        removed,
                        inserted: 1,
                    }),
                    Ok(other) => Ok(other),
                    Err(err) => {
                        if removed > 0 {
                            Ok(UpdateOutcome::Renamed {
                                removed,
                                inserted: 0,
                            })
                        } else {
                            Err(err)
                        }
                    }
                }
            }
            FileEventKind::Rescan => Ok(UpdateOutcome::NeedsRescan),
        }
    }

    fn upsert_path(&mut self, path: &Path) -> Result<UpdateOutcome> {
        let record = gfm_fs::record_for_path(path, None, false)?;
        self.index.insert(record);
        Ok(UpdateOutcome::Upserted)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateOutcome {
    Upserted,
    Removed { records: usize },
    Renamed { removed: usize, inserted: usize },
    NeedsRescan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentIndexOptions {
    pub batch_size: usize,
    pub segment_prefix: String,
}

impl Default for ContentIndexOptions {
    fn default() -> Self {
        Self {
            batch_size: 1024,
            segment_prefix: "content".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentIndexReport {
    pub indexed: usize,
    pub skipped: usize,
    pub terms: usize,
    pub segments: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentIndexJobSpec {
    pub root: PathBuf,
    pub segment_dir: PathBuf,
    pub records_path: PathBuf,
    pub content_path: PathBuf,
    pub batch_size: usize,
}

impl ContentIndexJobSpec {
    pub fn new(
        root: impl Into<PathBuf>,
        segment_dir: impl Into<PathBuf>,
        records_path: impl Into<PathBuf>,
        content_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            root: root.into(),
            segment_dir: segment_dir.into(),
            records_path: records_path.into(),
            content_path: content_path.into(),
            batch_size: ContentIndexOptions::default().batch_size,
        }
    }

    pub fn options(&self) -> ContentIndexOptions {
        ContentIndexOptions {
            batch_size: self.batch_size,
            segment_prefix: ContentIndexOptions::default().segment_prefix,
        }
    }

    pub fn write(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let file = fs::File::create(path).map_err(|err| GfmError::io(path, err))?;
        let mut writer = BufWriter::new(file);
        writeln!(writer, "gfm-content-job-v1").map_err(|err| GfmError::io(path, err))?;
        writeln!(writer, "root\t{}", escape_path(&self.root))
            .map_err(|err| GfmError::io(path, err))?;
        writeln!(writer, "segment_dir\t{}", escape_path(&self.segment_dir))
            .map_err(|err| GfmError::io(path, err))?;
        writeln!(writer, "records_path\t{}", escape_path(&self.records_path))
            .map_err(|err| GfmError::io(path, err))?;
        writeln!(writer, "content_path\t{}", escape_path(&self.content_path))
            .map_err(|err| GfmError::io(path, err))?;
        writeln!(writer, "batch_size\t{}", self.batch_size)
            .map_err(|err| GfmError::io(path, err))?;
        writer.flush().map_err(|err| GfmError::io(path, err))
    }

    pub fn read(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let file = fs::File::open(path).map_err(|err| GfmError::io(path, err))?;
        let mut lines = BufReader::new(file).lines();
        match lines.next() {
            Some(Ok(header)) if header == "gfm-content-job-v1" => {}
            Some(Ok(header)) => {
                return Err(GfmError::Format(format!(
                    "unsupported content job header `{header}` in {}",
                    path.display()
                )))
            }
            Some(Err(err)) => return Err(GfmError::io(path, err)),
            None => {
                return Err(GfmError::Format(format!(
                    "empty content job {}",
                    path.display()
                )))
            }
        }

        let mut root = None;
        let mut segment_dir = None;
        let mut records_path = None;
        let mut content_path = None;
        let mut batch_size = None;
        for (line_index, line) in lines.enumerate() {
            let line = line.map_err(|err| GfmError::io(path, err))?;
            let (key, value) = line.split_once('\t').ok_or_else(|| {
                GfmError::Format(format!(
                    "{} line {}: expected key and value",
                    path.display(),
                    line_index + 2
                ))
            })?;
            match key {
                "root" => root = Some(PathBuf::from(unescape(value)?)),
                "segment_dir" => segment_dir = Some(PathBuf::from(unescape(value)?)),
                "records_path" => records_path = Some(PathBuf::from(unescape(value)?)),
                "content_path" => content_path = Some(PathBuf::from(unescape(value)?)),
                "batch_size" => {
                    batch_size = Some(value.parse().map_err(|err| {
                        GfmError::Format(format!("invalid content job batch size `{value}`: {err}"))
                    })?)
                }
                other => {
                    return Err(GfmError::Format(format!(
                        "{}: unknown content job field `{other}`",
                        path.display()
                    )))
                }
            }
        }

        Ok(Self {
            root: required_field(root, "root", path)?,
            segment_dir: required_field(segment_dir, "segment_dir", path)?,
            records_path: required_field(records_path, "records_path", path)?,
            content_path: required_field(content_path, "content_path", path)?,
            batch_size: required_field(batch_size, "batch_size", path)?,
        })
    }
}

#[derive(Debug, Clone)]
pub struct BackgroundContentIndexer {
    extractor: Extractor,
    options: ContentIndexOptions,
}

impl BackgroundContentIndexer {
    pub fn new(extractor: Extractor, options: ContentIndexOptions) -> Self {
        Self { extractor, options }
    }

    pub fn run_to_segments(
        &self,
        snapshot: &IndexSnapshot,
        output_dir: impl AsRef<Path>,
        cancellation: &Cancellation,
    ) -> Result<ContentIndexReport> {
        let output_dir = output_dir.as_ref();
        fs::create_dir_all(output_dir).map_err(|err| gfm_types::GfmError::io(output_dir, err))?;
        let batch_size = self.options.batch_size.max(1);
        let mut report = ContentIndexReport {
            indexed: 0,
            skipped: 0,
            terms: 0,
            segments: Vec::new(),
        };

        for (batch_index, records) in snapshot.records.chunks(batch_size).enumerate() {
            cancellation.check()?;
            let segment_path = output_dir.join(format!(
                "{}-{:08}.gfmseg",
                self.options.segment_prefix, batch_index
            ));
            let mut live = LiveIndex::from_records(records.to_vec());
            let indexed = live.index_content(&self.extractor)?;
            report.indexed += indexed;
            report.skipped += records.len().saturating_sub(indexed);
            let postings = live.content_postings();
            report.terms += postings.len();
            write_content_segment(
                &segment_path,
                &ContentSegment {
                    tombstones: Vec::new(),
                    postings,
                },
            )?;
            report.segments.push(segment_path);
        }
        Ok(report)
    }

    pub fn run_and_compact(
        &self,
        snapshot: &IndexSnapshot,
        segment_dir: impl AsRef<Path>,
        content_path: impl AsRef<Path>,
        cancellation: &Cancellation,
    ) -> Result<ContentIndexReport> {
        let mut report = self.run_to_segments(snapshot, segment_dir, cancellation)?;
        cancellation.check()?;
        report.terms = compact_content_segments(content_path, &report.segments)?.len();
        Ok(report)
    }
}

impl Default for BackgroundContentIndexer {
    fn default() -> Self {
        Self::new(Extractor::default(), ContentIndexOptions::default())
    }
}

#[derive(Debug, Clone)]
pub struct Indexer {
    options: ScanOptions,
}

impl Indexer {
    pub fn new(options: ScanOptions) -> Self {
        Self { options }
    }

    pub fn build(&self, root: impl AsRef<Path>) -> Result<IndexSnapshot> {
        scan_tree(root, self.options.clone()).map(IndexSnapshot::from_page)
    }

    pub fn build_persistent(
        &self,
        root: impl AsRef<Path>,
        records_path: impl AsRef<Path>,
        state_path: impl AsRef<Path>,
    ) -> Result<IndexVolumeState> {
        let records_path = records_path.as_ref();
        let state_path = state_path.as_ref();
        let previous = state_path
            .exists()
            .then(|| IndexVolumeState::read(state_path))
            .transpose()?;
        let snapshot = self.build(root)?;
        snapshot.save(records_path)?;
        let state = snapshot.volume_state(records_path.to_path_buf(), previous.as_ref())?;
        state.write(state_path)?;
        Ok(state)
    }

    pub fn load(&self, path: impl AsRef<Path>) -> Result<IndexSnapshot> {
        Ok(IndexSnapshot {
            root: PathBuf::new(),
            records: read_records(path)?,
            inaccessible: Vec::new(),
        })
    }

    pub fn load_live_with_content(
        &self,
        records_path: impl AsRef<Path>,
        content_path: impl AsRef<Path>,
    ) -> Result<LiveIndex> {
        let mut live = self.load(records_path)?.into_live();
        live.load_content_postings(content_path)?;
        Ok(live)
    }

    pub fn compact_content_segments(
        &self,
        output: impl AsRef<Path>,
        segments: &[impl AsRef<Path>],
    ) -> Result<usize> {
        compact_content_segments(output, segments).map(|postings| postings.len())
    }
}

impl Default for Indexer {
    fn default() -> Self {
        Self::new(ScanOptions::default())
    }
}

fn required_field<T>(value: Option<T>, field: &str, path: &Path) -> Result<T> {
    value.ok_or_else(|| {
        GfmError::Format(format!(
            "{}: missing content job field `{field}`",
            path.display()
        ))
    })
}

fn escape_path(path: &Path) -> String {
    escape(&path.to_string_lossy())
}

fn escape(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\\' => output.push_str("\\\\"),
            '\t' => output.push_str("\\t"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            other => output.push(other),
        }
    }
    output
}

fn unescape(input: &str) -> Result<String> {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            output.push(ch);
            continue;
        }
        match chars.next() {
            Some('\\') => output.push('\\'),
            Some('t') => output.push('\t'),
            Some('n') => output.push('\n'),
            Some('r') => output.push('\r'),
            Some(other) => {
                return Err(GfmError::Format(format!(
                    "invalid content job escape `\\{other}`"
                )))
            }
            None => return Err(GfmError::Format("trailing content job escape".to_string())),
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests;
