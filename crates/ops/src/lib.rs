use gfm_types::{GfmError, Result};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictPolicy {
    Fail,
    Replace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    Copy { from: PathBuf, to: PathBuf },
    Move { from: PathBuf, to: PathBuf },
    Rename { from: PathBuf, to: PathBuf },
    Delete { path: PathBuf },
    Trash { path: PathBuf },
}

impl Operation {
    pub fn target_path(&self) -> Option<&Path> {
        match self {
            Self::Copy { to, .. } | Self::Move { to, .. } | Self::Rename { to, .. } => Some(to),
            Self::Delete { .. } | Self::Trash { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationStatus {
    Started,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyMethod {
    ApfsClone,
    ByteCopy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    pub id: u128,
    pub status: OperationStatus,
    pub operation: Operation,
    pub message: Option<String>,
    pub timestamp_nanos: u128,
}

#[derive(Debug, Clone)]
pub struct OperationContext {
    pub conflict: ConflictPolicy,
    pub journal_path: PathBuf,
}

impl OperationContext {
    pub fn new(journal_path: impl Into<PathBuf>) -> Self {
        Self {
            conflict: ConflictPolicy::Fail,
            journal_path: journal_path.into(),
        }
    }

    pub fn with_conflict(mut self, conflict: ConflictPolicy) -> Self {
        self.conflict = conflict;
        self
    }
}

pub struct Operator {
    context: OperationContext,
}

impl Operator {
    pub fn new(context: OperationContext) -> Self {
        Self { context }
    }

    pub fn execute(&self, operation: Operation) -> Result<JournalEntry> {
        let id = now_nanos();
        self.append(JournalEntry::started(id, operation.clone()))?;
        match self.apply(&operation) {
            Ok(()) => {
                let entry = JournalEntry::completed(id, operation);
                self.append(entry.clone())?;
                Ok(entry)
            }
            Err(err) => {
                let entry = JournalEntry::failed(id, operation, err.to_string());
                let _ = self.append(entry);
                Err(err)
            }
        }
    }

    pub fn journal(&self) -> Result<Vec<JournalEntry>> {
        read_journal(&self.context.journal_path)
    }

    fn apply(&self, operation: &Operation) -> Result<()> {
        match operation {
            Operation::Copy { from, to } => copy_path(from, to, self.context.conflict),
            Operation::Move { from, to } | Operation::Rename { from, to } => {
                move_path(from, to, self.context.conflict)
            }
            Operation::Delete { path } => delete_path(path),
            Operation::Trash { path } => trash_path(path),
        }
    }

    fn append(&self, entry: JournalEntry) -> Result<()> {
        append_journal(&self.context.journal_path, &entry)
    }
}

impl JournalEntry {
    fn started(id: u128, operation: Operation) -> Self {
        Self {
            id,
            status: OperationStatus::Started,
            operation,
            message: None,
            timestamp_nanos: now_nanos(),
        }
    }

    fn completed(id: u128, operation: Operation) -> Self {
        Self {
            id,
            status: OperationStatus::Completed,
            operation,
            message: None,
            timestamp_nanos: now_nanos(),
        }
    }

    fn failed(id: u128, operation: Operation, message: String) -> Self {
        Self {
            id,
            status: OperationStatus::Failed,
            operation,
            message: Some(message),
            timestamp_nanos: now_nanos(),
        }
    }
}

fn copy_path(from: &Path, to: &Path, conflict: ConflictPolicy) -> Result<()> {
    ensure_source_exists(from)?;
    prepare_destination(to, conflict)?;
    let metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
    if metadata.is_dir() {
        copy_directory(from, to)
    } else {
        copy_file(from, to).map(|_| ())
    }
}

fn move_path(from: &Path, to: &Path, conflict: ConflictPolicy) -> Result<()> {
    ensure_source_exists(from)?;
    prepare_destination(to, conflict)?;
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(rename_err) => {
            copy_path(from, to, ConflictPolicy::Replace)?;
            delete_path(from).map_err(|delete_err| {
                GfmError::Format(format!(
                    "moved copy to {} but failed to remove source {}: {}; original rename error: {}",
                    to.display(),
                    from.display(),
                    delete_err,
                    rename_err
                ))
            })
        }
    }
}

fn delete_path(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|err| GfmError::io(path, err))?;
    if metadata.is_dir() {
        fs::remove_dir_all(path).map_err(|err| GfmError::io(path, err))
    } else {
        fs::remove_file(path).map_err(|err| GfmError::io(path, err))
    }
}

fn trash_path(path: &Path) -> Result<()> {
    ensure_source_exists(path)?;
    trash::delete(path).map_err(|err| GfmError::io(path, err))
}

fn copy_directory(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to).map_err(|err| GfmError::io(to, err))?;
    let metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
    preserve_metadata(from, to, &metadata)?;

    for entry in fs::read_dir(from).map_err(|err| GfmError::io(from, err))? {
        let entry = entry.map_err(|err| GfmError::io(from, err))?;
        let source = entry.path();
        let destination = to.join(entry.file_name());
        let child_metadata =
            fs::symlink_metadata(&source).map_err(|err| GfmError::io(&source, err))?;
        if child_metadata.is_dir() {
            copy_directory(&source, &destination)?;
        } else {
            let _ = copy_file(&source, &destination)?;
        }
    }
    Ok(())
}

fn copy_file(from: &Path, to: &Path) -> Result<CopyMethod> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    let metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
    match clone_file(from, to) {
        Ok(()) => {
            preserve_metadata(from, to, &metadata)?;
            Ok(CopyMethod::ApfsClone)
        }
        Err(err) if clone_fallback_allowed(&err) => {
            remove_failed_clone_destination(to)?;
            fs::copy(from, to).map_err(|err| GfmError::io(from, err))?;
            preserve_metadata(from, to, &metadata)?;
            Ok(CopyMethod::ByteCopy)
        }
        Err(err) => Err(GfmError::io(from, err)),
    }
}

fn preserve_metadata(from: &Path, to: &Path, metadata: &fs::Metadata) -> Result<()> {
    preserve_permissions(to, metadata)?;
    preserve_times(to, metadata)?;
    preserve_xattrs(from, to)
}

fn preserve_permissions(to: &Path, metadata: &fs::Metadata) -> Result<()> {
    fs::set_permissions(to, metadata.permissions()).map_err(|err| GfmError::io(to, err))
}

fn preserve_times(to: &Path, metadata: &fs::Metadata) -> Result<()> {
    let atime = filetime::FileTime::from_last_access_time(metadata);
    let mtime = filetime::FileTime::from_last_modification_time(metadata);
    filetime::set_file_times(to, atime, mtime).map_err(|err| GfmError::io(to, err))
}

fn preserve_xattrs(from: &Path, to: &Path) -> Result<()> {
    let names = match xattr::list(from) {
        Ok(names) => names,
        Err(err) if xattr_copy_unsupported(&err) => return Ok(()),
        Err(err) => return Err(GfmError::io(from, err)),
    };
    for name in names {
        let value = match xattr::get(from, &name) {
            Ok(Some(value)) => value,
            Ok(None) => continue,
            Err(err) if xattr_copy_unsupported(&err) => continue,
            Err(err) => return Err(GfmError::io(from, err)),
        };
        match xattr::set(to, &name, &value) {
            Ok(()) => {}
            Err(err) if xattr_copy_unsupported(&err) => {}
            Err(err) => return Err(GfmError::io(to, err)),
        }
    }
    Ok(())
}

fn xattr_copy_unsupported(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(libc::ENOTSUP)
            | Some(libc::ENODATA)
            | Some(libc::ENOATTR)
            | Some(libc::EPERM)
            | Some(libc::EACCES)
    ) || matches!(
        err.kind(),
        io::ErrorKind::Unsupported | io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
    )
}

