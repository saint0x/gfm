use gfm_types::{GfmError, Result};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableCommit {
    pub path: PathBuf,
    pub temporary: PathBuf,
    pub bytes: u64,
    pub synced_file: bool,
    pub synced_parent: bool,
}

impl DurableCommit {
    pub fn as_tsv(&self) -> String {
        format!(
            "durable-commit\tpath={}\ttemporary={}\tbytes={}\tsynced-file={}\tsynced-parent={}",
            self.path.display(),
            self.temporary.display(),
            self.bytes,
            self.synced_file,
            self.synced_parent
        )
    }
}

pub fn atomic_write(
    path: impl AsRef<Path>,
    write: impl FnOnce(&mut dyn Write) -> std::io::Result<()>,
) -> Result<DurableCommit> {
    let path = path.as_ref();
    let temporary = temporary_path(path);
    let file = File::create(&temporary).map_err(|err| GfmError::io(&temporary, err))?;
    let mut writer = BufWriter::new(file);
    write(&mut writer).map_err(|err| GfmError::io(&temporary, err))?;
    writer
        .flush()
        .map_err(|err| GfmError::io(&temporary, err))?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|err| GfmError::io(&temporary, err))?;
    let bytes = writer
        .get_ref()
        .metadata()
        .map_err(|err| GfmError::io(&temporary, err))?
        .len();
    drop(writer);
    fs::rename(&temporary, path).map_err(|err| GfmError::io(path, err))?;
    let synced_parent = sync_parent(path)?;
    Ok(DurableCommit {
        path: path.to_path_buf(),
        temporary,
        bytes,
        synced_file: true,
        synced_parent,
    })
}

pub fn atomic_write_checked(
    path: impl AsRef<Path>,
    mut check_control: impl FnMut() -> Result<()>,
    write: impl FnOnce(&mut dyn Write, &mut dyn FnMut() -> Result<()>) -> Result<()>,
) -> Result<DurableCommit> {
    let path = path.as_ref();
    check_control()?;
    let temporary = temporary_path(path);
    let result = (|| {
        let file = File::create(&temporary).map_err(|err| GfmError::io(&temporary, err))?;
        check_control()?;
        let mut writer = BufWriter::new(file);
        write(&mut writer, &mut check_control)?;
        check_control()?;
        writer
            .flush()
            .map_err(|err| GfmError::io(&temporary, err))?;
        check_control()?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|err| GfmError::io(&temporary, err))?;
        check_control()?;
        let bytes = writer
            .get_ref()
            .metadata()
            .map_err(|err| GfmError::io(&temporary, err))?
            .len();
        drop(writer);
        fs::rename(&temporary, path).map_err(|err| GfmError::io(path, err))?;
        let synced_parent = sync_parent(path)?;
        Ok(DurableCommit {
            path: path.to_path_buf(),
            temporary: temporary.clone(),
            bytes,
            synced_file: true,
            synced_parent,
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub(crate) fn sync_parent_for_path(path: &Path) -> Result<bool> {
    sync_parent(path)
}

fn sync_parent(path: &Path) -> Result<bool> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    match File::open(parent) {
        Ok(file) => match file.sync_all() {
            Ok(()) => Ok(true),
            Err(_) => Ok(false),
        },
        Err(_) => Ok(false),
    }
}

fn temporary_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("store");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    path.with_file_name(format!(".{file_name}.{}.tmp", std::process::id()))
        .with_extension(format!("{nonce}.tmp"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    static CWD_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn atomic_write_accepts_relative_leaf_path() {
        let _cwd = CWD_LOCK.lock().unwrap();
        let root = temp_path("durable-relative-root");
        let previous = std::env::current_dir().unwrap();
        fs::create_dir_all(&root).unwrap();
        std::env::set_current_dir(&root).unwrap();
        let path = PathBuf::from("records.gfm");

        let commit =
            atomic_write(&path, |writer| writer.write_all(b"relative durable write")).unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "relative durable write");
        assert_eq!(commit.path, path);
        assert!(commit.synced_file);
        assert!(commit.synced_parent);
        std::env::set_current_dir(previous).unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn atomic_write_checked_honors_pre_cancelled_control_before_file_create() {
        let path = temp_path("durable-pre-cancel");

        let result = atomic_write_checked(&path, || Err(GfmError::Cancelled), |_, _| Ok(()));

        assert_eq!(result, Err(GfmError::Cancelled));
        assert!(!path.exists());
        assert!(!has_atomic_temp_file(&path));
    }

    #[test]
    fn atomic_write_checked_removes_temporary_and_preserves_existing_on_cancelled_write() {
        let path = temp_path("durable-cancelled-temp");
        fs::write(&path, b"stable").unwrap();

        let result = atomic_write_checked(
            &path,
            || Ok(()),
            |writer, _| {
                writer
                    .write_all(b"partial")
                    .map_err(|err| GfmError::io(&path, err))?;
                Err(GfmError::Cancelled)
            },
        );

        assert_eq!(result, Err(GfmError::Cancelled));
        assert_eq!(fs::read(&path).unwrap(), b"stable");
        assert!(!has_atomic_temp_file(&path));
        fs::remove_file(path).unwrap();
    }

    fn temp_path(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("gfm-store-{label}-{}-{nanos}", std::process::id()))
    }

    fn has_atomic_temp_file(path: &Path) -> bool {
        let Some(parent) = path.parent() else {
            return false;
        };
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            return false;
        };
        let prefix = format!(".{file_name}.{}.", std::process::id());
        fs::read_dir(parent)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .any(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".tmp"))
            })
    }
}
