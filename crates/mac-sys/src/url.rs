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
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return NativePathUrl::Missing(format!("{noun} does not exist: {}", path.display()));
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "gfm-native-url-{label}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn existing_path_url_reports_missing_from_single_metadata_probe() {
        let path = unique_path("missing");

        let result = existing_path_url(&path, "native path");

        match result {
            NativePathUrl::Missing(reason) => {
                assert!(reason.contains("native path does not exist"));
                assert!(reason.contains(&path.display().to_string()));
            }
            NativePathUrl::Ready(_) | NativePathUrl::Unavailable(_) | NativePathUrl::Invalid(_) => {
                panic!("missing path should report a typed missing URL")
            }
        }
    }

    #[test]
    fn existing_path_url_reports_metadata_unavailable_for_unprobeable_path() {
        let path = unique_path(&"unprobeable".repeat(32));

        let result = existing_path_url(&path, "native path");

        match result {
            NativePathUrl::Unavailable(reason) => {
                assert!(reason.contains("native path metadata unavailable"));
            }
            NativePathUrl::Ready(_) | NativePathUrl::Missing(_) | NativePathUrl::Invalid(_) => {
                panic!("unprobeable path should report typed unavailable URL")
            }
        }
    }

    #[test]
    fn existing_path_url_builds_file_url_for_regular_file() {
        let path = unique_path("ready");
        fs::write(&path, "native url").unwrap();

        let result = existing_path_url(&path, "native path");

        match result {
            NativePathUrl::Ready(_) => {}
            NativePathUrl::Missing(_)
            | NativePathUrl::Unavailable(_)
            | NativePathUrl::Invalid(_) => {
                panic!("existing regular file should build a native URL")
            }
        }
        fs::remove_file(path).unwrap();
    }
}
