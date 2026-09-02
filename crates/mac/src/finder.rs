use gfm_mac_sys::{copy_kind_string_for_path, NativeFinderKindStatus};
use gfm_types::{FileKind, FileRecord};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinderKindSource {
    LaunchServices,
    FilesystemFallback,
}

impl FinderKindSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LaunchServices => "launchservices",
            Self::FilesystemFallback => "filesystem-fallback",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeFinderKindReport {
    pub kind_string: String,
    pub source: FinderKindSource,
    pub reason: Option<String>,
}

impl NativeFinderKindReport {
    pub fn from_record(record: &FileRecord) -> Self {
        let native = copy_kind_string_for_path(&record.path);
        if native.status == NativeFinderKindStatus::Available {
            if let Some(kind_string) = native.kind.filter(|kind| !kind.trim().is_empty()) {
                return Self {
                    kind_string,
                    source: FinderKindSource::LaunchServices,
                    reason: None,
                };
            }
        }

        Self {
            kind_string: fallback_kind(record),
            source: FinderKindSource::FilesystemFallback,
            reason: native.reason,
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "native-finder-kind\tkind={}\tsource={}\treason={}",
            escape_field(&self.kind_string),
            self.source.as_str(),
            self.reason
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
        )
    }
}

fn fallback_kind(record: &FileRecord) -> String {
    match record.kind {
        FileKind::Directory => "Folder".to_string(),
        FileKind::File => record
            .extension()
            .map(extension_title)
            .map(|extension| format!("{extension} Document"))
            .unwrap_or_else(|| "Document".to_string()),
        FileKind::Symlink => "Alias".to_string(),
        FileKind::Other => "Item".to_string(),
    }
}

fn extension_title(extension: &str) -> String {
    let mut chars = extension.trim().chars();
    let Some(first) = chars.next() else {
        return "File".to_string();
    };
    format!(
        "{}{}",
        first.to_uppercase().collect::<String>(),
        chars.as_str().to_ascii_lowercase()
    )
}

fn escape_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfm_types::{FileId, VolumeId};
    use std::path::PathBuf;

    #[test]
    fn fallback_kind_uses_finder_style_document_names() {
        let record = record("/tmp/Report.md", FileKind::File);

        let report = NativeFinderKindReport {
            kind_string: fallback_kind(&record),
            source: FinderKindSource::FilesystemFallback,
            reason: Some("forced\tfallback\nwithout\rlaunchservices".to_string()),
        };

        assert_eq!(report.kind_string, "Md Document");
        assert!(report.as_tsv().contains("source=filesystem-fallback"));
        assert_eq!(report.as_tsv().lines().count(), 1);
        assert!(report
            .as_tsv()
            .contains("reason=forced\\tfallback\\nwithout\\rlaunchservices"));
    }

    #[test]
    fn resolves_launchservices_kind_for_existing_file() {
        let path = std::env::temp_dir().join(format!(
            "gfm-native-finder-kind-report-{}.txt",
            std::process::id()
        ));
        std::fs::write(&path, "kind").unwrap();
        let record = record(path.to_str().unwrap(), FileKind::File);

        let report = NativeFinderKindReport::from_record(&record);

        assert_eq!(report.source, FinderKindSource::LaunchServices);
        assert!(!report.kind_string.trim().is_empty());
        std::fs::remove_file(path).unwrap();
    }

    fn record(path: &str, kind: FileKind) -> FileRecord {
        let path = PathBuf::from(path);
        FileRecord {
            id: FileId::new(VolumeId(1), 9),
            parent: None,
            name: path.file_name().unwrap().to_string_lossy().to_string(),
            path,
            kind,
            len: 0,
            mode: 0,
            owner: 0,
            group: 0,
            xattrs_digest: 0,
            created: None,
            modified: None,
            changed: None,
            hidden: false,
            tags: Vec::new(),
            finder_comment: None,
        }
    }
}
