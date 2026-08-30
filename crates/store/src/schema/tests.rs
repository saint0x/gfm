use super::*;
use crate::{
    metadata_postings_from_records, prefix_postings_from_records, read_content_postings,
    read_metadata_postings, read_records, write_content_postings, write_dictionary,
    write_fuzzy_postings, write_metadata_postings, write_prefix_postings, write_record_columns,
    write_records, write_substring_postings, ContentArchiveManifestEntry, ContentMergeTier,
    MetadataField, MetadataPosting,
};
use gfm_types::{ContentPosting, FileId, FileKind, FileRecord, GfmError, VolumeId};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[test]
fn inspects_current_archive_schemas_with_real_readers() {
    let records = temp_path("gfm-schema-records", "gfmidx");
    let columns = temp_path("gfm-schema-columns", "gfmcols");
    let metadata = temp_path("gfm-schema-metadata", "gfmmeta");
    let prefixes = temp_path("gfm-schema-prefixes", "gfmprefix");
    let substrings = temp_path("gfm-schema-substrings", "gfmsubstr");
    let fuzzy = temp_path("gfm-schema-fuzzy", "gfmfuzzy");
    let dictionary = temp_path("gfm-schema-dictionary", "gfmdict");
    let content = temp_path("gfm-schema-content", "gfmcontent");
    let manifest = temp_path("gfm-schema-manifest", "gfmmanifest");
    let rows = vec![record()];

    write_records(&records, &rows).unwrap();
    write_record_columns(&columns, &rows).unwrap();
    write_metadata_postings(&metadata, &metadata_postings_from_records(&rows)).unwrap();
    write_prefix_postings(&prefixes, &prefix_postings_from_records(&rows)).unwrap();
    write_substring_postings(&substrings, &crate::substring_postings_from_records(&rows)).unwrap();
    write_fuzzy_postings(&fuzzy, &crate::fuzzy_postings_from_records(&rows)).unwrap();
    write_dictionary(&dictionary, &crate::dictionary_terms_from_records(&rows)).unwrap();
    write_content_postings(
        &content,
        &[ContentPosting {
            term: "instant".to_string(),
            ids: vec![FileId::new(VolumeId(1), 2)],
            positions: Vec::new(),
        }],
    )
    .unwrap();
    ContentArchiveManifest::new(vec![ContentArchiveManifestEntry {
        tier: ContentMergeTier::Hot,
        path: content.clone(),
    }])
    .unwrap()
    .write(&manifest)
    .unwrap();

    for (kind, path) in [
        (ArchiveSchemaKind::Records, records.as_path()),
        (ArchiveSchemaKind::Columns, columns.as_path()),
        (ArchiveSchemaKind::Metadata, metadata.as_path()),
        (ArchiveSchemaKind::Prefixes, prefixes.as_path()),
        (ArchiveSchemaKind::Substrings, substrings.as_path()),
        (ArchiveSchemaKind::Fuzzy, fuzzy.as_path()),
        (ArchiveSchemaKind::Dictionary, dictionary.as_path()),
        (ArchiveSchemaKind::Content, content.as_path()),
        (ArchiveSchemaKind::ContentManifest, manifest.as_path()),
    ] {
        let report = inspect_archive_schema(kind, path);
        assert_eq!(report.status, ArchiveSchemaStatus::Current, "{report:?}");
        assert_eq!(report.schema.as_deref(), Some(kind.current_schema()));
    }

    for path in [
        records, columns, metadata, prefixes, substrings, fuzzy, dictionary, content, manifest,
    ] {
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn classifies_legacy_missing_unsupported_and_unreadable_archives() {
    let legacy = temp_path("gfm-schema-legacy", "gfmidx");
    let missing = temp_path("gfm-schema-missing", "gfmidx");
    let unsupported = temp_path("gfm-schema-unsupported", "gfmidx");
    let corrupt = temp_path("gfm-schema-corrupt", "gfmprefix");

    std::fs::write(
        &legacy,
        "gfm-store-v1\n1\t2\t0\tf\t1\t0\t0\t0\t0\t/tmp/legacy.txt\n",
    )
    .unwrap();
    std::fs::write(&unsupported, "not-gfm\nbody\n").unwrap();
    std::fs::write(&corrupt, PREFIX_CURRENT).unwrap();

    assert_eq!(
        inspect_archive_schema(ArchiveSchemaKind::Records, &legacy).status,
        ArchiveSchemaStatus::Legacy
    );
    assert_eq!(
        inspect_archive_schema(ArchiveSchemaKind::Records, &missing).status,
        ArchiveSchemaStatus::Missing
    );
    assert_eq!(
        inspect_archive_schema(ArchiveSchemaKind::Records, &unsupported).status,
        ArchiveSchemaStatus::Unsupported
    );
    let corrupt_report = inspect_archive_schema(ArchiveSchemaKind::Prefixes, &corrupt);
    assert_eq!(corrupt_report.status, ArchiveSchemaStatus::Unreadable);
    assert_eq!(corrupt_report.schema.as_deref(), Some("gfm-prefix-v1"));

    std::fs::remove_file(legacy).unwrap();
    std::fs::remove_file(unsupported).unwrap();
    std::fs::remove_file(corrupt).unwrap();
}

#[test]
fn archive_schema_inspection_surfaces_path_probe_failures() {
    let dir = temp_dir("gfm-schema-probe");
    let path = dir.join("archive-schema-unavailable".repeat(64));

    let report = inspect_archive_schema(ArchiveSchemaKind::Records, &path);

    assert_eq!(report.status, ArchiveSchemaStatus::Unreadable);
    assert!(report
        .detail
        .as_deref()
        .is_some_and(|detail| detail.contains("archive schema existence unavailable")));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn checked_archive_schema_inspection_honors_pre_cancelled_control_before_probe() {
    let dir = temp_dir("gfm-schema-cancel");
    let path = dir.join("archive-schema-unavailable".repeat(64));

    let result = inspect_archive_schema_checked(ArchiveSchemaKind::Records, &path, || {
        Err(GfmError::Cancelled)
    });

    assert!(matches!(result, Err(GfmError::Cancelled)));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn migrates_legacy_record_archive_to_current_schema_with_backup() {
    let dir = temp_dir("gfm-schema-record-migration");
    let records = dir.join("legacy.gfmidx");
    let backup = dir.join("backup");
    std::fs::write(
        &records,
        "gfm-store-v1\n1\t2\t0\tf\t1\t0\t0\t0\t0\t/tmp/legacy.txt\n",
    )
    .unwrap();

    let plan = plan_record_archive_migration(&records);
    assert_eq!(plan.action, RecordArchiveMigrationAction::Migrate);

    let migration = migrate_record_archive(&records, &backup).unwrap();

    assert_eq!(migration.migrated_records, 1);
    assert_eq!(migration.after.status, ArchiveSchemaStatus::Current);
    assert_eq!(read_records(&records).unwrap()[0].name, "legacy.txt");
    let backup_path = migration.backup_path.unwrap();
    assert!(backup_path.exists());
    assert_eq!(
        inspect_archive_schema(ArchiveSchemaKind::Records, &backup_path).status,
        ArchiveSchemaStatus::Legacy
    );

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn migrates_legacy_content_archive_to_current_schema_with_backup() {
    let dir = temp_dir("gfm-schema-content-migration");
    let content = dir.join("legacy.gfmcontent");
    let backup = dir.join("backup");
    let postings = vec![
        ContentPosting {
            term: "alpha".to_string(),
            ids: vec![FileId::new(VolumeId(1), 2), FileId::new(VolumeId(1), 4)],
            positions: Vec::new(),
        },
        ContentPosting {
            term: "beta".to_string(),
            ids: vec![FileId::new(VolumeId(2), 1)],
            positions: Vec::new(),
        },
    ];
    write_legacy_content_archive(&content, &postings);

    let plan = plan_content_archive_migration(&content);
    assert_eq!(plan.action, ContentArchiveMigrationAction::Migrate);

    let migration = migrate_content_archive(&content, &backup).unwrap();

    assert_eq!(migration.migrated_postings, 2);
    assert_eq!(migration.after.status, ArchiveSchemaStatus::Current);
    assert_eq!(read_content_postings(&content).unwrap(), postings);
    assert!(MmapContentArchive::open(&content).unwrap().is_checksummed());
    let backup_path = migration.backup_path.unwrap();
    assert!(backup_path.exists());
    assert_eq!(
        inspect_archive_schema(ArchiveSchemaKind::Content, &backup_path).status,
        ArchiveSchemaStatus::Legacy
    );

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn checked_content_archive_migration_cancels_before_legacy_posting_read() {
    let dir = temp_dir("gfm-schema-content-migration-cancel");
    let content = dir.join("legacy.gfmcontent");
    let backup = dir.join("backup");
    let postings = vec![ContentPosting {
        term: "alpha".to_string(),
        ids: vec![FileId::new(VolumeId(1), 2)],
        positions: Vec::new(),
    }];
    write_legacy_content_archive(&content, &postings);
    let original = std::fs::read(&content).unwrap();
    let mut checks = 0;

    let result = migrate_content_archive_checked(&content, &backup, || {
        checks += 1;
        if checks >= 8 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert_eq!(std::fs::read(&content).unwrap(), original);
    assert!(!backup.exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn checked_record_archive_migration_honors_pre_cancelled_control_before_probe() {
    let dir = temp_dir("gfm-schema-record-migration-pre-cancelled");
    let records = dir.join("legacy.gfmidx");
    let backup = dir.join("backup");
    std::fs::write(
        &records,
        "gfm-store-v1\n1\t2\t0\tf\t1\t0\t0\t0\t0\t/tmp/legacy.txt\n",
    )
    .unwrap();
    let original = std::fs::read(&records).unwrap();

    let result = migrate_record_archive_checked(&records, &backup, || Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert_eq!(std::fs::read(&records).unwrap(), original);
    assert!(!backup.exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn checked_metadata_archive_migration_honors_pre_cancelled_control_before_probe() {
    let dir = temp_dir("gfm-schema-metadata-migration-pre-cancelled");
    let metadata = dir.join("legacy.gfmmeta");
    let backup = dir.join("backup");
    let postings = vec![MetadataPosting {
        field: MetadataField::Tag,
        term: "important".to_string(),
        ids: vec![FileId::new(VolumeId(1), 2)],
    }];
    write_legacy_metadata_archive(&metadata, &postings);
    let original = std::fs::read(&metadata).unwrap();

    let result = migrate_metadata_archive_checked(&metadata, &backup, || Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert_eq!(std::fs::read(&metadata).unwrap(), original);
    assert!(!backup.exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn checked_record_archive_migration_plan_honors_pre_cancelled_control_before_probe() {
    let path = temp_path("gfm-schema-record-plan-pre-cancelled", "gfmidx");
    let result = plan_record_archive_migration_checked(&path, || Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(!path.exists());
}

#[test]
fn checked_content_archive_migration_plan_honors_pre_cancelled_control_before_probe() {
    let path = temp_path("gfm-schema-content-plan-pre-cancelled", "gfmcontent");
    let result = plan_content_archive_migration_checked(&path, || Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(!path.exists());
}

#[test]
fn checked_metadata_archive_migration_plan_honors_pre_cancelled_control_before_probe() {
    let path = temp_path("gfm-schema-metadata-plan-pre-cancelled", "gfmmeta");
    let result = plan_metadata_archive_migration_checked(&path, || Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(!path.exists());
}

#[test]
fn migrates_legacy_metadata_archive_to_current_schema_with_backup() {
    let dir = temp_dir("gfm-schema-metadata-migration");
    let metadata = dir.join("legacy.gfmmeta");
    let backup = dir.join("backup");
    let postings = vec![
        MetadataPosting {
            field: MetadataField::Tag,
            term: "important".to_string(),
            ids: vec![FileId::new(VolumeId(1), 2), FileId::new(VolumeId(1), 4)],
        },
        MetadataPosting {
            field: MetadataField::Comment,
            term: "handoff".to_string(),
            ids: vec![FileId::new(VolumeId(2), 1)],
        },
    ];
    write_legacy_metadata_archive(&metadata, &postings);

    let plan = plan_metadata_archive_migration(&metadata);
    assert_eq!(plan.action, MetadataArchiveMigrationAction::Migrate);

    let migration = migrate_metadata_archive(&metadata, &backup).unwrap();

    assert_eq!(migration.migrated_postings, 2);
    assert_eq!(migration.after.status, ArchiveSchemaStatus::Current);
    assert_eq!(read_metadata_postings(&metadata).unwrap(), postings);
    assert!(MmapMetadataArchive::open(&metadata)
        .unwrap()
        .is_checksummed());
    let backup_path = migration.backup_path.unwrap();
    assert!(backup_path.exists());
    assert_eq!(
        inspect_archive_schema(ArchiveSchemaKind::Metadata, &backup_path).status,
        ArchiveSchemaStatus::Legacy
    );

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn current_record_archive_migration_is_noop() {
    let dir = temp_dir("gfm-schema-record-migration-current");
    let records = dir.join("current.gfmidx");
    let backup = dir.join("backup");
    write_records(&records, &[record()]).unwrap();

    let migration = migrate_record_archive(&records, &backup).unwrap();

    assert_eq!(migration.migrated_records, 0);
    assert_eq!(migration.after.status, ArchiveSchemaStatus::Current);
    assert!(migration.backup_path.is_none());
    assert!(!backup.exists());

    std::fs::remove_dir_all(dir).unwrap();
}

fn record() -> FileRecord {
    FileRecord {
        id: FileId::new(VolumeId(1), 2),
        parent: Some(FileId::new(VolumeId(1), 1)),
        path: PathBuf::from("/tmp/schema/Instant Search.md"),
        name: "Instant Search.md".to_string(),
        kind: FileKind::File,
        len: 128,
        mode: 0o100644,
        owner: 501,
        group: 20,
        xattrs_digest: 7,
        created: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
        modified: Some(UNIX_EPOCH + Duration::from_secs(2)),
        changed: None,
        hidden: false,
        tags: vec!["fast".to_string()],
        finder_comment: Some("instant lookup".to_string()),
    }
}

fn temp_path(prefix: &str, extension: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    path.push(format!("{prefix}-{nanos}.{extension}"));
    path
}

fn temp_dir(prefix: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    path.push(format!("{prefix}-{nanos}"));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn write_legacy_content_archive(path: &Path, postings: &[ContentPosting]) {
    let mut bytes = Vec::new();
    bytes.extend(b"gfm-content-v1\n");
    push_varint(&mut bytes, postings.len() as u64);
    for posting in postings {
        push_varint(&mut bytes, posting.term.len() as u64);
        bytes.extend(posting.term.as_bytes());
        write_legacy_file_ids(&mut bytes, &posting.ids);
    }
    std::fs::write(path, bytes).unwrap();
}

fn write_legacy_metadata_archive(path: &Path, postings: &[MetadataPosting]) {
    let mut bytes = Vec::new();
    bytes.extend(b"gfm-metadata-v1\n");
    push_varint(&mut bytes, postings.len() as u64);
    let mut directory = Vec::new();
    let mut postings = postings.to_vec();
    postings.sort_by(|left, right| {
        (metadata_field_code(left.field), left.term.as_str())
            .cmp(&(metadata_field_code(right.field), right.term.as_str()))
    });
    for posting in &postings {
        let offset = bytes.len() as u64;
        write_legacy_metadata_posting(&mut bytes, posting);
        directory.push((
            posting.field,
            posting.term.clone(),
            offset,
            bytes.len() as u64 - offset,
        ));
    }
    let directory_offset = bytes.len() as u64;
    push_varint(&mut bytes, directory.len() as u64);
    for (field, term, offset, len) in directory {
        bytes.push(metadata_field_code(field));
        push_varint(&mut bytes, term.len() as u64);
        bytes.extend(term.as_bytes());
        push_varint(&mut bytes, offset);
        push_varint(&mut bytes, len);
    }
    bytes.extend(directory_offset.to_le_bytes());
    bytes.extend(b"gfm-metadata-index-v1\n");
    std::fs::write(path, bytes).unwrap();
}

fn write_legacy_metadata_posting(bytes: &mut Vec<u8>, posting: &MetadataPosting) {
    bytes.push(metadata_field_code(posting.field));
    push_varint(bytes, posting.term.len() as u64);
    bytes.extend(posting.term.as_bytes());
    write_legacy_file_ids(bytes, &posting.ids);
}

fn metadata_field_code(field: MetadataField) -> u8 {
    match field {
        MetadataField::Tag => b't',
        MetadataField::Comment => b'c',
    }
}

fn write_legacy_file_ids(bytes: &mut Vec<u8>, ids: &[FileId]) {
    let mut ids = ids.to_vec();
    ids.sort();
    push_varint(bytes, ids.len() as u64);
    let mut previous = FileId::new(VolumeId(0), 0);
    for id in ids {
        push_varint(bytes, id.volume.0.saturating_sub(previous.volume.0));
        let node_delta = if id.volume == previous.volume {
            id.node.saturating_sub(previous.node)
        } else {
            id.node
        };
        push_varint(bytes, node_delta);
        previous = id;
    }
}

fn push_varint(bytes: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        bytes.push(((value as u8) & 0x7f) | 0x80);
        value >>= 7;
    }
    bytes.push(value as u8);
}
