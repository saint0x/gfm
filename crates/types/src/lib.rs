use std::fmt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VolumeId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId {
    pub volume: VolumeId,
    pub node: u64,
}

impl FileId {
    pub const fn new(volume: VolumeId, node: u64) -> Self {
        Self { volume, node }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileKind {
    Directory,
    File,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRecord {
    pub id: FileId,
    pub parent: Option<FileId>,
    pub path: PathBuf,
    pub name: String,
    pub kind: FileKind,
    pub len: u64,
    pub created: Option<SystemTime>,
    pub modified: Option<SystemTime>,
    pub changed: Option<SystemTime>,
    pub hidden: bool,
}

impl FileRecord {
    pub fn extension(&self) -> Option<&str> {
        self.path.extension().and_then(|ext| ext.to_str())
    }

    pub fn is_dir(&self) -> bool {
        self.kind == FileKind::Directory
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryPage {
    pub root: PathBuf,
    pub entries: Vec<FileRecord>,
    pub inaccessible: Vec<ScanIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanIssue {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub record: FileRecord,
    pub score: i64,
    pub reason: MatchReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentPosting {
    pub term: String,
    pub ids: Vec<FileId>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContentSegment {
    pub tombstones: Vec<FileId>,
    pub postings: Vec<ContentPosting>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchReason {
    ExactName,
    PrefixName,
    SubstringName,
    Extension,
    PathComponent,
    Content,
    FuzzyName,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEvent {
    pub path: PathBuf,
    pub kind: FileEventKind,
    pub observed_at: SystemTime,
}

impl FileEvent {
    pub fn new(path: impl Into<PathBuf>, kind: FileEventKind) -> Self {
        Self {
            path: path.into(),
            kind,
            observed_at: SystemTime::now(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileEventKind {
    Create,
    Modify,
    Remove,
    Rename { from: PathBuf, to: PathBuf },
    Rescan,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GfmError {
    Io { path: PathBuf, message: String },
    Format(String),
    Cancelled,
    Conflict { path: PathBuf, message: String },
}

impl GfmError {
    pub fn io(path: impl AsRef<Path>, error: impl fmt::Display) -> Self {
        Self::Io {
            path: path.as_ref().to_path_buf(),
            message: error.to_string(),
        }
    }
}

impl fmt::Display for GfmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => write!(f, "{}: {}", path.display(), message),
            Self::Format(message) => f.write_str(message),
            Self::Cancelled => f.write_str("operation was cancelled"),
            Self::Conflict { path, message } => write!(f, "{}: {}", path.display(), message),
        }
    }
}

impl std::error::Error for GfmError {}

pub type Result<T> = std::result::Result<T, GfmError>;
