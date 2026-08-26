use crate::journal::now_nanos;
use gfm_types::{GfmError, Result};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashRestoreMetadata {
    pub name: String,
    pub original_path: PathBuf,
    pub deleted_at_nanos: u128,
    pub can_restore: bool,
    pub can_delete_permanently: bool,
    pub permission_issue: Option<String>,
}

impl TrashRestoreMetadata {
    pub(crate) fn from_original_path(path: &Path) -> Result<Self> {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                GfmError::Format(format!(
                    "could not derive trash metadata name for {}",
                    path.display()
                ))
            })?;
        Ok(Self {
            name,
            original_path: path.to_path_buf(),
            deleted_at_nanos: now_nanos(),
            can_restore: true,
            can_delete_permanently: true,
            permission_issue: None,
        })
    }

    fn as_tsv(&self) -> String {
        [
            escape(&self.name),
            escape(&path_string(&self.original_path)),
            self.deleted_at_nanos.to_string(),
            self.can_restore.to_string(),
            self.can_delete_permanently.to_string(),
            escape(self.permission_issue.as_deref().unwrap_or("")),
        ]
        .join("\t")
    }
}

pub fn read_trash_metadata(
    path: impl AsRef<Path>,
) -> Result<BTreeMap<String, TrashRestoreMetadata>> {
    let path = path.as_ref();
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let file = File::open(path).map_err(|err| GfmError::io(path, err))?;
    let reader = BufReader::new(file);
    let mut entries = BTreeMap::new();
    for (line_index, line) in reader.lines().enumerate() {
        let line = line.map_err(|err| GfmError::io(path, err))?;
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() != 6 {
            return Err(GfmError::Format(format!(
                "{}:{} expected 6 tab-separated fields: name, original_path, deleted_at_nanos, can_restore, can_delete_permanently, permission_issue",
                path.display(),
                line_index + 1
            )));
        }
        let name = unescape(fields[0]).map_err(GfmError::Format)?;
        let original_path = PathBuf::from(unescape(fields[1]).map_err(GfmError::Format)?);
        let deleted_at_nanos = fields[2].parse().map_err(|err| {
            GfmError::Format(format!(
                "{}:{} invalid deleted_at_nanos `{}`: {err}",
                path.display(),
                line_index + 1,
                fields[2]
            ))
        })?;
        let can_restore = parse_bool_field(fields[3], "can_restore", path, line_index + 1)?;
        let can_delete_permanently =
            parse_bool_field(fields[4], "can_delete_permanently", path, line_index + 1)?;
        let permission_issue = unescape(fields[5])
            .map_err(GfmError::Format)
            .map(|value| (!value.is_empty()).then_some(value))?;
        entries.insert(
            name.clone(),
            TrashRestoreMetadata {
                name,
                original_path,
                deleted_at_nanos,
                can_restore,
                can_delete_permanently,
                permission_issue,
            },
        );
    }
    Ok(entries)
}

pub(crate) fn append_trash_metadata(path: &Path, original_path: &Path) -> Result<()> {
    append_trash_metadata_entry(
        path,
        &TrashRestoreMetadata::from_original_path(original_path)?,
    )
}

pub(crate) fn append_trash_metadata_entry(path: &Path, entry: &TrashRestoreMetadata) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    let mut entries = read_trash_metadata(path)?;
    entries.insert(entry.name.clone(), entry.clone());
    write_trash_metadata(path, entries.values())
}

pub(crate) fn remove_trash_metadata(path: &Path, trashed_path: &Path) -> Result<()> {
    let Some(name) = trashed_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
    else {
        return Ok(());
    };
    let mut entries = read_trash_metadata(path)?;
    entries.remove(&name);
    write_trash_metadata(path, entries.values())
}

pub(crate) fn reconcile_empty_trash_metadata(
    metadata_path: Option<&Path>,
    trash_dir: &Path,
) -> Result<()> {
    let Some(metadata_path) = metadata_path else {
        return Ok(());
    };
    let mut entries = read_trash_metadata(metadata_path)?;
    let before = entries.len();
    entries.retain(|name, _| path_exists_or_symlink(&trash_dir.join(name)));
    if entries.len() != before {
        write_trash_metadata(metadata_path, entries.values())?;
    }
    Ok(())
}

fn write_trash_metadata<'a>(
    path: &Path,
    entries: impl IntoIterator<Item = &'a TrashRestoreMetadata>,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut file = File::create(&tmp).map_err(|err| GfmError::io(&tmp, err))?;
        for entry in entries {
            writeln!(file, "{}", entry.as_tsv()).map_err(|err| GfmError::io(&tmp, err))?;
        }
        file.flush().map_err(|err| GfmError::io(&tmp, err))?;
    }
    fs::rename(&tmp, path).map_err(|err| GfmError::io(path, err))
}

fn parse_bool_field(value: &str, name: &str, path: &Path, line: usize) -> Result<bool> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(GfmError::Format(format!(
            "{}:{} invalid {name} `{other}`",
            path.display(),
            line
        ))),
    }
}

fn path_exists_or_symlink(path: &Path) -> bool {
    path.exists() || fs::symlink_metadata(path).is_ok()
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

    #[test]
    fn metadata_round_trips_escaped_paths_and_permission_state() {
        let path = std::env::temp_dir().join(format!(
            "gfm-trashmeta-roundtrip-{}-{}.tsv",
            std::process::id(),
            now_nanos()
        ));
        let entry = TrashRestoreMetadata {
            name: "report\tone.md".to_string(),
            original_path: PathBuf::from("/tmp/Documents/line\nbreak\\report.md"),
            deleted_at_nanos: 42,
            can_restore: false,
            can_delete_permanently: true,
            permission_issue: Some("full\tdisk\naccess".to_string()),
        };

        append_trash_metadata_entry(&path, &entry).unwrap();
        let entries = read_trash_metadata(&path).unwrap();

        assert_eq!(entries.get("report\tone.md"), Some(&entry));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn empty_trash_reconciliation_removes_only_missing_entries() {
        let root = std::env::temp_dir().join(format!(
            "gfm-trashmeta-reconcile-{}-{}",
            std::process::id(),
            now_nanos()
        ));
        let trash_dir = root.join("Trash");
        let metadata = root.join("trash.tsv");
        fs::create_dir_all(&trash_dir).unwrap();
        fs::write(trash_dir.join("present.md"), "still here").unwrap();
        append_trash_metadata_entry(
            &metadata,
            &TrashRestoreMetadata {
                name: "present.md".to_string(),
                original_path: root.join("Documents").join("present.md"),
                deleted_at_nanos: 1,
                can_restore: true,
                can_delete_permanently: true,
                permission_issue: None,
            },
        )
        .unwrap();
        append_trash_metadata_entry(
            &metadata,
            &TrashRestoreMetadata {
                name: "missing.md".to_string(),
                original_path: root.join("Documents").join("missing.md"),
                deleted_at_nanos: 2,
                can_restore: true,
                can_delete_permanently: true,
                permission_issue: None,
            },
        )
        .unwrap();

        reconcile_empty_trash_metadata(Some(&metadata), &trash_dir).unwrap();
        let entries = read_trash_metadata(&metadata).unwrap();

        assert!(entries.contains_key("present.md"));
        assert!(!entries.contains_key("missing.md"));
        fs::remove_dir_all(root).unwrap();
    }
}
