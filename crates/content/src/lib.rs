mod archive;
mod ooxml;
mod pdf;
mod rich;
mod structured;

use archive::{extract_archive_metadata, ArchiveExtractStatus, ArchiveKind};
use gfm_types::{FileKind, FileRecord, GfmError, Result, SearchSnippet, SnippetHighlight};
use ooxml::{extract_ooxml, OoxmlExtractStatus, OoxmlKind};
use pdf::{extract_pdf, PdfExtractStatus};
use rich::{extract_rich, RichKind};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use structured::{extract_structured, StructuredExtractStatus, StructuredKind};

#[derive(Debug, Clone)]
pub struct ExtractionPolicy {
    pub max_bytes: u64,
    pub max_pdf_bytes: u64,
    pub max_pdf_pages: usize,
    pub max_pdf_objects: usize,
    pub max_office_bytes: u64,
    pub max_office_entries: usize,
    pub max_office_entry_bytes: u64,
    pub max_office_text_bytes: usize,
    pub max_archive_bytes: u64,
    pub max_archive_entries: usize,
    pub max_archive_text_bytes: usize,
    pub max_structured_text_bytes: usize,
    pub extensions: BTreeSet<String>,
}

impl Default for ExtractionPolicy {
    fn default() -> Self {
        Self {
            max_bytes: 2 * 1024 * 1024,
            max_pdf_bytes: 16 * 1024 * 1024,
            max_pdf_pages: 256,
            max_pdf_objects: 20_000,
            max_office_bytes: 32 * 1024 * 1024,
            max_office_entries: 10_000,
            max_office_entry_bytes: 8 * 1024 * 1024,
            max_office_text_bytes: 4 * 1024 * 1024,
            max_archive_bytes: 64 * 1024 * 1024,
            max_archive_entries: 20_000,
            max_archive_text_bytes: 2 * 1024 * 1024,
            max_structured_text_bytes: 4 * 1024 * 1024,
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
        } else if office_kind(&record.path).is_some() {
            self.policy.max_office_bytes
        } else if archive_kind(&record.path).is_some() {
            self.policy.max_archive_bytes
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
        let office = office_kind(path);
        let rich = rich_kind(path);
        let archive = archive_kind(path);
        let structured = structured_kind(path);
        let is_pdf = path_is_pdf(path);
        let max_bytes = if is_pdf {
            self.policy.max_pdf_bytes
        } else if office.is_some() {
            self.policy.max_office_bytes
        } else if archive.is_some() {
            self.policy.max_archive_bytes
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

        if let Some(kind) = office {
            let (status, document) = extract_ooxml(&bytes, kind, &self.policy);
            return match status {
                OoxmlExtractStatus::Extracted
                | OoxmlExtractStatus::Unsupported
                | OoxmlExtractStatus::TooLarge
                | OoxmlExtractStatus::TooManyEntries
                | OoxmlExtractStatus::EntryTooLarge
                | OoxmlExtractStatus::Corrupt => Ok(document),
            };
        }

        if let Some(kind) = archive {
            let (status, document) = extract_archive_metadata(&bytes, kind, &self.policy);
            return match status {
                ArchiveExtractStatus::Extracted
                | ArchiveExtractStatus::Unsupported
                | ArchiveExtractStatus::TooLarge
                | ArchiveExtractStatus::TooManyEntries
                | ArchiveExtractStatus::Corrupt => Ok(document),
            };
        }

        if let Some(kind) = rich {
            return Ok(extract_rich(&bytes, kind));
        }

        if let Some(kind) = structured {
            let (status, document) = extract_structured(&bytes, kind, &self.policy);
            return match status {
                StructuredExtractStatus::Extracted
                | StructuredExtractStatus::Unsupported
                | StructuredExtractStatus::TooLarge
                | StructuredExtractStatus::Corrupt => Ok(document),
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
            || office_kind(path).is_some()
            || archive_kind(path).is_some()
            || rich_kind(path).is_some()
            || structured_kind(path).is_some()
            || path
                .extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| self.policy.extensions.contains(&extension.to_lowercase()))
                .unwrap_or(false)
    }
}

fn structured_kind(path: &Path) -> Option<StructuredKind> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "json" => Some(StructuredKind::Json),
        "csv" => Some(StructuredKind::Csv),
        "plist" => Some(StructuredKind::Plist),
        _ => None,
    }
}

fn rich_kind(path: &Path) -> Option<RichKind> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "html" | "htm" => Some(RichKind::Html),
        "rtf" => Some(RichKind::Rtf),
        "eml" => Some(RichKind::Email),
        _ => None,
    }
}

fn archive_kind(path: &Path) -> Option<ArchiveKind> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "zip" => Some(ArchiveKind::Zip),
        _ => None,
    }
}

