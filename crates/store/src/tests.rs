use super::*;

#[test]
fn round_trips_records() {
    let path = temp_path("gfm-store", "idx");
    let records = vec![FileRecord {
        id: FileId::new(VolumeId(4), 12),
        parent: Some(FileId::new(VolumeId(4), 1)),
        path: PathBuf::from("/tmp/a b/report.txt"),
        name: "report.txt".to_string(),
        kind: FileKind::File,
        len: 42,
        mode: 0o100644,
        owner: 501,
        group: 20,
        xattrs_digest: 99,
        created: None,
        modified: Some(UNIX_EPOCH + Duration::from_secs(10)),
        changed: None,
        hidden: false,
        tags: vec!["Important".to_string(), "Review, Later".to_string()],
        finder_comment: Some("handoff notes".to_string()),
    }];

    write_records(&path, &records).unwrap();
    let read = read_records(&path).unwrap();

    assert_eq!(read, records);
    assert!(has_record_checksum_footer(&std::fs::read(&path).unwrap()));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn checked_record_read_honors_pre_cancelled_control_before_file_open() {
    let path = temp_path("gfm-store-pre-cancelled-read", "idx");
    let result = read_records_checked(&path, || Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(!path.exists());
}

#[test]
fn checked_record_write_preserves_existing_file_when_cancelled_before_publish() {
    let path = temp_path("gfm-store-record-write-cancel", "idx");
    let original = vec![sample_file_record(12, "stable.txt")];
    let replacement = vec![sample_file_record(13, "replacement.txt")];
    write_records(&path, &original).unwrap();
    let before = std::fs::read(&path).unwrap();
    let mut checks = 0usize;

    let result = write_records_checked(&path, &replacement, || {
        checks += 1;
        if checks >= 5 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert_eq!(result, Err(GfmError::Cancelled));
    assert!(checks >= 5);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(read_records(&path).unwrap(), original);
    assert!(!has_store_atomic_temp_file(&path));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn checked_prefix_write_preserves_existing_file_when_cancelled_before_publish() {
    let path = temp_path("gfm-store-prefix-write-cancel", "gfmprefix");
    let original = vec![PrefixPosting {
        prefix: "stable".to_string(),
        ids: vec![FileId::new(VolumeId(4), 12)],
    }];
    let replacement = vec![PrefixPosting {
        prefix: "replacement".to_string(),
        ids: vec![FileId::new(VolumeId(4), 13)],
    }];
    write_prefix_postings(&path, &original).unwrap();
    let before = std::fs::read(&path).unwrap();
    let mut checks = 0usize;

    let result = write_prefix_postings_checked(&path, &replacement, || {
        checks += 1;
        if checks >= 5 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert_eq!(result, Err(GfmError::Cancelled));
    assert!(checks >= 5);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(read_prefix_postings(&path).unwrap(), original);
    assert!(!has_store_atomic_temp_file(&path));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn checked_substring_write_preserves_existing_file_when_cancelled_before_publish() {
    let path = temp_path("gfm-store-substring-write-cancel", "gfmsubstr");
    let original = vec![SubstringPosting {
        gram: "sta".to_string(),
        ids: vec![FileId::new(VolumeId(4), 12)],
    }];
    let replacement = vec![SubstringPosting {
        gram: "rep".to_string(),
        ids: vec![FileId::new(VolumeId(4), 13)],
    }];
    write_substring_postings(&path, &original).unwrap();
    let before = std::fs::read(&path).unwrap();
    let mut checks = 0usize;

    let result = write_substring_postings_checked(&path, &replacement, || {
        checks += 1;
        if checks >= 5 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert_eq!(result, Err(GfmError::Cancelled));
    assert!(checks >= 5);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(read_substring_postings(&path).unwrap(), original);
    assert!(!has_store_atomic_temp_file(&path));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn checked_fuzzy_write_preserves_existing_file_when_cancelled_before_publish() {
    let path = temp_path("gfm-store-fuzzy-write-cancel", "gfmfuzzy");
    let original = vec![FuzzyPosting {
        key: "stabl".to_string(),
        terms: vec!["stable".to_string()],
    }];
    let replacement = vec![FuzzyPosting {
        key: "replacemen".to_string(),
        terms: vec!["replacement".to_string()],
    }];
    write_fuzzy_postings(&path, &original).unwrap();
    let before = std::fs::read(&path).unwrap();
    let mut checks = 0usize;

    let result = write_fuzzy_postings_checked(&path, &replacement, || {
        checks += 1;
        if checks >= 5 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert_eq!(result, Err(GfmError::Cancelled));
    assert!(checks >= 5);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(read_fuzzy_postings(&path).unwrap(), original);
    assert!(!has_store_atomic_temp_file(&path));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn checked_metadata_write_preserves_existing_file_when_cancelled_before_publish() {
    let path = temp_path("gfm-store-metadata-write-cancel", "gfmmeta");
    let original = vec![MetadataPosting {
        field: MetadataField::Tag,
        term: "stable".to_string(),
        ids: vec![FileId::new(VolumeId(4), 12)],
    }];
    let replacement = vec![MetadataPosting {
        field: MetadataField::Tag,
        term: "replacement".to_string(),
        ids: vec![FileId::new(VolumeId(4), 13)],
    }];
    write_metadata_postings(&path, &original).unwrap();
    let before = std::fs::read(&path).unwrap();
    let mut checks = 0usize;

    let result = write_metadata_postings_checked(&path, &replacement, || {
        checks += 1;
        if checks >= 5 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert_eq!(result, Err(GfmError::Cancelled));
    assert!(checks >= 5);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(read_metadata_postings(&path).unwrap(), original);
    assert!(!has_store_atomic_temp_file(&path));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn checked_record_columns_write_preserves_existing_file_when_cancelled_before_publish() {
    let path = temp_path("gfm-store-columns-write-cancel", "gfmcols");
    let original = vec![sample_file_record(12, "stable.txt")];
    let replacement = vec![sample_file_record(13, "replacement.txt")];
    write_record_columns(&path, &original).unwrap();
    let before = std::fs::read(&path).unwrap();
    let mut checks = 0usize;

    let result = write_record_columns_checked(&path, &replacement, || {
        checks += 1;
        if checks >= 5 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert_eq!(result, Err(GfmError::Cancelled));
    assert!(checks >= 5);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let archive = MmapRecordColumns::open(&path).unwrap();
    assert_eq!(archive.len(), 1);
    assert_eq!(archive.column(0).unwrap().id, original[0].id);
    assert!(!has_store_atomic_temp_file(&path));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn checked_dictionary_write_preserves_existing_file_when_cancelled_before_publish() {
    let path = temp_path("gfm-store-dictionary-write-cancel", "gfmdict");
    let original = vec!["stable".to_string(), "alpha".to_string()];
    let replacement = vec!["replacement".to_string(), "beta".to_string()];
    write_dictionary(&path, &original).unwrap();
    let before = std::fs::read(&path).unwrap();
    let mut checks = 0usize;

    let result = write_dictionary_checked(&path, &replacement, || {
        checks += 1;
        if checks >= 5 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert_eq!(result, Err(GfmError::Cancelled));
    assert!(checks >= 5);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(
        read_dictionary(&path).unwrap(),
        vec!["alpha".to_string(), "stable".to_string()]
    );
    assert!(!has_store_atomic_temp_file(&path));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn mmap_record_archive_hydrates_records_from_immutable_map() {
    let path = temp_path("gfm-store-mmap", "idx");
    let records = vec![
        FileRecord {
            id: FileId::new(VolumeId(4), 12),
            parent: Some(FileId::new(VolumeId(4), 1)),
            path: PathBuf::from("/tmp/a/report.txt"),
            name: "report.txt".to_string(),
            kind: FileKind::File,
            len: 42,
            mode: 0o100644,
            owner: 501,
            group: 20,
            xattrs_digest: 99,
            created: Some(UNIX_EPOCH + Duration::from_secs(1)),
            modified: Some(UNIX_EPOCH + Duration::from_secs(10)),
            changed: None,
            hidden: false,
            tags: vec!["Important".to_string()],
            finder_comment: Some("notes".to_string()),
        },
        FileRecord {
            id: FileId::new(VolumeId(4), 13),
            parent: Some(FileId::new(VolumeId(4), 1)),
            path: PathBuf::from("/tmp/a/archive.md"),
            name: "archive.md".to_string(),
            kind: FileKind::File,
            len: 11,
            mode: 0o100644,
            owner: 501,
            group: 20,
            xattrs_digest: 0,
            created: None,
            modified: None,
            changed: None,
            hidden: false,
            tags: Vec::new(),
            finder_comment: None,
        },
    ];

    write_records(&path, &records).unwrap();
    let archive = MmapRecordArchive::open(&path).unwrap();

    assert_eq!(archive.len(), 2);
    assert!(!archive.is_empty());
    assert!(archive.mapped_len() > 0);
    assert!(archive.is_checksummed());
    assert_eq!(archive.record(1).unwrap(), records[1]);
    assert_eq!(
        archive.find(FileId::new(VolumeId(4), 13)).unwrap(),
        Some(records[1].clone())
    );
    assert_eq!(archive.find(FileId::new(VolumeId(4), 999)).unwrap(), None);
    assert_eq!(archive.records().unwrap(), records);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn mmap_record_archive_checked_record_honors_pre_cancelled_control() {
    let path = temp_path("gfm-store-mmap-record-cancel", "idx");
    let records = vec![FileRecord {
        id: FileId::new(VolumeId(4), 12),
        parent: Some(FileId::new(VolumeId(4), 1)),
        path: PathBuf::from("/tmp/a/report.txt"),
        name: "report.txt".to_string(),
        kind: FileKind::File,
        len: 42,
        mode: 0o100644,
        owner: 501,
        group: 20,
        xattrs_digest: 99,
        created: Some(UNIX_EPOCH + Duration::from_secs(1)),
        modified: Some(UNIX_EPOCH + Duration::from_secs(10)),
        changed: None,
        hidden: false,
        tags: vec!["Important".to_string()],
        finder_comment: Some("notes".to_string()),
    }];
    write_records(&path, &records).unwrap();
    let archive = MmapRecordArchive::open(&path).unwrap();

    let result = archive.record_checked(0, || Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn mmap_record_archive_finds_records_when_rows_are_not_id_sorted() {
    let path = temp_path("gfm-store-mmap-find", "idx");
    let records = vec![
        FileRecord {
            id: FileId::new(VolumeId(4), 40),
            parent: Some(FileId::new(VolumeId(4), 1)),
            path: PathBuf::from("/tmp/a/zeta.txt"),
            name: "zeta.txt".to_string(),
            kind: FileKind::File,
            len: 40,
            mode: 0o100644,
            owner: 501,
            group: 20,
            xattrs_digest: 0,
            created: None,
            modified: None,
            changed: None,
            hidden: false,
            tags: Vec::new(),
            finder_comment: None,
        },
        FileRecord {
            id: FileId::new(VolumeId(4), 10),
            parent: Some(FileId::new(VolumeId(4), 1)),
            path: PathBuf::from("/tmp/a/alpha.txt"),
            name: "alpha.txt".to_string(),
            kind: FileKind::File,
            len: 10,
            mode: 0o100644,
            owner: 501,
            group: 20,
            xattrs_digest: 0,
            created: None,
            modified: None,
            changed: None,
            hidden: false,
            tags: Vec::new(),
            finder_comment: None,
        },
        FileRecord {
            id: FileId::new(VolumeId(5), 1),
            parent: None,
            path: PathBuf::from("/Volumes/other/root.txt"),
            name: "root.txt".to_string(),
            kind: FileKind::File,
            len: 1,
            mode: 0o100644,
            owner: 501,
            group: 20,
            xattrs_digest: 0,
            created: None,
            modified: None,
            changed: None,
            hidden: false,
            tags: Vec::new(),
            finder_comment: None,
        },
    ];

    write_records(&path, &records).unwrap();
    let archive = MmapRecordArchive::open(&path).unwrap();

    assert_eq!(
        archive.find(FileId::new(VolumeId(4), 10)).unwrap(),
        Some(records[1].clone())
    );
    assert_eq!(
        archive.find(FileId::new(VolumeId(5), 1)).unwrap(),
        Some(records[2].clone())
    );
    assert_eq!(archive.find(FileId::new(VolumeId(6), 1)).unwrap(), None);
    assert!(archive.contains_volume(VolumeId(4)));
    assert!(archive.contains_volume(VolumeId(5)));
    assert!(!archive.contains_volume(VolumeId(6)));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn mmap_record_archive_checked_open_honors_pre_cancelled_control_before_file_open() {
    let path = temp_path("gfm-store-mmap-open-cancel", "idx");

    let result = MmapRecordArchive::open_checked(&path, || Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(!path.exists());
}

#[test]
fn mmap_record_archive_batch_hydrates_sorted_ids_in_one_directory_pass() {
    let path = temp_path("gfm-store-mmap-batch-find", "idx");
    let records = vec![
        FileRecord {
            id: FileId::new(VolumeId(4), 40),
            parent: Some(FileId::new(VolumeId(4), 1)),
            path: PathBuf::from("/tmp/a/zeta.txt"),
            name: "zeta.txt".to_string(),
            kind: FileKind::File,
            len: 40,
            mode: 0o100644,
            owner: 501,
            group: 20,
            xattrs_digest: 0,
            created: None,
            modified: None,
            changed: None,
            hidden: false,
            tags: Vec::new(),
            finder_comment: None,
        },
        FileRecord {
            id: FileId::new(VolumeId(4), 10),
            parent: Some(FileId::new(VolumeId(4), 1)),
            path: PathBuf::from("/tmp/a/alpha.txt"),
            name: "alpha.txt".to_string(),
            kind: FileKind::File,
            len: 10,
            mode: 0o100644,
            owner: 501,
            group: 20,
            xattrs_digest: 0,
            created: None,
            modified: None,
            changed: None,
            hidden: false,
            tags: Vec::new(),
            finder_comment: None,
        },
        FileRecord {
            id: FileId::new(VolumeId(5), 1),
            parent: None,
            path: PathBuf::from("/Volumes/other/root.txt"),
            name: "root.txt".to_string(),
            kind: FileKind::File,
            len: 1,
            mode: 0o100644,
            owner: 501,
            group: 20,
            xattrs_digest: 0,
            created: None,
            modified: None,
            changed: None,
            hidden: false,
            tags: Vec::new(),
            finder_comment: None,
        },
    ];
    write_records(&path, &records).unwrap();
    let archive = MmapRecordArchive::open(&path).unwrap();

    let batch = archive
        .records_for_sorted_ids([
            FileId::new(VolumeId(4), 10),
            FileId::new(VolumeId(4), 10),
            FileId::new(VolumeId(4), 40),
            FileId::new(VolumeId(4), 999),
            FileId::new(VolumeId(5), 1),
        ])
        .unwrap();

    assert_eq!(
        batch.records,
        vec![records[1].clone(), records[0].clone(), records[2].clone()]
    );
    assert_eq!(batch.missing, 1);
    assert!(archive
        .records_for_sorted_ids([FileId::new(VolumeId(4), 40), FileId::new(VolumeId(4), 10),])
        .is_err());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn mmap_record_archive_checked_batch_hydration_honors_pre_cancelled_control() {
    let path = temp_path("gfm-store-mmap-batch-cancel", "idx");
    let records = vec![FileRecord {
        id: FileId::new(VolumeId(4), 10),
        parent: Some(FileId::new(VolumeId(4), 1)),
        path: PathBuf::from("/tmp/a/alpha.txt"),
        name: "alpha.txt".to_string(),
        kind: FileKind::File,
        len: 10,
        mode: 0o100644,
        owner: 501,
        group: 20,
        xattrs_digest: 0,
        created: None,
        modified: None,
        changed: None,
        hidden: false,
        tags: Vec::new(),
        finder_comment: None,
    }];
    write_records(&path, &records).unwrap();
    let archive = MmapRecordArchive::open(&path).unwrap();

    let result = archive.records_for_sorted_ids_checked([FileId::new(VolumeId(4), 10)], || {
        Err(GfmError::Cancelled)
    });

    assert!(matches!(result, Err(GfmError::Cancelled)));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn mmap_record_archive_checked_full_hydration_can_cancel_between_records() {
    let path = temp_path("gfm-store-mmap-records-cancel", "idx");
    let records = vec![
        FileRecord {
            id: FileId::new(VolumeId(4), 10),
            parent: Some(FileId::new(VolumeId(4), 1)),
            path: PathBuf::from("/tmp/a/alpha.txt"),
            name: "alpha.txt".to_string(),
            kind: FileKind::File,
            len: 10,
            mode: 0o100644,
            owner: 501,
            group: 20,
            xattrs_digest: 0,
            created: None,
            modified: None,
            changed: None,
            hidden: false,
            tags: Vec::new(),
            finder_comment: None,
        },
        FileRecord {
            id: FileId::new(VolumeId(4), 11),
            parent: Some(FileId::new(VolumeId(4), 1)),
            path: PathBuf::from("/tmp/a/beta.txt"),
            name: "beta.txt".to_string(),
            kind: FileKind::File,
            len: 11,
            mode: 0o100644,
            owner: 501,
            group: 20,
            xattrs_digest: 0,
            created: None,
            modified: None,
            changed: None,
            hidden: false,
            tags: Vec::new(),
            finder_comment: None,
        },
    ];
    write_records(&path, &records).unwrap();
    let archive = MmapRecordArchive::open(&path).unwrap();
    let mut checks = 0usize;

    let result = archive.records_checked(|| {
        checks += 1;
        if checks >= 2 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(checks >= 2);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn mmap_record_archive_checked_records_honors_pre_cancelled_control() {
    let path = temp_path("gfm-store-mmap-records-cancel", "idx");
    let records = vec![FileRecord {
        id: FileId::new(VolumeId(4), 10),
        parent: Some(FileId::new(VolumeId(4), 1)),
        path: PathBuf::from("/tmp/a/alpha.txt"),
        name: "alpha.txt".to_string(),
        kind: FileKind::File,
        len: 10,
        mode: 0o100644,
        owner: 501,
        group: 20,
        xattrs_digest: 0,
        created: None,
        modified: None,
        changed: None,
        hidden: false,
        tags: Vec::new(),
        finder_comment: None,
    }];
    write_records(&path, &records).unwrap();
    let archive = MmapRecordArchive::open(&path).unwrap();

    let result = archive.records_checked(|| Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn mmap_record_archive_checked_batch_honors_pre_cancelled_control() {
    let path = temp_path("gfm-store-mmap-batch-cancel", "idx");
    let records = vec![FileRecord {
        id: FileId::new(VolumeId(4), 10),
        parent: Some(FileId::new(VolumeId(4), 1)),
        path: PathBuf::from("/tmp/a/alpha.txt"),
        name: "alpha.txt".to_string(),
        kind: FileKind::File,
        len: 10,
        mode: 0o100644,
        owner: 501,
        group: 20,
        xattrs_digest: 0,
        created: None,
        modified: None,
        changed: None,
        hidden: false,
        tags: Vec::new(),
        finder_comment: None,
    }];
    write_records(&path, &records).unwrap();
    let archive = MmapRecordArchive::open(&path).unwrap();

    let result = archive.records_for_sorted_ids_checked([FileId::new(VolumeId(4), 10)], || {
        Err(GfmError::Cancelled)
    });

    assert!(matches!(result, Err(GfmError::Cancelled)));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn checksummed_record_archive_rejects_corruption() {
    let path = temp_path("gfm-store-checksum", "idx");
    let records = vec![FileRecord {
        id: FileId::new(VolumeId(4), 12),
        parent: Some(FileId::new(VolumeId(4), 1)),
        path: PathBuf::from("/tmp/a/important.txt"),
        name: "important.txt".to_string(),
        kind: FileKind::File,
        len: 42,
        mode: 0o100644,
        owner: 501,
        group: 20,
        xattrs_digest: 99,
        created: None,
        modified: Some(UNIX_EPOCH + Duration::from_secs(10)),
        changed: None,
        hidden: false,
        tags: vec!["Important".to_string()],
        finder_comment: Some("notes".to_string()),
    }];

    write_records(&path, &records).unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    let offset = bytes
        .windows(b"important".len())
        .position(|window| window == b"important")
        .expect("archive should contain the test path");
    bytes[offset] = b'z';
    std::fs::write(&path, bytes).unwrap();

    let read_error = read_records(&path).unwrap_err().to_string();
    let mmap_error = MmapRecordArchive::open(&path).unwrap_err().to_string();

    assert!(read_error.contains("checksum mismatch"), "{read_error}");
    assert!(mmap_error.contains("checksum mismatch"), "{mmap_error}");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn reads_legacy_record_store_without_tags() {
    let path = temp_path("gfm-store-legacy", "idx");
    std::fs::write(
        &path,
        "gfm-store-v1\n4\t12\t1\tf\t42\t0\t0\t0\t0\t/tmp/legacy.txt\n",
    )
    .unwrap();

    let read = read_records(&path).unwrap();

    assert_eq!(read.len(), 1);
    assert_eq!(read[0].name, "legacy.txt");
    assert!(read[0].tags.is_empty());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn atomic_write_preserves_existing_file_on_write_failure() {
    let path = temp_path("gfm-store-atomic", "txt");
    std::fs::write(&path, "stable").unwrap();

    let result = atomic_write(&path, |writer| {
        writer.write_all(b"partial")?;
        Err(std::io::Error::other("simulated crash"))
    });

    assert!(result.is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "stable");
    std::fs::remove_file(path).unwrap();
}

fn temp_path(prefix: &str, extension: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "{}-{}.{}",
        prefix,
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        extension
    ))
}

fn sample_file_record(node: u64, name: &str) -> FileRecord {
    FileRecord {
        id: FileId::new(VolumeId(4), node),
        parent: Some(FileId::new(VolumeId(4), 1)),
        path: PathBuf::from(format!("/tmp/a/{name}")),
        name: name.to_string(),
        kind: FileKind::File,
        len: 42,
        mode: 0o100644,
        owner: 501,
        group: 20,
        xattrs_digest: 99,
        created: None,
        modified: Some(UNIX_EPOCH + Duration::from_secs(10)),
        changed: None,
        hidden: false,
        tags: vec!["Important".to_string()],
        finder_comment: Some("notes".to_string()),
    }
}

fn has_store_atomic_temp_file(path: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let prefix = format!(".{file_name}.{}.", std::process::id());
    std::fs::read_dir(parent)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .any(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".tmp"))
        })
}
