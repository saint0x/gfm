use super::*;
use gfm_types::{FileId, GfmError, VolumeId};
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
    };

    let doc = Extractor::default()
        .extract_record(&record)
        .unwrap()
        .unwrap();

    assert_eq!(doc.text, "hello content index");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extraction_budget_profile_scales_by_volume_and_host_pressure() {
    let profile = ExtractionBudgetProfile {
        volume: ExtractionVolumeClass::Network,
        thermal: ExtractionThermalState::Serious,
        battery: ExtractionBatteryState::LowPower,
        user_activity: ExtractionUserActivity::Active,
    };

    let policy = profile.policy();

    assert_eq!(profile.scale_percent(), 50);
    assert_eq!(policy.max_bytes, 1024 * 1024);
    assert_eq!(policy.max_text_bytes, 1024 * 1024);
    assert_eq!(policy.max_pdf_bytes, 8 * 1024 * 1024);
    assert_eq!(policy.max_rich_text_bytes, 1024 * 1024);
    assert_eq!(policy.max_office_entries, 5_000);
}

#[test]
fn text_output_budget_truncates_without_splitting_utf8() {
    let root = unique_temp_dir("gfm-content-text-output-budget");
    let path = root.join("large.md");
    fs::write(&path, "alpha 東京 beta").unwrap();
    let extractor = Extractor::new(ExtractionPolicy {
        max_text_bytes: "alpha 東".len(),
        ..ExtractionPolicy::default()
    });

    let report = extractor.extract_path_report(&path).unwrap();
    let document = report.document.as_ref().unwrap();

    assert_eq!(report.status, ExtractionStatus::Extracted);
    assert_eq!(document.bytes_read, "alpha 東京 beta".len());
    assert_eq!(document.text, "alpha 東");
    assert_eq!(
            report.as_tsv(),
            format!(
                "extract\tpath={}\tformat=text\tstatus=extracted\treason=ok\tversion={TEXT_EXTRACTOR_VERSION}\tbytes-read={}\ttext-bytes={}",
                path.display(),
                "alpha 東京 beta".len(),
                "alpha 東".len()
            )
        );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pressure_budget_skips_large_text_before_reading_content() {
    let root = unique_temp_dir("gfm-content-pressure-budget");
    let path = root.join("large.txt");
    fs::write(&path, "x".repeat(1024 * 1024 + 1)).unwrap();
    let extractor = Extractor::with_budget_profile(ExtractionBudgetProfile {
        volume: ExtractionVolumeClass::Network,
        thermal: ExtractionThermalState::Serious,
        battery: ExtractionBatteryState::LowPower,
        user_activity: ExtractionUserActivity::Active,
    });

    let report = extractor.extract_path_report(&path).unwrap();

    assert_eq!(report.status, ExtractionStatus::Skipped("too-large"));
    assert!(report.document.is_none());
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
fn reports_versioned_pdf_extraction_fingerprints() {
    let root = unique_temp_dir("gfm-content-pdf-report");
    let path = root.join("brief.pdf");
    fs::write(&path, minimal_pdf("versioned pdfneedle")).unwrap();

    let report = Extractor::default().extract_path_report(&path).unwrap();

    assert_eq!(report.format, ExtractionFormat::Pdf);
    assert_eq!(report.status, ExtractionStatus::Extracted);
    assert_eq!(report.fingerprint.extractor_version, PDF_EXTRACTOR_VERSION);
    assert!(report
        .fingerprint
        .cache_key(&path)
        .starts_with(&format!("v{PDF_EXTRACTOR_VERSION}:")));
    assert!(report.as_tsv().contains("\tstatus=extracted\t"));
    assert!(report
        .document
        .unwrap()
        .text
        .contains("versioned pdfneedle"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extractor_versions_are_scoped_by_extraction_format() {
    assert_eq!(
        extractor_version_for_path(Path::new("note.txt")),
        TEXT_EXTRACTOR_VERSION
    );
    assert_eq!(
        extractor_version_for_path(Path::new("brief.pdf")),
        PDF_EXTRACTOR_VERSION
    );
    assert_eq!(
        extractor_version_for_path(Path::new("deck.pptx")),
        OFFICE_EXTRACTOR_VERSION
    );
    assert_eq!(
        extractor_version_for_path(Path::new("message.eml")),
        RICH_EXTRACTOR_VERSION
    );
    assert_eq!(
        extractor_version_for_path(Path::new("bundle.tar.gz")),
        ARCHIVE_EXTRACTOR_VERSION
    );
    assert_eq!(
        extractor_version_for_path(Path::new("data.json")),
        STRUCTURED_EXTRACTOR_VERSION
    );
    assert_eq!(
        extractor_version_for_path(Path::new("binary.unknown")),
        UNSUPPORTED_EXTRACTOR_VERSION
    );
    assert_ne!(TEXT_EXTRACTOR_VERSION, ARCHIVE_EXTRACTOR_VERSION);
    assert_ne!(RICH_EXTRACTOR_VERSION, TEXT_EXTRACTOR_VERSION);
}

#[test]
fn quarantines_repeated_corrupt_pdf_failures_by_content_fingerprint() {
    let root = unique_temp_dir("gfm-content-pdf-quarantine");
    let path = root.join("corrupt.pdf");
    fs::write(
        &path,
        b"%PDF-1.4
1 0 obj
<< /Type /Page /Contents 2 0 R >>
endobj
2 0 obj
<< /Length 12 /Filter /FlateDecode >>
stream
not-valid-zlib
endstream
endobj",
    )
    .unwrap();
    let extractor = Extractor::default();
    let mut quarantine = ExtractionQuarantine::new(2);

    let first = extractor.extract_path_report(&path).unwrap();
    assert_eq!(first.status, ExtractionStatus::Quarantined("corrupt-pdf"));
    assert_eq!(quarantine.record_report(&first), QuarantineDecision::Allow);
    let second = extractor.extract_path_report(&path).unwrap();
    let decision = quarantine.record_report(&second);

    assert!(matches!(decision, QuarantineDecision::Quarantined(_)));
    assert!(matches!(
        quarantine.before_extract(&path, &second.fingerprint),
        QuarantineDecision::Quarantined(_)
    ));
    assert!(decision.as_tsv().contains("\treason=corrupt-pdf\t"));
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

#[test]
fn cached_extractor_hits_for_unchanged_file_identity_and_signature() {
    let root = unique_temp_dir("gfm-content-cache-hit");
    let path = root.join("cache.md");
    fs::write(&path, "cached needle").unwrap();
    let record = record_for_path(&path);
    let mut cached = CachedExtractor::default();

    let first = cached.extract_record_report(&record).unwrap();
    let second = cached.extract_record_report(&record).unwrap();

    assert_eq!(first.status, ExtractionCacheStatus::Miss);
    assert_eq!(second.status, ExtractionCacheStatus::Hit);
    assert_eq!(first.key.extractor_version, TEXT_EXTRACTOR_VERSION);
    assert_eq!(first.key, second.key);
    assert_eq!(cached.cache_len(), 1);
    assert!(second.as_tsv().contains("status=hit"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extraction_cache_keys_use_format_scoped_versions() {
    let root = unique_temp_dir("gfm-content-cache-format-versions");
    let text_path = root.join("cache.md");
    let archive_path = root.join("bundle.zip");
    fs::write(&text_path, "cached text needle").unwrap();
    fs::write(
        &archive_path,
        zip_package(&[("docs/cacheneedle.txt", "payload")]),
    )
    .unwrap();

    let text_key = ExtractionCacheKey::for_record(&record_for_path(&text_path)).unwrap();
    let archive_key = ExtractionCacheKey::for_record(&record_for_path(&archive_path)).unwrap();

    assert_eq!(text_key.extractor_version, TEXT_EXTRACTOR_VERSION);
    assert_eq!(archive_key.extractor_version, ARCHIVE_EXTRACTOR_VERSION);
    assert_ne!(text_key.extractor_version, archive_key.extractor_version);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cached_extractor_misses_after_content_signature_changes() {
    let root = unique_temp_dir("gfm-content-cache-content-change");
    let path = root.join("cache.md");
    fs::write(&path, "cached needle").unwrap();
    let mut record = record_for_path(&path);
    let mut cached = CachedExtractor::default();

    let first = cached.extract_record_report(&record).unwrap();
    fs::write(&path, "cached changed needle").unwrap();
    record = record_for_path(&path);
    let second = cached.extract_record_report(&record).unwrap();

    assert_eq!(first.status, ExtractionCacheStatus::Miss);
    assert_eq!(second.status, ExtractionCacheStatus::Miss);
    assert_ne!(first.key.content, second.key.content);
    assert_eq!(cached.cache_len(), 2);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cached_extractor_misses_after_metadata_epoch_changes() {
    let root = unique_temp_dir("gfm-content-cache-metadata-change");
    let path = root.join("cache.md");
    fs::write(&path, "cached needle").unwrap();
    let mut record = record_for_path(&path);
    let mut cached = CachedExtractor::default();

    let first = cached.extract_record_report(&record).unwrap();
    record.xattrs_digest = record.xattrs_digest.wrapping_add(1);
    let second = cached.extract_record_report(&record).unwrap();

    assert_eq!(first.status, ExtractionCacheStatus::Miss);
    assert_eq!(second.status, ExtractionCacheStatus::Miss);
    assert_ne!(first.key.metadata_epoch, second.key.metadata_epoch);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn quarantine_blocks_repeated_timeout_failures() {
    let root = unique_temp_dir("gfm-content-timeout-quarantine");
    let path = root.join("slow.pdf");
    fs::write(&path, minimal_pdf("slow")).unwrap();
    let fingerprint = ExtractionFingerprint::for_path(&path).unwrap();
    let mut quarantine = ExtractionQuarantine::new(2);

    assert_eq!(
        quarantine.record_failure(
            &path,
            &fingerprint,
            QuarantineFailureKind::Timeout,
            "worker-timeout"
        ),
        QuarantineDecision::Allow
    );
    let blocked = quarantine.record_failure(
        &path,
        &fingerprint,
        QuarantineFailureKind::Timeout,
        "worker-timeout",
    );

    assert!(matches!(blocked, QuarantineDecision::Quarantined(_)));
    assert!(blocked.as_tsv().contains("\treason=worker-timeout\t"));
    assert!(matches!(
        quarantine.before_extract(&path, &fingerprint),
        QuarantineDecision::Quarantined(_)
    ));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn quarantine_persists_crash_failures_across_restart() {
    let root = unique_temp_dir("gfm-content-crash-quarantine");
    let path = root.join("crash.docx");
    let store = root.join("quarantine.gfmquarantine");
    fs::write(
        &path,
        ooxml_package(&[("word/document.xml", "<w:t>crash</w:t>")]),
    )
    .unwrap();
    let fingerprint = ExtractionFingerprint::for_path(&path).unwrap();
    let mut quarantine = ExtractionQuarantine::new(1);
    let blocked = quarantine.record_failure(
        &path,
        &fingerprint,
        QuarantineFailureKind::Crash,
        "worker-crash",
    );

    assert!(matches!(blocked, QuarantineDecision::Quarantined(_)));
    quarantine.write(&store).unwrap();
    let reloaded = ExtractionQuarantine::read(&store).unwrap();

    assert!(matches!(
        reloaded.before_extract(&path, &fingerprint),
        QuarantineDecision::Quarantined(_)
    ));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extraction_quarantine_checked_read_honors_pre_cancelled_control_before_file_open() {
    let root = unique_temp_dir("gfm-content-quarantine-read-cancel");
    let store = root.join("quarantine.gfmquarantine");

    let result = ExtractionQuarantine::read_checked(&store, || Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(!store.exists());
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

fn record_for_path(path: &Path) -> FileRecord {
    let metadata = fs::metadata(path).unwrap();
    FileRecord {
        id: FileId::new(VolumeId(1), 1),
        parent: None,
        path: path.to_path_buf(),
        name: path.file_name().unwrap().to_string_lossy().into_owned(),
        kind: FileKind::File,
        len: metadata.len(),
        mode: 0,
        owner: 0,
        group: 0,
        xattrs_digest: 0,
        created: metadata.created().ok(),
        modified: metadata.modified().ok(),
        changed: metadata.modified().ok(),
        hidden: false,
        tags: Vec::new(),
        finder_comment: None,
    }
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
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, text) in parts {
        writer.start_file(*name, options).unwrap();
        writer.write_all(text.as_bytes()).unwrap();
    }
    writer.finish().unwrap().into_inner()
}
