use crate::journal::now_nanos;
use gfm_types::{GfmError, Result};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
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
    read_trash_metadata_checked(path, || Ok(()))
}

pub(crate) fn read_trash_metadata_checked(
    path: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<BTreeMap<String, TrashRestoreMetadata>> {
    check_control()?;
    if !path
        .try_exists()
        .map_err(|err| GfmError::io(path, format!("trash metadata existence unavailable: {err}")))?
    {
        return Ok(BTreeMap::new());
    }
    check_control()?;
    let file = File::open(path).map_err(|err| GfmError::io(path, err))?;
    check_control()?;
    let reader = BufReader::new(file);
    let mut entries = BTreeMap::new();
    for (line_index, line) in reader.lines().enumerate() {
        check_control()?;
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
        check_control()?;
    }
    check_control()?;
    Ok(entries)
}

#[cfg(test)]
pub(crate) fn append_trash_metadata(path: &Path, original_path: &Path) -> Result<()> {
    append_trash_metadata_entry(
        path,
        &TrashRestoreMetadata::from_original_path(original_path)?,
    )
}

#[cfg(test)]
pub(crate) fn append_trash_metadata_entry(path: &Path, entry: &TrashRestoreMetadata) -> Result<()> {
    append_trash_metadata_entry_checked(path, entry, || Ok(()))
}

pub(crate) fn append_trash_metadata_checked(
    path: &Path,
    original_path: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    check_control()?;
    append_trash_metadata_entry_checked(
        path,
        &TrashRestoreMetadata::from_original_path(original_path)?,
        check_control,
    )
}

pub(crate) fn append_trash_metadata_entry_checked(
    path: &Path,
    entry: &TrashRestoreMetadata,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    check_control()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    check_control()?;
    let mut entries = read_trash_metadata_checked(path, &mut check_control)?;
    check_control()?;
    entries.insert(entry.name.clone(), entry.clone());
    check_control()?;
    write_trash_metadata_checked(path, entries.values(), &mut check_control)
}

pub(crate) fn remove_trash_metadata_checked(
    path: &Path,
    trashed_path: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    check_control()?;
    let Some(name) = trashed_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
    else {
        return Ok(());
    };
    check_control()?;
    let mut entries = read_trash_metadata_checked(path, &mut check_control)?;
    check_control()?;
    entries.remove(&name);
    check_control()?;
    write_trash_metadata_checked(path, entries.values(), &mut check_control)
}

#[cfg(test)]
pub(crate) fn reconcile_empty_trash_metadata(
    metadata_path: Option<&Path>,
    trash_dir: &Path,
) -> Result<()> {
    reconcile_empty_trash_metadata_checked(metadata_path, trash_dir, || Ok(()))
}

pub(crate) fn reconcile_empty_trash_metadata_checked(
    metadata_path: Option<&Path>,
    trash_dir: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    let Some(metadata_path) = metadata_path else {
        return Ok(());
    };
    check_control()?;
    let mut entries = read_trash_metadata_checked(metadata_path, &mut check_control)?;
    check_control()?;
    let before = entries.len();
    let mut stale = Vec::new();
    for name in entries.keys() {
        check_control()?;
        if !path_exists_or_symlink(&trash_dir.join(name))? {
            stale.push(name.clone());
        }
    }
    for name in stale {
        check_control()?;
        entries.remove(&name);
    }
    if entries.len() != before {
        write_trash_metadata_checked(metadata_path, entries.values(), &mut check_control)?;
    }
    check_control()?;
    Ok(())
}

fn write_trash_metadata_checked<'a>(
    path: &Path,
    entries: impl IntoIterator<Item = &'a TrashRestoreMetadata>,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    check_control()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    check_control()?;
    let temporary = temporary_path(path);
    let result = (|| {
        let file = File::create(&temporary).map_err(|err| GfmError::io(&temporary, err))?;
        check_control()?;
        let mut file = BufWriter::new(file);
        for entry in entries {
            check_control()?;
            writeln!(file, "{}", entry.as_tsv()).map_err(|err| GfmError::io(&temporary, err))?;
        }
        check_control()?;
        file.flush().map_err(|err| GfmError::io(&temporary, err))?;
        check_control()?;
        file.get_ref()
            .sync_all()
            .map_err(|err| GfmError::io(&temporary, err))?;
        check_control()?;
        drop(file);
        fs::rename(&temporary, path).map_err(|err| GfmError::io(path, err))?;
        sync_parent(path);
        check_control()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn temporary_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("trash-metadata");
    path.with_file_name(format!(
        ".{file_name}.{}.{nonce}.tmp",
        std::process::id(),
        nonce = now_nanos()
    ))
}

fn sync_parent(path: &Path) {
    let Some(parent) = path.parent() else {
        return;
    };
    if let Ok(file) = File::open(parent) {
        let _ = file.sync_all();
    }
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

fn path_exists_or_symlink(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(GfmError::io(
            path,
            format!("trash item existence unavailable: {err}"),
        )),
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
    fn checked_read_honors_pre_cancelled_control_before_file_probe() {
        let root = std::env::temp_dir().join(format!(
            "gfm-trashmeta-read-cancel-{}-{}",
            std::process::id(),
            now_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("trash.tsv");
        fs::write(&path, "not valid metadata\n").unwrap();

        let error = read_trash_metadata_checked(&path, || Err(GfmError::Cancelled)).unwrap_err();

        assert_eq!(error, GfmError::Cancelled);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn checked_write_preserves_existing_metadata_on_cancel() {
        let root = std::env::temp_dir().join(format!(
            "gfm-trashmeta-write-cancel-{}-{}",
            std::process::id(),
            now_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("trash.tsv");
        let existing = TrashRestoreMetadata {
            name: "existing.md".to_string(),
            original_path: root.join("Documents").join("existing.md"),
            deleted_at_nanos: 1,
            can_restore: true,
            can_delete_permanently: true,
            permission_issue: None,
        };
        let replacement = TrashRestoreMetadata {
            name: "replacement.md".to_string(),
            original_path: root.join("Documents").join("replacement.md"),
            deleted_at_nanos: 2,
            can_restore: true,
            can_delete_permanently: true,
            permission_issue: None,
        };
        write_trash_metadata_checked(&path, std::iter::once(&existing), || Ok(())).unwrap();
        let before = fs::read(&path).unwrap();
        let mut checks = 0usize;

        let error = write_trash_metadata_checked(&path, std::iter::once(&replacement), || {
            checks += 1;
            if checks >= 5 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        })
        .unwrap_err();

        assert_eq!(error, GfmError::Cancelled);
        assert!(checks >= 5);
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(
            read_trash_metadata(&path).unwrap().get("existing.md"),
            Some(&existing)
        );
        assert_eq!(trash_metadata_temp_count(&path), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn checked_append_preserves_existing_metadata_on_cancel() {
        let root = std::env::temp_dir().join(format!(
            "gfm-trashmeta-append-cancel-{}-{}",
            std::process::id(),
            now_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("trash.tsv");
        let existing = TrashRestoreMetadata {
            name: "existing.md".to_string(),
            original_path: root.join("Documents").join("existing.md"),
            deleted_at_nanos: 1,
            can_restore: true,
            can_delete_permanently: true,
            permission_issue: None,
        };
        let appended = TrashRestoreMetadata {
            name: "appended.md".to_string(),
            original_path: root.join("Documents").join("appended.md"),
            deleted_at_nanos: 2,
            can_restore: true,
            can_delete_permanently: true,
            permission_issue: None,
        };
        append_trash_metadata_entry(&path, &existing).unwrap();
        let before = fs::read(&path).unwrap();
        let mut checks = 0usize;

        let error = append_trash_metadata_entry_checked(&path, &appended, || {
            checks += 1;
            if checks >= 8 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        })
        .unwrap_err();

        assert_eq!(error, GfmError::Cancelled);
        assert!(checks >= 8);
        assert_eq!(fs::read(&path).unwrap(), before);
        let entries = read_trash_metadata(&path).unwrap();
        assert!(entries.contains_key("existing.md"));
        assert!(!entries.contains_key("appended.md"));
        assert_eq!(trash_metadata_temp_count(&path), 0);
        fs::remove_dir_all(root).unwrap();
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

    #[test]
    fn read_trash_metadata_surfaces_path_probe_failures() {
        let root = std::env::temp_dir().join(format!(
            "gfm-trashmeta-probe-root-{}-{}",
            std::process::id(),
            now_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let metadata = root.join("trash-metadata-unavailable".repeat(64));

        let err = read_trash_metadata(&metadata).unwrap_err();

        assert!(err
            .to_string()
            .contains("trash metadata existence unavailable"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn empty_trash_reconciliation_surfaces_item_probe_failures() {
        let root = std::env::temp_dir().join(format!(
            "gfm-trashmeta-item-probe-root-{}-{}",
            std::process::id(),
            now_nanos()
        ));
        let trash_dir = root.join("Trash");
        let metadata = root.join("trash.tsv");
        fs::create_dir_all(&trash_dir).unwrap();
        let name = "trash-item-unavailable".repeat(64);
        append_trash_metadata_entry(
            &metadata,
            &TrashRestoreMetadata {
                name,
                original_path: root.join("Documents").join("missing.md"),
                deleted_at_nanos: 1,
                can_restore: true,
                can_delete_permanently: true,
                permission_issue: None,
            },
        )
        .unwrap();

        let err = reconcile_empty_trash_metadata(Some(&metadata), &trash_dir).unwrap_err();

        assert!(err.to_string().contains("trash item existence unavailable"));
        fs::remove_dir_all(root).unwrap();
    }

    fn trash_metadata_temp_count(path: &Path) -> usize {
        let Some(parent) = path.parent() else {
            return 0;
        };
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            return 0;
        };
        let prefix = format!(".{file_name}.{}.", std::process::id());
        fs::read_dir(parent)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
            .count()
    }
}
