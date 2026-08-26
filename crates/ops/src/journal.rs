use crate::Operation;
use gfm_types::GfmError;
use gfm_types::Result;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationStatus {
    Started,
    Completed,
    Skipped,
    Paused,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    pub id: u128,
    pub status: OperationStatus,
    pub operation: Operation,
    pub message: Option<String>,
    pub timestamp_nanos: u128,
}

impl JournalEntry {
    pub(crate) fn started(id: u128, operation: Operation) -> Self {
        Self {
            id,
            status: OperationStatus::Started,
            operation,
            message: None,
            timestamp_nanos: now_nanos(),
        }
    }

    pub(crate) fn completed(id: u128, operation: Operation) -> Self {
        Self {
            id,
            status: OperationStatus::Completed,
            operation,
            message: None,
            timestamp_nanos: now_nanos(),
        }
    }

    pub(crate) fn skipped(id: u128, operation: Operation) -> Self {
        Self {
            id,
            status: OperationStatus::Skipped,
            operation,
            message: Some("operation skipped by conflict policy".to_string()),
            timestamp_nanos: now_nanos(),
        }
    }

    pub(crate) fn paused(id: u128, operation: Operation) -> Self {
        Self {
            id,
            status: OperationStatus::Paused,
            operation,
            message: None,
            timestamp_nanos: now_nanos(),
        }
    }

    pub(crate) fn cancelled(id: u128, operation: Operation) -> Self {
        Self {
            id,
            status: OperationStatus::Cancelled,
            operation,
            message: None,
            timestamp_nanos: now_nanos(),
        }
    }

    pub(crate) fn failed(id: u128, operation: Operation, message: String) -> Self {
        Self {
            id,
            status: OperationStatus::Failed,
            operation,
            message: Some(message),
            timestamp_nanos: now_nanos(),
        }
    }

    pub(crate) fn from_error(id: u128, operation: Operation, err: &GfmError) -> Self {
        if matches!(err, GfmError::Paused) {
            Self::paused(id, operation)
        } else if matches!(err, GfmError::Cancelled) {
            Self::cancelled(id, operation)
        } else {
            Self::failed(id, operation, err.to_string())
        }
    }
}

pub(crate) fn operation_status_from_error(err: &GfmError) -> OperationStatus {
    if matches!(err, GfmError::Paused) {
        OperationStatus::Paused
    } else if matches!(err, GfmError::Cancelled) {
        OperationStatus::Cancelled
    } else {
        OperationStatus::Failed
    }
}

pub(crate) fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

pub(crate) fn append_journal(path: &Path, entry: &JournalEntry) -> Result<()> {
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
        Operation::EmptyTrash { path } => ("empty-trash", path_string(path), String::new()),
        Operation::Restore { from, to } => ("restore", path_string(from), path_string(to)),
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
        "empty-trash" => Ok(Operation::EmptyTrash {
            path: PathBuf::from(from),
        }),
        "restore" => Ok(Operation::Restore {
            from: PathBuf::from(from),
            to: PathBuf::from(to),
        }),
        other => Err(format!("unknown operation `{other}`")),
    }
}

fn encode_status(status: OperationStatus) -> &'static str {
    match status {
        OperationStatus::Started => "started",
        OperationStatus::Completed => "completed",
        OperationStatus::Skipped => "skipped",
        OperationStatus::Paused => "paused",
        OperationStatus::Cancelled => "cancelled",
        OperationStatus::Failed => "failed",
    }
}

fn decode_status(value: &str) -> std::result::Result<OperationStatus, String> {
    match value {
        "started" => Ok(OperationStatus::Started),
        "completed" => Ok(OperationStatus::Completed),
        "skipped" => Ok(OperationStatus::Skipped),
        "paused" => Ok(OperationStatus::Paused),
        "cancelled" => Ok(OperationStatus::Cancelled),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn operation() -> Operation {
        Operation::Delete {
            path: "/tmp/gfm-journal-entry".into(),
        }
    }

    #[test]
    fn skipped_entries_carry_the_conflict_policy_message() {
        let entry = JournalEntry::skipped(5, operation());

        assert_eq!(entry.status, OperationStatus::Skipped);
        assert_eq!(
            entry.message.as_deref(),
            Some("operation skipped by conflict policy")
        );
    }

    #[test]
    fn status_mapping_preserves_control_errors() {
        assert_eq!(
            operation_status_from_error(&GfmError::Cancelled),
            OperationStatus::Cancelled
        );
        assert_eq!(
            operation_status_from_error(&GfmError::Paused),
            OperationStatus::Paused
        );
        assert_eq!(
            operation_status_from_error(&GfmError::Format("bad journal".to_string())),
            OperationStatus::Failed
        );
    }

    #[test]
    fn error_entries_preserve_failure_messages() {
        let entry =
            JournalEntry::from_error(7, operation(), &GfmError::Format("bad entry".to_string()));

        assert_eq!(entry.status, OperationStatus::Failed);
        assert_eq!(entry.message.as_deref(), Some("bad entry"));
    }

    #[test]
    fn journal_round_trips_escaped_paths_and_messages() {
        let path = std::env::temp_dir().join(format!(
            "gfm-journal-roundtrip-{}-{}.log",
            std::process::id(),
            now_nanos()
        ));
        let operation = Operation::Move {
            from: PathBuf::from("/tmp/source\twith\ncontrol"),
            to: PathBuf::from("/tmp/destination\\with\rcontrol"),
        };
        let entry = JournalEntry {
            id: 99,
            status: OperationStatus::Failed,
            operation,
            message: Some("line one\nline two\twith slash \\".to_string()),
            timestamp_nanos: 123_456,
        };

        append_journal(&path, &entry).unwrap();
        let entries = read_journal(&path).unwrap();

        assert_eq!(entries, vec![entry]);
        fs::remove_file(path).unwrap();
    }
}
