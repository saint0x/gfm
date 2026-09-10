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
    let root = std::env::temp_dir().join(format!("gfm-ocr-queue-parent-{}", std::process::id()));
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
    let root = std::env::temp_dir().join(format!("gfm-ocr-queue-cancel-{}", std::process::id()));
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

#[test]
fn recognition_cache_round_trips_deterministically_with_control_text() {
    let root = std::env::temp_dir().join(format!("gfm-ocr-cache-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let store = root.join("ocr-cache.tsv");
    let screenshot = OcrCandidate {
        path: root.join("Screen\tshot.png"),
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
    let screenshot_recognition = OcrRecognition {
        candidate: screenshot.clone(),
        text: "hello\tfrom\nocr\r世界".to_string(),
    };
    let pdf_recognition = OcrRecognition {
        candidate: pdf.clone(),
        text: "scanned agreement searchable text".to_string(),
    };

    OcrRecognitionCache::new([screenshot_recognition.clone(), pdf_recognition.clone()])
        .write(&store)
        .unwrap();
    let text = fs::read_to_string(&store).unwrap();
    let reloaded = OcrRecognitionCache::read(&store).unwrap();

    assert!(text.contains("gfm-ocr-recognition-cache-v1"));
    assert!(text.contains("Screen\\tshot.png"), "{text}");
    assert!(text.contains("text-hex="), "{text}");
    assert_eq!(reloaded.get(&screenshot), Some(&screenshot_recognition));
    assert_eq!(reloaded.get(&pdf), Some(&pdf_recognition));
    assert_eq!(
        reloaded.into_recognitions(),
        vec![pdf_recognition, screenshot_recognition]
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn recognition_cache_document_reports_text_bytes() {
    let recognition = OcrRecognition {
        candidate: OcrCandidate {
            path: PathBuf::from("/tmp/Screenshot.png"),
            kind: OcrCandidateKind::ScreenshotImage,
            fingerprint: ExtractionFingerprint {
                extractor_version: OCR_EXTRACTOR_VERSION,
                len: 1,
                modified_ns: None,
            },
        },
        text: "searchable".to_string(),
    };

    assert_eq!(
        recognition.document(),
        ContentDocument {
            bytes_read: "searchable".len(),
            text: "searchable".to_string()
        }
    );
}

#[test]
fn recognition_cache_write_creates_parent_directory() {
    let root = std::env::temp_dir().join(format!("gfm-ocr-cache-parent-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let store = root.join("nested").join("ocr-cache.tsv");
    let cache = OcrRecognitionCache::new([OcrRecognition {
        candidate: OcrCandidate {
            path: root.join("Screenshot.png"),
            kind: OcrCandidateKind::ScreenshotImage,
            fingerprint: ExtractionFingerprint {
                extractor_version: OCR_EXTRACTOR_VERSION,
                len: 1,
                modified_ns: None,
            },
        },
        text: "visible text".to_string(),
    }]);

    cache.write(&store).unwrap();

    assert_eq!(OcrRecognitionCache::read(&store).unwrap().len(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn recognition_cache_temp_paths_are_unique_within_process() {
    let first = recognition_cache_temp_path(Path::new("/tmp/ocr-cache.tsv"));
    let second = recognition_cache_temp_path(Path::new("/tmp/ocr-cache.tsv"));

    assert_ne!(first, second);
    assert_eq!(first.parent(), Some(Path::new("/tmp")));
    assert!(first
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".tmp")));
}

#[test]
fn recognition_cache_rejects_corrupt_text_hex() {
    let root = std::env::temp_dir().join(format!("gfm-ocr-cache-corrupt-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let store = root.join("ocr-cache.tsv");
    fs::write(
            &store,
            "gfm-ocr-recognition-cache-v1\nschema_version\t1\nocr-recognition\tpath=/tmp/a.png\tkind=screenshot-image\tversion=1\tlen=1\tmodified-ns=-\ttext-hex=0\n",
        )
        .unwrap();

    let err = OcrRecognitionCache::read(&store).unwrap_err();

    assert!(
        err.to_string().contains("invalid OCR text hex length"),
        "{err}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn recognition_cache_write_cancellation_preserves_existing_store() {
    let root = std::env::temp_dir().join(format!("gfm-ocr-cache-cancel-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let store = root.join("ocr-cache.tsv");
    fs::write(&store, "existing").unwrap();
    let cache = OcrRecognitionCache::new([OcrRecognition {
        candidate: OcrCandidate {
            path: root.join("Screenshot.png"),
            kind: OcrCandidateKind::ScreenshotImage,
            fingerprint: ExtractionFingerprint {
                extractor_version: OCR_EXTRACTOR_VERSION,
                len: 1,
                modified_ns: None,
            },
        },
        text: "visible text".to_string(),
    }]);

    let mut checks = 0;
    let result = cache.write_checked(&store, || {
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

#[test]
fn failure_quarantine_blocks_after_threshold_and_round_trips() {
    let root = std::env::temp_dir().join(format!("gfm-ocr-failure-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let store = root.join("ocr-failure.tsv");
    let candidate = OcrCandidate {
        path: root.join("Screen\tshot\nDraft\r.png"),
        kind: OcrCandidateKind::ScreenshotImage,
        fingerprint: ExtractionFingerprint {
            extractor_version: OCR_EXTRACTOR_VERSION,
            len: 12,
            modified_ns: Some(34),
        },
    };
    let mut quarantine = OcrFailureQuarantine::new(2);

    assert_eq!(
        quarantine.before_recognize(&candidate),
        OcrFailureDecision::Allow
    );
    assert_eq!(
        quarantine.record_failure(candidate.clone(), OcrFailureKind::Failed, "vision\tfailed"),
        OcrFailureDecision::Allow
    );
    let blocked = quarantine.record_failure(
        candidate.clone(),
        OcrFailureKind::Unavailable,
        "vision\nunavailable",
    );
    assert!(matches!(blocked, OcrFailureDecision::Quarantined(_)));
    quarantine.write(&store).unwrap();
    let text = fs::read_to_string(&store).unwrap();
    let reloaded = OcrFailureQuarantine::read(&store).unwrap();

    assert!(text.contains("gfm-ocr-failure-quarantine-v1"));
    assert!(text.contains("Screen\\tshot\\nDraft\\r.png"), "{text}");
    assert!(text.contains("failure-kind=unavailable"), "{text}");
    assert!(text.contains("reason=vision\\nunavailable"), "{text}");
    assert!(matches!(
        reloaded.before_recognize(&candidate),
        OcrFailureDecision::Quarantined(_)
    ));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn failure_quarantine_success_removes_candidate_entry() {
    let candidate = OcrCandidate {
        path: PathBuf::from("/tmp/Screenshot.png"),
        kind: OcrCandidateKind::ScreenshotImage,
        fingerprint: ExtractionFingerprint {
            extractor_version: OCR_EXTRACTOR_VERSION,
            len: 1,
            modified_ns: None,
        },
    };
    let mut quarantine = OcrFailureQuarantine::new(1);

    quarantine.record_failure(candidate.clone(), OcrFailureKind::Missing, "missing");
    assert!(quarantine.has_entry(&candidate));
    assert!(matches!(
        quarantine.before_recognize(&candidate),
        OcrFailureDecision::Quarantined(_)
    ));

    assert_eq!(
        quarantine.record_success(&candidate),
        OcrFailureDecision::Allow
    );
    assert_eq!(
        quarantine.before_recognize(&candidate),
        OcrFailureDecision::Allow
    );
    assert!(!quarantine.has_entry(&candidate));
}

#[test]
fn failure_quarantine_temp_paths_are_unique_within_process() {
    let first = failure_quarantine_temp_path(Path::new("/tmp/ocr-failure.tsv"));
    let second = failure_quarantine_temp_path(Path::new("/tmp/ocr-failure.tsv"));

    assert_ne!(first, second);
    assert_eq!(first.parent(), Some(Path::new("/tmp")));
    assert!(first
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".tmp")));
}

#[test]
fn failure_quarantine_write_cancellation_preserves_existing_store() {
    let root = std::env::temp_dir().join(format!("gfm-ocr-failure-cancel-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let store = root.join("ocr-failure.tsv");
    fs::write(&store, "existing").unwrap();
    let mut quarantine = OcrFailureQuarantine::new(1);
    quarantine.record_failure(
        OcrCandidate {
            path: root.join("Screenshot.png"),
            kind: OcrCandidateKind::ScreenshotImage,
            fingerprint: ExtractionFingerprint {
                extractor_version: OCR_EXTRACTOR_VERSION,
                len: 1,
                modified_ns: None,
            },
        },
        OcrFailureKind::Failed,
        "failed",
    );

    let mut checks = 0;
    let result = quarantine.write_checked(&store, || {
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
