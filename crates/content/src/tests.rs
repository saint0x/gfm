use super::*;
use bzip2::write::BzEncoder;
use flate2::{
    write::{GzEncoder, ZlibEncoder},
    Compression,
};
use gfm_types::{FileId, GfmError, VolumeId};
use std::fs;
use std::io::{Cursor, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use xz2::write::XzEncoder;
use zip::write::SimpleFileOptions;

const TEST_OLE_FREESECT: u32 = 0xFFFF_FFFF;
const TEST_OLE_ENDOFCHAIN: u32 = 0xFFFF_FFFE;
const TEST_OLE_FATSECT: u32 = 0xFFFF_FFFD;
const TEST_OLE_DIFSECT: u32 = 0xFFFF_FFFC;

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
fn extraction_report_tsv_escapes_path_and_reason_control_characters() {
    let report = ExtractionReport {
        path: PathBuf::from("/tmp/Reports\tQ3\nDraft\rFinal.md"),
        format: ExtractionFormat::Text,
        status: ExtractionStatus::Quarantined("worker\ttimeout\nwhile\rreading"),
        fingerprint: ExtractionFingerprint {
            extractor_version: TEXT_EXTRACTOR_VERSION,
            len: 12,
            modified_ns: None,
        },
        document: None,
    };
    let tsv = report.as_tsv();

    assert_eq!(tsv.lines().count(), 1, "{tsv}");
    assert!(!tsv.contains('\r'), "{tsv}");
    assert!(
        tsv.contains("path=/tmp/Reports\\tQ3\\nDraft\\rFinal.md"),
        "{tsv}"
    );
    assert!(
        tsv.contains("reason=worker\\ttimeout\\nwhile\\rreading"),
        "{tsv}"
    );
    assert_eq!(tsv.split('\t').count(), 8, "{tsv}");
}

#[test]
fn extraction_report_checked_honors_pre_cancelled_control_before_metadata_probe() {
    let root = unique_temp_dir("gfm-content-extract-report-pre-cancel");
    let path = root.join("missing.md");

    let result =
        Extractor::default().extract_path_report_checked(&path, || Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(!path.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extraction_report_checked_honors_cancellation_before_reading_content_bytes() {
    let root = unique_temp_dir("gfm-content-extract-report-read-cancel");
    let path = root.join("note.md");
    fs::write(&path, "content that should not be indexed").unwrap();
    let mut checks = 0usize;

    let result = Extractor::default().extract_path_report_checked(&path, || {
        checks += 1;
        if checks == 6 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert!(matches!(result, Err(GfmError::Cancelled)));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extraction_report_checked_can_cancel_while_normalizing_plain_text() {
    let root = unique_temp_dir("gfm-content-text-normalize-cancel");
    let path = root.join("large.md");
    fs::write(&path, "plain text needle ".repeat(64 * 1024)).unwrap();
    let mut checks = 0usize;

    let result = Extractor::default().extract_path_report_checked(&path, || {
        checks += 1;
        if checks >= 512 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert!(matches!(result, Err(GfmError::Cancelled)));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extraction_report_checked_can_cancel_while_reading_ooxml_entry() {
    let root = unique_temp_dir("gfm-content-ooxml-entry-cancel");
    let path = root.join("large.docx");
    let body = format!("<w:t>{}</w:t>", "large body ".repeat(16 * 1024));
    fs::write(
        &path,
        ooxml_package(&[("word/document.xml", body.as_str())]),
    )
    .unwrap();
    let mut checks = 0usize;

    let result = Extractor::default().extract_path_report_checked(&path, || {
        checks += 1;
        if checks >= 15 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert!(matches!(result, Err(GfmError::Cancelled)));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extraction_report_checked_can_cancel_while_decoding_tar_gz_archive() {
    let root = unique_temp_dir("gfm-content-targz-decode-cancel");
    let path = root.join("large.tar.gz");
    fs::write(
        &path,
        tar_gz_package(&[("large.txt", &"payload ".repeat(128 * 1024))]),
    )
    .unwrap();
    let mut checks = 0usize;

    let result = Extractor::default().extract_path_report_checked(&path, || {
        checks += 1;
        if checks >= 15 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert!(matches!(result, Err(GfmError::Cancelled)));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extracts_bzip2_and_xz_tar_metadata_through_public_report_path() {
    let root = unique_temp_dir("gfm-content-compressed-tar-archives");
    let tar_bz2 = root.join("bundle.tbz2");
    let tar_xz = root.join("bundle.txz");
    fs::write(
        &tar_bz2,
        tar_bz2_package(&[("docs/tbz2-public-needle.txt", "body")]),
    )
    .unwrap();
    fs::write(
        &tar_xz,
        tar_xz_package(&[("docs/txz-public-needle.txt", "body")]),
    )
    .unwrap();

    let extractor = Extractor::default();
    let bzip = extractor.extract_path_report(&tar_bz2).unwrap();
    let xz = extractor.extract_path_report(&tar_xz).unwrap();

    assert_eq!(bzip.format, ExtractionFormat::Archive);
    assert_eq!(xz.format, ExtractionFormat::Archive);
    assert_eq!(bzip.status, ExtractionStatus::Extracted);
    assert_eq!(xz.status, ExtractionStatus::Extracted);
    assert_eq!(
        bzip.fingerprint.extractor_version,
        ARCHIVE_EXTRACTOR_VERSION
    );
    assert_eq!(xz.fingerprint.extractor_version, ARCHIVE_EXTRACTOR_VERSION);
    assert!(bzip
        .document
        .as_ref()
        .unwrap()
        .text
        .contains("docs/tbz2-public-needle.txt"));
    assert!(xz
        .document
        .as_ref()
        .unwrap()
        .text
        .contains("docs/txz-public-needle.txt"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extracts_single_stream_archive_metadata_through_public_report_path() {
    let root = unique_temp_dir("gfm-content-compressed-stream-archives");
    let gzip = root.join("payload.gz");
    let bzip = root.join("payload.bz2");
    let xz = root.join("payload.xz");
    fs::write(&gzip, gzip_stream("gzip public body")).unwrap();
    fs::write(&bzip, bzip2_stream("bzip public body")).unwrap();
    fs::write(&xz, xz_stream("xz public body")).unwrap();

    let extractor = Extractor::default();
    let gzip = extractor.extract_path_report(&gzip).unwrap();
    let bzip = extractor.extract_path_report(&bzip).unwrap();
    let xz = extractor.extract_path_report(&xz).unwrap();

    assert_eq!(gzip.format, ExtractionFormat::Archive);
    assert_eq!(bzip.format, ExtractionFormat::Archive);
    assert_eq!(xz.format, ExtractionFormat::Archive);
    assert_eq!(gzip.status, ExtractionStatus::Extracted);
    assert_eq!(bzip.status, ExtractionStatus::Extracted);
    assert_eq!(xz.status, ExtractionStatus::Extracted);
    assert!(gzip.document.unwrap().text.contains("gzip-stream"));
    assert!(bzip.document.unwrap().text.contains("bzip2-stream"));
    assert!(xz.document.unwrap().text.contains("xz-stream"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn quarantines_encrypted_zip_archive_without_reporting_corruption() {
    let root = unique_temp_dir("gfm-content-encrypted-zip");
    let path = root.join("locked.zip");
    fs::write(
        &path,
        encrypted_zip_package(&[("docs/secret.txt", "payload")]),
    )
    .unwrap();
    let mut quarantine = ExtractionQuarantine::new(1);

    let report = Extractor::default().extract_path_report(&path).unwrap();
    let decision = quarantine.record_report(&report);

    assert_eq!(report.format, ExtractionFormat::Archive);
    assert_eq!(
        report.status,
        ExtractionStatus::Quarantined("encrypted-archive")
    );
    assert!(report.document.is_none());
    assert!(matches!(decision, QuarantineDecision::Quarantined(_)));
    assert!(decision.as_tsv().contains("\treason=encrypted-archive\t"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extracts_bounded_rar_and_7z_metadata_through_public_report_path() {
    let root = unique_temp_dir("gfm-content-rar-7z-metadata");
    let rar = root.join("bundle.rar");
    let sevenzip = root.join("bundle.7z");
    fs::write(&rar, rar4_package(&[("docs/rar-needle.txt", 12)])).unwrap();
    fs::write(&sevenzip, sevenzip_package(&["docs/7z-needle.txt"])).unwrap();

    let extractor = Extractor::default();
    let rar_report = extractor.extract_path_report(&rar).unwrap();
    let sevenzip_report = extractor.extract_path_report(&sevenzip).unwrap();

    assert_eq!(rar_report.format, ExtractionFormat::Archive);
    assert_eq!(sevenzip_report.format, ExtractionFormat::Archive);
    assert_eq!(rar_report.status, ExtractionStatus::Extracted);
    assert_eq!(sevenzip_report.status, ExtractionStatus::Extracted);
    assert_eq!(
        rar_report.fingerprint.extractor_version,
        ARCHIVE_EXTRACTOR_VERSION
    );
    assert_eq!(
        sevenzip_report.fingerprint.extractor_version,
        ARCHIVE_EXTRACTOR_VERSION
    );
    assert!(rar_report
        .document
        .as_ref()
        .unwrap()
        .text
        .contains("docs/rar-needle.txt 12 bytes"));
    assert!(sevenzip_report
        .document
        .as_ref()
        .unwrap()
        .text
        .contains("docs/7z-needle.txt"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extracts_rar5_metadata_through_public_report_path() {
    let root = unique_temp_dir("gfm-content-rar5-metadata");
    let path = root.join("bundle.rar");
    fs::write(
        &path,
        rar5_package(&[("docs/rar5-needle.txt", 19), ("media/image.png", 4096)]),
    )
    .unwrap();

    let report = Extractor::default().extract_path_report(&path).unwrap();

    assert_eq!(report.format, ExtractionFormat::Archive);
    assert_eq!(report.status, ExtractionStatus::Extracted);
    let text = &report.document.as_ref().unwrap().text;
    assert!(text.contains("docs/rar5-needle.txt 19 bytes"), "{text}");
    assert!(text.contains("media/image.png 4096 bytes"), "{text}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn quarantines_rar5_encrypted_headers_without_reporting_corruption() {
    let root = unique_temp_dir("gfm-content-rar5-encrypted");
    let path = root.join("locked.rar");
    fs::write(&path, rar5_encrypted_header_package()).unwrap();
    let mut quarantine = ExtractionQuarantine::new(1);

    let report = Extractor::default().extract_path_report(&path).unwrap();
    let decision = quarantine.record_report(&report);

    assert_eq!(report.format, ExtractionFormat::Archive);
    assert_eq!(
        report.status,
        ExtractionStatus::Quarantined("encrypted-archive")
    );
    assert!(report.document.is_none());
    assert!(matches!(decision, QuarantineDecision::Quarantined(_)));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn skips_rar5_multi_volume_archives_until_spanning_import_lands() {
    let root = unique_temp_dir("gfm-content-rar5-volume");
    let path = root.join("part1.rar");
    fs::write(&path, rar5_multivolume_package()).unwrap();

    let report = Extractor::default().extract_path_report(&path).unwrap();

    assert_eq!(report.format, ExtractionFormat::Archive);
    assert_eq!(
        report.status,
        ExtractionStatus::Skipped("unsupported-archive")
    );
    assert!(report.document.is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn quarantines_rar5_file_headers_with_corrupt_extra_area() {
    let root = unique_temp_dir("gfm-content-rar5-corrupt-file-extra");
    let path = root.join("corrupt-extra.rar");
    fs::write(&path, rar5_corrupt_file_extra_package()).unwrap();

    let report = Extractor::default().extract_path_report(&path).unwrap();

    assert_eq!(report.format, ExtractionFormat::Archive);
    assert_eq!(
        report.status,
        ExtractionStatus::Quarantined("corrupt-archive")
    );
    assert!(report.document.is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn quarantines_rar5_service_headers_with_corrupt_extra_area() {
    let root = unique_temp_dir("gfm-content-rar5-corrupt-service-extra");
    let path = root.join("corrupt-service-extra.rar");
    fs::write(&path, rar5_corrupt_service_extra_package()).unwrap();

    let report = Extractor::default().extract_path_report(&path).unwrap();

    assert_eq!(report.format, ExtractionFormat::Archive);
    assert_eq!(
        report.status,
        ExtractionStatus::Quarantined("corrupt-archive")
    );
    assert!(report.document.is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn quarantines_rar5_oversized_variable_integer_header_without_panicking() {
    let root = unique_temp_dir("gfm-content-rar5-oversized-vint");
    let path = root.join("oversized-vint.rar");
    fs::write(&path, rar5_oversized_vint_header_package()).unwrap();

    let report = Extractor::default().extract_path_report(&path).unwrap();

    assert_eq!(report.format, ExtractionFormat::Archive);
    assert_eq!(
        report.status,
        ExtractionStatus::Quarantined("corrupt-archive")
    );
    assert!(report.document.is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn quarantines_encrypted_7z_header_without_reporting_corruption() {
    let root = unique_temp_dir("gfm-content-7z-encrypted-header");
    let path = root.join("locked.7z");
    fs::write(
        &path,
        sevenzip_encoded_header_package(&[0x06, 0xf1, 0x07, 0x01]),
    )
    .unwrap();
    let mut quarantine = ExtractionQuarantine::new(1);

    let report = Extractor::default().extract_path_report(&path).unwrap();
    let decision = quarantine.record_report(&report);

    assert_eq!(report.format, ExtractionFormat::Archive);
    assert_eq!(
        report.status,
        ExtractionStatus::Quarantined("encrypted-archive")
    );
    assert!(report.document.is_none());
    assert!(matches!(decision, QuarantineDecision::Quarantined(_)));
    assert!(decision.as_tsv().contains("\treason=encrypted-archive\t"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn skips_non_encrypted_7z_encoded_header_without_reporting_corruption() {
    let root = unique_temp_dir("gfm-content-7z-encoded-header");
    let path = root.join("encoded.7z");
    fs::write(&path, sevenzip_encoded_header_package(&[0x03, 0x01, 0x01])).unwrap();

    let report = Extractor::default().extract_path_report(&path).unwrap();

    assert_eq!(report.format, ExtractionFormat::Archive);
    assert_eq!(
        report.status,
        ExtractionStatus::Skipped("unsupported-archive")
    );
    assert!(report.document.is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn skips_real_7zz_encoded_header_fixture_without_reporting_corruption() {
    let root = unique_temp_dir("gfm-content-real-7zz-encoded-header");
    let path = root.join("encoded.7z");
    fs::write(
        &path,
        include_bytes!("../fixtures/archive/encoded-header-7zz.7z"),
    )
    .unwrap();

    let report = Extractor::default().extract_path_report(&path).unwrap();

    assert_eq!(report.format, ExtractionFormat::Archive);
    assert_eq!(
        report.status,
        ExtractionStatus::Skipped("unsupported-archive")
    );
    assert!(report.document.is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extracts_real_7zz_metadata_fixture_through_public_report_path() {
    let root = unique_temp_dir("gfm-content-real-7zz-metadata");
    let path = root.join("plain.7z");
    fs::write(&path, include_bytes!("../fixtures/archive/plain-7zz.7z")).unwrap();

    let report = Extractor::default().extract_path_report(&path).unwrap();

    assert_eq!(report.format, ExtractionFormat::Archive);
    assert_eq!(report.status, ExtractionStatus::Extracted);
    assert!(report
        .document
        .as_ref()
        .unwrap()
        .text
        .contains("fixture-needle.txt"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn skips_real_split_7zz_volumes_without_reporting_corruption() {
    let root = unique_temp_dir("gfm-content-real-7zz-split-volume");
    let first = root.join("split.7z.001");
    let second = root.join("split.7z.002");
    fs::write(
        &first,
        include_bytes!("../fixtures/archive/split-7zz.7z.001"),
    )
    .unwrap();
    fs::write(
        &second,
        include_bytes!("../fixtures/archive/split-7zz.7z.002"),
    )
    .unwrap();

    for path in [&first, &second] {
        let report = Extractor::default().extract_path_report(path).unwrap();

        assert_eq!(report.format, ExtractionFormat::Archive);
        assert_eq!(
            report.status,
            ExtractionStatus::Skipped("unsupported-archive")
        );
        assert!(report.document.is_none());
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn quarantines_real_7zz_encrypted_header_fixture_without_reporting_corruption() {
    let root = unique_temp_dir("gfm-content-real-7zz-encrypted-header");
    let path = root.join("encrypted.7z");
    fs::write(
        &path,
        include_bytes!("../fixtures/archive/encrypted-header-7zz.7z"),
    )
    .unwrap();
    let mut quarantine = ExtractionQuarantine::new(1);

    let report = Extractor::default().extract_path_report(&path).unwrap();
    let decision = quarantine.record_report(&report);

    assert_eq!(report.format, ExtractionFormat::Archive);
    assert_eq!(
        report.status,
        ExtractionStatus::Quarantined("encrypted-archive")
    );
    assert!(report.document.is_none());
    assert!(matches!(decision, QuarantineDecision::Quarantined(_)));
    assert!(decision.as_tsv().contains("\treason=encrypted-archive\t"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn quarantines_corrupt_rar_and_7z_archives() {
    for extension in ["rar", "7z"] {
        let root = unique_temp_dir(&format!("gfm-content-corrupt-archive-{extension}"));
        let path = root.join(format!("payload.{extension}"));
        fs::write(&path, b"not an archive").unwrap();
        let mut quarantine = ExtractionQuarantine::new(1);

        let report = Extractor::default().extract_path_report(&path).unwrap();
        let decision = quarantine.record_report(&report);

        assert_eq!(report.format, ExtractionFormat::Archive);
        assert_eq!(
            report.status,
            ExtractionStatus::Quarantined("corrupt-archive")
        );
        assert_eq!(
            report.fingerprint.extractor_version,
            ARCHIVE_EXTRACTOR_VERSION
        );
        assert!(report.document.is_none());
        assert!(matches!(decision, QuarantineDecision::Quarantined(_)));
        assert!(decision.as_tsv().contains("\treason=corrupt-archive\t"));
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn applies_archive_byte_budget_to_unsupported_archive_formats() {
    let root = unique_temp_dir("gfm-content-unsupported-archive-budget");
    let path = root.join("large.7z");
    fs::write(
        &path,
        [sevenzip_package(&["docs/budget.txt"]), vec![0_u8; 128]].concat(),
    )
    .unwrap();
    let extractor = Extractor::new(ExtractionPolicy {
        max_archive_bytes: 16,
        ..ExtractionPolicy::default()
    });

    let report = extractor.extract_path_report(&path).unwrap();

    assert_eq!(report.format, ExtractionFormat::Archive);
    assert_eq!(report.status, ExtractionStatus::Skipped("too-large"));
    assert!(report.document.is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extraction_fingerprint_checked_honors_pre_cancelled_control_before_metadata_probe() {
    let root = unique_temp_dir("gfm-content-fingerprint-pre-cancel");
    let path = root.join("missing.md");

    let result = ExtractionFingerprint::for_path_checked(&path, || Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(!path.exists());
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
fn snippet_for_record_checked_can_cancel_while_lowercasing_document_text() {
    let root = unique_temp_dir("gfm-content-snippet-cancel");
    let path = root.join("large.md");
    fs::write(
        &path,
        format!("{} exact snippet marker", "before ".repeat(64 * 1024)),
    )
    .unwrap();
    let record = record_for_path(&path);
    let mut checks = 0usize;

    let result = Extractor::default().snippet_for_record_checked(
        &record,
        &["marker".to_string()],
        &[],
        8,
        || {
            checks += 1;
            if checks >= 512 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        },
    );

    assert!(matches!(result, Err(GfmError::Cancelled)));
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
fn extraction_report_checked_can_cancel_while_inflating_compressed_pdf_stream() {
    let root = unique_temp_dir("gfm-content-pdf-flate-cancel");
    let path = root.join("large.pdf");
    fs::write(&path, compressed_pdf(&"pdf body ".repeat(32 * 1024))).unwrap();
    let mut checks = 0usize;

    let result = Extractor::default().extract_path_report_checked(&path, || {
        checks += 1;
        if checks >= 15 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert!(matches!(result, Err(GfmError::Cancelled)));
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
fn reports_image_only_pdf_without_indexing_empty_text() {
    let root = unique_temp_dir("gfm-content-image-only-pdf");
    let path = root.join("scan.pdf");
    fs::write(&path, image_only_pdf()).unwrap();

    let report = Extractor::default().extract_path_report(&path).unwrap();

    assert_eq!(report.format, ExtractionFormat::Pdf);
    assert_eq!(report.status, ExtractionStatus::Skipped("image-only-pdf"));
    assert_eq!(report.fingerprint.extractor_version, PDF_EXTRACTOR_VERSION);
    assert!(report.document.is_none());
    assert!(report
        .fingerprint
        .cache_key(&path)
        .starts_with(&format!("v{PDF_EXTRACTOR_VERSION}:")));
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
        extractor_version_for_path(Path::new("legacy.DOC")),
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
        extractor_version_for_path(Path::new("compressed.7z")),
        ARCHIVE_EXTRACTOR_VERSION
    );
    assert_eq!(
        extractor_version_for_path(Path::new("compressed.7z.001")),
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
fn classifies_legacy_office_as_bounded_skipped_office_content() {
    for (extension, stream) in [
        ("doc", "WordDocument"),
        ("xls", "Workbook"),
        ("ppt", "PowerPoint Document"),
    ] {
        let root = unique_temp_dir(&format!("gfm-content-legacy-office-{extension}"));
        let path = root.join(format!("legacy.{extension}"));
        fs::write(&path, legacy_office_bytes(&[stream])).unwrap();

        let report = Extractor::default().extract_path_report(&path).unwrap();

        assert_eq!(report.format, ExtractionFormat::Office);
        assert_eq!(report.status, ExtractionStatus::Skipped("legacy-office"));
        assert_eq!(
            report.fingerprint.extractor_version,
            OFFICE_EXTRACTOR_VERSION
        );
        assert!(report.document.is_none());
        assert!(report
            .fingerprint
            .cache_key(&path)
            .starts_with(&format!("v{OFFICE_EXTRACTOR_VERSION}:")));
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn extracts_legacy_doc_text_from_worddocument_stream() {
    let root = unique_temp_dir("gfm-content-legacy-doc-text");
    let path = root.join("brief.doc");
    fs::write(
        &path,
        legacy_office_bytes_with_stream("WordDocument", b"\0\0legacydocneedle launch plan\0\0"),
    )
    .unwrap();

    let report = Extractor::default().extract_path_report(&path).unwrap();

    assert_eq!(report.format, ExtractionFormat::Office);
    assert_eq!(report.status, ExtractionStatus::Extracted);
    let document = report.document.unwrap();
    assert!(document.bytes_read > 0);
    assert!(document.text.contains("legacydocneedle launch plan"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extracts_legacy_doc_text_when_fat_sector_is_reached_through_difat() {
    let root = unique_temp_dir("gfm-content-legacy-doc-difat");
    let path = root.join("brief.doc");
    fs::write(
        &path,
        legacy_office_bytes_with_difat_stream("WordDocument", b"\0\0legacydifatneedle launch plan"),
    )
    .unwrap();

    let report = Extractor::default().extract_path_report(&path).unwrap();

    assert_eq!(report.format, ExtractionFormat::Office);
    assert_eq!(report.status, ExtractionStatus::Extracted);
    let document = report.document.unwrap();
    assert!(document.bytes_read > 0);
    assert!(document.text.contains("legacydifatneedle launch plan"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extracts_legacy_office_utf16_text_from_required_streams() {
    for (extension, stream, needle) in [
        ("xls", "Workbook", "legacyxlsneedle"),
        ("ppt", "PowerPoint Document", "legacypptneedle"),
    ] {
        let root = unique_temp_dir(&format!("gfm-content-legacy-{extension}-utf16"));
        let path = root.join(format!("brief.{extension}"));
        let payload = format!("{needle} quarterly plan")
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        fs::write(&path, legacy_office_bytes_with_stream(stream, &payload)).unwrap();

        let report = Extractor::default().extract_path_report(&path).unwrap();

        assert_eq!(report.format, ExtractionFormat::Office);
        assert_eq!(report.status, ExtractionStatus::Extracted);
        assert!(report.document.unwrap().text.contains(needle));
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn quarantines_encrypted_ooxml_without_reporting_corruption() {
    let root = unique_temp_dir("gfm-content-encrypted-office");
    let path = root.join("locked.docx");
    fs::write(
        &path,
        legacy_office_bytes(&["WordDocument", "EncryptionInfo"]),
    )
    .unwrap();
    let mut quarantine = ExtractionQuarantine::new(1);

    let report = Extractor::default().extract_path_report(&path).unwrap();
    let decision = quarantine.record_report(&report);

    assert_eq!(report.format, ExtractionFormat::Office);
    assert_eq!(
        report.status,
        ExtractionStatus::Quarantined("encrypted-office")
    );
    assert_eq!(
        report.fingerprint.extractor_version,
        OFFICE_EXTRACTOR_VERSION
    );
    assert!(report.document.is_none());
    assert!(matches!(decision, QuarantineDecision::Quarantined(_)));
    assert!(decision.as_tsv().contains("\treason=encrypted-office\t"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn quarantines_encrypted_legacy_office_without_reporting_corruption() {
    let root = unique_temp_dir("gfm-content-encrypted-legacy-office");
    let path = root.join("locked.doc");
    fs::write(
        &path,
        legacy_office_bytes(&["WordDocument", "EncryptionInfo"]),
    )
    .unwrap();
    let mut quarantine = ExtractionQuarantine::new(1);

    let report = Extractor::default().extract_path_report(&path).unwrap();
    let decision = quarantine.record_report(&report);

    assert_eq!(report.format, ExtractionFormat::Office);
    assert_eq!(
        report.status,
        ExtractionStatus::Quarantined("encrypted-office")
    );
    assert_eq!(
        report.fingerprint.extractor_version,
        OFFICE_EXTRACTOR_VERSION
    );
    assert!(report.document.is_none());
    assert!(matches!(decision, QuarantineDecision::Quarantined(_)));
    assert!(decision.as_tsv().contains("\treason=encrypted-office\t"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn quarantines_legacy_doc_fib_encryption_flags_without_reporting_corruption() {
    let root = unique_temp_dir("gfm-content-encrypted-legacy-doc-fib");
    let path = root.join("locked.doc");
    let mut payload = vec![0_u8; 16];
    payload[0x00..0x02].copy_from_slice(&0xa5ec_u16.to_le_bytes());
    payload[0x0a..0x0c].copy_from_slice(&(1_u16 << 8).to_le_bytes());
    fs::write(
        &path,
        legacy_office_bytes_with_stream("WordDocument", &payload),
    )
    .unwrap();
    let mut quarantine = ExtractionQuarantine::new(1);

    let report = Extractor::default().extract_path_report(&path).unwrap();
    let decision = quarantine.record_report(&report);

    assert_eq!(report.format, ExtractionFormat::Office);
    assert_eq!(
        report.status,
        ExtractionStatus::Quarantined("encrypted-office")
    );
    assert!(report.document.is_none());
    assert!(matches!(decision, QuarantineDecision::Quarantined(_)));
    assert!(decision.as_tsv().contains("\treason=encrypted-office\t"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn quarantines_legacy_xls_filepass_records_without_reporting_corruption() {
    let root = unique_temp_dir("gfm-content-encrypted-legacy-xls-filepass");
    let path = root.join("locked.xls");
    let payload = [
        0x09, 0x08, 0x00, 0x00, // BOF with no body in this minimal stream.
        0x2f, 0x00, 0x00, 0x00, // FILEPASS with no body.
    ];
    fs::write(&path, legacy_office_bytes_with_stream("Workbook", &payload)).unwrap();
    let mut quarantine = ExtractionQuarantine::new(1);

    let report = Extractor::default().extract_path_report(&path).unwrap();
    let decision = quarantine.record_report(&report);

    assert_eq!(report.format, ExtractionFormat::Office);
    assert_eq!(
        report.status,
        ExtractionStatus::Quarantined("encrypted-office")
    );
    assert!(report.document.is_none());
    assert!(matches!(decision, QuarantineDecision::Quarantined(_)));
    assert!(decision.as_tsv().contains("\treason=encrypted-office\t"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn quarantines_case_varied_legacy_office_encryption_streams() {
    let root = unique_temp_dir("gfm-content-case-varied-encrypted-legacy-office");
    let path = root.join("locked.doc");
    fs::write(
        &path,
        legacy_office_bytes(&["WordDocument", "encryptioninfo"]),
    )
    .unwrap();

    let report = Extractor::default().extract_path_report(&path).unwrap();

    assert_eq!(report.format, ExtractionFormat::Office);
    assert_eq!(
        report.status,
        ExtractionStatus::Quarantined("encrypted-office")
    );
    assert!(report.document.is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn quarantines_control_prefixed_legacy_office_dataspaces_storage() {
    let root = unique_temp_dir("gfm-content-dataspaces-encrypted-legacy-office");
    let path = root.join("locked.doc");
    fs::write(
        &path,
        legacy_office_bytes(&["WordDocument", "\u{0006}DataSpaces"]),
    )
    .unwrap();

    let report = Extractor::default().extract_path_report(&path).unwrap();

    assert_eq!(report.format, ExtractionFormat::Office);
    assert_eq!(
        report.status,
        ExtractionStatus::Quarantined("encrypted-office")
    );
    assert!(report.document.is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn quarantines_corrupt_legacy_office_compound_file_without_required_stream() {
    let root = unique_temp_dir("gfm-content-corrupt-legacy-office");
    let path = root.join("bad.doc");
    fs::write(&path, legacy_office_bytes(&["NotOffice"])).unwrap();
    let mut quarantine = ExtractionQuarantine::new(1);

    let report = Extractor::default().extract_path_report(&path).unwrap();
    let decision = quarantine.record_report(&report);

    assert_eq!(report.format, ExtractionFormat::Office);
    assert_eq!(
        report.status,
        ExtractionStatus::Quarantined("corrupt-office")
    );
    assert!(report.document.is_none());
    assert!(matches!(decision, QuarantineDecision::Quarantined(_)));
    assert!(decision.as_tsv().contains("\treason=corrupt-office\t"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn quarantines_encrypted_ooxml_zip_entry_without_reporting_corruption() {
    let root = unique_temp_dir("gfm-content-encrypted-ooxml-zip");
    let path = root.join("locked.docx");
    fs::write(
        &path,
        encrypted_zip_package(&[(
            "word/document.xml",
            "<w:document><w:body><w:p><w:r><w:t>secret</w:t></w:r></w:p></w:body></w:document>",
        )]),
    )
    .unwrap();
    let mut quarantine = ExtractionQuarantine::new(1);

    let report = Extractor::default().extract_path_report(&path).unwrap();
    let decision = quarantine.record_report(&report);

    assert_eq!(report.format, ExtractionFormat::Office);
    assert_eq!(
        report.status,
        ExtractionStatus::Quarantined("encrypted-office")
    );
    assert!(report.document.is_none());
    assert!(matches!(decision, QuarantineDecision::Quarantined(_)));
    assert!(decision.as_tsv().contains("\treason=encrypted-office\t"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn quarantines_malformed_ooxml_package_without_required_part() {
    for extension in ["docx", "xlsx", "pptx"] {
        let root = unique_temp_dir(&format!("gfm-content-malformed-office-{extension}"));
        let path = root.join(format!("not-office.{extension}"));
        fs::write(
            &path,
            ooxml_package(&[("docs/payload.txt", "not actually an office package")]),
        )
        .unwrap();
        let mut quarantine = ExtractionQuarantine::new(1);

        let report = Extractor::default().extract_path_report(&path).unwrap();
        let decision = quarantine.record_report(&report);

        assert_eq!(report.format, ExtractionFormat::Office);
        assert_eq!(
            report.status,
            ExtractionStatus::Quarantined("corrupt-office")
        );
        assert!(report.document.is_none());
        assert!(matches!(decision, QuarantineDecision::Quarantined(_)));
        assert!(decision.as_tsv().contains("\treason=corrupt-office\t"));
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn applies_office_byte_budget_to_legacy_office() {
    let root = unique_temp_dir("gfm-content-legacy-office-budget");
    let path = root.join("large.DOC");
    fs::write(
        &path,
        [legacy_office_bytes(&["WordDocument"]), vec![0_u8; 128]].concat(),
    )
    .unwrap();
    let extractor = Extractor::new(ExtractionPolicy {
        max_office_bytes: 16,
        ..ExtractionPolicy::default()
    });

    let report = extractor.extract_path_report(&path).unwrap();

    assert_eq!(report.format, ExtractionFormat::Office);
    assert_eq!(report.status, ExtractionStatus::Skipped("too-large"));
    assert!(report.document.is_none());
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
fn extraction_report_checked_can_cancel_while_parsing_html_rich_text() {
    let root = unique_temp_dir("gfm-content-html-cancel");
    let path = root.join("large.html");
    fs::write(
        &path,
        format!(
            "<html><body>{}</body></html>",
            "<p>htmlneedle</p>".repeat(4096)
        ),
    )
    .unwrap();
    let mut checks = 0usize;

    let result = Extractor::default().extract_path_report_checked(&path, || {
        checks += 1;
        if checks >= 512 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert!(matches!(result, Err(GfmError::Cancelled)));
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
fn extraction_report_checked_can_cancel_while_parsing_rtf_rich_text() {
    let root = unique_temp_dir("gfm-content-rtf-cancel");
    let path = root.join("large.rtf");
    fs::write(
        &path,
        format!(r"{{\rtf1\ansi {}}}", r"rtfneedle\par ".repeat(4096)),
    )
    .unwrap();
    let mut checks = 0usize;

    let result = Extractor::default().extract_path_report_checked(&path, || {
        checks += 1;
        if checks >= 512 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert!(matches!(result, Err(GfmError::Cancelled)));
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
fn extraction_report_checked_can_cancel_while_decoding_email_rich_text() {
    let root = unique_temp_dir("gfm-content-email-cancel");
    let path = root.join("large.eml");
    let body = "YWxwaGEgYmV0YSBnYW1tYQ==\n".repeat(4096);
    fs::write(
        &path,
        format!("Subject: Encoded\nContent-Transfer-Encoding: base64\n\n{body}"),
    )
    .unwrap();
    let mut checks = 0usize;

    let result = Extractor::default().extract_path_report_checked(&path, || {
        checks += 1;
        if checks >= 512 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert!(matches!(result, Err(GfmError::Cancelled)));
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
fn extraction_report_checked_can_cancel_while_parsing_json_structure() {
    let root = unique_temp_dir("gfm-content-json-cancel");
    let path = root.join("large.json");
    let json = format!(
        "{{\"items\":[{}]}}",
        (0..4096)
            .map(|index| format!("\"jsonneedle-{index}\""))
            .collect::<Vec<_>>()
            .join(",")
    );
    fs::write(&path, json).unwrap();
    let mut checks = 0usize;

    let result = Extractor::default().extract_path_report_checked(&path, || {
        checks += 1;
        if checks >= 512 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert!(matches!(result, Err(GfmError::Cancelled)));
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
fn extraction_report_checked_can_cancel_while_parsing_csv_structure() {
    let root = unique_temp_dir("gfm-content-csv-cancel");
    let path = root.join("large.csv");
    let csv = (0..4096)
        .map(|index| format!("row-{index},csvneedle-{index}\n"))
        .collect::<String>();
    fs::write(&path, csv).unwrap();
    let mut checks = 0usize;

    let result = Extractor::default().extract_path_report_checked(&path, || {
        checks += 1;
        if checks >= 512 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert!(matches!(result, Err(GfmError::Cancelled)));
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
fn extraction_report_checked_can_cancel_while_walking_plist_structure() {
    let root = unique_temp_dir("gfm-content-plist-cancel");
    let path = root.join("large.plist");
    let mut dictionary = plist::Dictionary::new();
    for index in 0..4096 {
        dictionary.insert(
            format!("Key{index}"),
            plist::Value::String(format!("plist-{index}")),
        );
    }
    let mut bytes = Vec::new();
    plist::Value::Dictionary(dictionary)
        .to_writer_binary(&mut bytes)
        .unwrap();
    fs::write(&path, bytes).unwrap();
    let mut checks = 0usize;

    let result = Extractor::default().extract_path_report_checked(&path, || {
        checks += 1;
        if checks >= 512 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert!(matches!(result, Err(GfmError::Cancelled)));
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
fn cached_extraction_report_tsv_escapes_control_characters() {
    let path = PathBuf::from("/tmp/Cache\tRoot\nDraft\rFinal.md");
    let report = CachedExtractionReport {
        status: ExtractionCacheStatus::Hit,
        key: ExtractionCacheKey {
            file_id: FileId::new(VolumeId(7), 42),
            extractor_version: TEXT_EXTRACTOR_VERSION,
            content: ExtractionContentSignature {
                len: 512,
                modified_ns: Some(99),
                sample_hash: 0xfeed_cafe,
            },
            metadata_epoch: 0xface,
        },
        report: ExtractionReport {
            path,
            format: ExtractionFormat::Text,
            status: ExtractionStatus::Extracted,
            fingerprint: ExtractionFingerprint {
                extractor_version: TEXT_EXTRACTOR_VERSION,
                len: 512,
                modified_ns: Some(99),
            },
            document: Some(ContentDocument {
                bytes_read: 12,
                text: "cached text".to_string(),
            }),
        },
    };
    let tsv = report.as_tsv();

    assert_eq!(tsv.lines().count(), 1, "{tsv}");
    assert!(!tsv.contains('\r'), "{tsv}");
    assert!(
        tsv.contains("path=/tmp/Cache\\tRoot\\nDraft\\rFinal.md"),
        "{tsv}"
    );
    assert_eq!(tsv.split('\t').count(), 10, "{tsv}");
}

#[test]
fn extraction_cache_key_checked_honors_pre_cancelled_control_before_file_open() {
    let root = unique_temp_dir("gfm-content-cache-key-cancel");
    let path = root.join("cache.md");
    let record = FileRecord {
        id: FileId::new(VolumeId(1), 1),
        parent: None,
        path: path.clone(),
        name: "cache.md".to_string(),
        kind: FileKind::File,
        len: 13,
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

    let result = ExtractionCacheKey::for_record_checked(&record, || Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(!path.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cached_extractor_checked_honors_pre_cancelled_control_before_file_open() {
    let root = unique_temp_dir("gfm-cached-extractor-cancel");
    let path = root.join("cache.md");
    let record = FileRecord {
        id: FileId::new(VolumeId(1), 1),
        parent: None,
        path: path.clone(),
        name: "cache.md".to_string(),
        kind: FileKind::File,
        len: 13,
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
    let mut cached = CachedExtractor::default();

    let result = cached.extract_record_report_checked(&record, || Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(!path.exists());
    assert_eq!(cached.cache_len(), 0);
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
fn quarantine_decision_tsv_escapes_control_characters() {
    let entry = QuarantineEntry {
        path: PathBuf::from("/tmp/Reports\tQ3\nDraft\rFinal.pdf"),
        cache_key: "cache\tkey\ncontent\rsha".to_string(),
        kind: QuarantineFailureKind::Timeout,
        reason: "worker\ttimeout\nwhile\rreading".to_string(),
        failures: 2,
    };
    let tsv = QuarantineDecision::Quarantined(entry).as_tsv();

    assert_eq!(tsv.lines().count(), 1, "{tsv}");
    assert!(!tsv.contains('\r'), "{tsv}");
    assert!(
        tsv.contains("path=/tmp/Reports\\tQ3\\nDraft\\rFinal.pdf"),
        "{tsv}"
    );
    assert!(
        tsv.contains("reason=worker\\ttimeout\\nwhile\\rreading"),
        "{tsv}"
    );
    assert!(
        tsv.contains("cache-key=cache\\tkey\\ncontent\\rsha"),
        "{tsv}"
    );
    assert_eq!(tsv.split('\t').count(), 6, "{tsv}");
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
fn quarantine_store_round_trips_control_characters() {
    let root = unique_temp_dir("gfm-content-quarantine-control-characters");
    let path = root.join("Reports\tQ3\nDraft\rFinal.pdf");
    let store = root.join("quarantine.gfmquarantine");
    fs::write(&path, minimal_pdf("slow")).unwrap();
    let fingerprint = ExtractionFingerprint::for_path(&path).unwrap();
    let mut quarantine = ExtractionQuarantine::new(1);
    let blocked = quarantine.record_failure(
        &path,
        &fingerprint,
        QuarantineFailureKind::Timeout,
        "worker\ttimeout\nwhile\rreading",
    );

    assert!(matches!(blocked, QuarantineDecision::Quarantined(_)));
    quarantine.write(&store).unwrap();
    let stored = fs::read_to_string(&store).unwrap();
    assert_eq!(stored.lines().count(), 4, "{stored}");
    assert!(
        stored.contains("Reports\\tQ3\\nDraft\\rFinal.pdf"),
        "{stored}"
    );
    assert!(
        stored.contains("worker\\ttimeout\\nwhile\\rreading"),
        "{stored}"
    );

    let reloaded = ExtractionQuarantine::read(&store).unwrap();

    match reloaded.before_extract(&path, &fingerprint) {
        QuarantineDecision::Quarantined(entry) => {
            assert_eq!(entry.path, path);
            assert_eq!(entry.kind, QuarantineFailureKind::Timeout);
            assert_eq!(entry.reason, "worker\ttimeout\nwhile\rreading");
            assert_eq!(entry.failures, 1);
        }
        QuarantineDecision::Allow => panic!("expected persisted quarantine entry"),
    }
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

#[test]
fn extraction_quarantine_checked_write_honors_pre_cancelled_control_before_file_create() {
    let root = unique_temp_dir("gfm-content-quarantine-write-pre-cancel");
    let store = root.join("quarantine.gfmquarantine");
    let quarantine = ExtractionQuarantine::new(2);

    let result = quarantine.write_checked(&store, || Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(!store.exists());
    assert!(!has_quarantine_temp_file(&root, "quarantine.gfmquarantine"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extraction_quarantine_checked_write_removes_temp_file_after_cancelled_entry_write() {
    let root = unique_temp_dir("gfm-content-quarantine-write-entry-cancel");
    let path = root.join("slow.pdf");
    let store = root.join("quarantine.gfmquarantine");
    fs::write(&path, minimal_pdf("slow")).unwrap();
    let fingerprint = ExtractionFingerprint::for_path(&path).unwrap();
    let mut quarantine = ExtractionQuarantine::new(2);
    quarantine.record_failure(
        &path,
        &fingerprint,
        QuarantineFailureKind::Timeout,
        "worker-timeout",
    );
    let mut checks = 0usize;

    let result = quarantine.write_checked(&store, || {
        checks += 1;
        if checks >= 9 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(checks >= 9);
    assert!(!store.exists());
    assert!(!has_quarantine_temp_file(&root, "quarantine.gfmquarantine"));
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

fn has_quarantine_temp_file(root: &Path, prefix: &str) -> bool {
    fs::read_dir(root).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(&format!("{prefix}.tmp."))
    })
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

fn compressed_pdf(text: &str) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(format!("BT /F1 12 Tf 72 720 Td ({text}) Tj ET").as_bytes())
        .unwrap();
    let compressed = encoder.finish().unwrap();
    let mut pdf = b"%PDF-1.4
1 0 obj
<< /Type /Page /Contents 2 0 R >>
endobj
2 0 obj
<< /Length "
        .to_vec();
    pdf.extend(compressed.len().to_string().as_bytes());
    pdf.extend(
        b" /Filter /FlateDecode >>
stream
",
    );
    pdf.extend(compressed);
    pdf.extend(
        b"
endstream
endobj
%%EOF",
    );
    pdf
}

fn multi_page_pdf(pages: usize) -> Vec<u8> {
    let mut pdf = b"%PDF-1.4\n".to_vec();
    for index in 0..pages {
        pdf.extend(format!("{index} 0 obj << /Type /Page >> endobj\n").as_bytes());
    }
    pdf.extend(b"%%EOF");
    pdf
}

fn image_only_pdf() -> Vec<u8> {
    b"%PDF-1.4
1 0 obj
<< /Type /Page /Resources << /XObject << /Im0 2 0 R >> >> >>
endobj
2 0 obj
<< /Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length 12 >>
stream
abcdefghijkl
endstream
endobj
%%EOF"
        .to_vec()
}

fn ooxml_package(parts: &[(&str, &str)]) -> Vec<u8> {
    zip_package(parts)
}

fn encrypted_zip_package(parts: &[(&str, &str)]) -> Vec<u8> {
    let mut bytes = zip_package(parts);
    set_zip_encrypted_flags(&mut bytes);
    bytes
}

fn set_zip_encrypted_flags(bytes: &mut [u8]) {
    for index in 0..bytes.len().saturating_sub(10) {
        if bytes[index..].starts_with(b"PK\x03\x04") {
            bytes[index + 6] |= 1;
        } else if bytes[index..].starts_with(b"PK\x01\x02") {
            bytes[index + 8] |= 1;
        }
    }
}

fn legacy_office_bytes(streams: &[&str]) -> Vec<u8> {
    const HEADER_BYTES: usize = 512;
    const DIRECTORY_ENTRY_BYTES: usize = 128;

    let mut header = vec![0_u8; HEADER_BYTES];
    header[..8].copy_from_slice(b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1");
    header[24..26].copy_from_slice(&0x003e_u16.to_le_bytes());
    header[26..28].copy_from_slice(&0x0003_u16.to_le_bytes());
    header[28..30].copy_from_slice(&0xfffe_u16.to_le_bytes());
    header[30..32].copy_from_slice(&9_u16.to_le_bytes());
    header[32..34].copy_from_slice(&6_u16.to_le_bytes());
    header[44..48].copy_from_slice(&1_u32.to_le_bytes());
    header[48..52].copy_from_slice(&1_u32.to_le_bytes());
    header[56..60].copy_from_slice(&4096_u32.to_le_bytes());
    header[60..64].copy_from_slice(&TEST_OLE_ENDOFCHAIN.to_le_bytes());
    header[68..72].copy_from_slice(&TEST_OLE_ENDOFCHAIN.to_le_bytes());
    header[76..80].copy_from_slice(&0_u32.to_le_bytes());
    for offset in (80..HEADER_BYTES).step_by(4) {
        header[offset..offset + 4].copy_from_slice(&TEST_OLE_FREESECT.to_le_bytes());
    }

    let mut fat = vec![0xff_u8; HEADER_BYTES];
    write_fat_entry(&mut fat, 0, TEST_OLE_FATSECT);
    write_fat_entry(&mut fat, 1, TEST_OLE_ENDOFCHAIN);

    let mut directory = vec![0_u8; HEADER_BYTES];
    write_directory_entry(&mut directory[0..DIRECTORY_ENTRY_BYTES], "Root Entry", 5);
    for (index, stream) in streams.iter().take(3).enumerate() {
        let offset = (index + 1) * DIRECTORY_ENTRY_BYTES;
        write_directory_entry(
            &mut directory[offset..offset + DIRECTORY_ENTRY_BYTES],
            stream,
            2,
        );
    }

    [header, fat, directory].concat()
}

fn legacy_office_bytes_with_stream(stream_name: &str, payload: &[u8]) -> Vec<u8> {
    const HEADER_BYTES: usize = 512;
    const DIRECTORY_ENTRY_BYTES: usize = 128;

    let mut header = vec![0_u8; HEADER_BYTES];
    header[..8].copy_from_slice(b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1");
    header[24..26].copy_from_slice(&0x003e_u16.to_le_bytes());
    header[26..28].copy_from_slice(&0x0003_u16.to_le_bytes());
    header[28..30].copy_from_slice(&0xfffe_u16.to_le_bytes());
    header[30..32].copy_from_slice(&9_u16.to_le_bytes());
    header[32..34].copy_from_slice(&6_u16.to_le_bytes());
    header[44..48].copy_from_slice(&1_u32.to_le_bytes());
    header[48..52].copy_from_slice(&1_u32.to_le_bytes());
    header[56..60].copy_from_slice(&4096_u32.to_le_bytes());
    header[60..64].copy_from_slice(&TEST_OLE_ENDOFCHAIN.to_le_bytes());
    header[68..72].copy_from_slice(&TEST_OLE_ENDOFCHAIN.to_le_bytes());
    header[76..80].copy_from_slice(&0_u32.to_le_bytes());
    for offset in (80..HEADER_BYTES).step_by(4) {
        header[offset..offset + 4].copy_from_slice(&TEST_OLE_FREESECT.to_le_bytes());
    }

    let mut fat = vec![0xff_u8; HEADER_BYTES];
    write_fat_entry(&mut fat, 0, TEST_OLE_FATSECT);
    write_fat_entry(&mut fat, 1, TEST_OLE_ENDOFCHAIN);
    write_fat_entry(&mut fat, 2, TEST_OLE_ENDOFCHAIN);

    let mut directory = vec![0_u8; HEADER_BYTES];
    write_directory_entry(&mut directory[0..DIRECTORY_ENTRY_BYTES], "Root Entry", 5);
    write_directory_stream_entry(
        &mut directory[DIRECTORY_ENTRY_BYTES..DIRECTORY_ENTRY_BYTES * 2],
        stream_name,
        2,
        payload.len(),
    );

    let mut stream = vec![0_u8; HEADER_BYTES];
    stream[..payload.len().min(HEADER_BYTES)]
        .copy_from_slice(&payload[..payload.len().min(HEADER_BYTES)]);

    [header, fat, directory, stream].concat()
}

fn legacy_office_bytes_with_difat_stream(stream_name: &str, payload: &[u8]) -> Vec<u8> {
    const HEADER_BYTES: usize = 512;
    const DIRECTORY_ENTRY_BYTES: usize = 128;

    let mut header = vec![0_u8; HEADER_BYTES];
    header[..8].copy_from_slice(b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1");
    header[24..26].copy_from_slice(&0x003e_u16.to_le_bytes());
    header[26..28].copy_from_slice(&0x0003_u16.to_le_bytes());
    header[28..30].copy_from_slice(&0xfffe_u16.to_le_bytes());
    header[30..32].copy_from_slice(&9_u16.to_le_bytes());
    header[32..34].copy_from_slice(&6_u16.to_le_bytes());
    header[44..48].copy_from_slice(&1_u32.to_le_bytes());
    header[48..52].copy_from_slice(&2_u32.to_le_bytes());
    header[56..60].copy_from_slice(&4096_u32.to_le_bytes());
    header[60..64].copy_from_slice(&TEST_OLE_ENDOFCHAIN.to_le_bytes());
    header[68..72].copy_from_slice(&0_u32.to_le_bytes());
    header[72..76].copy_from_slice(&1_u32.to_le_bytes());
    for offset in (76..HEADER_BYTES).step_by(4) {
        header[offset..offset + 4].copy_from_slice(&TEST_OLE_FREESECT.to_le_bytes());
    }

    let mut difat = vec![0xff_u8; HEADER_BYTES];
    difat[0..4].copy_from_slice(&1_u32.to_le_bytes());
    for offset in (4..HEADER_BYTES - 4).step_by(4) {
        difat[offset..offset + 4].copy_from_slice(&TEST_OLE_FREESECT.to_le_bytes());
    }
    difat[HEADER_BYTES - 4..HEADER_BYTES].copy_from_slice(&TEST_OLE_ENDOFCHAIN.to_le_bytes());

    let mut fat = vec![0xff_u8; HEADER_BYTES];
    write_fat_entry(&mut fat, 0, TEST_OLE_DIFSECT);
    write_fat_entry(&mut fat, 1, TEST_OLE_FATSECT);
    write_fat_entry(&mut fat, 2, TEST_OLE_ENDOFCHAIN);
    write_fat_entry(&mut fat, 3, TEST_OLE_ENDOFCHAIN);

    let mut directory = vec![0_u8; HEADER_BYTES];
    write_directory_entry(&mut directory[0..DIRECTORY_ENTRY_BYTES], "Root Entry", 5);
    write_directory_stream_entry(
        &mut directory[DIRECTORY_ENTRY_BYTES..DIRECTORY_ENTRY_BYTES * 2],
        stream_name,
        3,
        payload.len(),
    );

    let mut stream = vec![0_u8; HEADER_BYTES];
    stream[..payload.len().min(HEADER_BYTES)]
        .copy_from_slice(&payload[..payload.len().min(HEADER_BYTES)]);

    [header, difat, fat, directory, stream].concat()
}

fn write_fat_entry(fat: &mut [u8], index: usize, value: u32) {
    let offset = index * 4;
    fat[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_directory_entry(entry: &mut [u8], name: &str, object_type: u8) {
    let mut utf16 = name.encode_utf16().collect::<Vec<_>>();
    utf16.push(0);
    for (index, unit) in utf16.iter().enumerate() {
        let offset = index * 2;
        entry[offset..offset + 2].copy_from_slice(&unit.to_le_bytes());
    }
    entry[64..66].copy_from_slice(&((utf16.len() * 2) as u16).to_le_bytes());
    entry[66] = object_type;
    entry[67] = 1;
    entry[68..72].copy_from_slice(&TEST_OLE_FREESECT.to_le_bytes());
    entry[72..76].copy_from_slice(&TEST_OLE_FREESECT.to_le_bytes());
    entry[76..80].copy_from_slice(&TEST_OLE_FREESECT.to_le_bytes());
    entry[116..120].copy_from_slice(&TEST_OLE_ENDOFCHAIN.to_le_bytes());
}

fn write_directory_stream_entry(
    entry: &mut [u8],
    name: &str,
    start_sector: u32,
    stream_size: usize,
) {
    write_directory_entry(entry, name, 2);
    entry[116..120].copy_from_slice(&start_sector.to_le_bytes());
    entry[120..124].copy_from_slice(&(stream_size as u32).to_le_bytes());
}

fn sevenzip_package(names: &[&str]) -> Vec<u8> {
    let mut header = vec![0x01, 0x05];
    push_7z_uint(&mut header, names.len() as u64);
    header.push(0x11);
    let names_size_index = header.len();
    header.push(0);
    header.push(0);
    let names_start = header.len();
    for name in names {
        for unit in name.encode_utf16() {
            header.extend_from_slice(&unit.to_le_bytes());
        }
        header.extend_from_slice(&0_u16.to_le_bytes());
    }
    let names_len = header.len() - names_start;
    assert!(names_len < 127);
    header[names_size_index] = (names_len + 1) as u8;
    header.push(0);

    sevenzip_with_header(header)
}

fn sevenzip_encoded_header_package(method_id: &[u8]) -> Vec<u8> {
    let mut header = vec![0x17, 0x06];
    push_7z_uint(&mut header, 0);
    push_7z_uint(&mut header, 1);
    header.push(0x09);
    push_7z_uint(&mut header, 32);
    header.push(0);
    header.push(0x07);
    header.push(0x0b);
    push_7z_uint(&mut header, 1);
    header.push(0);
    push_7z_uint(&mut header, 1);
    header.push(method_id.len() as u8);
    header.extend_from_slice(method_id);
    header.push(0x0c);
    push_7z_uint(&mut header, 64);
    header.push(0);
    header.push(0);

    sevenzip_with_header(header)
}

fn sevenzip_with_header(header: Vec<u8>) -> Vec<u8> {
    let mut bytes = vec![0_u8; 32];
    bytes[..6].copy_from_slice(b"7z\xbc\xaf\x27\x1c");
    bytes[6] = 0;
    bytes[7] = 4;
    bytes[12..20].copy_from_slice(&0_u64.to_le_bytes());
    bytes[20..28].copy_from_slice(&(header.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&header);
    bytes
}

fn push_7z_uint(output: &mut Vec<u8>, value: u64) {
    assert!(value < 0x80);
    output.push(value as u8);
}

fn rar4_package(entries: &[(&str, u64)]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"Rar!\x1a\x07\x00");
    push_rar4_block(&mut bytes, 0x73, 0, &[0_u8; 6]);
    for (name, unpacked_size) in entries {
        let mut body = Vec::new();
        body.extend_from_slice(&0_u32.to_le_bytes());
        body.extend_from_slice(&(*unpacked_size as u32).to_le_bytes());
        body.push(3);
        body.extend_from_slice(&0_u32.to_le_bytes());
        body.extend_from_slice(&0_u32.to_le_bytes());
        body.push(29);
        body.push(48);
        body.extend_from_slice(&(name.len() as u16).to_le_bytes());
        body.extend_from_slice(&0_u32.to_le_bytes());
        body.extend_from_slice(name.as_bytes());
        push_rar4_block(&mut bytes, 0x74, 0x8000, &body);
    }
    bytes
}

fn rar5_package(entries: &[(&str, u64)]) -> Vec<u8> {
    let mut bytes = b"Rar!\x1a\x07\x01\x00".to_vec();
    push_rar5_block(&mut bytes, 1, 0, 0, &rar5_main_body(0), &[], 0);
    for (name, unpacked_size) in entries {
        push_rar5_block(
            &mut bytes,
            2,
            0x0002,
            0,
            &rar5_file_body(name, *unpacked_size),
            &[],
            *unpacked_size,
        );
    }
    push_rar5_block(&mut bytes, 5, 0, 0, &[], &[], 0);
    bytes
}

fn rar5_encrypted_header_package() -> Vec<u8> {
    let mut bytes = b"Rar!\x1a\x07\x01\x00".to_vec();
    push_rar5_block(&mut bytes, 4, 0, 0, &[0, 0, 0], &[], 0);
    bytes
}

fn rar5_multivolume_package() -> Vec<u8> {
    let mut bytes = b"Rar!\x1a\x07\x01\x00".to_vec();
    push_rar5_block(&mut bytes, 1, 0, 0, &rar5_main_body(0x0001), &[], 0);
    push_rar5_block(&mut bytes, 5, 0, 0, &[], &[], 0);
    bytes
}

fn rar5_corrupt_file_extra_package() -> Vec<u8> {
    let mut bytes = b"Rar!\x1a\x07\x01\x00".to_vec();
    push_rar5_block(&mut bytes, 1, 0, 0, &rar5_main_body(0), &[], 0);
    push_rar5_block(
        &mut bytes,
        2,
        0,
        0,
        &rar5_file_body("docs/corrupt-extra.txt", 31),
        &[0],
        0,
    );
    push_rar5_block(&mut bytes, 5, 0, 0, &[], &[], 0);
    bytes
}

fn rar5_corrupt_service_extra_package() -> Vec<u8> {
    let mut bytes = b"Rar!\x1a\x07\x01\x00".to_vec();
    push_rar5_block(&mut bytes, 1, 0, 0, &rar5_main_body(0), &[], 0);
    push_rar5_block(&mut bytes, 3, 0, 0, &[], &[0], 0);
    push_rar5_block(&mut bytes, 5, 0, 0, &[], &[], 0);
    bytes
}

fn rar5_oversized_vint_header_package() -> Vec<u8> {
    let mut bytes = b"Rar!\x1a\x07\x01\x00".to_vec();
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&[0xff; 9]);
    bytes.push(0x02);
    bytes
}

fn rar5_main_body(flags: u64) -> Vec<u8> {
    let mut body = Vec::new();
    push_rar5_vint(&mut body, flags);
    body
}

fn rar5_file_body(name: &str, unpacked_size: u64) -> Vec<u8> {
    let mut body = Vec::new();
    push_rar5_vint(&mut body, 0);
    push_rar5_vint(&mut body, unpacked_size);
    push_rar5_vint(&mut body, 0);
    push_rar5_vint(&mut body, 0);
    push_rar5_vint(&mut body, 1);
    push_rar5_vint(&mut body, name.len() as u64);
    body.extend_from_slice(name.as_bytes());
    body
}

fn push_rar5_block(
    output: &mut Vec<u8>,
    kind: u64,
    common_flags: u64,
    extra_flags: u64,
    body: &[u8],
    extra: &[u8],
    data_size: u64,
) {
    let mut header = Vec::new();
    push_rar5_vint(&mut header, kind);
    let mut flags = common_flags | extra_flags;
    if !extra.is_empty() {
        flags |= 0x0001;
    }
    if data_size > 0 {
        flags |= 0x0002;
    }
    push_rar5_vint(&mut header, flags);
    if !extra.is_empty() {
        push_rar5_vint(&mut header, extra.len() as u64);
    }
    if data_size > 0 {
        push_rar5_vint(&mut header, data_size);
    }
    header.extend_from_slice(body);
    header.extend_from_slice(extra);
    output.extend_from_slice(&0_u32.to_le_bytes());
    push_rar5_vint(output, header.len() as u64);
    output.extend_from_slice(&header);
    output.resize(output.len() + data_size as usize, 0);
}

fn push_rar5_vint(output: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        output.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn push_rar4_block(output: &mut Vec<u8>, kind: u8, flags: u16, body: &[u8]) {
    output.extend_from_slice(&0_u16.to_le_bytes());
    output.push(kind);
    output.extend_from_slice(&flags.to_le_bytes());
    output.extend_from_slice(&((7 + body.len()) as u16).to_le_bytes());
    output.extend_from_slice(body);
}

fn tar_gz_package(parts: &[(&str, &str)]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&tar_package(parts)).unwrap();
    encoder.finish().unwrap()
}

fn tar_bz2_package(parts: &[(&str, &str)]) -> Vec<u8> {
    let mut encoder = BzEncoder::new(Vec::new(), bzip2::Compression::default());
    encoder.write_all(&tar_package(parts)).unwrap();
    encoder.finish().unwrap()
}

fn tar_xz_package(parts: &[(&str, &str)]) -> Vec<u8> {
    let mut encoder = XzEncoder::new(Vec::new(), 6);
    encoder.write_all(&tar_package(parts)).unwrap();
    encoder.finish().unwrap()
}

fn gzip_stream(text: &str) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(text.as_bytes()).unwrap();
    encoder.finish().unwrap()
}

fn bzip2_stream(text: &str) -> Vec<u8> {
    let mut encoder = BzEncoder::new(Vec::new(), bzip2::Compression::default());
    encoder.write_all(text.as_bytes()).unwrap();
    encoder.finish().unwrap()
}

fn xz_stream(text: &str) -> Vec<u8> {
    let mut encoder = XzEncoder::new(Vec::new(), 6);
    encoder.write_all(text.as_bytes()).unwrap();
    encoder.finish().unwrap()
}

fn tar_package(parts: &[(&str, &str)]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (name, text) in parts {
        let mut header = [0u8; 512];
        write_tar_string(&mut header[0..100], name);
        write_tar_octal(&mut header[100..108], 0o644);
        write_tar_octal(&mut header[108..116], 0);
        write_tar_octal(&mut header[116..124], 0);
        write_tar_octal(&mut header[124..136], text.len() as u64);
        write_tar_octal(&mut header[136..148], 0);
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        for byte in &mut header[148..156] {
            *byte = b' ';
        }
        let checksum: u32 = header.iter().map(|byte| u32::from(*byte)).sum();
        write_tar_checksum(&mut header[148..156], checksum);
        bytes.extend_from_slice(&header);
        bytes.extend_from_slice(text.as_bytes());
        let padding = (512 - (text.len() % 512)) % 512;
        bytes.extend(std::iter::repeat_n(0, padding));
    }
    bytes.extend([0u8; 1024]);
    bytes
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

fn write_tar_string(field: &mut [u8], value: &str) {
    let bytes = value.as_bytes();
    let len = bytes.len().min(field.len());
    field[..len].copy_from_slice(&bytes[..len]);
}

fn write_tar_octal(field: &mut [u8], value: u64) {
    let encoded = format!("{value:0width$o}\0", width = field.len() - 1);
    field.copy_from_slice(encoded.as_bytes());
}

fn write_tar_checksum(field: &mut [u8], value: u32) {
    let encoded = format!("{value:06o}\0 ",);
    field.copy_from_slice(encoded.as_bytes());
}
