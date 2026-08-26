use super::*;
use gfm_content::ExtractionQuarantine;
use gfm_fs::FinderMetadataReport;
use gfm_store::{
    fuzzy_postings_from_records, metadata_postings_from_records, prefix_postings_from_records,
    substring_postings_from_records, write_content_postings, write_fuzzy_postings,
    write_metadata_postings, write_prefix_postings, write_record_columns, write_substring_postings,
    FuzzyPosting, MetadataField, MmapMetadataArchive, PrefixPosting, SubstringPosting,
};
use gfm_types::{
    ContentPositions, ContentPosting, FileKind, MatchReason, SecondaryMetadataRecord, VolumeId,
};
use std::collections::HashSet;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn extracts_content_terms_for_query_sidecar_loading() {
    assert_eq!(
        content_query_terms(r#"tag:Important "Launch Notes" near:4:alpha_beta:gamma"#),
        vec!["alpha", "beta", "gamma", "launch", "notes"]
    );
    assert_eq!(comment_query_terms("launch notes"), vec!["launch", "notes"]);
    assert_eq!(
        tag_query_terms("tag:Important -tag:Cold"),
        vec!["important"]
    );
    assert_eq!(prefix_query_terms("project-plan"), vec!["plan", "project"]);
    assert_eq!(
        substring_candidate_grams("report"),
        vec!["epo", "ort", "por", "rep"]
    );
    assert!(fuzzy_query_keys("project").contains(&"project".to_string()));
    assert!(content_query_terms("tag:Important kind:file").is_empty());
}

#[test]
fn builds_saves_loads_and_searches_snapshot() {
    let root = unique_temp_dir("gfm-index-root");
    let output = unique_temp_path("gfm-index", "gfmidx");
    fs::create_dir_all(root.join("Design")).unwrap();
    fs::write(root.join("Design").join("FinderParity.md"), "notes").unwrap();

    let indexer = Indexer::default();
    let snapshot = indexer.build(&root).unwrap();
    snapshot.save(&output).unwrap();
    let loaded = indexer.load(&output).unwrap();
    let hits = loaded.search("parity", 5);

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.name, "FinderParity.md");

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(output).unwrap();
}

#[test]
fn query_session_reuses_loaded_index_for_repeated_queries() {
    let root = unique_temp_dir("gfm-query-session-root");
    let output = unique_temp_path("gfm-query-session", "gfmidx");
    fs::create_dir_all(root.join("Design")).unwrap();
    fs::write(root.join("Design").join("FinderParity.md"), "notes").unwrap();
    fs::write(root.join("LatencyNotes.md"), "fast search").unwrap();

    let indexer = Indexer::default();
    let snapshot = indexer.build(&root).unwrap();
    let records = snapshot.records.len();
    snapshot.save(&output).unwrap();
    let session = indexer.load_query_session(&output).unwrap();

    let parity = session.search("parity", 5);
    let latency = session.search("latency", 5);

    assert_eq!(session.indexed_records(), records);
    assert_eq!(parity.len(), 1);
    assert_eq!(parity[0].record.name, "FinderParity.md");
    assert_eq!(latency.len(), 1);
    assert_eq!(latency[0].record.name, "LatencyNotes.md");

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(output).unwrap();
}

