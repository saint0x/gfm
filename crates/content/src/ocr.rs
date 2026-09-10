use crate::report::{ExtractionFingerprint, ExtractionFormat, ExtractionReport, ExtractionStatus};
use gfm_types::{FileKind, FileRecord};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

pub const OCR_EXTRACTOR_VERSION: u32 = 1;
pub const OCR_CANDIDATE_QUEUE_SCHEMA_VERSION: u32 = 1;
static OCR_QUEUE_TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcrCandidateKind {
    ImageOnlyPdf,
    ScreenshotImage,
}

impl OcrCandidateKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ImageOnlyPdf => "image-only-pdf",
            Self::ScreenshotImage => "screenshot-image",
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        match input {
            "image-only-pdf" => Some(Self::ImageOnlyPdf),
            "screenshot-image" => Some(Self::ScreenshotImage),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrCandidate {
    pub path: PathBuf,
    pub kind: OcrCandidateKind,
    pub fingerprint: ExtractionFingerprint,
}

impl OcrCandidate {
    pub fn queue_key(&self) -> String {
        format!(
            "{}:{}",
            self.kind.as_str(),
            self.fingerprint.cache_key(&self.path)
        )
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "ocr-candidate\tpath={}\tkind={}\tversion={}\tlen={}\tmodified-ns={}",
            escape_field(&self.path.to_string_lossy()),
            self.kind.as_str(),
            self.fingerprint.extractor_version,
            self.fingerprint.len,
            self.fingerprint
                .modified_ns
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string())
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OcrCandidateQueue {
    candidates: BTreeMap<String, OcrCandidate>,
}

impl OcrCandidateQueue {
    pub fn new(candidates: impl IntoIterator<Item = OcrCandidate>) -> Self {
        let mut queue = Self::default();
        for candidate in candidates {
            queue.insert(candidate);
        }
        queue
    }

    pub fn insert(&mut self, candidate: OcrCandidate) {
        self.candidates.insert(candidate.queue_key(), candidate);
    }

    pub fn len(&self) -> usize {
        self.candidates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    pub fn candidates(&self) -> impl Iterator<Item = &OcrCandidate> {
        self.candidates.values()
    }

    pub fn into_candidates(self) -> Vec<OcrCandidate> {
        self.candidates.into_values().collect()
    }

    pub fn write(&self, path: impl AsRef<Path>) -> crate::Result<()> {
        self.write_checked(path, || Ok(()))
    }

    pub fn write_checked(
        &self,
        path: impl AsRef<Path>,
        mut check_control: impl FnMut() -> crate::Result<()>,
    ) -> crate::Result<()> {
        let path = path.as_ref();
        check_control()?;
        let parent = real_parent_or_cwd(path);
        fs::create_dir_all(parent).map_err(|err| gfm_types::GfmError::io(parent, err))?;
        check_control()?;
        let temp = queue_temp_path(path);
        let result = (|| {
            let file = File::create(&temp).map_err(|err| gfm_types::GfmError::io(&temp, err))?;
            check_control()?;
            let mut writer = BufWriter::new(file);
            write_queue_line_checked(
                &mut writer,
                &temp,
                "gfm-ocr-candidates-v1\n",
                &mut check_control,
            )?;
            write_queue_line_checked(
                &mut writer,
                &temp,
                &format!("schema_version\t{OCR_CANDIDATE_QUEUE_SCHEMA_VERSION}\n"),
                &mut check_control,
            )?;
            for candidate in self.candidates.values() {
                write_queue_line_checked(
                    &mut writer,
                    &temp,
                    &format!("{}\n", candidate.as_tsv()),
                    &mut check_control,
                )?;
            }
            check_control()?;
            writer
                .flush()
                .map_err(|err| gfm_types::GfmError::io(&temp, err))?;
            check_control()?;
            writer
                .get_ref()
                .sync_all()
                .map_err(|err| gfm_types::GfmError::io(&temp, err))?;
            check_control()?;
            fs::rename(&temp, path).map_err(|err| gfm_types::GfmError::io(path, err))?;
            check_control()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result
    }

    pub fn read(path: impl AsRef<Path>) -> crate::Result<Self> {
        Self::read_checked(path, || Ok(()))
    }

    pub fn read_checked(
        path: impl AsRef<Path>,
        mut check_control: impl FnMut() -> crate::Result<()>,
    ) -> crate::Result<Self> {
        let path = path.as_ref();
        check_control()?;
        let file = File::open(path).map_err(|err| gfm_types::GfmError::io(path, err))?;
        check_control()?;
        let mut lines = BufReader::new(file).lines();
        let header = lines
            .next()
            .transpose()
            .map_err(|err| gfm_types::GfmError::io(path, err))?
            .ok_or_else(|| queue_format_error(path, "missing header"))?;
        if header != "gfm-ocr-candidates-v1" {
            return Err(queue_format_error(path, "unsupported OCR queue header"));
        }
        let mut schema_version = None;
        let mut queue = Self::default();
        for line in lines {
            check_control()?;
            let line = line.map_err(|err| gfm_types::GfmError::io(path, err))?;
            check_control()?;
            let mut parts = line.split('\t');
            match parts.next() {
                Some("schema_version") => {
                    schema_version = Some(parse_u32(parts.next(), path, "schema_version")?);
                }
                Some("ocr-candidate") => {
                    let candidate = parse_candidate_row(parts, path)?;
                    queue.insert(candidate);
                }
                Some("") | None => {}
                Some(_) => return Err(queue_format_error(path, "unknown OCR queue row")),
            }
        }
        check_control()?;
        if schema_version != Some(OCR_CANDIDATE_QUEUE_SCHEMA_VERSION) {
            return Err(queue_format_error(
                path,
                "unsupported OCR queue schema version",
            ));
        }
        Ok(queue)
    }
}

pub fn ocr_candidate_for_record(record: &FileRecord) -> Option<OcrCandidate> {
    if record.kind != FileKind::File || !path_is_screenshot_image(&record.path) {
        return None;
    }
    Some(OcrCandidate {
        path: record.path.clone(),
        kind: OcrCandidateKind::ScreenshotImage,
        fingerprint: record_ocr_fingerprint(record),
    })
}

pub fn ocr_candidate_for_extraction(report: &ExtractionReport) -> Option<OcrCandidate> {
    if report.format != ExtractionFormat::Pdf
        || report.status != ExtractionStatus::Skipped("image-only-pdf")
    {
        return None;
    }
    Some(OcrCandidate {
        path: report.path.clone(),
        kind: OcrCandidateKind::ImageOnlyPdf,
        fingerprint: ExtractionFingerprint {
            extractor_version: OCR_EXTRACTOR_VERSION,
            len: report.fingerprint.len,
            modified_ns: report.fingerprint.modified_ns,
        },
    })
}

fn record_ocr_fingerprint(record: &FileRecord) -> ExtractionFingerprint {
    ExtractionFingerprint {
        extractor_version: OCR_EXTRACTOR_VERSION,
        len: record.len,
        modified_ns: record
            .modified
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos()),
    }
}

fn path_is_screenshot_image(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
        return false;
    };
    if !matches!(
        extension.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "heic" | "heif" | "tif" | "tiff"
    ) {
        return false;
    }
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let name = name.to_ascii_lowercase();
    name.contains("screenshot") || name.contains("screen shot")
}

fn parse_candidate_row<'a>(
    parts: impl Iterator<Item = &'a str>,
    path: &Path,
) -> crate::Result<OcrCandidate> {
    let mut candidate_path = None;
    let mut kind = None;
    let mut version = None;
    let mut len = None;
    let mut modified_ns = None;
    for part in parts {
        let Some((key, value)) = part.split_once('=') else {
            return Err(queue_format_error(path, "invalid OCR candidate field"));
        };
        match key {
            "path" => candidate_path = Some(PathBuf::from(unescape_field(value))),
            "kind" => {
                kind =
                    Some(OcrCandidateKind::parse(value).ok_or_else(|| {
                        queue_format_error(path, "unsupported OCR candidate kind")
                    })?);
            }
            "version" => version = Some(parse_u32(Some(value), path, "version")?),
            "len" => len = Some(parse_u64(Some(value), path, "len")?),
            "modified-ns" => modified_ns = Some(parse_optional_u128(value, path, "modified-ns")?),
            _ => return Err(queue_format_error(path, "unknown OCR candidate field")),
        }
    }
    Ok(OcrCandidate {
        path: candidate_path.ok_or_else(|| queue_format_error(path, "missing candidate path"))?,
        kind: kind.ok_or_else(|| queue_format_error(path, "missing candidate kind"))?,
        fingerprint: ExtractionFingerprint {
            extractor_version: version
                .ok_or_else(|| queue_format_error(path, "missing candidate version"))?,
            len: len.ok_or_else(|| queue_format_error(path, "missing candidate len"))?,
            modified_ns: modified_ns
                .ok_or_else(|| queue_format_error(path, "missing candidate modified-ns"))?,
        },
    })
}

fn escape_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn unescape_field(value: &str) -> String {
    let mut output = String::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('t') => output.push('\t'),
                Some('n') => output.push('\n'),
                Some('r') => output.push('\r'),
                Some('\\') => output.push('\\'),
                Some(other) => {
                    output.push('\\');
                    output.push(other);
                }
                None => output.push('\\'),
            }
        } else {
            output.push(ch);
        }
    }
    output
}