#[cfg(target_os = "macos")]
fn clone_file(from: &Path, to: &Path) -> io::Result<()> {
    let source = File::open(from)?;
    rustix::fs::fclonefileat(
        &source,
        rustix::fs::CWD,
        to,
        rustix::fs::CloneFlags::empty(),
    )
    .map_err(io::Error::from)
}

#[cfg(not(target_os = "macos"))]
fn clone_file(_from: &Path, _to: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "native clonefile is only available on macOS",
    ))
}

fn clone_fallback_allowed(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(libc::ENOTSUP) | Some(libc::EXDEV) | Some(libc::EINVAL)
    ) || matches!(
        err.kind(),
        io::ErrorKind::Unsupported | io::ErrorKind::InvalidInput
    )
}

fn remove_failed_clone_destination(to: &Path) -> Result<()> {
    match fs::symlink_metadata(to) {
        Ok(metadata) if metadata.is_dir() => {
            fs::remove_dir_all(to).map_err(|err| GfmError::io(to, err))
        }
        Ok(_) => fs::remove_file(to).map_err(|err| GfmError::io(to, err)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(GfmError::io(to, err)),
    }
}

fn ensure_source_exists(path: &Path) -> Result<()> {
    if path.exists() || fs::symlink_metadata(path).is_ok() {
        Ok(())
    } else {
        Err(GfmError::Io {
            path: path.to_path_buf(),
            message: "source does not exist".to_string(),
        })
    }
}

fn prepare_destination(path: &Path, conflict: ConflictPolicy) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }

    match conflict {
        ConflictPolicy::Fail => Err(GfmError::Conflict {
            path: path.to_path_buf(),
            message: "destination already exists".to_string(),
        }),
        ConflictPolicy::Replace => delete_path(path),
    }
}

