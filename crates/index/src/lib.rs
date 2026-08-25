use gfm_content::Extractor;
use gfm_fs::{scan_tree, ScanOptions};
use gfm_jobs::Cancellation;
use gfm_search::SearchIndex;
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
        let mut index = SearchIndex::new();
        for record in self.records.iter().cloned() {
            index.insert(record);
        }
        index.query(query, limit)
    }

    pub fn search_cancellable(
        &self,
        query: &str,
        limit: usize,
        cancellation: &Cancellation,
    ) -> Result<Vec<SearchHit>> {
        let mut index = SearchIndex::new();
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

    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        write_records(path, &self.records)
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
    index: SearchIndex,
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

    pub fn search_cancellable(
        &self,
        query: &str,
        limit: usize,
        cancellation: &Cancellation,
    ) -> Result<Vec<SearchHit>> {
        self.index.query_cancellable(query, limit, cancellation)
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

#[derive(Debug, Clone, Copy)]
pub struct Indexer {
    options: ScanOptions,
}

impl Indexer {
    pub fn new(options: ScanOptions) -> Self {
        Self { options }
    }

    pub fn build(&self, root: impl AsRef<Path>) -> Result<IndexSnapshot> {
        scan_tree(root, self.options).map(IndexSnapshot::from_page)
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
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn builds_saves_loads_and_searches_snapshot() {
        let root = unique_temp_dir("gfm-index-root");
        let output = unique_temp_path("gfm-index", "gfmidx");
        fs::create_dir_all(root.join("Design")).unwrap();
        fs::write(root.join("Design").join("FinderParity.md"), "notes").unwrap();

        let indexer = Indexer::default();
        let snapshot = indexer.build(&root).unwrap();
        snapshot.save(&output).unwrap();
        let loaded = indexer.load(&output).unwrap();
        let hits = loaded.search("parity", 5);

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].record.name, "FinderParity.md");

        fs::remove_dir_all(root).unwrap();
        fs::remove_file(output).unwrap();
    }

    #[test]
    fn live_index_applies_create_modify_and_remove_events() {
        let root = unique_temp_dir("gfm-live-root");
        let target = root.join("Needle.txt");
        fs::write(&target, "first").unwrap();

        let mut live = LiveIndex::new();
        let created = FileEvent::new(&target, FileEventKind::Create);
        assert_eq!(live.apply_event(&created).unwrap(), UpdateOutcome::Upserted);
        assert_eq!(live.search("needle", 5).len(), 1);

        fs::write(&target, "second").unwrap();
        let modified = FileEvent::new(&target, FileEventKind::Modify);
        assert_eq!(
            live.apply_event(&modified).unwrap(),
            UpdateOutcome::Upserted
        );
        assert_eq!(live.search("needle", 5).len(), 1);

        fs::remove_file(&target).unwrap();
        let removed = FileEvent::new(&target, FileEventKind::Remove);
        assert_eq!(
            live.apply_event(&removed).unwrap(),
            UpdateOutcome::Removed { records: 1 }
        );
        assert!(live.search("needle", 5).is_empty());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn snapshot_can_search_text_content() {
        let root = unique_temp_dir("gfm-content-index-root");
        fs::write(root.join("notes.md"), "needle appears inside the file body").unwrap();

        let snapshot = Indexer::default().build(&root).unwrap();
        let hits = snapshot.search_with_content("needle", 5).unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].record.name, "notes.md");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn live_search_honors_cancellation() {
        let root = unique_temp_dir("gfm-cancelled-search-root");
        fs::write(root.join("notes.md"), "needle").unwrap();
        let snapshot = Indexer::default().build(&root).unwrap();
        let live = snapshot.into_live();
        let cancellation = Cancellation::default();
        cancellation.cancel();

        let result = live.search_cancellable("needle", 5, &cancellation);

        assert!(matches!(result, Err(GfmError::Cancelled)));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn durable_content_postings_survive_reload() {
        let root = unique_temp_dir("gfm-durable-content-root");
        let records = unique_temp_path("gfm-durable-content-records", "gfmidx");
        let content = unique_temp_path("gfm-durable-content-postings", "gfmcontent");
        fs::write(root.join("journal.md"), "a durable superneedle lives here").unwrap();

        let indexer = Indexer::default();
        let snapshot = indexer.build(&root).unwrap();
        let indexed = snapshot
            .save_with_content(&records, &content, &Extractor::default())
            .unwrap();
        let reloaded = indexer.load_live_with_content(&records, &content).unwrap();
        let hits = reloaded.search("superneedle", 5);

        assert_eq!(indexed, 1);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].record.name, "journal.md");

        fs::remove_dir_all(root).unwrap();
        fs::remove_file(records).unwrap();
        fs::remove_file(content).unwrap();
    }

    #[test]
    fn durable_content_positions_support_phrase_search_after_reload() {
        let root = unique_temp_dir("gfm-durable-phrase-root");
        let records = unique_temp_path("gfm-durable-phrase-records", "gfmidx");
        let content = unique_temp_path("gfm-durable-phrase-content", "gfmcontent");
        fs::write(
            root.join("keep.md"),
            "the exact durable phrase appears here",
        )
        .unwrap();
        fs::write(
            root.join("skip.md"),
            "the durable exact phrase appears in a different order",
        )
        .unwrap();

        let indexer = Indexer::default();
        let snapshot = indexer.build(&root).unwrap();
        snapshot
            .save_with_content(&records, &content, &Extractor::default())
            .unwrap();
        let reloaded = indexer.load_live_with_content(&records, &content).unwrap();
        let hits = reloaded.search(r#""exact durable phrase""#, 10);

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].record.name, "keep.md");

        fs::remove_dir_all(root).unwrap();
        fs::remove_file(records).unwrap();
        fs::remove_file(content).unwrap();
    }

    #[test]
    fn snapshot_can_write_content_segment_for_compaction() {
        let root = unique_temp_dir("gfm-content-segment-root");
        let segment = unique_temp_path("gfm-content-segment-index", "gfmseg");
        let content = unique_temp_path("gfm-content-segment-compact", "gfmcontent");
        fs::write(root.join("segment.md"), "segmenttoken appears here").unwrap();

        let indexer = Indexer::default();
        let snapshot = indexer.build(&root).unwrap();
        let indexed = snapshot
            .save_content_segment(&segment, &Extractor::default(), Vec::new())
            .unwrap();
        let terms = indexer
            .compact_content_segments(&content, &[&segment])
            .unwrap();
        let mut live = snapshot.into_live();
        live.load_content_postings(&content).unwrap();
        let hits = live.search("segmenttoken", 5);

        assert_eq!(indexed, 1);
        assert!(terms > 0);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].record.name, "segment.md");

        fs::remove_dir_all(root).unwrap();
        fs::remove_file(segment).unwrap();
        fs::remove_file(content).unwrap();
    }

    #[test]
    fn compacted_content_segments_preserve_phrase_positions() {
        let root = unique_temp_dir("gfm-content-phrase-segment-root");
        let segment = unique_temp_path("gfm-content-phrase-segment", "gfmseg");
        let content = unique_temp_path("gfm-content-phrase-compact", "gfmcontent");
        fs::write(root.join("phrase.md"), "segment phrase marker survives").unwrap();

        let indexer = Indexer::default();
        let snapshot = indexer.build(&root).unwrap();
        snapshot
            .save_content_segment(&segment, &Extractor::default(), Vec::new())
            .unwrap();
        indexer
            .compact_content_segments(&content, &[&segment])
            .unwrap();
        let mut live = snapshot.into_live();
        live.load_content_postings(&content).unwrap();
        let hits = live.search(r#""segment phrase marker""#, 5);

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].record.name, "phrase.md");

        fs::remove_dir_all(root).unwrap();
        fs::remove_file(segment).unwrap();
        fs::remove_file(content).unwrap();
    }

    #[test]
    fn background_content_indexer_batches_segments_and_compacts() {
        let root = unique_temp_dir("gfm-background-content-root");
        let segments = unique_temp_dir("gfm-background-content-segments");
        let content = unique_temp_path("gfm-background-content-compact", "gfmcontent");
        fs::write(root.join("first.md"), "first backgroundtoken").unwrap();
        fs::write(root.join("second.md"), "second backgroundtoken").unwrap();
        fs::write(root.join("third.md"), "third backgroundtoken").unwrap();

        let snapshot = Indexer::default().build(&root).unwrap();
        let worker = BackgroundContentIndexer::new(
            Extractor::default(),
            ContentIndexOptions {
                batch_size: 2,
                segment_prefix: "batch".to_string(),
            },
        );
        let report = worker
            .run_and_compact(&snapshot, &segments, &content, &Cancellation::default())
            .unwrap();
        let mut live = snapshot.into_live();
        live.load_content_postings(&content).unwrap();
        let hits = live.search("backgroundtoken", 10);

        assert_eq!(report.indexed, 3);
        assert_eq!(report.segments.len(), 2);
        assert!(report.terms > 0);
        assert_eq!(hits.len(), 3);

        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(segments).unwrap();
        fs::remove_file(content).unwrap();
    }

    #[test]
    fn content_index_job_spec_round_trips() {
        let path = unique_temp_path("gfm-content-job", "job");
        let spec = ContentIndexJobSpec {
            root: PathBuf::from("/tmp/root with spaces"),
            segment_dir: PathBuf::from("/tmp/segments"),
            records_path: PathBuf::from("/tmp/records.gfmidx"),
            content_path: PathBuf::from("/tmp/content.gfmcontent"),
            batch_size: 17,
        };

        spec.write(&path).unwrap();
        let read = ContentIndexJobSpec::read(&path).unwrap();

        assert_eq!(read, spec);
        fs::remove_file(path).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let path = unique_temp_path(prefix, "");
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn unique_temp_path(prefix: &str, extension: &str) -> PathBuf {
        let mut name = format!(
            "{}-{}",
            prefix,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        if !extension.is_empty() {
            name.push('.');
            name.push_str(extension);
        }
        std::env::temp_dir().join(name)
    }
}