fn write_queue_line_checked(
    writer: &mut impl Write,
    path: &Path,
    line: &str,
    mut check_control: impl FnMut() -> crate::Result<()>,
) -> crate::Result<()> {
    check_control()?;
    writer
        .write_all(line.as_bytes())
        .map_err(|err| gfm_types::GfmError::io(path, err))?;
    check_control()?;
    Ok(())
}

fn parse_u32(value: Option<&str>, path: &Path, name: &str) -> crate::Result<u32> {
    required_part(value, path, name)?
        .parse()
        .map_err(|_| queue_format_error(path, &format!("invalid {name}")))
}

fn parse_u64(value: Option<&str>, path: &Path, name: &str) -> crate::Result<u64> {
    required_part(value, path, name)?
        .parse()
        .map_err(|_| queue_format_error(path, &format!("invalid {name}")))
}

fn parse_optional_u128(value: &str, path: &Path, name: &str) -> crate::Result<Option<u128>> {
    if value == "-" {
        Ok(None)
    } else {
        value
            .parse()
            .map(Some)
            .map_err(|_| queue_format_error(path, &format!("invalid {name}")))
    }
}

fn required_part<'a>(value: Option<&'a str>, path: &Path, name: &str) -> crate::Result<&'a str> {
    value.ok_or_else(|| queue_format_error(path, &format!("missing {name}")))
}

