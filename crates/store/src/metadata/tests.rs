use super::*;
use gfm_types::{FileKind, SecondaryMetadataRecord};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn mmap_metadata_archive_reads_tags_and_comment_terms() {
    let path = temp_path("gfm-metadata-mmap", "gfmmeta");
    let first = FileRecord {
        id: FileId::new(VolumeId(4), 12),
        parent: None,
        path: PathBuf::from("/tmp/a/report.md"),
        name: "report.md".to_string(),
        kind: FileKind::File,
        len: 1,
        mode: 0,
        owner: 0,
        group: 0,
        xattrs_digest: 0,
        created: None,
        modified: None,
        changed: None,
        hidden: false,
        tags: vec!["Important".to_string(), "Client".to_string()],
        finder_comment: Some("client handoff notes".to_string()),
    };
    let second = FileRecord {
        id: FileId::new(VolumeId(4), 13),
        tags: vec!["Later".to_string()],
        finder_comment: Some("archived later".to_string()),
        path: PathBuf::from("/tmp/a/archive.md"),
        name: "archive.md".to_string(),
        ..first.clone()
    };
    let postings = metadata_postings_from_records(&[first.clone(), second.clone()]);

    write_metadata_postings(&path, &postings).unwrap();
    let read = read_metadata_postings(&path).unwrap();
    let archive = MmapMetadataArchive::open(&path).unwrap();

    assert_eq!(read, postings);
    assert_eq!(archive.postings().unwrap(), postings);
    assert!(archive.indexed_terms() >= 5);
    assert!(archive.mapped_len() > 0);
    assert_eq!(
        archive.ids_for(MetadataField::Tag, "important").unwrap(),
        vec![first.id]
    );
    assert_eq!(
        archive.ids_for(MetadataField::Comment, "client").unwrap(),
        vec![first.id]
    );
    assert_eq!(
        archive.ids_for(MetadataField::Tag, "missing").unwrap(),
        Vec::new()
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn checked_metadata_postings_read_honors_pre_cancelled_control_before_file_open() {
    let path = temp_path("gfm-metadata-pre-cancelled-read", "gfmmeta");
    let result = read_metadata_postings_checked(&path, || Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(!path.exists());
}

#[test]
fn checked_metadata_postings_read_can_cancel_during_checksum_load() {
    let path = temp_path("gfm-metadata-checksum-cancel", "gfmmeta");
    let posting = MetadataPosting {
        field: MetadataField::Tag,
        term: "important".to_string(),
        ids: (0..40_000)
            .map(|node| FileId::new(VolumeId(4), node))
            .collect(),
    };
    write_metadata_postings(&path, &[posting]).unwrap();
    let mut checks = 0usize;

    let result = read_metadata_postings_checked(&path, || {
        checks += 1;
        if checks >= 8 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(checks >= 8);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn metadata_postings_merge_secondary_spotlight_records() {
    let primary = FileRecord {
        id: FileId::new(VolumeId(4), 12),
        parent: None,
        path: PathBuf::from("/tmp/a/report.md"),
        name: "report.md".to_string(),
        kind: FileKind::File,
        len: 1,
        mode: 0,
        owner: 0,
        group: 0,
        xattrs_digest: 0,
        created: None,
        modified: None,
        changed: None,
        hidden: false,
        tags: vec!["Important".to_string()],
        finder_comment: Some("client handoff".to_string()),
    };
    let secondary = SecondaryMetadataRecord {
        id: primary.id,
        tags: vec!["Important".to_string(), "Blue".to_string()],
        comments: vec![
            "PDF document".to_string(),
            "https://example.com/source".to_string(),
        ],
    };

    let postings =
        metadata_postings_from_records_and_secondary(std::slice::from_ref(&primary), &[secondary]);

    assert_eq!(
        postings
            .iter()
            .find(|posting| posting.field == MetadataField::Tag && posting.term == "important")
            .unwrap()
            .ids,
        vec![primary.id]
    );
    assert_eq!(
        postings
            .iter()
            .find(|posting| posting.field == MetadataField::Tag && posting.term == "blue")
            .unwrap()
            .ids,
        vec![primary.id]
    );
    assert!(postings
        .iter()
        .any(|posting| posting.field == MetadataField::Comment && posting.term == "pdf"));
    assert!(postings
        .iter()
        .any(|posting| posting.field == MetadataField::Comment && posting.term == "example"));
}

#[test]
fn mmap_metadata_archive_reads_one_compressed_id_block() {
    let path = temp_path("gfm-metadata-blocked", "gfmmeta");
    let ids = (0..300)
        .map(|node| FileId::new(VolumeId(12), 10_000 + node))
        .collect::<Vec<_>>();
    let posting = MetadataPosting {
        field: MetadataField::Tag,
        term: "important".to_string(),
        ids: ids.clone(),
    };

    write_metadata_postings(&path, std::slice::from_ref(&posting)).unwrap();
    let archive = MmapMetadataArchive::open(&path).unwrap();
    let block = archive
        .id_block_for(MetadataField::Tag, "important", 1)
        .unwrap();

    assert_eq!(
        archive.ids_for(MetadataField::Tag, "important").unwrap(),
        ids
    );
    assert_eq!(
        archive
            .postings_for(MetadataField::Tag, ["missing", "important", "important"])
            .unwrap(),
        vec![posting]
    );
    assert_eq!(block.len(), 128);
    assert_eq!(block[0], FileId::new(VolumeId(12), 10_128));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn mmap_metadata_archive_reads_bounded_selected_postings_for_query_import() {
    let path = temp_path("gfm-metadata-bounded-postings", "gfmmeta");
    let postings = vec![
        MetadataPosting {
            field: MetadataField::Tag,
            term: "important".to_string(),
            ids: (0..8)
                .map(|node| FileId::new(VolumeId(12), 10_000 + node))
                .collect(),
        },
        MetadataPosting {
            field: MetadataField::Comment,
            term: "handoff".to_string(),
            ids: vec![FileId::new(VolumeId(7), 1), FileId::new(VolumeId(7), 2)],
        },
    ];

    write_metadata_postings(&path, &postings).unwrap();
    let archive = MmapMetadataArchive::open(&path).unwrap();
    let (ids, truncated) = archive
        .ids_for_limit(MetadataField::Tag, "IMPORTANT", 3)
        .unwrap();
    let limited = archive
        .postings_for_limit(MetadataField::Tag, ["missing", "important", "important"], 3)
        .unwrap();
    let comments = archive
        .postings_for_limit(MetadataField::Comment, ["handoff"], 3)
        .unwrap();

    assert!(truncated);
    assert_eq!(ids.len(), 3);
    assert_eq!(ids[0], FileId::new(VolumeId(12), 10_000));
    assert_eq!(limited.len(), 1);
    assert_eq!(limited[0].term, "important");
    assert_eq!(limited[0].ids.len(), 3);
    assert_eq!(comments, vec![postings[1].clone()]);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn mmap_metadata_archive_checked_lookup_honors_pre_cancelled_control() {
    let path = temp_path("gfm-metadata-checked-lookup-cancel", "gfmmeta");
    write_metadata_postings(
        &path,
        &[MetadataPosting {
            field: MetadataField::Tag,
            term: "important".to_string(),
            ids: vec![FileId::new(VolumeId(12), 10_000)],
        }],
    )
    .unwrap();
    let archive = MmapMetadataArchive::open(&path).unwrap();

    assert!(matches!(
        archive.ids_for_checked(MetadataField::Tag, "important", || Err(GfmError::Cancelled)),
        Err(GfmError::Cancelled)
    ));
    assert!(matches!(
        archive.postings_for_limit_checked(MetadataField::Tag, ["important"], 8, || Err(
            GfmError::Cancelled
        )),
        Err(GfmError::Cancelled)
    ));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn mmap_metadata_archive_reads_bounded_sorted_terms_in_one_pass() {
    let path = temp_path("gfm-metadata-batch-postings", "gfmmeta");
    let postings = vec![
        MetadataPosting {
            field: MetadataField::Tag,
            term: "cold".to_string(),
            ids: vec![FileId::new(VolumeId(3), 1)],
        },
        MetadataPosting {
            field: MetadataField::Tag,
            term: "important".to_string(),
            ids: (0..5)
                .map(|node| FileId::new(VolumeId(12), 20_000 + node))
                .collect(),
        },
        MetadataPosting {
            field: MetadataField::Comment,
            term: "handoff".to_string(),
            ids: (0..4)
                .map(|node| FileId::new(VolumeId(7), 100 + node))
                .collect(),
        },
    ];

    write_metadata_postings(&path, &postings).unwrap();
    let archive = MmapMetadataArchive::open(&path).unwrap();
    let tags = archive
        .postings_for_sorted_terms_limit(
            MetadataField::Tag,
            ["cold", "important", "important", "missing"],
            3,
        )
        .unwrap();
    let comments = archive
        .postings_for_sorted_terms_limit(MetadataField::Comment, ["handoff"], 2)
        .unwrap();

    assert_eq!(tags.len(), 2);
    assert_eq!(tags[0].posting.term, "cold");
    assert_eq!(tags[0].posting.ids, postings[0].ids);
    assert!(!tags[0].truncated);
    assert_eq!(tags[1].posting.term, "important");
    assert_eq!(tags[1].posting.ids, postings[1].ids[..3]);
    assert!(tags[1].truncated);
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].posting.term, "handoff");
    assert_eq!(comments[0].posting.ids, postings[2].ids[..2]);
    assert!(comments[0].truncated);
    assert!(archive
        .postings_for_sorted_terms_limit(MetadataField::Tag, ["important", "cold"], 3)
        .is_err());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn checksummed_metadata_archive_rejects_corruption() {
    let path = temp_path("gfm-metadata-checksum", "gfmmeta");
    let posting = MetadataPosting {
        field: MetadataField::Tag,
        term: "important".to_string(),
        ids: vec![FileId::new(VolumeId(12), 10_000)],
    };

    write_metadata_postings(&path, &[posting]).unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    let offset = bytes
        .windows(b"important".len())
        .position(|window| window == b"important")
        .expect("archive should contain the test term");
    bytes[offset] = b'z';
    std::fs::write(&path, bytes).unwrap();

    let read_error = read_metadata_postings(&path).unwrap_err().to_string();
    let mmap_error = MmapMetadataArchive::open(&path).unwrap_err().to_string();

    assert!(read_error.contains("checksum mismatch"), "{read_error}");
    assert!(mmap_error.contains("checksum mismatch"), "{mmap_error}");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn mmap_metadata_archive_checked_open_honors_pre_cancelled_control_before_file_open() {
    let path = temp_path("gfm-metadata-open-cancel", "gfmmeta");

    let result = MmapMetadataArchive::open_checked(&path, || Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(!path.exists());
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
