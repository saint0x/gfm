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

fn sync_parent(path: &Path) -> Result<bool> {
    let Some(parent) = path.parent() else {
        return Ok(false);
    };
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
