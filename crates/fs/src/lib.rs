use gfm_types::{
    DirectoryPage, FileId, FileKind, FileRecord, GfmError, Result, ScanIssue, VolumeId,
};
use std::collections::{BTreeSet, VecDeque};
use std::fs::{self, Metadata};
use std::io::Cursor;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

mod metadata;

pub use metadata::{
    FinderLabelColor, FinderLinkRole, FinderMetadataReport, FinderTagEntry, FinderTypeRole,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanOptions {
    pub max_depth: usize,
    pub follow_symlinks: bool,
    pub include_hidden: bool,
    pub exclude_generated: bool,
    pub package_policy: PackagePolicy,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            max_depth: usize::MAX,
            follow_symlinks: false,
            include_hidden: true,
            exclude_generated: true,
            package_policy: PackagePolicy::default(),
        }
    }
}

impl ScanOptions {
    pub fn with_package_traversal(mut self, traversal: PackageTraversalMode) -> Self {
        self.package_policy.traversal = traversal;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageTraversalMode {
    Opaque,
    Traverse,
}

impl PackageTraversalMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Opaque => "opaque",
            Self::Traverse => "traverse",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageKind {
    Application,
    Bundle,
    Framework,
    KernelExtension,
    XcodeProject,
    Playground,
    PhotosLibrary,
    MediaLibrary,
    DocumentPackage,
    GenericPackage,
}

impl PackageKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Application => "application",
            Self::Bundle => "bundle",
            Self::Framework => "framework",
            Self::KernelExtension => "kernel-extension",
            Self::XcodeProject => "xcode-project",
            Self::Playground => "playground",
            Self::PhotosLibrary => "photos-library",
            Self::MediaLibrary => "media-library",
            Self::DocumentPackage => "document-package",
            Self::GenericPackage => "generic-package",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackagePolicy {
    pub traversal: PackageTraversalMode,
    package_extensions: BTreeSet<String>,
}

impl PackagePolicy {
    pub fn new(traversal: PackageTraversalMode) -> Self {
        Self {
            traversal,
            package_extensions: default_package_extensions(),
        }
    }

    pub fn with_extension(mut self, extension: impl Into<String>) -> Self {
        self.package_extensions.insert(
            extension
                .into()
                .trim_start_matches('.')
                .to_ascii_lowercase(),
        );
        self
    }

    pub fn classify(&self, path: &Path, kind: FileKind) -> Option<PackageKind> {
        if kind != FileKind::Directory {
            return None;
        }
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        if !self.package_extensions.contains(&extension) {
            return None;
        }
        Some(match extension.as_str() {
            "app" => PackageKind::Application,
            "framework" => PackageKind::Framework,
            "kext" => PackageKind::KernelExtension,
            "xcodeproj" | "xcworkspace" => PackageKind::XcodeProject,
            "playground" => PackageKind::Playground,
            "photoslibrary" => PackageKind::PhotosLibrary,
            "imovielibrary" | "theater" | "band" | "logicx" => PackageKind::MediaLibrary,
            "pages" | "numbers" | "key" | "rtfd" => PackageKind::DocumentPackage,
            "bundle" | "plugin" | "appex" => PackageKind::Bundle,
            _ => PackageKind::GenericPackage,
        })
    }

    pub fn should_descend(&self, path: &Path, kind: FileKind) -> bool {
        self.traversal == PackageTraversalMode::Traverse || self.classify(path, kind).is_none()
    }
}

impl Default for PackagePolicy {
    fn default() -> Self {
        Self::new(PackageTraversalMode::Opaque)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageTraversalReport {
    pub root: std::path::PathBuf,
    pub mode: PackageTraversalMode,
    pub total_entries: usize,
    pub package_entries: Vec<PackageEntrySpec>,
}

impl PackageTraversalReport {
    pub fn from_page(page: &DirectoryPage, policy: &PackagePolicy) -> Self {
        let package_entries = page
            .entries
            .iter()
            .filter_map(|record| {
                policy
                    .classify(&record.path, record.kind)
                    .map(|package_kind| PackageEntrySpec {
                        path: record.path.clone(),
                        name: record.name.clone(),
                        package_kind,
                        traversed: policy.traversal == PackageTraversalMode::Traverse,
                    })
            })
            .collect();

        Self {
            root: page.root.clone(),
            mode: policy.traversal,
            total_entries: page.entries.len(),
            package_entries,
        }
    }

    pub fn as_tsv(&self) -> String {
        let mut lines = vec![format!(
            "package-traversal\tmode={}\ttotal_entries={}\tpackage_entries={}",
            self.mode.as_str(),
            self.total_entries,
            self.package_entries.len()
        )];
        for entry in &self.package_entries {
            lines.push(format!(
                "package\t{}\t{}\t{}\t{}",
                entry.package_kind.as_str(),
                entry.traversed,
                entry.name,
                entry.path.display()
            ));
        }
        lines.join("\n")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageEntrySpec {
    pub path: std::path::PathBuf,
    pub name: String,
    pub package_kind: PackageKind,
    pub traversed: bool,
}

pub fn read_directory(path: impl AsRef<Path>) -> Result<DirectoryPage> {
    let root = path.as_ref().to_path_buf();
    let mut entries = Vec::new();
    let mut inaccessible = Vec::new();

    let dir = fs::read_dir(&root).map_err(|err| GfmError::io(&root, err))?;
    for entry in dir {
        match entry {
            Ok(entry) => match record_for_path(entry.path(), None, false) {
                Ok(record) => entries.push(record),
                Err(GfmError::Io { path, message }) => inaccessible.push(ScanIssue {
                    path,
                    reason: message,
                }),
                Err(err) => return Err(err),
            },
            Err(err) => inaccessible.push(ScanIssue {
                path: root.clone(),
                reason: err.to_string(),
            }),
        }
    }

    entries.sort_by_key(finder_order);
    Ok(DirectoryPage {
        root,
        entries,
        inaccessible,
    })
}

pub fn scan_tree(root: impl AsRef<Path>, options: ScanOptions) -> Result<DirectoryPage> {
    let root = root.as_ref().to_path_buf();
    let mut entries = Vec::new();
    let mut inaccessible = Vec::new();
    let mut queue = VecDeque::from([(root.clone(), 0usize, None)]);

    while let Some((path, depth, parent)) = queue.pop_front() {
        let record = match record_for_path(path.clone(), parent, options.follow_symlinks) {
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
            && depth < options.max_depth
            && !(options.exclude_generated && depth > 0 && is_generated_directory(&record.name))
            && options.package_policy.should_descend(&path, record.kind);
        let should_include = options.include_hidden || !record.hidden || depth == 0;
        if should_include {
            entries.push(record);
        }

        if should_descend {
            let dir = match fs::read_dir(&path) {
                Ok(dir) => dir,
                Err(err) => {
                    inaccessible.push(ScanIssue {
                        path: path.clone(),
                        reason: err.to_string(),
                    });
                    continue;
                }
            };

            for child in dir {
                match child {
                    Ok(child) => queue.push_back((child.path(), depth + 1, Some(record_id))),
                    Err(err) => inaccessible.push(ScanIssue {
                        path: path.clone(),
                        reason: err.to_string(),
                    }),
                }
            }
        }
    }

    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(DirectoryPage {
        root,
        entries,
        inaccessible,
    })
}

pub fn record_for_path(
    path: impl AsRef<Path>,
    parent: Option<FileId>,
    follow_symlinks: bool,
) -> Result<FileRecord> {
    let path = path.as_ref().to_path_buf();
    let metadata = if follow_symlinks {
        fs::metadata(&path)
    } else {
        fs::symlink_metadata(&path)
    }
    .map_err(|err| GfmError::io(&path, err))?;

    let file_type = metadata.file_type();
    let kind = if file_type.is_dir() {
        FileKind::Directory
    } else if file_type.is_file() {
        FileKind::File
    } else if file_type.is_symlink() {
        FileKind::Symlink
    } else {
        FileKind::Other
    };

    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.display().to_string());
    let hidden = name.starts_with('.');

    Ok(FileRecord {
        id: file_id(&metadata),
        parent,
        tags: finder_tags(&path),
        path,
        name,
        kind,
        len: metadata.len(),
        created: metadata.created().ok(),
        modified: metadata.modified().ok(),
        changed: changed_time(&metadata),
        hidden,
    })
}

fn finder_tags(path: &Path) -> Vec<String> {
    let Some(raw) = xattr::get(path, "com.apple.metadata:_kMDItemUserTags")
        .ok()
        .flatten()
    else {
        return Vec::new();
    };
    let Ok(plist::Value::Array(values)) = plist::Value::from_reader(Cursor::new(raw)) else {
        return Vec::new();
    };

    let mut tags: Vec<_> = values
        .into_iter()
        .filter_map(|value| match value {
            plist::Value::String(tag) => finder_tag_name(&tag),
            _ => None,
        })
        .collect();
    tags.sort();
    tags.dedup();
    tags
}

fn finder_tag_name(raw: &str) -> Option<String> {
    let tag = raw
        .split_once('\n')
        .map(|(tag, _)| tag)
        .unwrap_or(raw)
        .trim();
    (!tag.is_empty()).then(|| tag.to_string())
}

fn finder_order(record: &FileRecord) -> (u8, String) {
    let group = if record.kind == FileKind::Directory {
        0
    } else {
        1
    };
    (group, record.name.to_lowercase())
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

fn default_package_extensions() -> BTreeSet<String> {
    [
        "app",
        "appex",
        "band",
        "bundle",
        "framework",
        "imovielibrary",
        "key",
        "kext",
        "logicx",
        "numbers",
        "pages",
        "photoslibrary",
        "playground",
        "plugin",
        "rtfd",
        "theater",
        "xcodeproj",
        "xcworkspace",
    ]
    .into_iter()
    .map(ToOwned::to_owned)
    .collect()
}

#[cfg(unix)]
fn file_id(metadata: &Metadata) -> FileId {
    FileId::new(VolumeId(metadata.dev()), metadata.ino())
}

#[cfg(not(unix))]
fn file_id(metadata: &Metadata) -> FileId {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    metadata.len().hash(&mut hasher);
    metadata.modified().ok().hash(&mut hasher);
    FileId::new(VolumeId(0), hasher.finish())
}

#[cfg(unix)]
fn changed_time(metadata: &Metadata) -> Option<std::time::SystemTime> {
    let secs = metadata.ctime();
    let nanos = metadata.ctime_nsec();
    if secs < 0 || nanos < 0 {
        None
    } else {
        Some(std::time::UNIX_EPOCH + std::time::Duration::new(secs as u64, nanos as u32))
    }
}

#[cfg(not(unix))]
fn changed_time(_metadata: &Metadata) -> Option<std::time::SystemTime> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn scans_real_tree_with_identity() {
        let root = unique_temp_dir();
        fs::create_dir_all(root.join("folder")).unwrap();
        let mut file = fs::File::create(root.join("folder").join("note.txt")).unwrap();
        writeln!(file, "hello").unwrap();

        let page = scan_tree(
            &root,
            ScanOptions {
                max_depth: 4,
                follow_symlinks: false,
                include_hidden: true,
                exclude_generated: true,
                package_policy: PackagePolicy::default(),
            },
        )
        .unwrap();

        assert!(page.entries.iter().any(|record| record.name == "note.txt"));
        assert!(page.inaccessible.is_empty());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn skips_generated_directories_by_default() {
        let root = unique_temp_dir();
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join("target").join("artifact.txt"), "generated").unwrap();
        fs::write(root.join("source.txt"), "authored").unwrap();

        let page = scan_tree(&root, ScanOptions::default()).unwrap();
        let paths: Vec<_> = page
            .entries
            .iter()
            .map(|record| record.path.strip_prefix(&root).unwrap().to_path_buf())
            .collect();

        assert!(paths.iter().any(|path| path == Path::new("source.txt")));
        assert!(paths.iter().any(|path| path == Path::new("target")));
        assert!(!paths
            .iter()
            .any(|path| path == Path::new("target/artifact.txt")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn treats_packages_as_opaque_by_default() {
        let root = unique_temp_dir();
        fs::create_dir_all(root.join("Demo.app").join("Contents")).unwrap();
        fs::write(
            root.join("Demo.app").join("Contents").join("Info.plist"),
            "plist",
        )
        .unwrap();
        fs::create_dir_all(root.join("PlainFolder")).unwrap();
        fs::write(root.join("PlainFolder").join("note.txt"), "note").unwrap();

        let page = scan_tree(&root, ScanOptions::default()).unwrap();
        let paths: Vec<_> = page
            .entries
            .iter()
            .map(|record| record.path.strip_prefix(&root).unwrap().to_path_buf())
            .collect();

        assert!(paths.iter().any(|path| path == Path::new("Demo.app")));
        assert!(!paths
            .iter()
            .any(|path| path == Path::new("Demo.app/Contents/Info.plist")));
        assert!(paths
            .iter()
            .any(|path| path == Path::new("PlainFolder/note.txt")));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn traverses_packages_when_policy_allows_it() {
        let root = unique_temp_dir();
        fs::create_dir_all(root.join("Doc.pages").join("Data")).unwrap();
        fs::write(root.join("Doc.pages").join("Data").join("Index.zip"), "zip").unwrap();

        let page = scan_tree(
            &root,
            ScanOptions::default().with_package_traversal(PackageTraversalMode::Traverse),
        )
        .unwrap();
        let paths: Vec<_> = page
            .entries
            .iter()
            .map(|record| record.path.strip_prefix(&root).unwrap().to_path_buf())
            .collect();

        assert!(paths.iter().any(|path| path == Path::new("Doc.pages")));
        assert!(paths
            .iter()
            .any(|path| path == Path::new("Doc.pages/Data/Index.zip")));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reports_package_traversal_contract() {
        let root = unique_temp_dir();
        fs::create_dir_all(root.join("Demo.app")).unwrap();
        fs::create_dir_all(root.join("Slides.key")).unwrap();

        let options = ScanOptions::default();
        let page = scan_tree(&root, options.clone()).unwrap();
        let report = PackageTraversalReport::from_page(&page, &options.package_policy);
        let tsv = report.as_tsv();

        assert!(tsv.contains("package-traversal\tmode=opaque"));
        assert!(tsv.contains("package\tapplication\tfalse\tDemo.app"));
        assert!(tsv.contains("package\tdocument-package\tfalse\tSlides.key"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parses_finder_tag_names() {
        assert_eq!(finder_tag_name("Important\n6").unwrap(), "Important");
        assert_eq!(finder_tag_name("Plain").unwrap(), "Plain");
        assert!(finder_tag_name("\n6").is_none());
    }

    #[test]
    fn reads_finder_tags_from_xattr_when_supported() {
        let root = unique_temp_dir();
        let path = root.join("tagged.txt");
        fs::write(&path, "tagged").unwrap();
        let value = plist::Value::Array(vec![plist::Value::String("Important\n6".to_string())]);
        let mut payload = Vec::new();
        value.to_writer_binary(&mut payload).unwrap();
        if xattr::set(&path, "com.apple.metadata:_kMDItemUserTags", &payload).is_err() {
            fs::remove_dir_all(root).unwrap();
            return;
        }

        let record = record_for_path(&path, None, false).unwrap();

        assert_eq!(record.tags, vec!["Important"]);
        fs::remove_dir_all(root).unwrap();
    }

    fn unique_temp_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "gfm-fs-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::SeqCst),
        ));
        let path = path.with_extension(format!(
            "{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