fn queue_format_error(path: &Path, message: &str) -> gfm_types::GfmError {
    gfm_types::GfmError::Format(format!("{}: {message}", path.display()))
}

fn queue_temp_path(path: &Path) -> PathBuf {
    let mut temp_name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| "ocr-candidates".into());
    let sequence = OCR_QUEUE_TEMP_FILE_SEQUENCE.fetch_add(1, AtomicOrdering::Relaxed);
    temp_name.push(format!(".{}.{}.tmp", std::process::id(), sequence));
    path.with_file_name(temp_name)
}

fn real_parent_or_cwd(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::ContentDocument;
    use gfm_types::{FileId, VolumeId};
    use std::path::PathBuf;
    use std::time::UNIX_EPOCH;

    #[test]
    fn plans_screenshot_image_ocr_from_record_metadata() {
        let record = FileRecord {
            id: FileId::new(VolumeId(1), 7),
            parent: None,
            path: PathBuf::from("/tmp/Screenshot 2026-08-24 at 6.59.43 PM.png"),
            name: "Screenshot 2026-08-24 at 6.59.43 PM.png".to_string(),
            kind: FileKind::File,
            len: 2048,
            mode: 0,
            owner: 0,
            group: 0,
            xattrs_digest: 0,
            created: None,
            modified: Some(UNIX_EPOCH + std::time::Duration::from_nanos(42)),
            changed: None,
            hidden: false,
            tags: Vec::new(),
            finder_comment: None,
        };

        let candidate = ocr_candidate_for_record(&record).unwrap();

        assert_eq!(candidate.kind, OcrCandidateKind::ScreenshotImage);
        assert_eq!(
            candidate.fingerprint.extractor_version,
            OCR_EXTRACTOR_VERSION
        );
        assert_eq!(candidate.fingerprint.len, 2048);
        assert_eq!(candidate.fingerprint.modified_ns, Some(42));
        assert!(candidate.as_tsv().contains("\tkind=screenshot-image\t"));
    }

    #[test]
    fn plans_image_only_pdf_ocr_from_extraction_report() {
        let report = ExtractionReport {
            path: PathBuf::from("/tmp/scan.pdf"),
            format: ExtractionFormat::Pdf,
            status: ExtractionStatus::Skipped("image-only-pdf"),
            fingerprint: ExtractionFingerprint {
                extractor_version: 4,
                len: 4096,
                modified_ns: Some(99),
            },
            document: None,
        };

        let candidate = ocr_candidate_for_extraction(&report).unwrap();

        assert_eq!(candidate.kind, OcrCandidateKind::ImageOnlyPdf);
        assert_eq!(
            candidate.fingerprint.extractor_version,
            OCR_EXTRACTOR_VERSION
        );
        assert_eq!(candidate.fingerprint.len, 4096);
        assert_eq!(candidate.fingerprint.modified_ns, Some(99));
    }

    #[test]
    fn does_not_plan_ocr_for_extracted_pdf_text() {
        let report = ExtractionReport {
            path: PathBuf::from("/tmp/text.pdf"),
            format: ExtractionFormat::Pdf,
            status: ExtractionStatus::Extracted,
            fingerprint: ExtractionFingerprint {
                extractor_version: 4,
                len: 1024,
                modified_ns: None,
            },
            document: Some(ContentDocument {
                bytes_read: 1024,
                text: "already indexed".to_string(),
            }),
        };

        assert!(ocr_candidate_for_extraction(&report).is_none());
    }

    #[test]
    fn candidate_queue_round_trips_deterministically_with_control_paths() {
        let root = std::env::temp_dir().join(format!("gfm-ocr-queue-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = root.join("ocr.tsv");
        let screenshot = OcrCandidate {
            path: root.join("Screen\tshot\nDraft\r.png"),
            kind: OcrCandidateKind::ScreenshotImage,
            fingerprint: ExtractionFingerprint {
                extractor_version: OCR_EXTRACTOR_VERSION,
                len: 12,
                modified_ns: Some(34),
            },
        };
        let pdf = OcrCandidate {
            path: root.join("Scan.pdf"),
            kind: OcrCandidateKind::ImageOnlyPdf,
            fingerprint: ExtractionFingerprint {
                extractor_version: OCR_EXTRACTOR_VERSION,
                len: 56,
                modified_ns: None,
            },
        };

        OcrCandidateQueue::new([pdf.clone(), screenshot.clone()])
            .write(&store)
            .unwrap();
        let text = fs::read_to_string(&store).unwrap();
        let reloaded = OcrCandidateQueue::read(&store).unwrap();
        let candidates = reloaded.into_candidates();

        assert!(text.contains("gfm-ocr-candidates-v1"));
        assert!(text.contains("Screen\\tshot\\nDraft\\r.png"), "{text}");
        assert_eq!(candidates, vec![pdf, screenshot]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn candidate_queue_write_creates_parent_directory() {
        let root =
            std::env::temp_dir().join(format!("gfm-ocr-queue-parent-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let store = root.join("nested").join("ocr.tsv");
        let queue = OcrCandidateQueue::new([OcrCandidate {
            path: root.join("Screenshot.png"),
            kind: OcrCandidateKind::ScreenshotImage,
            fingerprint: ExtractionFingerprint {
                extractor_version: OCR_EXTRACTOR_VERSION,
                len: 1,
                modified_ns: None,
            },
        }]);

        queue.write(&store).unwrap();

        assert_eq!(OcrCandidateQueue::read(&store).unwrap().len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn candidate_queue_temp_paths_are_unique_within_process() {
        let first = queue_temp_path(Path::new("/tmp/ocr.tsv"));
        let second = queue_temp_path(Path::new("/tmp/ocr.tsv"));

        assert_ne!(first, second);
        assert_eq!(first.parent(), Some(Path::new("/tmp")));
        assert!(first
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".tmp")));
    }

    #[test]
    fn candidate_queue_write_cancellation_preserves_existing_store() {
        let root =
            std::env::temp_dir().join(format!("gfm-ocr-queue-cancel-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = root.join("ocr.tsv");
        fs::write(&store, "existing").unwrap();
        let queue = OcrCandidateQueue::new([OcrCandidate {
            path: root.join("Screenshot.png"),
            kind: OcrCandidateKind::ScreenshotImage,
            fingerprint: ExtractionFingerprint {
                extractor_version: OCR_EXTRACTOR_VERSION,
                len: 1,
                modified_ns: None,
            },
        }]);

        let mut checks = 0;
        let result = queue.write_checked(&store, || {
            checks += 1;
            if checks >= 3 {
                Err(gfm_types::GfmError::Cancelled)
            } else {
                Ok(())
            }
        });

        assert!(matches!(result, Err(gfm_types::GfmError::Cancelled)));
        assert_eq!(fs::read_to_string(&store).unwrap(), "existing");
        fs::remove_dir_all(root).unwrap();
    }
}
