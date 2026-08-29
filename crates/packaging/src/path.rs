use gfm_types::{GfmError, Result};
use std::fs;
use std::path::Path;

pub(crate) fn recreate_dir(path: &Path, label: &str) -> Result<()> {
    match path.try_exists() {
        Ok(true) => fs::remove_dir_all(path).map_err(|err| GfmError::io(path, err))?,
        Ok(false) => {}
        Err(err) => {
            return Err(GfmError::io(
                path,
                format!("{label} directory probe unavailable: {err}"),
            ));
        }
    }
    create_dir(path)
}

pub(crate) fn create_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|err| GfmError::io(path, err))
}

pub(crate) fn remove_existing_file(path: &Path, label: &str) -> Result<()> {
    match path.try_exists() {
        Ok(true) => fs::remove_file(path).map_err(|err| GfmError::io(path, err)),
        Ok(false) => Ok(()),
        Err(err) => Err(GfmError::io(
            path,
            format!("{label} probe unavailable: {err}"),
        )),
    }
}

pub(crate) fn ensure_dir(path: &Path, label: &str) -> Result<()> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(GfmError::Format(format!(
            "{} is not a directory",
            path.display()
        ))),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Err(GfmError::Format(format!(
            "{} is missing or is not a directory",
            path.display()
        ))),
        Err(err) => Err(GfmError::io(
            path,
            format!("{label} directory metadata unavailable: {err}"),
        )),
    }
}

pub(crate) fn ensure_file(path: &Path, label: &str) -> Result<()> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err(GfmError::Format(format!(
            "{} is not a file",
            path.display()
        ))),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Err(GfmError::Format(format!(
            "{} is missing or is not a file",
            path.display()
        ))),
        Err(err) => Err(GfmError::io(
            path,
            format!("{label} file metadata unavailable: {err}"),
        )),
    }
}