fn office_kind(path: &Path) -> Option<OoxmlKind> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "docx" => Some(OoxmlKind::Docx),
        "xlsx" => Some(OoxmlKind::Xlsx),
        "pptx" => Some(OoxmlKind::Pptx),
        _ => None,
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
    use std::io::{Cursor, Write};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};
    use zip::write::SimpleFileOptions;

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

    #[test]
    fn extracts_docx_text() {
        let root = unique_temp_dir("gfm-content-docx");
        let path = root.join("brief.docx");
        fs::write(
            &path,
            ooxml_package(&[(
                "word/document.xml",
                "<w:document><w:body><w:p><w:r><w:t>docxneedle proposal</w:t></w:r></w:p></w:body></w:document>",
            )]),
        )
        .unwrap();

        let doc = Extractor::default().extract_path(&path).unwrap().unwrap();

        assert_eq!(doc.text, "docxneedle proposal");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extracts_xlsx_text() {
        let root = unique_temp_dir("gfm-content-xlsx");
        let path = root.join("numbers.xlsx");
        fs::write(
            &path,
            ooxml_package(&[(
                "xl/sharedStrings.xml",
                "<sst><si><t>sheetneedle</t></si><si><t>Revenue &amp; Margin</t></si></sst>",
            )]),
        )
        .unwrap();

        let doc = Extractor::default().extract_path(&path).unwrap().unwrap();

        assert_eq!(doc.text, "sheetneedle Revenue & Margin");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extracts_pptx_text() {
        let root = unique_temp_dir("gfm-content-pptx");
        let path = root.join("deck.pptx");
        fs::write(
            &path,
            ooxml_package(&[(
                "ppt/slides/slide1.xml",
                "<p:sld><p:cSld><a:t>slideneedle launch plan</a:t></p:cSld></p:sld>",
            )]),
        )
        .unwrap();

        let doc = Extractor::default().extract_path(&path).unwrap().unwrap();

        assert_eq!(doc.text, "slideneedle launch plan");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extracts_html_visible_text() {
        let root = unique_temp_dir("gfm-content-html");
        let path = root.join("page.html");
        fs::write(
            &path,
            "<html><body><h1>Visible &amp; searchable</h1><script>hiddenneedle</script><p>htmlneedle</p></body></html>",
        )
        .unwrap();

        let doc = Extractor::default().extract_path(&path).unwrap().unwrap();

        assert_eq!(doc.text, "Visible & searchable htmlneedle");
        assert!(!doc.text.contains("hiddenneedle"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extracts_rtf_text() {
        let root = unique_temp_dir("gfm-content-rtf");
        let path = root.join("note.rtf");
        fs::write(&path, br"{\rtf1\ansi rtfneedle\par rich text}").unwrap();

        let doc = Extractor::default().extract_path(&path).unwrap().unwrap();

        assert_eq!(doc.text, "rtfneedle rich text");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extracts_email_text() {
        let root = unique_temp_dir("gfm-content-email");
        let path = root.join("message.eml");
        fs::write(
            &path,
            b"From: Ada <ada@example.com>\r\nTo: Team\r\nSubject: Email Needle\r\n\r\nBody has emailneedle=20text",
        )
        .unwrap();

        let doc = Extractor::default().extract_path(&path).unwrap().unwrap();

        assert!(doc.text.contains("Email Needle"));
        assert!(doc.text.contains("emailneedle text"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extracts_zip_archive_metadata() {
        let root = unique_temp_dir("gfm-content-zip");
        let path = root.join("bundle.zip");
        fs::write(&path, zip_package(&[("docs/zipneedle.txt", "payload")])).unwrap();

        let doc = Extractor::default().extract_path(&path).unwrap().unwrap();

        assert!(doc.text.contains("docs/zipneedle.txt"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extracts_json_structure() {
        let root = unique_temp_dir("gfm-content-json");
        let path = root.join("data.json");
        fs::write(
            &path,
            br#"{"client":"Aperture","items":[{"name":"jsonneedle","count":3}]}"#,
        )
        .unwrap();

        let doc = Extractor::default().extract_path(&path).unwrap().unwrap();

        assert!(doc.text.contains("client"));
        assert!(doc.text.contains("Aperture"));
        assert!(doc.text.contains("jsonneedle"));
        assert!(doc.text.contains("3"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extracts_csv_cells() {
        let root = unique_temp_dir("gfm-content-csv");
        let path = root.join("rows.csv");
        fs::write(&path, "name,notes\nAda,\"csvneedle, quoted\"\n").unwrap();

        let doc = Extractor::default().extract_path(&path).unwrap().unwrap();

        assert_eq!(doc.text, "name notes Ada csvneedle, quoted");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extracts_binary_plist_values() {
        let root = unique_temp_dir("gfm-content-bplist");
        let path = root.join("settings.plist");
        let mut dictionary = plist::Dictionary::new();
        dictionary.insert("Owner".into(), plist::Value::String("plistneedle".into()));
        let mut bytes = Vec::new();
        plist::Value::Dictionary(dictionary)
            .to_writer_binary(&mut bytes)
            .unwrap();
        fs::write(&path, bytes).unwrap();

        let doc = Extractor::default().extract_path(&path).unwrap().unwrap();

        assert_eq!(doc.text, "Owner plistneedle");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn applies_office_entry_budget() {
        let root = unique_temp_dir("gfm-content-office-budget");
        let path = root.join("brief.docx");
        fs::write(
            &path,
            ooxml_package(&[("word/document.xml", "<w:t>large office text</w:t>")]),
        )
        .unwrap();
        let extractor = Extractor::new(ExtractionPolicy {
            max_office_entry_bytes: 4,
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

    fn ooxml_package(parts: &[(&str, &str)]) -> Vec<u8> {
        zip_package(parts)
    }

    fn zip_package(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, text) in parts {
            writer.start_file(*name, options).unwrap();
            writer.write_all(text.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }
}
