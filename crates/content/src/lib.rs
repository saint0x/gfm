mod pdf;

use gfm_types::{FileKind, FileRecord, GfmError, Result, SearchSnippet, SnippetHighlight};
use pdf::{extract_pdf, PdfExtractStatus};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::Read;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct ExtractionPolicy {
    pub max_bytes: u64,
    pub max_pdf_bytes: u64,
    pub max_pdf_pages: usize,
    pub max_pdf_objects: usize,
    pub extensions: BTreeSet<String>,
}

impl Default for ExtractionPolicy {
    fn default() -> Self {
        Self {
            max_bytes: 2 * 1024 * 1024,
            max_pdf_bytes: 16 * 1024 * 1024,
            max_pdf_pages: 256,
            max_pdf_objects: 20_000,
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
        if record.kind != FileKind::File {
            return Ok(None);
        }
        let max_bytes = if path_is_pdf(&record.path) {
            self.policy.max_pdf_bytes
        } else {
            self.policy.max_bytes
        };
        if record.len > max_bytes {
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
        let is_pdf = path_is_pdf(path);
        let max_bytes = if is_pdf {
            self.policy.max_pdf_bytes
        } else {
            self.policy.max_bytes
        };
        if metadata.len() > max_bytes {
            return Ok(None);
        }

        let file = File::open(path).map_err(|err| GfmError::io(path, err))?;
        let mut bytes = Vec::with_capacity(metadata.len().min(max_bytes) as usize);
        file.take(max_bytes)
            .read_to_end(&mut bytes)
            .map_err(|err| GfmError::io(path, err))?;

        if is_pdf {
            let (status, document) = extract_pdf(&bytes, &self.policy);
            return match status {
                PdfExtractStatus::Extracted
                | PdfExtractStatus::Unsupported
                | PdfExtractStatus::TooLarge
                | PdfExtractStatus::TooManyPages
                | PdfExtractStatus::TooManyObjects => Ok(document),
            };
        }

        if is_binary(&bytes) {
            return Ok(None);
        }

        let Ok(text) = String::from_utf8(bytes) else {
            return Ok(None);
        };
        let text = normalize_text(&text);

        Ok(Some(ContentDocument {
            bytes_read: text.len(),
            text,
        }))
    }

    fn accepts_path(&self, path: &Path) -> bool {
        path_is_pdf(path)
            || path
                .extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| self.policy.extensions.contains(&extension.to_lowercase()))
                .unwrap_or(false)
    }
}

fn path_is_pdf(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
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
    if sample.is_empty() {
        return false;
    }
    if has_binary_signature(sample) {
        return true;
    }
    if sample.contains(&0) {
        return true;
    }

    let controls = sample
        .iter()
        .filter(|byte| matches!(**byte, 0x01..=0x08 | 0x0e..=0x1f | 0x7f))
        .count();
    sample.len() >= 32 && controls * 100 / sample.len() > 10
}

fn has_binary_signature(bytes: &[u8]) -> bool {
    const SIGNATURES: &[&[u8]] = &[
        b"\x7fELF",
        b"\x89PNG\r\n\x1a\n",
        b"\xff\xd8\xff",
        b"GIF87a",
        b"GIF89a",
        b"PK\x03\x04",
        b"PK\x05\x06",
        b"PK\x07\x08",
        b"\x1f\x8b",
        b"7z\xbc\xaf\x27\x1c",
        b"Rar!\x1a\x07\x00",
        b"Rar!\x1a\x07\x01\x00",
        b"\xca\xfe\xba\xbe",
        b"\xcf\xfa\xed\xfe",
        b"\xce\xfa\xed\xfe",
        b"\xfe\xed\xfa\xcf",
        b"\xfe\xed\xfa\xce",
        b"%PDF-",
        b"SQLite format 3\0",
        b"bplist00",
    ];
    SIGNATURES
        .iter()
        .any(|signature| bytes.starts_with(signature))
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
    fn skips_known_binary_signatures_even_with_text_extension() {
        let root = unique_temp_dir("gfm-content-binary-signature");
        let path = root.join("image.txt");
        fs::write(&path, b"\x89PNG\r\n\x1a\nsuperneedle in binary payload").unwrap();

        let doc = Extractor::default().extract_path(&path).unwrap();

        assert!(doc.is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn skips_high_control_byte_payloads() {
        let root = unique_temp_dir("gfm-content-control-bytes");
        let path = root.join("controls.log");
        let mut bytes = b"prefix readable ".to_vec();
        bytes.extend([1, 2, 3, 4, 5, 6, 7, 8, 14, 15, 16, 17, 18, 19, 20, 21]);
        bytes.extend(b" suffix");
        fs::write(&path, bytes).unwrap();

        let doc = Extractor::default().extract_path(&path).unwrap();

        assert!(doc.is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn accepts_multibyte_utf8_text() {
        let root = unique_temp_dir("gfm-content-utf8");
        let path = root.join("note.txt");
        fs::write(&path, "cafe naive resume 東京").unwrap();

        let doc = Extractor::default().extract_path(&path).unwrap().unwrap();

        assert!(doc.text.contains("東京"));
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

    #[test]
    fn extracts_uncompressed_pdf_text() {
        let root = unique_temp_dir("gfm-content-pdf");
        let path = root.join("brief.pdf");
        fs::write(&path, minimal_pdf("pdfneedle inside document")).unwrap();

        let doc = Extractor::default().extract_path(&path).unwrap().unwrap();

        assert!(doc.text.contains("pdfneedle inside document"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn applies_pdf_byte_budget_to_records() {
        let root = unique_temp_dir("gfm-content-pdf-budget");
        let path = root.join("large.pdf");
        fs::write(&path, minimal_pdf("large pdf text")).unwrap();
        let record = FileRecord {
            id: FileId::new(VolumeId(1), 1),
            parent: None,
            path: path.clone(),
            name: "large.pdf".to_string(),
            kind: FileKind::File,
            len: fs::metadata(&path).unwrap().len(),
            created: None,
            modified: None,
            changed: None,
            hidden: false,
            tags: Vec::new(),
        };
        let extractor = Extractor::new(ExtractionPolicy {
            max_pdf_bytes: 12,
            ..ExtractionPolicy::default()
        });

        let doc = extractor.extract_record(&record).unwrap();

        assert!(doc.is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn skips_pdf_when_page_budget_is_exceeded() {
        let root = unique_temp_dir("gfm-content-pdf-pages");
        let path = root.join("many.pdf");
        fs::write(&path, multi_page_pdf(4)).unwrap();
        let extractor = Extractor::new(ExtractionPolicy {
            max_pdf_pages: 3,
            ..ExtractionPolicy::default()
        });

        let doc = extractor.extract_path(&path).unwrap();

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

    fn minimal_pdf(text: &str) -> Vec<u8> {
        format!(
            "%PDF-1.4
1 0 obj
<< /Type /Page /Contents 2 0 R >>
endobj
2 0 obj
<< /Length {} >>
stream
BT /F1 12 Tf 72 720 Td ({}) Tj ET
endstream
endobj
%%EOF",
            text.len() + 31,
            text
        )
        .into_bytes()
    }

    fn multi_page_pdf(pages: usize) -> Vec<u8> {
        let mut pdf = b"%PDF-1.4\n".to_vec();
        for index in 0..pages {
            pdf.extend(format!("{index} 0 obj << /Type /Page >> endobj\n").as_bytes());
        }
        pdf.extend(b"%%EOF");
        pdf
    }
}
