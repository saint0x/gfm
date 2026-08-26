use crate::IndexSnapshot;
use gfm_fs::ScanOptions;
use gfm_jobs::Cancellation;
use gfm_types::{DirectoryPage, FileId, GfmError, Result, ScanIssue};
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanLane {
    Visible,
    Background,
}

impl ScanLane {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Visible => "visible",
            Self::Background => "background",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FairScanSummary {
    pub records: usize,
    pub inaccessible: usize,
    pub visible_records: usize,
    pub background_records: usize,
    pub batches: usize,
    pub max_background_gap: usize,
}

impl FairScanSummary {
    pub fn as_tsv(&self) -> String {
        format!(
            "fair-scan\trecords={}\tinaccessible={}\tvisible-records={}\tbackground-records={}\tbatches={}\tmax-background-gap={}",
            self.records,
            self.inaccessible,
            self.visible_records,
            self.background_records,
            self.batches,
            self.max_background_gap
        )
    }
}

#[derive(Debug, Clone)]
pub struct FairScanReport {
    pub snapshot: IndexSnapshot,
    pub summary: FairScanSummary,
}

impl FairScanReport {
    pub fn as_tsv(&self) -> String {
        self.summary.as_tsv()
    }
}

#[derive(Debug, Clone)]
pub struct FairScanScheduler {
    options: ScanOptions,
    visible_burst: usize,
}

impl FairScanScheduler {
    pub fn new(options: ScanOptions, visible_burst: usize) -> Self {
        Self {
            options,
            visible_burst: visible_burst.max(1),
        }
    }

    pub fn scan(
        &self,
        root: impl AsRef<Path>,
        visible_roots: &[PathBuf],
    ) -> Result<FairScanReport> {
        self.scan_cancellable(root, visible_roots, &Cancellation::default())
    }

    pub fn scan_cancellable(
        &self,
        root: impl AsRef<Path>,
        visible_roots: &[PathBuf],
        cancellation: &Cancellation,
    ) -> Result<FairScanReport> {
        let root = root.as_ref().to_path_buf();
        let mut queue = ScanQueue::new(self.visible_burst);
        let mut visited = HashSet::new();
        queue.push(ScanWork::new(root.clone(), 0, None, ScanLane::Background));
        for visible in visible_roots {
            queue.push(ScanWork::new(visible.clone(), 0, None, ScanLane::Visible));
        }

        let mut entries = Vec::new();
        let mut inaccessible = Vec::new();
        let mut visible_records = 0;
        let mut background_records = 0;
        let mut batches = 0;
        let mut background_gap = 0;
        let mut max_background_gap = 0;

        while let Some(work) = queue.pop() {
            cancellation.check()?;
            if !visited.insert(path_key(&work.path)) {
                continue;
            }
            batches += 1;
            if work.lane == ScanLane::Background {
                max_background_gap = max_background_gap.max(background_gap);
                background_gap = 0;
            } else {
                background_gap += 1;
            }

            let record = match gfm_fs::record_for_path(
                work.path.clone(),
                work.parent,
                self.options.follow_symlinks,
            ) {
                Ok(record) => record,
                Err(GfmError::Io { path, message }) => {
                    inaccessible.push(ScanIssue {
                        path,
                        reason: message,
                    });
                    continue;
                }
                Err(err) => return Err(err),
            };

            let record_id = record.id;
            let should_descend = record.is_dir()
                && work.depth < self.options.max_depth
                && !(self.options.exclude_generated
                    && work.depth > 0
                    && is_generated_directory(&record.name))
                && self
                    .options
                    .package_policy
                    .should_descend(&work.path, record.kind);
            let should_include = self.options.include_hidden || !record.hidden || work.depth == 0;
            if should_include {
                if work.lane == ScanLane::Visible {
                    visible_records += 1;
                } else {
                    background_records += 1;
                }
                entries.push(record);
            }

            if should_descend {
                enqueue_children(
                    &mut queue,
                    &mut inaccessible,
                    &work,
                    record_id,
                    cancellation,
                )?;
            }
        }
        cancellation.check()?;
        max_background_gap = max_background_gap.max(background_gap);

        entries.sort_by(|a, b| a.path.cmp(&b.path));
        cancellation.check()?;
        let snapshot = IndexSnapshot::from_page(DirectoryPage {
            root,
            entries,
            inaccessible,
        });
        let summary = FairScanSummary {
            records: snapshot.records.len(),
            inaccessible: snapshot.inaccessible.len(),
            visible_records,
            background_records,
            batches,
            max_background_gap,
        };
        Ok(FairScanReport { snapshot, summary })
    }
}

#[derive(Debug, Clone)]
struct ScanWork {
    path: PathBuf,
    depth: usize,
    parent: Option<FileId>,
    lane: ScanLane,
}

impl ScanWork {
    fn new(path: PathBuf, depth: usize, parent: Option<FileId>, lane: ScanLane) -> Self {
        Self {
            path,
            depth,
            parent,
            lane,
        }
    }
}

#[derive(Debug, Clone)]
struct ScanQueue {
    visible_burst: usize,
    visible_credit: usize,
    visible: VecDeque<ScanWork>,
    background: VecDeque<ScanWork>,
}

impl ScanQueue {
    fn new(visible_burst: usize) -> Self {
        Self {
            visible_burst,
            visible_credit: 0,
            visible: VecDeque::new(),
            background: VecDeque::new(),
        }
    }

    fn push(&mut self, work: ScanWork) {
        match work.lane {
            ScanLane::Visible => self.visible.push_back(work),
            ScanLane::Background => self.background.push_back(work),
        }
    }

    fn pop(&mut self) -> Option<ScanWork> {
        if self.visible_credit < self.visible_burst {
            if let Some(work) = self.visible.pop_front() {
                self.visible_credit += 1;
                return Some(work);
            }
        }
        if let Some(work) = self.background.pop_front() {
            self.visible_credit = 0;
            return Some(work);
        }
        self.visible.pop_front()
    }
}

fn enqueue_children(
    queue: &mut ScanQueue,
    inaccessible: &mut Vec<ScanIssue>,
    work: &ScanWork,
    parent: FileId,
    cancellation: &Cancellation,
) -> Result<()> {
    cancellation.check()?;
    let dir = match fs::read_dir(&work.path) {
        Ok(dir) => dir,
        Err(err) => {
            inaccessible.push(ScanIssue {
                path: work.path.clone(),
                reason: err.to_string(),
            });
            return Ok(());
        }
    };

    let mut children = Vec::new();
    for child in dir {
        cancellation.check()?;
        match child {
            Ok(child) => children.push(child.path()),
            Err(err) => inaccessible.push(ScanIssue {
                path: work.path.clone(),
                reason: err.to_string(),
            }),
        }
    }
    cancellation.check()?;
    children.sort();
    for child in children {
        cancellation.check()?;
        queue.push(ScanWork::new(
            child,
            work.depth + 1,
            Some(parent),
            work.lane,
        ));
    }
    Ok(())
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn is_generated_directory(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".hg"
            | ".svn"
            | ".fozzy"
            | ".next"
            | ".turbo"
            | ".cache"
            | "target"
            | "node_modules"
            | "dist"
            | "build"
            | ".venv"
            | "__pycache__"
    )
}