#[test]
fn snapshot_query_session_streams_without_rebuilding_per_query() {
    let root = unique_temp_dir("gfm-query-session-stream-root");
    fs::write(root.join("needle.md"), "metadata match").unwrap();
    let snapshot = Indexer::default().build(&root).unwrap();
    let records = snapshot.records.len();
    let session = snapshot.query_session();

    let batches = session.stream_search("needle", 10).unwrap();

    assert_eq!(session.indexed_records(), records);
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].stage, SearchStreamStage::Hot);
    assert_eq!(batches[0].hits[0].record.name, "needle.md");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn query_session_honors_cancellation() {
    let root = unique_temp_dir("gfm-query-session-cancel-root");
    fs::write(root.join("notes.md"), "needle").unwrap();
    let snapshot = Indexer::default().build(&root).unwrap();
    let session = snapshot.query_session();
    let cancellation = Cancellation::default();
    cancellation.cancel();

    let result = session.search_cancellable("needle", 5, &cancellation);

    assert!(matches!(result, Err(GfmError::Cancelled)));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn publishes_secondary_metadata_to_mmap_archive() {
    let metadata_path = unique_temp_path("gfm-secondary-metadata", "gfmmeta");
    let record = FileRecord {
        id: FileId::new(VolumeId(7), 99),
        parent: None,
        path: PathBuf::from("/tmp/report.md"),
        name: "report.md".to_string(),
        kind: FileKind::File,
        len: 10,
        mode: 0,
        owner: 0,
        group: 0,
        xattrs_digest: 0,
        created: None,
        modified: None,
        changed: None,
        hidden: false,
        tags: vec!["Primary".to_string()],
        finder_comment: Some("local note".to_string()),
    };
    let secondary = SecondaryMetadataRecord {
        id: record.id,
        tags: vec!["Spotlight".to_string()],
        comments: vec!["Markdown Document imported from example.com".to_string()],
    };

    let report = publish_secondary_metadata(
        std::slice::from_ref(&record),
        std::slice::from_ref(&secondary),
        &metadata_path,
    )
    .unwrap();
    let archive = MmapMetadataArchive::open(&metadata_path).unwrap();

    assert_eq!(report.primary_records, 1);
    assert_eq!(report.secondary_records, 1);
    assert!(report.postings >= 6);
    assert_eq!(
        archive.ids_for(MetadataField::Tag, "spotlight").unwrap(),
        vec![record.id]
    );
    assert_eq!(
        archive.ids_for(MetadataField::Comment, "markdown").unwrap(),
        vec![record.id]
    );
    assert!(report
        .as_tsv()
        .starts_with("secondary-metadata-publication\t"));
    fs::remove_file(metadata_path).unwrap();
}

#[test]
fn publishes_finder_visible_metadata_to_query_sidecar() {
    let root = unique_temp_dir("gfm-finder-visible-metadata-root");
    let metadata_path = unique_temp_path("gfm-finder-visible-metadata", "gfmmeta");
    let file = root.join("LaunchNotes.md");
    fs::write(&file, "notes").unwrap();
    let report = FinderMetadataReport::read_path(&file).unwrap();
    let secondary = report.secondary_metadata_record();
    let record = report.record.clone();

    publish_secondary_metadata(
        std::slice::from_ref(&record),
        std::slice::from_ref(&secondary),
        &metadata_path,
    )
    .unwrap();
    let archive = MmapMetadataArchive::open(&metadata_path).unwrap();

    assert_eq!(
        archive
            .ids_for(MetadataField::Comment, "launchnotes")
            .unwrap(),
        vec![record.id]
    );
    assert_eq!(
        archive.ids_for(MetadataField::Comment, "document").unwrap(),
        vec![record.id]
    );
    fs::remove_dir_all(root).unwrap();
    fs::remove_file(metadata_path).unwrap();
}

#[test]
fn live_index_builds_from_records_with_columns_in_one_pass() {
    let record = FileRecord {
        id: FileId::new(VolumeId(1), 1),
        parent: None,
        path: PathBuf::from("/tmp/original.txt"),
        name: "original.txt".to_string(),
        kind: FileKind::File,
        len: 0,
        created: Some(UNIX_EPOCH),
        modified: Some(SystemTime::now()),
        changed: Some(SystemTime::now()),
        mode: 0o644,
        owner: 501,
        group: 20,
        hidden: false,
        tags: Vec::new(),
        finder_comment: None,
        xattrs_digest: 0,
    };

    let (live, applied) = LiveIndex::from_records_with_columns(
        vec![record.clone()],
        vec![SearchRecordColumns {
            id: record.id,
            name: "cached.md".to_string(),
            path: "/tmp/cached.md".to_string(),
            extension: Some("md".to_string()),
            tags: vec!["Important".to_string()],
            comment: Some("Launch Notes".to_string()),
        }],
    );

    assert_eq!(applied, 1);
    assert!(live.search("original", 5).is_empty());
    assert_eq!(
        live.search("cached tag:important launch ext:md", 5).len(),
        1
    );
}

#[test]
fn live_index_builds_records_with_deferred_sidecar_terms() {
    let record = FileRecord {
        id: FileId::new(VolumeId(1), 1),
        parent: None,
        path: PathBuf::from("/tmp/DeferredSidecar.md"),
        name: "DeferredSidecar.md".to_string(),
        kind: FileKind::File,
        len: 0,
        created: Some(UNIX_EPOCH),
        modified: Some(SystemTime::now()),
        changed: Some(SystemTime::now()),
        mode: 0o644,
        owner: 501,
        group: 20,
        hidden: false,
        tags: vec!["Important".to_string()],
        finder_comment: Some("Launch Notes".to_string()),
        xattrs_digest: 0,
    };

    let live = LiveIndex::from_records_deferred_sidecars(vec![record]);

    assert_eq!(live.indexed_records(), 1);
    assert_eq!(live.search("deferredsidecar", 5).len(), 1);
    assert!(live.search("defered", 5).is_empty());
}

#[test]
fn live_index_imports_fuzzy_sidecar_after_column_build() {
    let record = FileRecord {
        id: FileId::new(VolumeId(1), 1),
        parent: None,
        path: PathBuf::from("/tmp/original.txt"),
        name: "original.txt".to_string(),
        kind: FileKind::File,
        len: 0,
        created: Some(UNIX_EPOCH),
        modified: Some(SystemTime::now()),
        changed: Some(SystemTime::now()),
        mode: 0o644,
        owner: 501,
        group: 20,
        hidden: false,
        tags: Vec::new(),
        finder_comment: None,
        xattrs_digest: 0,
    };

    let (live, applied, fuzzy_keys) = LiveIndex::from_records_with_columns_and_fuzzy(
        vec![record.clone()],
        vec![SearchRecordColumns {
            id: record.id,
            name: "needl.md".to_string(),
            path: "/tmp/needl.md".to_string(),
            extension: Some("md".to_string()),
            tags: Vec::new(),
            comment: None,
        }],
        vec![SearchFuzzyPosting {
            key: "needl".to_string(),
            terms: vec!["needl".to_string()],
        }],
    );

    assert_eq!(applied, 1);
    assert_eq!(fuzzy_keys, 1);
    assert_eq!(live.search("needle", 5).len(), 1);
}

#[test]
fn live_index_imports_metadata_prefix_and_fuzzy_sidecars_after_column_build() {
    let record = FileRecord {
        id: FileId::new(VolumeId(1), 1),
        parent: None,
        path: PathBuf::from("/tmp/original.txt"),
        name: "original.txt".to_string(),
        kind: FileKind::File,
        len: 0,
        created: Some(UNIX_EPOCH),
        modified: Some(SystemTime::now()),
        changed: Some(SystemTime::now()),
        mode: 0o644,
        owner: 501,
        group: 20,
        hidden: false,
        tags: Vec::new(),
        finder_comment: None,
        xattrs_digest: 0,
    };

    let (live, applied, metadata_keys, prefix_keys, substring_keys, fuzzy_keys, content_keys) =
        LiveIndex::from_records_with_sidecars(
            vec![record.clone()],
            vec![SearchRecordColumns {
                id: record.id,
                name: "project-needl.md".to_string(),
                path: "/tmp/project-needl.md".to_string(),
                extension: Some("md".to_string()),
                tags: vec!["Important".to_string()],
                comment: Some("launch notes".to_string()),
            }],
            vec![
                SearchMetadataPosting {
                    field: SearchMetadataField::Tag,
                    term: "Important".to_string(),
                    ids: vec![record.id],
                },
                SearchMetadataPosting {
                    field: SearchMetadataField::Comment,
                    term: "launch".to_string(),
                    ids: vec![record.id],
                },
            ],
            vec![SearchPrefixPosting {
                prefix: "proj".to_string(),
                ids: vec![record.id],
            }],
            vec![SearchSubstringPosting {
                gram: "eed".to_string(),
                ids: vec![record.id],
            }],
            vec![SearchFuzzyPosting {
                key: "needl".to_string(),
                terms: vec!["needl".to_string()],
            }],
            vec![ContentPosting {
                term: "bodymarker".to_string(),
                ids: vec![record.id],
                positions: Vec::new(),
            }],
        );

    assert_eq!(applied, 1);
    assert_eq!(metadata_keys, 2);
    assert_eq!(prefix_keys, 1);
    assert_eq!(substring_keys, 1);
    assert_eq!(fuzzy_keys, 1);
    assert_eq!(content_keys, 1);
    assert_eq!(live.search("tag:Important", 5).len(), 1);
    assert_eq!(live.search("launch", 5).len(), 1);
    assert_eq!(live.search("proj", 5).len(), 1);
    assert_eq!(live.search("eed", 5).len(), 1);
    assert_eq!(live.search("needle", 5).len(), 1);
    assert_eq!(live.search("bodymarker", 5).len(), 1);
}

#[test]
fn live_index_queries_prefix_and_fuzzy_archive_lookup_without_importing_sidecars() {
    let record = FileRecord {
        id: FileId::new(VolumeId(1), 1),
        parent: None,
        path: PathBuf::from("/tmp/original.txt"),
        name: "original.txt".to_string(),
        kind: FileKind::File,
        len: 0,
        created: Some(UNIX_EPOCH),
        modified: Some(SystemTime::now()),
        changed: Some(SystemTime::now()),
        mode: 0o644,
        owner: 501,
        group: 20,
        hidden: false,
        tags: Vec::new(),
        finder_comment: None,
        xattrs_digest: 0,
    };
    let prefixes = unique_temp_path("gfm-prefix-archive-lookup", "gfmprefix");
    let substrings = unique_temp_path("gfm-substring-archive-lookup", "gfmsubstr");
    let fuzzy = unique_temp_path("gfm-fuzzy-archive-lookup", "gfmfuzzy");

    write_prefix_postings(
        &prefixes,
        &[PrefixPosting {
            prefix: "proj".to_string(),
            ids: vec![record.id],
        }],
    )
    .unwrap();
    write_substring_postings(
        &substrings,
        &[SubstringPosting {
            gram: "eed".to_string(),
            ids: vec![record.id],
        }],
    )
    .unwrap();
    write_fuzzy_postings(
        &fuzzy,
        &[FuzzyPosting {
            key: "needl".to_string(),
            terms: vec!["needl".to_string()],
        }],
    )
    .unwrap();

    let (live, applied, metadata_keys, prefix_keys, substring_keys, fuzzy_keys, content_keys) =
        LiveIndex::from_records_with_sidecars(
            vec![record.clone()],
            vec![SearchRecordColumns {
                id: record.id,
                name: "project-needl.md".to_string(),
                path: "/tmp/project-needl.md".to_string(),
                extension: Some("md".to_string()),
                tags: Vec::new(),
                comment: None,
            }],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
    let lookup = SearchArchiveLookup::open(&prefixes, &substrings, &fuzzy).unwrap();

    assert_eq!(applied, 1);
    assert_eq!(metadata_keys, 0);
    assert_eq!(prefix_keys, 0);
    assert_eq!(substring_keys, 0);
    assert_eq!(fuzzy_keys, 0);
    assert_eq!(content_keys, 0);
    assert_eq!(lookup.indexed_prefixes(), 1);
    assert_eq!(lookup.indexed_substring_grams(), 1);
    assert_eq!(lookup.indexed_fuzzy_keys(), 1);
    assert_eq!(live.search("proj", 5).len(), 0);
    assert_eq!(live.search("needle", 5).len(), 0);
    let prefix_hits = live.search_with_lookup("proj", 5, &lookup).unwrap();
    let substring_hits = live.search_with_lookup("eed", 5, &lookup).unwrap();
    let fuzzy_hits = live.search_with_lookup("needle", 5, &lookup).unwrap();
    assert_eq!(prefix_hits.len(), 1);
    assert_eq!(prefix_hits[0].reason, MatchReason::PrefixName);
    assert_eq!(substring_hits.len(), 1);
    assert_eq!(substring_hits[0].reason, MatchReason::SubstringName);
    assert_eq!(fuzzy_hits.len(), 1);
    assert_eq!(fuzzy_hits[0].reason, MatchReason::FuzzyName);
    let budget_lookup = SearchArchiveLookup::open(&prefixes, &substrings, &fuzzy).unwrap();
    let first_report = live
        .search_with_lookup_budget(
            "proj eed needle",
            5,
            &budget_lookup,
            SearchLookupBudget::default(),
        )
        .unwrap();
    let second_report = live
        .search_with_lookup_budget(
            "proj eed needle",
            5,
            &budget_lookup,
            SearchLookupBudget::default(),
        )
        .unwrap();
    assert!(first_report.lookup.prefix_cache_misses > 0);
    assert!(first_report.lookup.substring_cache_misses > 0);
    assert!(first_report.lookup.fuzzy_cache_misses > 0);
    assert!(second_report.lookup.prefix_cache_hits > 0);
    assert!(second_report.lookup.substring_cache_hits > 0);
    assert!(second_report.lookup.fuzzy_cache_hits > 0);

    fs::remove_file(prefixes).unwrap();
    fs::remove_file(substrings).unwrap();
    fs::remove_file(fuzzy).unwrap();
}

#[test]
fn query_sidecar_loader_hydrates_only_candidate_records() {
    let records_path = unique_temp_path("gfm-index-sidecar-candidates", "gfmidx");
    let columns_path = unique_temp_path("gfm-index-sidecar-candidates", "gfmcols");
    let metadata_path = unique_temp_path("gfm-index-sidecar-candidates", "gfmmeta");
    let prefixes_path = unique_temp_path("gfm-index-sidecar-candidates", "gfmprefix");
    let substrings_path = unique_temp_path("gfm-index-sidecar-candidates", "gfmsubstr");
    let fuzzy_path = unique_temp_path("gfm-index-sidecar-candidates", "gfmfuzzy");
    let content_path = unique_temp_path("gfm-index-sidecar-candidates", "gfmcontent");
    let hot = FileRecord {
        id: FileId::new(VolumeId(1), 1),
        parent: None,
        path: PathBuf::from("/tmp/hot/tagged.md"),
        name: "tagged.md".to_string(),
        kind: FileKind::File,
        len: 11,
        created: Some(UNIX_EPOCH),
        modified: Some(UNIX_EPOCH),
        changed: Some(UNIX_EPOCH),
        mode: 0o644,
        owner: 501,
        group: 20,
        hidden: false,
        tags: vec!["Important".to_string()],
        finder_comment: Some("bodymarker".to_string()),
        xattrs_digest: 0,
    };
    let cold = FileRecord {
        id: FileId::new(VolumeId(1), 2),
        parent: None,
        path: PathBuf::from("/tmp/cold/other.md"),
        name: "other.md".to_string(),
        kind: FileKind::File,
        len: 11,
        created: Some(UNIX_EPOCH),
        modified: Some(UNIX_EPOCH),
        changed: Some(UNIX_EPOCH),
        mode: 0o644,
        owner: 501,
        group: 20,
        hidden: false,
        tags: vec!["Cold".to_string()],
        finder_comment: Some("coldmarker".to_string()),
        xattrs_digest: 0,
    };
    let records = vec![hot.clone(), cold.clone()];
    write_records(&records_path, &records).unwrap();
    write_record_columns(&columns_path, &records).unwrap();
    write_metadata_postings(&metadata_path, &metadata_postings_from_records(&records)).unwrap();
    write_prefix_postings(&prefixes_path, &prefix_postings_from_records(&records)).unwrap();
    write_substring_postings(&substrings_path, &substring_postings_from_records(&records)).unwrap();
    write_fuzzy_postings(&fuzzy_path, &fuzzy_postings_from_records(&records)).unwrap();
    write_content_postings(
        &content_path,
        &[
            ContentPosting {
                term: "bodymarker".to_string(),
                ids: vec![hot.id],
                positions: vec![ContentPositions {
                    id: hot.id,
                    positions: vec![0],
                }],
            },
            ContentPosting {
                term: "coldmarker".to_string(),
                ids: vec![cold.id],
                positions: vec![ContentPositions {
                    id: cold.id,
                    positions: vec![0],
                }],
            },
        ],
    )
    .unwrap();

    let record_archive = MmapRecordArchive::open(&records_path).unwrap();
    let columns = MmapRecordColumns::open(&columns_path).unwrap();
    let metadata = MmapMetadataArchive::open(&metadata_path).unwrap();
    let lookup = SearchArchiveLookup::open(&prefixes_path, &substrings_path, &fuzzy_path).unwrap();
    let substrings = MmapSubstringArchive::open(&substrings_path).unwrap();
    let content = MmapContentArchive::open(&content_path).unwrap();
    let import = query_sidecar_imports(
        &metadata,
        &lookup,
        &substrings,
        &content,
        "bodymarker",
        SearchLookupBudget::default(),
    )
    .unwrap();
    let (live, report) =
        LiveIndex::from_mmap_records_with_sidecar_import(&record_archive, &columns, import)
            .unwrap();

    assert_eq!(report.records_loaded, 1);
    assert_eq!(report.records_missing, 0);
    assert_eq!(report.columns_applied, 1);
    assert_eq!(report.import.candidate_ids, 1);
    assert!(!report.import.requires_full_record_hydration);
    assert_eq!(live.search("bodymarker", 5)[0].record.id, hot.id);
    assert!(live.search("coldmarker", 5).is_empty());

    fs::remove_file(records_path).unwrap();
    fs::remove_file(columns_path).unwrap();
    fs::remove_file(metadata_path).unwrap();
    fs::remove_file(prefixes_path).unwrap();
    fs::remove_file(substrings_path).unwrap();
    fs::remove_file(fuzzy_path).unwrap();
    fs::remove_file(content_path).unwrap();
}

#[test]
fn sidecar_query_session_reuses_mmap_archives_and_lookup_cache() {
    let records_path = unique_temp_path("gfm-index-sidecar-session", "gfmidx");
    let columns_path = unique_temp_path("gfm-index-sidecar-session", "gfmcols");
    let metadata_path = unique_temp_path("gfm-index-sidecar-session", "gfmmeta");
    let prefixes_path = unique_temp_path("gfm-index-sidecar-session", "gfmprefix");
    let substrings_path = unique_temp_path("gfm-index-sidecar-session", "gfmsubstr");
    let fuzzy_path = unique_temp_path("gfm-index-sidecar-session", "gfmfuzzy");
    let content_path = unique_temp_path("gfm-index-sidecar-session", "gfmcontent");
    let record = FileRecord {
        id: FileId::new(VolumeId(2), 7),
        parent: None,
        path: PathBuf::from("/tmp/session/FinderLatency.md"),
        name: "FinderLatency.md".to_string(),
        kind: FileKind::File,
        len: 12,
        created: Some(UNIX_EPOCH),
        modified: Some(UNIX_EPOCH),
        changed: Some(UNIX_EPOCH),
        mode: 0o644,
        owner: 501,
        group: 20,
        hidden: false,
        tags: vec!["Important".to_string()],
        finder_comment: Some("instant search".to_string()),
        xattrs_digest: 0,
    };
    let records = vec![record.clone()];
    write_records(&records_path, &records).unwrap();
    write_record_columns(&columns_path, &records).unwrap();
    write_metadata_postings(&metadata_path, &metadata_postings_from_records(&records)).unwrap();
    write_prefix_postings(&prefixes_path, &prefix_postings_from_records(&records)).unwrap();
    write_substring_postings(&substrings_path, &substring_postings_from_records(&records)).unwrap();
    write_fuzzy_postings(&fuzzy_path, &fuzzy_postings_from_records(&records)).unwrap();
    write_content_postings(&content_path, &[]).unwrap();

    let session = SidecarIndexQuerySession::open(
        &records_path,
        &columns_path,
        &metadata_path,
        &prefixes_path,
        &substrings_path,
        &fuzzy_path,
        &content_path,
    )
    .unwrap();

    let first = session.search("finderlatency", 5).unwrap();
    let lookup_before_second = session.lookup_telemetry();
    let record_cache_before_second = session.record_cache_telemetry();
    let second = session.search("finderlatency", 5).unwrap();
    let lookup_after_second = session.lookup_telemetry();
    let record_cache_after_second = session.record_cache_telemetry();

    assert_eq!(session.indexed_records(), 1);
    assert_eq!(session.indexed_columns(), 1);
    assert_eq!(first.search.hits[0].record.id, record.id);
    assert_eq!(second.search.hits[0].record.id, record.id);
    assert_eq!(second.hydration.records_loaded, 1);
    assert_eq!(first.record_cache_hits, 0);
    assert_eq!(first.record_cache_misses, 1);
    assert_eq!(second.record_cache_hits, 1);
    assert_eq!(second.record_cache_misses, 0);
    assert!(record_cache_after_second.0 > record_cache_before_second.0);
    assert!(lookup_after_second.prefix_cache_hits > lookup_before_second.prefix_cache_hits);

    fs::remove_file(records_path).unwrap();
    fs::remove_file(columns_path).unwrap();
    fs::remove_file(metadata_path).unwrap();
    fs::remove_file(prefixes_path).unwrap();
    fs::remove_file(substrings_path).unwrap();
    fs::remove_file(fuzzy_path).unwrap();
    fs::remove_file(content_path).unwrap();
}

#[test]
fn query_sidecar_imports_enforce_query_level_lookup_budgets() {
    let metadata_path = unique_temp_path("gfm-index-sidecar-budget", "gfmmeta");
    let prefixes_path = unique_temp_path("gfm-index-sidecar-budget", "gfmprefix");
    let substrings_path = unique_temp_path("gfm-index-sidecar-budget", "gfmsubstr");
    let fuzzy_path = unique_temp_path("gfm-index-sidecar-budget", "gfmfuzzy");
    let content_path = unique_temp_path("gfm-index-sidecar-budget", "gfmcontent");
    let first = FileId::new(VolumeId(1), 1);
    let second = FileId::new(VolumeId(1), 2);
    let fuzzy_keys = fuzzy_query_keys("alpha");

    write_metadata_postings(&metadata_path, &[]).unwrap();
    write_prefix_postings(
        &prefixes_path,
        &[
            PrefixPosting {
                prefix: "alpha".to_string(),
                ids: vec![first],
            },
            PrefixPosting {
                prefix: "alpine".to_string(),
                ids: vec![second],
            },
        ],
    )
    .unwrap();
    write_substring_postings(
        &substrings_path,
        &[
            SubstringPosting {
                gram: "alp".to_string(),
                ids: vec![first],
            },
            SubstringPosting {
                gram: "lph".to_string(),
                ids: vec![second],
            },
        ],
    )
    .unwrap();
    write_fuzzy_postings(
        &fuzzy_path,
        &fuzzy_keys
            .iter()
            .take(2)
            .map(|key| FuzzyPosting {
                key: key.clone(),
                terms: vec!["alpha".to_string(), "alpine".to_string()],
            })
            .collect::<Vec<_>>(),
    )
    .unwrap();
    write_content_postings(&content_path, &[]).unwrap();

    let metadata = MmapMetadataArchive::open(&metadata_path).unwrap();
    let lookup = SearchArchiveLookup::open(&prefixes_path, &substrings_path, &fuzzy_path).unwrap();
    let substrings = MmapSubstringArchive::open(&substrings_path).unwrap();
    let content = MmapContentArchive::open(&content_path).unwrap();
    let import = query_sidecar_imports(
        &metadata,
        &lookup,
        &substrings,
        &content,
        "alpha",
        SearchLookupBudget {
            max_substring_grams_per_term: 1,
            max_fuzzy_keys_per_term: 1,
            max_fuzzy_terms_per_key: 2,
            max_fuzzy_candidates_per_term: 1,
            ..SearchLookupBudget::default()
        },
    )
    .unwrap();

    assert_eq!(import.report.substring_postings, 1);
    assert_eq!(import.report.prefix_postings, 1);
    assert_eq!(import.report.fuzzy_postings, 1);
    assert_eq!(
        import
            .fuzzy
            .iter()
            .map(|posting| posting.terms.len())
            .sum::<usize>(),
        1
    );
    assert_eq!(import.report.candidate_ids, 1);
    assert_eq!(lookup.cache_telemetry().prefix_lookup_requests, 1);
    assert_eq!(lookup.cache_telemetry().fuzzy_lookup_requests, 1);
    assert_eq!(lookup.cache_telemetry().fuzzy_cache_misses, 1);

    fs::remove_file(metadata_path).unwrap();
    fs::remove_file(prefixes_path).unwrap();
    fs::remove_file(substrings_path).unwrap();
    fs::remove_file(fuzzy_path).unwrap();
    fs::remove_file(content_path).unwrap();
}

#[test]
fn search_archive_lookup_caches_are_bounded() {
    let prefixes = unique_temp_path("gfm-prefix-cache-bound", "gfmprefix");
    let substrings = unique_temp_path("gfm-substring-cache-bound", "gfmsubstr");
    let fuzzy = unique_temp_path("gfm-fuzzy-cache-bound", "gfmfuzzy");
    let id = FileId::new(VolumeId(1), 1);

    write_prefix_postings(
        &prefixes,
        &[
            PrefixPosting {
                prefix: "pa".to_string(),
                ids: vec![id],
            },
            PrefixPosting {
                prefix: "pb".to_string(),
                ids: vec![id],
            },
            PrefixPosting {
                prefix: "pc".to_string(),
                ids: vec![id],
            },
        ],
    )
    .unwrap();
    write_substring_postings(
        &substrings,
        &[
            SubstringPosting {
                gram: "aaa".to_string(),
                ids: vec![id],
            },
            SubstringPosting {
                gram: "bbb".to_string(),
                ids: vec![id],
            },
            SubstringPosting {
                gram: "ccc".to_string(),
                ids: vec![id],
            },
        ],
    )
    .unwrap();
    write_fuzzy_postings(
        &fuzzy,
        &[
            FuzzyPosting {
                key: "fa".to_string(),
                terms: vec!["fa".to_string()],
            },
            FuzzyPosting {
                key: "fb".to_string(),
                terms: vec!["fb".to_string()],
            },
            FuzzyPosting {
                key: "fc".to_string(),
                terms: vec!["fc".to_string()],
            },
        ],
    )
    .unwrap();

    let lookup =
        SearchArchiveLookup::open_with_cache_capacity(&prefixes, &substrings, &fuzzy, 2).unwrap();
    assert_eq!(lookup.prefix_ids("pa").unwrap(), vec![id]);
    assert_eq!(lookup.prefix_ids("pb").unwrap(), vec![id]);
    assert_eq!(lookup.prefix_ids("pc").unwrap(), vec![id]);
    assert_eq!(lookup.substring_ids("aaa").unwrap(), vec![id]);
    assert_eq!(lookup.substring_ids("bbb").unwrap(), vec![id]);
    assert_eq!(lookup.substring_ids("ccc").unwrap(), vec![id]);
    assert_eq!(lookup.fuzzy_terms("fa").unwrap(), vec!["fa".to_string()]);
    assert_eq!(lookup.fuzzy_terms("fb").unwrap(), vec!["fb".to_string()]);
    assert_eq!(lookup.fuzzy_terms("fc").unwrap(), vec!["fc".to_string()]);

    assert_eq!(lookup.cache_entry_counts().unwrap(), (2, 2, 2));
    let before = lookup.cache_telemetry();
    assert_eq!(lookup.prefix_ids("pa").unwrap(), vec![id]);
    assert_eq!(lookup.substring_ids("aaa").unwrap(), vec![id]);
    assert_eq!(lookup.fuzzy_terms("fa").unwrap(), vec!["fa".to_string()]);
    let after = lookup.cache_telemetry();

    assert_eq!(after.prefix_cache_misses, before.prefix_cache_misses + 1);
    assert_eq!(
        after.substring_cache_misses,
        before.substring_cache_misses + 1
    );
    assert_eq!(after.fuzzy_cache_misses, before.fuzzy_cache_misses + 1);
    assert_eq!(lookup.cache_entry_counts().unwrap(), (2, 2, 2));

    fs::remove_file(prefixes).unwrap();
    fs::remove_file(substrings).unwrap();
    fs::remove_file(fuzzy).unwrap();
}

#[test]
fn search_archive_lookup_batches_fuzzy_misses_for_query_import() {
    let prefixes = unique_temp_path("gfm-prefix-fuzzy-batch", "gfmprefix");
    let substrings = unique_temp_path("gfm-substring-fuzzy-batch", "gfmsubstr");
    let fuzzy = unique_temp_path("gfm-fuzzy-batch", "gfmfuzzy");

    write_prefix_postings(&prefixes, &[]).unwrap();
    write_substring_postings(&substrings, &[]).unwrap();
    write_fuzzy_postings(
        &fuzzy,
        &[
            FuzzyPosting {
                key: "aplha".to_string(),
                terms: vec!["alpha".to_string(), "alphas".to_string()],
            },
            FuzzyPosting {
                key: "projet".to_string(),
                terms: vec!["project".to_string(), "projects".to_string()],
            },
        ],
    )
    .unwrap();

    let lookup = SearchArchiveLookup::open(&prefixes, &substrings, &fuzzy).unwrap();
    let first = lookup
        .fuzzy_postings_bounded(["projet", "missing", "aplha", "aplha"], 2)
        .unwrap();
    assert_eq!(first.len(), 3);
    assert_eq!(first[0].key, "aplha");
    assert_eq!(
        first[0].terms,
        vec!["alpha".to_string(), "alphas".to_string()]
    );
    assert_eq!(first[1].key, "missing");
    assert!(first[1].terms.is_empty());
    assert_eq!(first[2].key, "projet");
    assert_eq!(
        first[2].terms,
        vec!["project".to_string(), "projects".to_string()]
    );
    assert_eq!(lookup.cache_telemetry().fuzzy_lookup_requests, 3);
    assert_eq!(lookup.cache_telemetry().fuzzy_cache_misses, 3);
    assert_eq!(lookup.cache_entry_counts().unwrap().2, 3);

    let second = lookup
        .fuzzy_postings_bounded(["missing", "aplha"], 2)
        .unwrap();
    assert_eq!(second.len(), 2);
    assert_eq!(
        second[0].terms,
        vec!["alpha".to_string(), "alphas".to_string()]
    );
    assert!(second[1].terms.is_empty());
    assert_eq!(lookup.cache_telemetry().fuzzy_lookup_requests, 5);
    assert_eq!(lookup.cache_telemetry().fuzzy_cache_hits, 2);
    assert_eq!(lookup.cache_telemetry().fuzzy_cache_misses, 3);

    fs::remove_file(prefixes).unwrap();
    fs::remove_file(substrings).unwrap();
    fs::remove_file(fuzzy).unwrap();
}

#[test]
fn search_archive_bounded_lookup_does_not_cache_partial_results() {
    let prefixes = unique_temp_path("gfm-prefix-bounded-cache", "gfmprefix");
    let substrings = unique_temp_path("gfm-substring-bounded-cache", "gfmsubstr");
    let fuzzy = unique_temp_path("gfm-fuzzy-bounded-cache", "gfmfuzzy");
    let prefix_ids: Vec<_> = (0..300)
        .map(|node| FileId::new(VolumeId(9), 20_000 + node))
        .collect();
    let substring_ids: Vec<_> = (0..300)
        .map(|node| FileId::new(VolumeId(9), 30_000 + node))
        .collect();
    let fuzzy_terms: Vec<_> = (0..300).map(|index| format!("term{index}")).collect();
    write_prefix_postings(
        &prefixes,
        &[PrefixPosting {
            prefix: "pro".to_string(),
            ids: prefix_ids.clone(),
        }],
    )
    .unwrap();
    write_substring_postings(
        &substrings,
        &[SubstringPosting {
            gram: "por".to_string(),
            ids: substring_ids.clone(),
        }],
    )
    .unwrap();
    write_fuzzy_postings(
        &fuzzy,
        &[FuzzyPosting {
            key: "term".to_string(),
            terms: fuzzy_terms.clone(),
        }],
    )
    .unwrap();

    let lookup = SearchArchiveLookup::open(&prefixes, &substrings, &fuzzy).unwrap();
    let bounded_prefix = lookup.prefix_ids_bounded("pro", 129).unwrap();
    let bounded_substring = lookup.substring_ids_bounded("por", 129).unwrap();
    let bounded_fuzzy = lookup.fuzzy_terms_bounded("term", 129).unwrap();

    assert!(bounded_prefix.truncated);
    assert_eq!(bounded_prefix.ids.len(), 129);
    assert!(bounded_substring.truncated);
    assert_eq!(bounded_substring.ids.len(), 129);
    assert!(bounded_fuzzy.truncated);
    assert_eq!(bounded_fuzzy.terms.len(), 129);
    assert_eq!(lookup.cache_entry_counts().unwrap(), (0, 0, 0));
    assert_eq!(lookup.prefix_ids("pro").unwrap(), prefix_ids);
    assert_eq!(lookup.substring_ids("por").unwrap(), substring_ids);
    let mut sorted_fuzzy_terms = fuzzy_terms;
    sorted_fuzzy_terms.sort();
    assert_eq!(lookup.fuzzy_terms("term").unwrap(), sorted_fuzzy_terms);
    assert_eq!(lookup.cache_entry_counts().unwrap(), (1, 1, 1));

    fs::remove_file(prefixes).unwrap();
    fs::remove_file(substrings).unwrap();
    fs::remove_file(fuzzy).unwrap();
}

#[test]
fn live_index_applies_create_modify_and_remove_events() {
    let root = unique_temp_dir("gfm-live-root");
    let target = root.join("Needle.txt");
    fs::write(&target, "first").unwrap();

    let mut live = LiveIndex::new();
    let created = FileEvent::new(&target, FileEventKind::Create);
    assert_eq!(live.apply_event(&created).unwrap(), UpdateOutcome::Upserted);
    assert_eq!(live.search("needle", 5).len(), 1);

    fs::write(&target, "second").unwrap();
    let modified = FileEvent::new(&target, FileEventKind::Modify);
    assert!(matches!(
        live.apply_event(&modified).unwrap(),
        UpdateOutcome::MetadataUpdated { changed } if changed > 0
    ));
    assert_eq!(live.search("needle", 5).len(), 1);

    fs::remove_file(&target).unwrap();
    let removed = FileEvent::new(&target, FileEventKind::Remove);
    assert_eq!(
        live.apply_event(&removed).unwrap(),
        UpdateOutcome::Removed { records: 1 }
    );
    assert!(live.search("needle", 5).is_empty());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn live_index_applies_incremental_metadata_updates() {
    let root = unique_temp_dir("gfm-metadata-update-root");
    let target = root.join("Metadata.md");
    fs::write(&target, "first").unwrap();

    let snapshot = Indexer::default().build(&root).unwrap();
    let mut live = snapshot.into_live();
    let old_len = live.search("metadata", 5)[0].record.len;

    fs::write(&target, "first plus more").unwrap();
    let report = live.apply_metadata_update(&target).unwrap();
    let hits = live.search("metadata", 5);

    assert!(report.changed.contains(&"size"), "{report:?}");
    assert!(hits[0].record.len > old_len);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn live_index_applies_finder_xattr_metadata_events() {
    let root = unique_temp_dir("gfm-xattr-metadata-update-root");
    let target = root.join("Tagged.md");
    fs::write(&target, "tagged").unwrap();

    let snapshot = Indexer::default().build(&root).unwrap();
    let mut live = snapshot.into_live();
    assert!(live.search("tag:important", 5).is_empty());

    set_finder_tags(&target, &["Important\n6"]);
    let event = FileEvent::new(&target, FileEventKind::Metadata);
    let outcome = live.apply_event(&event).unwrap();

    assert!(matches!(
        outcome,
        UpdateOutcome::MetadataUpdated { changed } if changed > 0
    ));
    assert_eq!(live.search("tag:important", 5).len(), 1);

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn live_index_applies_chmod_metadata_updates() {
    use std::os::unix::fs::PermissionsExt;

    let root = unique_temp_dir("gfm-chmod-update-root");
    let target = root.join("Permissions.md");
    fs::write(&target, "mode").unwrap();

    let snapshot = Indexer::default().build(&root).unwrap();
    let mut live = snapshot.into_live();
    let old_mode = live.search("permissions", 5)[0].record.mode;

    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    let report = live.apply_metadata_update(&target).unwrap();
    let hits = live.search("permissions", 5);

    assert!(report.changed.contains(&"mode"), "{report:?}");
    assert_ne!(hits[0].record.mode, old_mode);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn event_backpressure_coalesces_duplicate_background_bursts() {
    let path = PathBuf::from("/tmp/hot.md");
    let mut queue = EventBackpressureQueue::new(8, 3);

    for _ in 0..20 {
        let report = queue.enqueue(
            EventPriority::Background,
            FileEvent::new(&path, FileEventKind::Modify),
        );
        assert!(report.accepted);
    }

    let snapshot = queue.snapshot();
    assert_eq!(snapshot.pending_background, 1);
    assert_eq!(snapshot.coalesced, 19);
    assert!(!snapshot.repair_required);
}

#[test]
fn event_backpressure_preserves_visible_progress_under_background_load() {
    let mut queue = EventBackpressureQueue::new(5, 2);
    for index in 0..8 {
        queue.enqueue(
            EventPriority::Background,
            FileEvent::new(format!("/tmp/background-{index}.md"), FileEventKind::Modify),
        );
    }
    queue.enqueue(
        EventPriority::Visible,
        FileEvent::new("/tmp/visible-a.md", FileEventKind::Modify),
    );
    queue.enqueue(
        EventPriority::Visible,
        FileEvent::new("/tmp/visible-b.md", FileEventKind::Modify),
    );

    let snapshot = queue.snapshot();
    assert!(snapshot.dropped > 0);
    assert!(snapshot.repair_required);

    let drained = queue.drain_batch(3);
    assert_eq!(drained[0].path, PathBuf::from("/tmp/visible-a.md"));
    assert_eq!(drained[1].path, PathBuf::from("/tmp/visible-b.md"));
    assert!(drained[2]
        .path
        .to_string_lossy()
        .starts_with("/tmp/background-"));
}

#[test]
fn fair_scan_prioritizes_visible_roots_during_background_crawl() {
    let root = unique_temp_dir("gfm-fair-scan-root");
    let visible = root.join("Visible");
    fs::create_dir_all(&visible).unwrap();
    fs::write(visible.join("Needle.md"), "visible first").unwrap();
    for index in 0..32 {
        let background = root.join(format!("Background{index:02}"));
        fs::create_dir_all(&background).unwrap();
        fs::write(background.join("Bulk.md"), "background").unwrap();
    }

    let report = Indexer::default().build_fair(&root, &[visible], 2).unwrap();

    assert!(report.summary.visible_records >= 2, "{:?}", report.summary);
    assert!(
        report.summary.background_records > report.summary.visible_records,
        "{:?}",
        report.summary
    );
    assert!(
        report.summary.max_background_gap <= 2,
        "{:?}",
        report.summary
    );
    assert!(report
        .snapshot
        .records
        .iter()
        .any(|record| record.name == "Needle.md"));
    assert!(report.as_tsv().starts_with("fair-scan\t"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn fair_scan_avoids_duplicate_visible_paths() {
    let root = unique_temp_dir("gfm-fair-scan-duplicates-root");
    let visible = root.join("Visible");
    fs::create_dir_all(&visible).unwrap();
    fs::write(visible.join("Identity.md"), "one identity").unwrap();

    let report = Indexer::default()
        .build_fair(&root, std::slice::from_ref(&visible), 4)
        .unwrap();
    let unique_paths = report
        .snapshot
        .records
        .iter()
        .map(|record| record.path.clone())
        .collect::<HashSet<_>>();

    assert_eq!(unique_paths.len(), report.snapshot.records.len());
    assert_eq!(
        report
            .snapshot
            .records
            .iter()
            .filter(|record| record.path == visible)
            .count(),
        1
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn volume_index_policy_includes_local_and_defers_remote_by_default() {
    let policy = VolumeIndexPolicy::default();
    let local = IndexVolumeDescriptor::new(
        "Macintosh HD",
        "/",
        IndexVolumeClass::System,
        IndexMountState::Mounted,
    );
    let external = IndexVolumeDescriptor::new(
        "Work Drive",
        "/Volumes/Work",
        IndexVolumeClass::External,
        IndexMountState::Mounted,
    );
    let network = IndexVolumeDescriptor::new(
        "Team Share",
        "/Volumes/Team",
        IndexVolumeClass::Network,
        IndexMountState::Mounted,
    );

    let plan = policy.plan(vec![network, external, local]);

    assert_eq!(plan.decisions[0].action, VolumeIndexAction::Include);
    assert_eq!(plan.decisions[1].action, VolumeIndexAction::DeferredOptIn);
    assert_eq!(plan.decisions[2].action, VolumeIndexAction::DeferredOptIn);
    assert_eq!(plan.included_roots(), vec![PathBuf::from("/")]);
    assert!(plan
        .as_tsv()
        .starts_with("volume-index-plan\tcount=3\tincluded=1"));
}

#[test]
fn volume_index_policy_applies_opt_in_and_remote_throttles() {
    let policy = VolumeIndexPolicy::new(
        gfm_config::VolumeIndexingPolicy::OptIn,
        gfm_config::VolumeIndexingPolicy::OptIn,
    )
    .with_opted_in_roots(vec![PathBuf::from("/Volumes/Work")]);
    let external = IndexVolumeDescriptor::new(
        "Work Drive",
        "/Volumes/Work",
        IndexVolumeClass::External,
        IndexMountState::Mounted,
    );
    let network = IndexVolumeDescriptor::new(
        "Team Share",
        "/Volumes/Team",
        IndexVolumeClass::Network,
        IndexMountState::Mounted,
    );

    let plan = policy.plan(vec![network, external]);
    let external = plan
        .decisions
        .iter()
        .find(|decision| decision.label == "Work Drive")
        .unwrap();
    let network = plan
        .decisions
        .iter()
        .find(|decision| decision.label == "Team Share")
        .unwrap();

    assert_eq!(external.action, VolumeIndexAction::Include);
    assert_eq!(external.throttle.class, VolumeThrottleClass::External);
    assert_eq!(external.throttle.max_concurrent_jobs, 2);
    assert_eq!(network.action, VolumeIndexAction::DeferredOptIn);
    assert_eq!(plan.included_roots(), vec![PathBuf::from("/Volumes/Work")]);
}

#[test]
fn volume_index_policy_suspends_disabled_and_disconnected_volumes() {
    let policy = VolumeIndexPolicy::new(
        gfm_config::VolumeIndexingPolicy::Enabled,
        gfm_config::VolumeIndexingPolicy::Disabled,
    );
    let disconnected_external = IndexVolumeDescriptor::new(
        "Backup",
        "/Volumes/Backup",
        IndexVolumeClass::External,
        IndexMountState::Stale,
    );
    let disabled_network = IndexVolumeDescriptor::new(
        "Team Share",
        "/Volumes/Team",
        IndexVolumeClass::Network,
        IndexMountState::Mounted,
    );

    let plan = policy.plan(vec![disabled_network, disconnected_external]);

    assert_eq!(plan.decisions[0].action, VolumeIndexAction::Disconnected);
    assert_eq!(
        plan.decisions[0].throttle.class,
        VolumeThrottleClass::Suspended
    );
    assert_eq!(plan.decisions[1].action, VolumeIndexAction::Disabled);
    assert!(plan.included_roots().is_empty());
}

#[test]
fn live_index_correlates_file_renames_without_identity_churn() {
    let root = unique_temp_dir("gfm-rename-file-root");
    let from = root.join("NeedleOld.txt");
    let to = root.join("NeedleNew.txt");
    fs::write(&from, "first").unwrap();

    let snapshot = Indexer::default().build(&root).unwrap();
    let mut live = snapshot.into_live();
    let original_id = live.search("needleold", 5)[0].record.id;

    fs::rename(&from, &to).unwrap();
    let event = FileEvent::new(
        &to,
        FileEventKind::Rename {
            from,
            to: to.clone(),
        },
    );

    assert_eq!(
        live.apply_event(&event).unwrap(),
        UpdateOutcome::Renamed {
            removed: 1,
            inserted: 1
        }
    );
    assert!(live.search("needleold", 5).is_empty());
    let hits = live.search("needlenew", 5);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.id, original_id);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn live_index_correlates_directory_renames_across_subtrees() {
    let root = unique_temp_dir("gfm-rename-dir-root");
    let from = root.join("OldProject");
    let to = root.join("NewProject");
    let nested = from.join("Nested");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("Needle.md"), "first").unwrap();

    let snapshot = Indexer::default().build(&root).unwrap();
    let mut live = snapshot.into_live();
    let original_file_id = live.search("needle", 5)[0].record.id;
    let removed_count = 3;

    fs::rename(&from, &to).unwrap();
    let event = FileEvent::new(
        &to,
        FileEventKind::Rename {
            from,
            to: to.clone(),
        },
    );

    assert_eq!(
        live.apply_event(&event).unwrap(),
        UpdateOutcome::Renamed {
            removed: removed_count,
            inserted: removed_count
        }
    );
    let hits = live.search("needle", 5);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.id, original_file_id);
    assert!(hits[0].record.path.ends_with("NewProject/Nested/Needle.md"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn snapshot_can_search_text_content() {
    let root = unique_temp_dir("gfm-content-index-root");
    fs::write(root.join("notes.md"), "needle appears inside the file body").unwrap();

    let snapshot = Indexer::default().build(&root).unwrap();
    let hits = snapshot.search_with_content("needle", 5).unwrap();

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.name, "notes.md");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn snapshot_can_search_text_content_with_snippets() {
    let root = unique_temp_dir("gfm-content-snippet-index-root");
    fs::write(
        root.join("notes.md"),
        "intro intro bounded snippet marker outro outro",
    )
    .unwrap();

    let snapshot = Indexer::default().build(&root).unwrap();
    let hits = snapshot
        .search_with_content_snippets(r#""snippet marker""#, 5, &Extractor::default(), 8)
        .unwrap();

    let snippet = hits[0].snippet.as_ref().unwrap();
    assert_eq!(hits.len(), 1);
    assert!(snippet.text.contains("snippet marker"));
    assert_eq!(
        &snippet.text[snippet.highlights[0].start..snippet.highlights[0].end],
        "snippet marker"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn live_search_honors_cancellation() {
    let root = unique_temp_dir("gfm-cancelled-search-root");
    fs::write(root.join("notes.md"), "needle").unwrap();
    let snapshot = Indexer::default().build(&root).unwrap();
    let live = snapshot.into_live();
    let cancellation = Cancellation::default();
    cancellation.cancel();

    let result = live.search_cancellable("needle", 5, &cancellation);

    assert!(matches!(result, Err(GfmError::Cancelled)));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn durable_content_postings_survive_reload() {
    let root = unique_temp_dir("gfm-durable-content-root");
    let records = unique_temp_path("gfm-durable-content-records", "gfmidx");
    let content = unique_temp_path("gfm-durable-content-postings", "gfmcontent");
    fs::write(root.join("journal.md"), "a durable superneedle lives here").unwrap();

    let indexer = Indexer::default();
    let snapshot = indexer.build(&root).unwrap();
    let indexed = snapshot
        .save_with_content(&records, &content, &Extractor::default())
        .unwrap();
    let reloaded = indexer.load_live_with_content(&records, &content).unwrap();
    let hits = reloaded.search("superneedle", 5);

    assert_eq!(indexed, 1);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.name, "journal.md");

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(content).unwrap();
}

#[test]
fn budgeted_content_set_loading_imports_bounded_postings() {
    let root = unique_temp_dir("gfm-budgeted-content-root");
    let records = unique_temp_path("gfm-budgeted-content-records", "gfmidx");
    let content = unique_temp_path("gfm-budgeted-content-postings", "gfmcontent");
    for node in 1..=8 {
        fs::write(root.join(format!("{node:03}.md")), "plain file").unwrap();
    }

    let indexer = Indexer::default();
    let snapshot = indexer.build(&root).unwrap();
    snapshot.save(&records).unwrap();
    let ids = snapshot
        .records
        .iter()
        .map(|record| record.id)
        .collect::<Vec<_>>();
    write_content_postings(
        &content,
        &[ContentPosting {
            term: "needle".to_string(),
            ids: ids.clone(),
            positions: ids
                .iter()
                .map(|id| ContentPositions {
                    id: *id,
                    positions: vec![1],
                })
                .collect(),
        }],
    )
    .unwrap();

    let mut live = indexer.load(&records).unwrap().into_live();
    let terms = live
        .load_content_set_postings_with_budget(
            &[&content],
            "needle",
            SearchLookupBudget {
                max_content_ids_per_term: 3,
                ..SearchLookupBudget::default()
            },
        )
        .unwrap();
    let hits = live.search("needle", 10);

    assert_eq!(terms, 1);
    assert_eq!(hits.len(), 3);

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(content).unwrap();
}

#[test]
fn budgeted_content_archive_loading_imports_bounded_query_postings() {
    let root = unique_temp_dir("gfm-budgeted-content-archive-root");
    let records = unique_temp_path("gfm-budgeted-content-archive-records", "gfmidx");
    let content = unique_temp_path("gfm-budgeted-content-archive-postings", "gfmcontent");
    for node in 1..=8 {
        fs::write(root.join(format!("{node:03}.md")), "plain file").unwrap();
    }

    let indexer = Indexer::default();
    let snapshot = indexer.build(&root).unwrap();
    snapshot.save(&records).unwrap();
    let ids = snapshot
        .records
        .iter()
        .map(|record| record.id)
        .collect::<Vec<_>>();
    write_content_postings(
        &content,
        &[ContentPosting {
            term: "archiveneedle".to_string(),
            ids: ids.clone(),
            positions: ids
                .iter()
                .map(|id| ContentPositions {
                    id: *id,
                    positions: vec![1],
                })
                .collect(),
        }],
    )
    .unwrap();

    let mut live = indexer.load(&records).unwrap().into_live();
    let terms = live
        .load_content_postings_with_budget(
            &content,
            "archiveneedle",
            SearchLookupBudget {
                max_content_ids_per_term: 3,
                ..SearchLookupBudget::default()
            },
        )
        .unwrap();
    let hits = live.search("archiveneedle", 10);

    assert_eq!(terms, 1);
    assert_eq!(hits.len(), 3);

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(content).unwrap();
}

#[test]
fn content_set_search_loader_uses_default_bounded_budget() {
    let root = unique_temp_dir("gfm-default-budgeted-content-root");
    let records = unique_temp_path("gfm-default-budgeted-content-records", "gfmidx");
    let content = unique_temp_path("gfm-default-budgeted-content-postings", "gfmcontent");
    for node in 1..=4100 {
        fs::write(root.join(format!("{node:04}.md")), "plain file").unwrap();
    }

    let indexer = Indexer::default();
    let snapshot = indexer.build(&root).unwrap();
    snapshot.save(&records).unwrap();
    let ids = snapshot
        .records
        .iter()
        .map(|record| record.id)
        .collect::<Vec<_>>();
    write_content_postings(
        &content,
        &[ContentPosting {
            term: "defaultbudgetneedle".to_string(),
            ids: ids.clone(),
            positions: ids
                .iter()
                .map(|id| ContentPositions {
                    id: *id,
                    positions: vec![1],
                })
                .collect(),
        }],
    )
    .unwrap();

    let (live, load) = indexer
        .load_live_with_content_set(&records, &[&content], "defaultbudgetneedle")
        .unwrap();
    let hits = live.search("defaultbudgetneedle", 5000);

    assert_eq!(load.content_keys, 1);
    assert_eq!(
        load.records_loaded,
        SearchLookupBudget::default().max_content_ids_per_term
    );
    assert!(!load.full_hydration);
    assert_eq!(
        hits.len(),
        SearchLookupBudget::default().max_content_ids_per_term
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(content).unwrap();
}

#[test]
fn content_query_session_reuses_archives_and_record_cache() {
    let root = unique_temp_dir("gfm-content-query-session-root");
    let records = unique_temp_path("gfm-content-query-session-records", "gfmidx");
    let first_content = unique_temp_path("gfm-content-query-session-first", "gfmcontent");
    let second_content = unique_temp_path("gfm-content-query-session-second", "gfmcontent");
    fs::write(root.join("SearchNotes.md"), "cached content").unwrap();
    fs::write(root.join("Other.md"), "other content").unwrap();

    let indexer = Indexer::default();
    let snapshot = indexer.build(&root).unwrap();
    snapshot.save(&records).unwrap();
    let matched = snapshot
        .records
        .iter()
        .find(|record| record.name == "SearchNotes.md")
        .unwrap()
        .id;
    let other = snapshot
        .records
        .iter()
        .find(|record| record.name == "Other.md")
        .unwrap()
        .id;
    write_content_postings(
        &first_content,
        &[ContentPosting {
            term: "contentcache".to_string(),
            ids: vec![matched],
            positions: vec![ContentPositions {
                id: matched,
                positions: vec![1],
            }],
        }],
    )
    .unwrap();
    write_content_postings(
        &second_content,
        &[ContentPosting {
            term: "elsewhere".to_string(),
            ids: vec![other],
            positions: vec![ContentPositions {
                id: other,
                positions: vec![1],
            }],
        }],
    )
    .unwrap();

    let session = indexer
        .load_content_set_query_session(&records, [&first_content, &second_content])
        .unwrap();
    let first = session.search("contentcache", 5).unwrap();
    let posting_before_second = session.posting_cache_telemetry();
    let before_second = session.record_cache_telemetry();
    let second = session.search("contentcache", 5).unwrap();
    let posting_after_second = session.posting_cache_telemetry();
    let after_second = session.record_cache_telemetry();

    assert_eq!(session.indexed_records(), snapshot.records.len());
    assert_eq!(session.archive_count(), 2);
    assert_eq!(first.load.content_keys, 1);
    assert_eq!(first.load.candidate_ids, 1);
    assert_eq!(first.search.hits[0].record.id, matched);
    assert_eq!(second.search.hits[0].record.id, matched);
    assert_eq!(first.posting_cache_hits, 0);
    assert_eq!(first.posting_cache_misses, 1);
    assert_eq!(second.posting_cache_hits, 1);
    assert_eq!(second.posting_cache_misses, 0);
    assert_eq!(first.record_cache_hits, 0);
    assert_eq!(first.record_cache_misses, 1);
    assert_eq!(second.record_cache_hits, 1);
    assert_eq!(second.record_cache_misses, 0);
    assert!(posting_after_second.0 > posting_before_second.0);
    assert!(after_second.0 > before_second.0);

    let missing_first = session.search("absentcontentneedle", 5).unwrap();
    let missing_posting_before_second = session.posting_cache_telemetry();
    let missing_record_before_second = session.record_cache_telemetry();
    let missing_second = session.search("absentcontentneedle", 5).unwrap();
    let missing_posting_after_second = session.posting_cache_telemetry();
    let missing_record_after_second = session.record_cache_telemetry();

    assert!(missing_first.search.hits.is_empty());
    assert!(missing_second.search.hits.is_empty());
    assert_eq!(missing_first.posting_cache_hits, 0);
    assert_eq!(missing_first.posting_cache_misses, 1);
    assert_eq!(missing_second.posting_cache_hits, 1);
    assert_eq!(missing_second.posting_cache_misses, 0);
    assert_eq!(missing_first.record_cache_hits, 0);
    assert_eq!(missing_first.record_cache_misses, 0);
    assert_eq!(missing_second.record_cache_hits, 0);
    assert_eq!(missing_second.record_cache_misses, 0);
    assert!(missing_posting_after_second.0 > missing_posting_before_second.0);
    assert_eq!(missing_record_after_second, missing_record_before_second);

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(first_content).unwrap();
    fs::remove_file(second_content).unwrap();
}

#[test]
fn content_archive_search_loader_uses_default_bounded_budget() {
    let root = unique_temp_dir("gfm-default-budgeted-content-archive-root");
    let records = unique_temp_path("gfm-default-budgeted-content-archive-records", "gfmidx");
    let content = unique_temp_path(
        "gfm-default-budgeted-content-archive-postings",
        "gfmcontent",
    );
    for node in 1..=4100 {
        fs::write(root.join(format!("{node:04}.md")), "plain file").unwrap();
    }

    let indexer = Indexer::default();
    let snapshot = indexer.build(&root).unwrap();
    snapshot.save(&records).unwrap();
    let ids = snapshot
        .records
        .iter()
        .map(|record| record.id)
        .collect::<Vec<_>>();
    write_content_postings(
        &content,
        &[ContentPosting {
            term: "defaultarchivebudgetneedle".to_string(),
            ids: ids.clone(),
            positions: ids
                .iter()
                .map(|id| ContentPositions {
                    id: *id,
                    positions: vec![1],
                })
                .collect(),
        }],
    )
    .unwrap();

    let (live, load) = indexer
        .load_live_with_content_for_query(&records, &content, "defaultarchivebudgetneedle")
        .unwrap();
    let hits = live.search("defaultarchivebudgetneedle", 5000);

    assert_eq!(load.content_keys, 1);
    assert_eq!(
        load.records_loaded,
        SearchLookupBudget::default().max_content_ids_per_term
    );
    assert!(!load.full_hydration);
    assert_eq!(
        hits.len(),
        SearchLookupBudget::default().max_content_ids_per_term
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(content).unwrap();
}

#[test]
fn durable_content_positions_support_phrase_search_after_reload() {
    let root = unique_temp_dir("gfm-durable-phrase-root");
    let records = unique_temp_path("gfm-durable-phrase-records", "gfmidx");
    let content = unique_temp_path("gfm-durable-phrase-content", "gfmcontent");
    fs::write(
        root.join("keep.md"),
        "the exact durable phrase appears here",
    )
    .unwrap();
    fs::write(
        root.join("skip.md"),
        "the durable exact phrase appears in a different order",
    )
    .unwrap();

    let indexer = Indexer::default();
    let snapshot = indexer.build(&root).unwrap();
    snapshot
        .save_with_content(&records, &content, &Extractor::default())
        .unwrap();
    let reloaded = indexer.load_live_with_content(&records, &content).unwrap();
    let hits = reloaded.search(r#""exact durable phrase""#, 10);

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.name, "keep.md");

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(content).unwrap();
}

#[test]
fn durable_content_positions_support_proximity_search_after_reload() {
    let root = unique_temp_dir("gfm-durable-proximity-root");
    let records = unique_temp_path("gfm-durable-proximity-records", "gfmidx");
    let content = unique_temp_path("gfm-durable-proximity-content", "gfmcontent");
    fs::write(root.join("keep.md"), "alpha one two beta survives").unwrap();
    fs::write(
        root.join("skip.md"),
        "alpha one two three four five beta does not",
    )
    .unwrap();

    let indexer = Indexer::default();
    let snapshot = indexer.build(&root).unwrap();
    snapshot
        .save_with_content(&records, &content, &Extractor::default())
        .unwrap();
    let reloaded = indexer.load_live_with_content(&records, &content).unwrap();
    let hits = reloaded.search("near:3:alpha,beta", 10);

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.name, "keep.md");

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(content).unwrap();
}

#[test]
fn live_index_streams_hot_then_deep_results() {
    let root = unique_temp_dir("gfm-live-stream-root");
    fs::write(root.join("needle.md"), "metadata match").unwrap();
    fs::write(root.join("deep.md"), "needle only in content").unwrap();

    let snapshot = Indexer::default().build(&root).unwrap();
    let mut live = snapshot.into_live();
    live.index_content(&Extractor::default()).unwrap();

    let batches = live.stream_search("needle", 10).unwrap();

    assert_eq!(batches.len(), 2);
    assert_eq!(batches[0].stage, SearchStreamStage::Hot);
    assert!(batches[0]
        .hits
        .iter()
        .any(|hit| hit.record.name == "needle.md"));
    assert_eq!(batches[1].stage, SearchStreamStage::Deep);
    assert!(batches[1]
        .hits
        .iter()
        .any(|hit| hit.record.name == "deep.md"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn snapshot_can_write_content_segment_for_compaction() {
    let root = unique_temp_dir("gfm-content-segment-root");
    let segment = unique_temp_path("gfm-content-segment-index", "gfmseg");
    let content = unique_temp_path("gfm-content-segment-compact", "gfmcontent");
    fs::write(root.join("segment.md"), "segmenttoken appears here").unwrap();

    let indexer = Indexer::default();
    let snapshot = indexer.build(&root).unwrap();
    let indexed = snapshot
        .save_content_segment(&segment, &Extractor::default(), Vec::new())
        .unwrap();
    let terms = indexer
        .compact_content_segments(&content, &[&segment])
        .unwrap();
    let mut live = snapshot.into_live();
    live.load_content_postings(&content).unwrap();
    let hits = live.search("segmenttoken", 5);

    assert_eq!(indexed, 1);
    assert!(terms > 0);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.name, "segment.md");

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(segment).unwrap();
    fs::remove_file(content).unwrap();
}

#[test]
fn compacted_content_segments_preserve_phrase_positions() {
    let root = unique_temp_dir("gfm-content-phrase-segment-root");
    let segment = unique_temp_path("gfm-content-phrase-segment", "gfmseg");
    let content = unique_temp_path("gfm-content-phrase-compact", "gfmcontent");
    fs::write(root.join("phrase.md"), "segment phrase marker survives").unwrap();

    let indexer = Indexer::default();
    let snapshot = indexer.build(&root).unwrap();
    snapshot
        .save_content_segment(&segment, &Extractor::default(), Vec::new())
        .unwrap();
    indexer
        .compact_content_segments(&content, &[&segment])
        .unwrap();
    let mut live = snapshot.into_live();
    live.load_content_postings(&content).unwrap();
    let hits = live.search(r#""segment phrase marker""#, 5);

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.name, "phrase.md");

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(segment).unwrap();
    fs::remove_file(content).unwrap();
}

#[test]
fn background_content_indexer_batches_segments_and_compacts() {
    let root = unique_temp_dir("gfm-background-content-root");
    let segments = unique_temp_dir("gfm-background-content-segments");
    let content = unique_temp_path("gfm-background-content-compact", "gfmcontent");
    fs::write(root.join("first.md"), "first backgroundtoken").unwrap();
    fs::write(root.join("second.md"), "second backgroundtoken").unwrap();
    fs::write(root.join("third.md"), "third backgroundtoken").unwrap();

    let snapshot = Indexer::default().build(&root).unwrap();
    let worker = BackgroundContentIndexer::new(
        Extractor::default(),
        ContentIndexOptions {
            batch_size: 2,
            segment_prefix: "batch".to_string(),
        },
    );
    let report = worker
        .run_and_compact(&snapshot, &segments, &content, &Cancellation::default())
        .unwrap();
    let mut live = snapshot.into_live();
    live.load_content_postings(&content).unwrap();
    let hits = live.search("backgroundtoken", 10);

    assert_eq!(report.indexed, 3);
    assert_eq!(report.segments.len(), 2);
    assert!(report.terms > 0);
    assert_eq!(hits.len(), 3);

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(segments).unwrap();
    fs::remove_file(content).unwrap();
}

#[test]
fn background_content_indexer_incrementally_updates_existing_archive() {
    let root = unique_temp_dir("gfm-background-content-incremental-root");
    let segments = unique_temp_dir("gfm-background-content-incremental-segments");
    let content = unique_temp_path("gfm-background-content-incremental", "gfmcontent");
    fs::write(root.join("keep.md"), "stable keeptoken").unwrap();
    fs::write(root.join("change.md"), "oldtoken before change").unwrap();
    fs::write(root.join("delete.md"), "deletetoken before removal").unwrap();

    let indexer = Indexer::default();
    let previous = indexer.build(&root).unwrap();
    BackgroundContentIndexer::default()
        .run_and_compact(&previous, &segments, &content, &Cancellation::default())
        .unwrap();

    fs::write(
        root.join("change.md"),
        "changedtoken after content mutation with a longer body",
    )
    .unwrap();
    fs::remove_file(root.join("delete.md")).unwrap();
    fs::write(root.join("add.md"), "added addedtoken").unwrap();
    let current = indexer.build(&root).unwrap();
    let report = BackgroundContentIndexer::default()
        .run_incremental_and_compact(
            &current,
            &previous.records,
            Some(&content),
            &segments,
            &content,
            &Cancellation::default(),
        )
        .unwrap();
    let mut live = current.into_live();
    live.load_content_postings(&content).unwrap();

    assert_eq!(report.indexed, 2);
    assert_eq!(report.unchanged, 1);
    assert_eq!(report.tombstoned, 2);
    assert_eq!(live.search("keeptoken", 5).len(), 1);
    assert_eq!(live.search("changedtoken", 5).len(), 1);
    assert_eq!(live.search("addedtoken", 5).len(), 1);
    assert!(live.search("oldtoken", 5).is_empty());
    assert!(live.search("deletetoken", 5).is_empty());

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(segments).unwrap();
    fs::remove_file(content).unwrap();
}

#[test]
fn background_content_indexer_persists_extraction_quarantine() {
    let root = unique_temp_dir("gfm-background-content-quarantine-root");
    let segments = unique_temp_dir("gfm-background-content-quarantine-segments");
    let content = unique_temp_path("gfm-background-content-quarantine", "gfmcontent");
    let path = root.join("corrupt.pdf");
    fs::write(&path, corrupt_pdf()).unwrap();

    let indexer = Indexer::default();
    let previous = indexer.build(&root).unwrap();
    let worker = BackgroundContentIndexer::default();
    let cancellation = Cancellation::default();
    let mut quarantine = ExtractionQuarantine::new(2);

    let first = worker
        .run_incremental_and_compact_with_quarantine(
            QuarantineContentIndexRequest {
                snapshot: &previous,
                previous_records: &[],
                previous_content_path: None,
                segment_dir: &segments,
                content_path: &content,
                cancellation: &cancellation,
            },
            &mut quarantine,
        )
        .unwrap();
    let second = worker
        .run_incremental_and_compact_with_quarantine(
            QuarantineContentIndexRequest {
                snapshot: &previous,
                previous_records: &[],
                previous_content_path: Some(&content),
                segment_dir: &segments,
                content_path: &content,
                cancellation: &cancellation,
            },
            &mut quarantine,
        )
        .unwrap();
    let third = worker
        .run_incremental_and_compact_with_quarantine(
            QuarantineContentIndexRequest {
                snapshot: &previous,
                previous_records: &[],
                previous_content_path: Some(&content),
                segment_dir: &segments,
                content_path: &content,
                cancellation: &cancellation,
            },
            &mut quarantine,
        )
        .unwrap();

    assert_eq!(first.indexed, 0);
    assert_eq!(first.quarantined, 1);
    assert_eq!(second.quarantined, 1);
    assert_eq!(third.quarantined, 1);
    assert!(third.skipped >= 1);
    assert_eq!(third.terms, 0);

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(segments).unwrap();
    fs::remove_file(content).unwrap();
}

#[test]
fn background_content_maintenance_compacts_segments_and_updates_manifest() {
    let root = unique_temp_dir("gfm-background-content-maintenance-root");
    let records = unique_temp_path("gfm-background-content-maintenance-records", "gfmidx");
    let initial_content =
        unique_temp_path("gfm-background-content-maintenance-initial", "gfmcontent");
    let output_content =
        unique_temp_path("gfm-background-content-maintenance-output", "gfmcontent");
    let manifest = unique_temp_path("gfm-background-content-maintenance", "gfmmanifest");
    let segments = unique_temp_dir("gfm-background-content-maintenance-segments");
    fs::write(root.join("maintained.md"), "body contains maintenancetoken").unwrap();

    let indexer = Indexer::default();
    let snapshot = indexer.build(&root).unwrap();
    snapshot
        .save_with_content(&records, &initial_content, &Extractor::default())
        .unwrap();
    ContentArchiveManifest::new(vec![ContentArchiveManifestEntry {
        tier: ContentMergeTier::Warm,
        path: initial_content.clone(),
    }])
    .unwrap()
    .write(&manifest)
    .unwrap();

    fs::create_dir_all(&segments).unwrap();
    let segment_paths = (0..4)
        .map(|index| segments.join(format!("hot-{index}.gfmseg")))
        .collect::<Vec<_>>();
    for segment in &segment_paths {
        snapshot
            .save_content_segment(segment, &Extractor::default(), Vec::new())
            .unwrap();
    }

    let report = BackgroundContentIndexer::default()
        .maintain_segments(
            &manifest,
            &output_content,
            &segment_paths,
            &ContentMaintenanceOptions::default(),
        )
        .unwrap();
    let (live, load) = indexer
        .load_live_with_content_manifest(&records, &manifest, "maintenancetoken")
        .unwrap();
    let hits = live.search("maintenancetoken", 5);

    assert!(report.scheduled);
    assert_eq!(report.merged_segments.len(), 4);
    assert_eq!(report.retained_segments.len(), 0);
    assert_eq!(report.manifest_archives, 2);
    assert_eq!(report.published_archive, Some(output_content.clone()));
    assert_eq!(load.content_keys, 1);
    assert_eq!(load.records_loaded, 1);
    assert!(!load.full_hydration);
    assert_eq!(hits.len(), 1);

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(segments).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(initial_content).unwrap();
    fs::remove_file(output_content).unwrap();
    fs::remove_file(manifest).unwrap();
}

#[test]
fn index_footprint_reports_sizes_and_schedules_segment_compaction() {
    let root = unique_temp_dir("gfm-index-footprint-root");
    let records = unique_temp_path("gfm-index-footprint-records", "gfmidx");
    let columns = unique_temp_path("gfm-index-footprint-columns", "gfmcols");
    let metadata = unique_temp_path("gfm-index-footprint-metadata", "gfmmeta");
    let prefixes = unique_temp_path("gfm-index-footprint-prefixes", "gfmprefix");
    let substrings = unique_temp_path("gfm-index-footprint-substrings", "gfmsubstr");
    let fuzzy = unique_temp_path("gfm-index-footprint-fuzzy", "gfmfuzzy");
    let content = unique_temp_path("gfm-index-footprint-content", "gfmcontent");
    let manifest = unique_temp_path("gfm-index-footprint-content", "gfmmanifest");
    let segment_dir = unique_temp_dir("gfm-index-footprint-segments");
    fs::write(
        root.join("project-needle.md"),
        "contentneedle with footprint telemetry",
    )
    .unwrap();

    let indexer = Indexer::default();
    let snapshot = indexer.build(&root).unwrap();
    snapshot
        .save_with_content(&records, &content, &Extractor::default())
        .unwrap();
    write_record_columns(&columns, &snapshot.records).unwrap();
    write_metadata_postings(
        &metadata,
        &metadata_postings_from_records(&snapshot.records),
    )
    .unwrap();
    write_prefix_postings(&prefixes, &prefix_postings_from_records(&snapshot.records)).unwrap();
    write_substring_postings(
        &substrings,
        &substring_postings_from_records(&snapshot.records),
    )
    .unwrap();
    write_fuzzy_postings(&fuzzy, &fuzzy_postings_from_records(&snapshot.records)).unwrap();
    ContentArchiveManifest::new(vec![ContentArchiveManifestEntry {
        tier: ContentMergeTier::Hot,
        path: content.clone(),
    }])
    .unwrap()
    .write(&manifest)
    .unwrap();
    fs::create_dir_all(&segment_dir).unwrap();
    let segments = (0..4)
        .map(|index| segment_dir.join(format!("footprint-{index}.gfmseg")))
        .collect::<Vec<_>>();
    for segment in &segments {
        snapshot
            .save_content_segment(segment, &Extractor::default(), Vec::new())
            .unwrap();
    }

    let mut spec = IndexFootprintSpec::new(&records);
    spec.columns = Some(columns.clone());
    spec.metadata = Some(metadata.clone());
    spec.prefixes = Some(prefixes.clone());
    spec.substrings = Some(substrings.clone());
    spec.fuzzy = Some(fuzzy.clone());
    spec.content_manifest = Some(manifest.clone());
    spec.content_segments = segments.clone();
    let report = inspect_index_footprint(&spec).unwrap();

    assert_eq!(report.record_count, snapshot.records.len());
    assert_eq!(report.column_count, snapshot.records.len());
    assert!(report.record_bytes > 0);
    assert!(report.column_bytes > 0);
    assert!(report.metadata_bytes > 0);
    assert!(report.prefix_keys > 0);
    assert!(report.substring_keys > 0);
    assert!(report.fuzzy_keys > 0);
    assert_eq!(report.content_archives, 1);
    assert!(report.content_terms > 0);
    assert_eq!(report.segment_count, 4);
    assert_eq!(report.compaction.reason, CompactionReason::TierPressure);
    assert!(report.compaction.scheduled);
    assert_eq!(report.compaction.action, CompactionAction::Run);
    assert_eq!(report.compaction.merge_segments, segments);
    assert_eq!(report.compaction.retained_segments.len(), 0);
    assert!(report.total_bytes >= report.segment_bytes);
    assert!(report.bytes_per_record > 0);

    spec.compaction_pressure = CompactionPressure {
        io: IoPressure::Elevated,
        thermal: ThermalState::Nominal,
        battery: BatteryState::AcPower,
        user_activity: UserActivity::Idle,
    };
    let throttled = inspect_index_footprint(&spec).unwrap();
    assert_eq!(throttled.compaction.action, CompactionAction::Throttle);
    assert!(throttled.compaction.scheduled);
    assert!(throttled.compaction.effective_max_merge_bytes < spec.merge_policy.max_merge_bytes);

    spec.compaction_pressure = CompactionPressure {
        io: IoPressure::Saturated,
        thermal: ThermalState::Nominal,
        battery: BatteryState::AcPower,
        user_activity: UserActivity::Idle,
    };
    let deferred = inspect_index_footprint(&spec).unwrap();
    assert_eq!(deferred.compaction.action, CompactionAction::Defer);
    assert!(!deferred.compaction.scheduled);
    assert_eq!(deferred.compaction.effective_max_merge_bytes, 0);

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(segment_dir).unwrap();
    for path in [
        records, columns, metadata, prefixes, substrings, fuzzy, content, manifest,
    ] {
        fs::remove_file(path).unwrap();
    }
}

#[test]
fn content_index_job_spec_round_trips() {
    let path = unique_temp_path("gfm-content-job", "job");
    let spec = ContentIndexJobSpec {
        root: PathBuf::from("/tmp/root with spaces"),
        segment_dir: PathBuf::from("/tmp/segments"),
        records_path: PathBuf::from("/tmp/records.gfmidx"),
        content_path: PathBuf::from("/tmp/content.gfmcontent"),
        volume: Some(VolumeId(42)),
        batch_size: 17,
    };

    spec.write(&path).unwrap();
    let read = ContentIndexJobSpec::read(&path).unwrap();

    assert_eq!(read, spec);
    fs::remove_file(path).unwrap();
}

#[test]
fn content_index_job_spec_reads_legacy_without_volume() {
    let path = unique_temp_path("gfm-content-job-legacy", "job");
    fs::write(
        &path,
        "gfm-content-job-v1\nroot\t/tmp/root\nsegment_dir\t/tmp/segments\nrecords_path\t/tmp/records.gfmidx\ncontent_path\t/tmp/content.gfmcontent\nbatch_size\t8\n",
    )
    .unwrap();

    let read = ContentIndexJobSpec::read(&path).unwrap();

    assert_eq!(read.root, PathBuf::from("/tmp/root"));
    assert_eq!(read.volume, None);
    assert_eq!(read.batch_size, 8);
    fs::remove_file(path).unwrap();
}

#[test]
fn persistent_index_state_tracks_volume_mount_and_epoch() {
    let root = unique_temp_dir("gfm-index-state-root");
    let records = unique_temp_path("gfm-index-state-records", "gfmidx");
    let state_path = unique_temp_path("gfm-index-state", "gfmstate");
    fs::write(root.join("Needle.md"), "state").unwrap();

    let indexer = Indexer::default();
    let first = indexer
        .build_persistent(&root, &records, &state_path)
        .unwrap();
    let second = indexer
        .build_persistent(&root, &records, &state_path)
        .unwrap();
    let reloaded = IndexVolumeState::read(&state_path).unwrap();
    let snapshot = indexer.load(&records).unwrap();

    assert_eq!(first.schema_version, INDEX_STATE_SCHEMA_VERSION);
    assert_eq!(first.scan_epoch, 1);
    assert_eq!(second.scan_epoch, 2);
    assert_eq!(second.volume_id, first.volume_id);
    assert_eq!(second.mount_id, first.mount_id);
    assert_eq!(reloaded, second);
    assert_eq!(snapshot.search("needle", 5).len(), 1);
    assert!(second.as_tsv().starts_with("index-state\t"));

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(state_path).unwrap();
}

#[test]
fn persistent_index_recovery_rebuilds_missing_or_stale_state() {
    let root = unique_temp_dir("gfm-index-recovery-state-root");
    let records = unique_temp_path("gfm-index-recovery-state-records", "gfmidx");
    let state_path = unique_temp_path("gfm-index-recovery-state", "gfmstate");
    let quarantine = unique_temp_dir("gfm-index-recovery-state-quarantine");
    fs::write(root.join("Recover.md"), "state").unwrap();

    let indexer = Indexer::default();
    indexer
        .build_persistent(&root, &records, &state_path)
        .unwrap();
    fs::remove_file(&state_path).unwrap();

    let plan = indexer.plan_persistent_recovery(&root, &records, &state_path);
    assert_eq!(plan.action, PersistentIndexAction::RebuildState);
    assert_eq!(plan.reason, PersistentIndexReason::MissingState);
    assert_eq!(plan.record_count, Some(2));

    let recovery = indexer
        .recover_persistent(&root, &records, &state_path, &quarantine)
        .unwrap();

    assert_eq!(recovery.before.action, PersistentIndexAction::RebuildState);
    assert!(recovery.rebuilt_state);
    assert!(!recovery.rebuilt_records);
    assert!(recovery.after.ready());
    assert_eq!(IndexVolumeState::read(&state_path).unwrap().record_count, 2);

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(state_path).unwrap();
    fs::remove_dir_all(quarantine).unwrap();
}

#[test]
fn persistent_index_recovery_quarantines_corrupt_records_before_rebuild() {
    let root = unique_temp_dir("gfm-index-recovery-records-root");
    let records = unique_temp_path("gfm-index-recovery-records", "gfmidx");
    let state_path = unique_temp_path("gfm-index-recovery-records-state", "gfmstate");
    let quarantine = unique_temp_dir("gfm-index-recovery-records-quarantine");
    fs::write(root.join("Recover.md"), "state").unwrap();

    let indexer = Indexer::default();
    indexer
        .build_persistent(&root, &records, &state_path)
        .unwrap();
    fs::write(&records, "gfm-records-v1\ncorrupt").unwrap();

    let plan = indexer.plan_persistent_recovery(&root, &records, &state_path);
    assert_eq!(
        plan.action,
        PersistentIndexAction::QuarantineRecordsAndRebuild
    );
    assert_eq!(plan.reason, PersistentIndexReason::UnreadableRecords);

    let recovery = indexer
        .recover_persistent(&root, &records, &state_path, &quarantine)
        .unwrap();

    assert!(recovery.rebuilt_records);
    assert!(recovery.rebuilt_state);
    assert!(recovery
        .quarantined_records_path
        .as_ref()
        .is_some_and(|path| path.exists()));
    assert!(recovery.after.ready());
    assert!(!indexer
        .load(&records)
        .unwrap()
        .search("recover", 5)
        .is_empty());

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(state_path).unwrap();
    fs::remove_dir_all(quarantine).unwrap();
}

#[test]
fn scan_progress_checkpoint_tracks_completed_scan_publication() {
    let root = unique_temp_dir("gfm-scan-progress-root");
    let records = unique_temp_path("gfm-scan-progress-records", "gfmidx");
    let progress = unique_temp_path("gfm-scan-progress", "gfmprogress");
    fs::write(root.join("Progress.md"), "state").unwrap();

    let indexer = Indexer::default();
    let checkpoint = indexer
        .build_with_progress(&root, &records, &progress)
        .unwrap();
    let reloaded = indexer.scan_progress(&progress).unwrap();
    let snapshot = indexer.load(&records).unwrap();

    assert_eq!(checkpoint, reloaded);
    assert!(checkpoint.completed);
    assert_eq!(checkpoint.published_segments, 1);
    assert!(snapshot
        .records
        .iter()
        .any(|record| record.name == "Progress.md"));
    assert!(checkpoint.as_tsv().starts_with("scan-progress\t"));

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(progress).unwrap();
}

#[test]
fn index_state_rejects_unsupported_schema_versions() {
    let path = unique_temp_path("gfm-index-state-bad", "gfmstate");
    fs::write(
        &path,
        "gfm-index-state-v1\nschema_version\t999\nroot\t/tmp/root\nrecords_path\t/tmp/index.gfmidx\nvolume_id\t1\nmount_id\tdev:1:root:/tmp/root\nscan_epoch\t1\nrecord_count\t1\ninaccessible_count\t0\n",
    )
    .unwrap();

    let error = IndexVolumeState::read(&path).unwrap_err();

    assert!(format!("{error}").contains("unsupported index state schema version 999"));
    fs::remove_file(path).unwrap();
}

#[test]
fn fsevents_cursor_checkpoints_and_resumes_from_next_event() {
    let root = unique_temp_dir("gfm-fsevents-cursor-root");
    let records = unique_temp_path("gfm-fsevents-cursor-records", "gfmidx");
    let state_path = unique_temp_path("gfm-fsevents-cursor-state", "gfmstate");
    let cursor_path = unique_temp_path("gfm-fsevents-cursor", "gfmcursor");
    fs::write(root.join("Evented.md"), "cursor").unwrap();

    let indexer = Indexer::default();
    indexer
        .build_persistent(&root, &records, &state_path)
        .unwrap();
    let cursor = indexer
        .checkpoint_fsevents_cursor(&state_path, &cursor_path, 42, FseventsCursorHealth::Clean)
        .unwrap();
    let plan = indexer
        .fsevents_resume_plan(&state_path, &cursor_path)
        .unwrap();

    assert_eq!(cursor.last_event_id, 42);
    assert_eq!(plan.action, FseventsResumeAction::Continue);
    assert_eq!(plan.from_event_id, Some(43));
    assert_eq!(plan.reason, "cursor-clean");
    assert!(cursor.as_tsv().starts_with("fsevents-cursor\t"));
    assert_eq!(
        plan.as_tsv(),
        "fsevents-resume\taction=continue\tfrom-event-id=43\treason=cursor-clean"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(state_path).unwrap();
    fs::remove_file(cursor_path).unwrap();
}

#[test]
fn fsevents_cursor_requires_rescan_for_missing_or_stale_state() {
    let root = unique_temp_dir("gfm-fsevents-rescan-root");
    let records = unique_temp_path("gfm-fsevents-rescan-records", "gfmidx");
    let state_path = unique_temp_path("gfm-fsevents-rescan-state", "gfmstate");
    let cursor_path = unique_temp_path("gfm-fsevents-rescan", "gfmcursor");
    fs::write(root.join("Repair.md"), "cursor").unwrap();

    let indexer = Indexer::default();
    let first = indexer
        .build_persistent(&root, &records, &state_path)
        .unwrap();
    let missing = indexer
        .fsevents_resume_plan(&state_path, &cursor_path)
        .unwrap();
    indexer
        .checkpoint_fsevents_cursor(
            &state_path,
            &cursor_path,
            100,
            FseventsCursorHealth::RepairRequired,
        )
        .unwrap();
    let repair = indexer
        .fsevents_resume_plan(&state_path, &cursor_path)
        .unwrap();
    let second = indexer
        .build_persistent(&root, &records, &state_path)
        .unwrap();
    let stale_epoch = indexer
        .fsevents_resume_plan(&state_path, &cursor_path)
        .unwrap();

    assert_eq!(missing.action, FseventsResumeAction::Rescan);
    assert_eq!(missing.reason, "missing-cursor");
    assert_eq!(repair.action, FseventsResumeAction::Rescan);
    assert_eq!(repair.reason, "repair-required");
    assert_eq!(second.scan_epoch, first.scan_epoch + 1);
    assert_eq!(stale_epoch.action, FseventsResumeAction::Rescan);
    assert_eq!(stale_epoch.reason, "scan-epoch-changed");

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(state_path).unwrap();
    fs::remove_file(cursor_path).unwrap();
}

#[test]
fn fsevents_cursor_rejects_unsupported_schema_versions() {
    let path = unique_temp_path("gfm-fsevents-cursor-bad", "gfmcursor");
    fs::write(
        &path,
        "gfm-fsevents-cursor-v1\nschema_version\t999\nvolume_id\t1\nmount_id\tdev:1:root:/tmp/root\nscan_epoch\t1\nlast_event_id\t10\nhealth\tclean\n",
    )
    .unwrap();

    let error = FseventsCursor::read(&path).unwrap_err();

    assert!(format!("{error}").contains("unsupported FSEvents cursor schema version 999"));
    fs::remove_file(path).unwrap();
}

#[test]
fn repair_schedule_detects_event_id_gaps() {
    let root = unique_temp_dir("gfm-repair-gap-root");
    let records = unique_temp_path("gfm-repair-gap-records", "gfmidx");
    let state_path = unique_temp_path("gfm-repair-gap-state", "gfmstate");
    let cursor_path = unique_temp_path("gfm-repair-gap-cursor", "gfmcursor");
    fs::write(root.join("Gap.md"), "repair").unwrap();

    let indexer = Indexer::default();
    indexer
        .build_persistent(&root, &records, &state_path)
        .unwrap();
    indexer
        .checkpoint_fsevents_cursor(&state_path, &cursor_path, 10, FseventsCursorHealth::Clean)
        .unwrap();

    let clean = indexer
        .repair_schedule(&state_path, &cursor_path, &[11, 12, 13], &[], None)
        .unwrap();
    let gap = indexer
        .repair_schedule(&state_path, &cursor_path, &[11, 14], &[], None)
        .unwrap();

    assert!(clean.jobs.is_empty());
    assert_eq!(clean.highest_observed_event_id, Some(13));
    assert_eq!(gap.jobs.len(), 1);
    assert_eq!(gap.jobs[0].path, root);
    assert_eq!(gap.jobs[0].priority, RepairPriority::High);
    assert_eq!(
        gap.jobs[0].reason,
        RepairReason::EventIdGap {
            expected: 12,
            observed: 14
        }
    );
    assert!(gap.as_tsv().contains("repair-schedule\taction=continue"));

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(state_path).unwrap();
    fs::remove_file(cursor_path).unwrap();
}

#[test]
fn repair_schedule_rescans_for_invalid_resume_and_coalesces_subtrees() {
    let root = unique_temp_dir("gfm-repair-coalesce-root");
    let records = unique_temp_path("gfm-repair-coalesce-records", "gfmidx");
    let state_path = unique_temp_path("gfm-repair-coalesce-state", "gfmstate");
    let cursor_path = unique_temp_path("gfm-repair-coalesce-cursor", "gfmcursor");
    fs::create_dir_all(root.join("Projects").join("Nested")).unwrap();
    fs::write(
        root.join("Projects").join("Nested").join("Drop.md"),
        "repair",
    )
    .unwrap();

    let indexer = Indexer::default();
    indexer
        .build_persistent(&root, &records, &state_path)
        .unwrap();
    indexer
        .checkpoint_fsevents_cursor(
            &state_path,
            &cursor_path,
            5,
            FseventsCursorHealth::RepairRequired,
        )
        .unwrap();

    let schedule = indexer
        .repair_schedule(
            &state_path,
            &cursor_path,
            &[6],
            &[
                PathBuf::from("Projects"),
                PathBuf::from("Projects").join("Nested"),
            ],
            Some("kernel-dropped"),
        )
        .unwrap();

    assert_eq!(schedule.resume.action, FseventsResumeAction::Rescan);
    assert_eq!(schedule.jobs.len(), 1);
    assert_eq!(schedule.jobs[0].path, root);
    assert_eq!(schedule.jobs[0].priority, RepairPriority::Critical);
    assert_eq!(
        schedule.jobs[0].reason,
        RepairReason::ResumeRequired("repair-required".to_string())
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(state_path).unwrap();
    fs::remove_file(cursor_path).unwrap();
}

#[test]
fn repair_schedule_coalesces_explicit_subtree_repairs() {
    let root = unique_temp_dir("gfm-repair-explicit-root");
    let records = unique_temp_path("gfm-repair-explicit-records", "gfmidx");
    let state_path = unique_temp_path("gfm-repair-explicit-state", "gfmstate");
    let cursor_path = unique_temp_path("gfm-repair-explicit-cursor", "gfmcursor");
    fs::create_dir_all(root.join("A").join("B")).unwrap();
    fs::write(root.join("A").join("B").join("C.md"), "repair").unwrap();

    let indexer = Indexer::default();
    indexer
        .build_persistent(&root, &records, &state_path)
        .unwrap();
    indexer
        .checkpoint_fsevents_cursor(&state_path, &cursor_path, 20, FseventsCursorHealth::Clean)
        .unwrap();

    let schedule = indexer
        .repair_schedule(
            &state_path,
            &cursor_path,
            &[21],
            &[PathBuf::from("A"), PathBuf::from("A").join("B")],
            Some("user-dropped"),
        )
        .unwrap();

    assert_eq!(schedule.jobs.len(), 1);
    assert_eq!(schedule.jobs[0].path, root.join("A"));
    assert_eq!(
        schedule.jobs[0].reason,
        RepairReason::ExplicitDrop("user-dropped".to_string())
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(state_path).unwrap();
    fs::remove_file(cursor_path).unwrap();
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let path = unique_temp_path(prefix, "");
    fs::create_dir_all(&path).unwrap();
    path
}

fn unique_temp_path(prefix: &str, extension: &str) -> PathBuf {
    let mut name = format!(
        "{}-{}",
        prefix,
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    if !extension.is_empty() {
        name.push('.');
        name.push_str(extension);
    }
    std::env::temp_dir().join(name)
}

fn set_finder_tags(path: &Path, tags: &[&str]) {
    let value = plist::Value::Array(
        tags.iter()
            .map(|tag| plist::Value::String((*tag).to_string()))
            .collect(),
    );
    let mut payload = Vec::new();
    value.to_writer_binary(&mut payload).unwrap();
    xattr::set(path, "com.apple.metadata:_kMDItemUserTags", &payload).unwrap();
}

fn corrupt_pdf() -> Vec<u8> {
    b"%PDF-1.4
1 0 obj
<< /Type /Page /Contents 2 0 R >>
endobj
2 0 obj
<< /Length 12 /Filter /FlateDecode >>
stream
not-valid-zlib
endstream
endobj"
        .to_vec()
}