fn append_journal(path: &Path, entry: &JournalEntry) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|err| GfmError::io(path, err))?;
    writeln!(file, "{}", encode_entry(entry)).map_err(|err| GfmError::io(path, err))?;
    file.flush().map_err(|err| GfmError::io(path, err))
}

pub fn read_journal(path: impl AsRef<Path>) -> Result<Vec<JournalEntry>> {
    let path = path.as_ref();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = File::open(path).map_err(|err| GfmError::io(path, err))?;
    let reader = BufReader::new(file);
    let mut entries = Vec::new();
    for (index, line) in reader.lines().enumerate() {
        let line = line.map_err(|err| GfmError::io(path, err))?;
        entries.push(parse_entry(&line).map_err(|err| {
            GfmError::Format(format!("{} line {}: {}", path.display(), index + 1, err))
        })?);
    }
    Ok(entries)
}

fn encode_entry(entry: &JournalEntry) -> String {
    let (op, from, to) = encode_operation(&entry.operation);
    [
        entry.id.to_string(),
        encode_status(entry.status).to_string(),
        entry.timestamp_nanos.to_string(),
        op.to_string(),
        escape(&from),
        escape(&to),
        escape(entry.message.as_deref().unwrap_or("")),
    ]
    .join("\t")
}

fn parse_entry(line: &str) -> std::result::Result<JournalEntry, String> {
    let parts: Vec<_> = line.split('\t').collect();
    if parts.len() != 7 {
        return Err(format!("expected 7 fields, got {}", parts.len()));
    }
    let id = parts[0]
        .parse()
        .map_err(|err| format!("invalid operation id `{}`: {err}", parts[0]))?;
    let status = decode_status(parts[1])?;
    let timestamp_nanos = parts[2]
        .parse()
        .map_err(|err| format!("invalid timestamp `{}`: {err}", parts[2]))?;
    let operation = decode_operation(parts[3], &unescape(parts[4])?, &unescape(parts[5])?)?;
    let message = unescape(parts[6])?;
    Ok(JournalEntry {
        id,
        status,
        operation,
        message: (!message.is_empty()).then_some(message),
        timestamp_nanos,
    })
}

fn encode_operation(operation: &Operation) -> (&'static str, String, String) {
    match operation {
        Operation::Copy { from, to } => ("copy", path_string(from), path_string(to)),
        Operation::Move { from, to } => ("move", path_string(from), path_string(to)),
        Operation::Rename { from, to } => ("rename", path_string(from), path_string(to)),
        Operation::Delete { path } => ("delete", path_string(path), String::new()),
        Operation::Trash { path } => ("trash", path_string(path), String::new()),
    }
}

fn decode_operation(kind: &str, from: &str, to: &str) -> std::result::Result<Operation, String> {
    match kind {
        "copy" => Ok(Operation::Copy {
            from: PathBuf::from(from),
            to: PathBuf::from(to),
        }),
        "move" => Ok(Operation::Move {
            from: PathBuf::from(from),
            to: PathBuf::from(to),
        }),
        "rename" => Ok(Operation::Rename {
            from: PathBuf::from(from),
            to: PathBuf::from(to),
        }),
        "delete" => Ok(Operation::Delete {
            path: PathBuf::from(from),
        }),
        "trash" => Ok(Operation::Trash {
            path: PathBuf::from(from),
        }),
        other => Err(format!("unknown operation `{other}`")),
    }
}

fn encode_status(status: OperationStatus) -> &'static str {
    match status {
        OperationStatus::Started => "started",
        OperationStatus::Completed => "completed",
        OperationStatus::Failed => "failed",
    }
}

