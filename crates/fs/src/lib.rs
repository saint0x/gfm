use gfm_types::{
    DirectoryPage, FileId, FileKind, FileRecord, GfmError, Result, ScanIssue, VolumeId,
};
use std::collections::VecDeque;
use std::fs::{self, Metadata};
use std::io::Cursor;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanOptions {
    pub max_depth: usize,
    pub follow_symlinks: bool,
    pub include_hidden: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            max_depth: usize::MAX,
            follow_symlinks: false,
            include_hidden: true,
        }
    }
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
        let should_descend = record.is_dir() && depth < options.max_depth;
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
            },
        )
        .unwrap();

        assert!(page.entries.iter().any(|record| record.name == "note.txt"));
        assert!(page.inaccessible.is_empty());

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
