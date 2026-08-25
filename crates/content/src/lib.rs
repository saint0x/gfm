use gfm_types::{FileKind, FileRecord, GfmError, Result};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::Read;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct ExtractionPolicy {
    pub max_bytes: u64,
    pub extensions: BTreeSet<String>,
}

impl Default for ExtractionPolicy {
    fn default() -> Self {
        Self {
            max_bytes: 2 * 1024 * 1024,
            extensions: text_extensions(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentDocument {
    pub bytes_read: usize,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Extractor {
    policy: ExtractionPolicy,
}

impl Extractor {
    pub fn new(policy: ExtractionPolicy) -> Self {
        Self { policy }
    }

    pub fn extract_record(&self, record: &FileRecord) -> Result<Option<ContentDocument>> {
        if record.kind != FileKind::File || record.len > self.policy.max_bytes {
            return Ok(None);
        }
        if !self.accepts_path(&record.path) {
            return Ok(None);
        }
        self.extract_path(&record.path)
    }

    pub fn extract_path(&self, path: impl AsRef<Path>) -> Result<Option<ContentDocument>> {
        let path = path.as_ref();
        if !self.accepts_path(path) {
            return Ok(None);
        }
        let metadata = std::fs::metadata(path).map_err(|err| GfmError::io(path, err))?;
        if metadata.len() > self.policy.max_bytes {
            return Ok(None);
        }

        let file = File::open(path).map_err(|err| GfmError::io(path, err))?;
        let mut bytes = Vec::with_capacity(metadata.len().min(self.policy.max_bytes) as usize);
        file.take(self.policy.max_bytes)
            .read_to_end(&mut bytes)
            .map_err(|err| GfmError::io(path, err))?;

        if is_binary(&bytes) {
            return Ok(None);
        }

        let text = String::from_utf8(bytes)
            .map(|text| normalize_text(&text))
            .map_err(|err| {
                GfmError::Format(format!("{} is not valid UTF-8 text: {err}", path.display()))
            })?;

        Ok(Some(ContentDocument {
            bytes_read: text.len(),
            text,
        }))
    }

    fn accepts_path(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| self.policy.extensions.contains(&extension.to_lowercase()))
            .unwrap_or(false)
    }
}

impl Default for Extractor {
    fn default() -> Self {
        Self::new(ExtractionPolicy::default())
    }
}

fn normalize_text(input: &str) -> String {
    input
        .chars()
        .map(|ch| {
            if ch.is_control() && ch != '\n' && ch != '\t' {
                ' '
            } else {
                ch
            }
        })
        .collect()
}

fn is_binary(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(4096)];
    sample.contains(&0)
}

fn text_extensions() -> BTreeSet<String> {
    [
        "bash", "c", "cc", "conf", "cpp", "css", "csv", "go", "h", "hpp", "html", "java", "js",
        "json", "jsx", "log", "md", "mjs", "plist", "py", "rb", "rs", "sh", "sql", "swift", "toml",
        "ts", "tsx", "txt", "xml", "yaml", "yml", "zsh",
    ]
    .into_iter()
    .map(ToOwned::to_owned)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfm_types::{FileId, VolumeId};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn extracts_utf8_text_with_byte_budget() {
        let root = unique_temp_dir("gfm-content");
        let path = root.join("note.md");
        fs::write(&path, "hello content index").unwrap();
        let record = FileRecord {
            id: FileId::new(VolumeId(1), 1),
            parent: None,
            path: path.clone(),
            name: "note.md".to_string(),
            kind: FileKind::File,
            len: 19,
            created: None,
            modified: None,
            changed: None,
            hidden: false,
        };

        let doc = Extractor::default()
            .extract_record(&record)
            .unwrap()
            .unwrap();

        assert_eq!(doc.text, "hello content index");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn skips_binary_files() {
        let root = unique_temp_dir("gfm-content-binary");
        let path = root.join("binary.txt");
        fs::write(&path, [0, 159, 146, 150]).unwrap();

        let doc = Extractor::default().extract_path(&path).unwrap();

        assert!(doc.is_none());
        fs::remove_dir_all(root).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "{}-{}",
            prefix,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
