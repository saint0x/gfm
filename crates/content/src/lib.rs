use gfm_types::{FileKind, FileRecord, GfmError, Result, SearchSnippet, SnippetHighlight};
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

    pub fn snippet_for_record(
        &self,
        record: &FileRecord,
        terms: &[String],
        phrases: &[String],
        context_bytes: usize,
    ) -> Result<Option<SearchSnippet>> {
        let Some(document) = self.extract_record(record)? else {
            return Ok(None);
        };
        Ok(build_snippet(
            &document.text,
            terms,
            phrases,
            context_bytes.max(1),
        ))
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

fn build_snippet(
    text: &str,
    terms: &[String],
    phrases: &[String],
    context_bytes: usize,
) -> Option<SearchSnippet> {
    let normalized = text.to_lowercase();
    let mut needles: Vec<_> = phrases
        .iter()
        .chain(terms.iter())
        .filter_map(|needle| {
            let needle = needle.trim().to_lowercase();
            (!needle.is_empty()).then_some(needle)
        })
        .collect();
    needles.sort_by_key(|needle| std::cmp::Reverse(needle.len()));

    let (match_start, match_end) = needles.iter().find_map(|needle| {
        normalized
            .find(needle)
            .map(|start| (start, start + needle.len()))
    })?;
    let snippet_start = floor_char_boundary(text, match_start.saturating_sub(context_bytes));
    let snippet_end = ceil_char_boundary(text, (match_end + context_bytes).min(text.len()));
    let snippet_text = text[snippet_start..snippet_end].to_string();
    let highlight_start = match_start.saturating_sub(snippet_start);
    let highlight_end = match_end.saturating_sub(snippet_start);

    Some(SearchSnippet {
        text: snippet_text,
        highlights: vec![SnippetHighlight {
            start: highlight_start,
            end: highlight_end,
        }],
    })
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
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
            tags: Vec::new(),
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

    #[test]
    fn extracts_bounded_snippet_with_highlight() {
        let root = unique_temp_dir("gfm-content-snippet");
        let path = root.join("note.md");
        fs::write(
            &path,
            "before before before exact snippet marker after after after",
        )
        .unwrap();
        let record = FileRecord {
            id: FileId::new(VolumeId(1), 1),
            parent: None,
            path: path.clone(),
            name: "note.md".to_string(),
            kind: FileKind::File,
            len: 57,
            created: None,
            modified: None,
            changed: None,
            hidden: false,
            tags: Vec::new(),
        };

        let snippet = Extractor::default()
            .snippet_for_record(&record, &[], &["exact snippet".to_string()], 8)
            .unwrap()
            .unwrap();

        assert!(snippet.text.contains("exact snippet"));
        assert!(snippet.text.len() < 57);
        assert_eq!(
            &snippet.text[snippet.highlights[0].start..snippet.highlights[0].end],
            "exact snippet"
        );
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
