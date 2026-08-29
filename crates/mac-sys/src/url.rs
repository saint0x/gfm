use core_foundation::url::CFURL;
use std::fs;
use std::path::Path;

pub(crate) enum NativePathUrl {
    Ready(CFURL),
    Missing(String),
    Unavailable(String),
    Invalid(String),
}

pub(crate) fn existing_path_url(path: &Path, noun: &str) -> NativePathUrl {
    if path.to_str().is_none() {
        return NativePathUrl::Invalid(format!("{noun} is not valid UTF-8: {}", path.display()));
    }
    match path.try_exists() {
        Ok(true) => {}
        Ok(false) => {
            return NativePathUrl::Missing(format!("{noun} does not exist: {}", path.display()));
        }
        Err(error) => {
            return NativePathUrl::Unavailable(format!(
                "{noun} existence unavailable: {}: {error}",
                path.display()
            ));
        }
    }
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => {
            return NativePathUrl::Unavailable(format!(
                "{noun} metadata unavailable: {}: {error}",
                path.display()
            ));
        }
    };
    match CFURL::from_path(path, metadata.is_dir()) {
        Some(url) => NativePathUrl::Ready(url),
        None => NativePathUrl::Invalid(format!("invalid {noun} URL: {}", path.display())),
    }
}