fn decode_status(value: &str) -> std::result::Result<OperationStatus, String> {
    match value {
        "started" => Ok(OperationStatus::Started),
        "completed" => Ok(OperationStatus::Completed),
        "failed" => Ok(OperationStatus::Failed),
        other => Err(format!("unknown status `{other}`")),
    }
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

fn unescape(input: &str) -> std::result::Result<String, String> {
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
            Some(other) => return Err(format!("invalid escape `\\{other}`")),
            None => return Err("trailing escape".to_string()),
        }
    }
    Ok(output)
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_directories_and_records_journal() {
        let root = unique_temp_dir("gfm-ops-copy");
        let journal = root.join("journal.log");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("nested").join("file.txt"), "hello").unwrap();

        let operator = Operator::new(OperationContext::new(&journal));
        let entry = operator
            .execute(Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap();

        assert_eq!(entry.status, OperationStatus::Completed);
        assert_eq!(
            fs::read_to_string(destination.join("nested").join("file.txt")).unwrap(),
            "hello"
        );
        let journal_entries = operator.journal().unwrap();
        assert_eq!(journal_entries.len(), 2);
        assert_eq!(journal_entries[0].status, OperationStatus::Started);
        assert_eq!(journal_entries[1].status, OperationStatus::Completed);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copy_file_reports_method_and_preserves_contents() {
        let root = unique_temp_dir("gfm-ops-copy-method");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "clone-aware copy").unwrap();

        let method = copy_file(&source, &destination).unwrap();

        assert!(matches!(
            method,
            CopyMethod::ApfsClone | CopyMethod::ByteCopy
        ));
        assert_eq!(
            fs::read_to_string(&destination).unwrap(),
            "clone-aware copy"
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn copy_file_uses_apfs_clone_when_host_supports_it() {
        let root = unique_temp_dir("gfm-ops-apfs-clone");
        let source = root.join("source.bin");
        let probe = root.join("probe.bin");
        let destination = root.join("destination.bin");
        fs::write(&source, b"copy-on-write candidate").unwrap();

        match clone_file(&source, &probe) {
            Ok(()) => {
                fs::remove_file(&probe).unwrap();
                let method = copy_file(&source, &destination).unwrap();
                assert_eq!(method, CopyMethod::ApfsClone);
                assert_eq!(fs::read(&destination).unwrap(), b"copy-on-write candidate");
            }
            Err(err) if clone_fallback_allowed(&err) => {
                let method = copy_file(&source, &destination).unwrap();
                assert_eq!(method, CopyMethod::ByteCopy);
            }
            Err(err) => panic!("unexpected clonefile failure: {err}"),
        }

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copy_preserves_xattrs_when_host_supports_them() {
        let root = unique_temp_dir("gfm-ops-xattrs");
        let journal = root.join("journal.log");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "finder metadata").unwrap();
        match xattr::set(&source, "user.gfm.test", b"tagged") {
            Ok(()) => {}
            Err(err) if xattr_copy_unsupported(&err) => {
                fs::remove_dir_all(root).unwrap();
                return;
            }
            Err(err) => panic!("unexpected xattr setup failure: {err}"),
        }

        Operator::new(OperationContext::new(&journal))
            .execute(Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap();

        assert_eq!(
            xattr::get(&destination, "user.gfm.test")
                .unwrap()
                .as_deref(),
            Some(b"tagged".as_slice())
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copy_preserves_modified_time() {
        let root = unique_temp_dir("gfm-ops-times");
        let journal = root.join("journal.log");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "dated").unwrap();
        let expected = filetime::FileTime::from_unix_time(1_700_000_000, 123_000_000);
        filetime::set_file_mtime(&source, expected).unwrap();

        Operator::new(OperationContext::new(&journal))
            .execute(Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap();

        let copied = fs::metadata(&destination).unwrap();
        assert_eq!(
            filetime::FileTime::from_last_modification_time(&copied),
            expected
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fails_on_destination_conflict_without_mutating_source() {
        let root = unique_temp_dir("gfm-ops-conflict");
        let journal = root.join("journal.log");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "source").unwrap();
        fs::write(&destination, "destination").unwrap();

        let operator = Operator::new(OperationContext::new(&journal));
        let err = operator
            .execute(Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap_err();

        assert!(matches!(err, GfmError::Conflict { .. }));
        assert_eq!(fs::read_to_string(&source).unwrap(), "source");
        assert_eq!(fs::read_to_string(&destination).unwrap(), "destination");
        let journal_entries = operator.journal().unwrap();
        assert_eq!(journal_entries.len(), 2);
        assert_eq!(journal_entries[1].status, OperationStatus::Failed);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn moves_files_with_replace_policy() {
        let root = unique_temp_dir("gfm-ops-move");
        let journal = root.join("journal.log");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "new").unwrap();
        fs::write(&destination, "old").unwrap();

        let operator =
            Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::Replace));
        operator
            .execute(Operation::Move {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap();

        assert!(!source.exists());
        assert_eq!(fs::read_to_string(&destination).unwrap(), "new");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn deletes_files_and_directories() {
        let root = unique_temp_dir("gfm-ops-delete");
        let journal = root.join("journal.log");
        let target = root.join("target");
        fs::create_dir_all(target.join("nested")).unwrap();
        fs::write(target.join("nested").join("file.txt"), "gone").unwrap();

        let operator = Operator::new(OperationContext::new(&journal));
        operator
            .execute(Operation::Delete {
                path: target.clone(),
            })
            .unwrap();

        assert!(!target.exists());
        fs::remove_dir_all(root).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("{prefix}-{}", now_nanos()));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
