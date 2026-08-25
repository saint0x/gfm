use super::*;
use gfm_store::{write_fuzzy_postings, write_prefix_postings, FuzzyPosting, PrefixPosting};
use gfm_types::{FileKind, MatchReason, VolumeId};
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

    let (live, applied, metadata_keys, prefix_keys, fuzzy_keys, content_keys) =
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
    assert_eq!(fuzzy_keys, 1);
    assert_eq!(content_keys, 1);
    assert_eq!(live.search("tag:Important", 5).len(), 1);
    assert_eq!(live.search("launch", 5).len(), 1);
    assert_eq!(live.search("proj", 5).len(), 1);
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
    let fuzzy = unique_temp_path("gfm-fuzzy-archive-lookup", "gfmfuzzy");

    write_prefix_postings(
        &prefixes,
        &[PrefixPosting {
            prefix: "proj".to_string(),
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

    let (live, applied, metadata_keys, prefix_keys, fuzzy_keys, content_keys) =
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
        );
    let lookup = SearchArchiveLookup::open(&prefixes, &fuzzy).unwrap();

    assert_eq!(applied, 1);
    assert_eq!(metadata_keys, 0);
    assert_eq!(prefix_keys, 0);
    assert_eq!(fuzzy_keys, 0);
    assert_eq!(content_keys, 0);
    assert_eq!(lookup.indexed_prefixes(), 1);
    assert_eq!(lookup.indexed_fuzzy_keys(), 1);
    let prefix_without_lookup = live.search("proj", 5);
    assert_eq!(prefix_without_lookup.len(), 1);
    assert_eq!(prefix_without_lookup[0].reason, MatchReason::SubstringName);
    assert_eq!(live.search("needle", 5).len(), 0);
    let prefix_hits = live.search_with_lookup("proj", 5, &lookup).unwrap();
    let fuzzy_hits = live.search_with_lookup("needle", 5, &lookup).unwrap();
    assert_eq!(prefix_hits.len(), 1);
    assert_eq!(prefix_hits[0].reason, MatchReason::PrefixName);
    assert_eq!(fuzzy_hits.len(), 1);
    assert_eq!(fuzzy_hits[0].reason, MatchReason::FuzzyName);

    fs::remove_file(prefixes).unwrap();
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
    let (live, keys) = indexer
        .load_live_with_content_manifest(&records, &manifest, "maintenancetoken")
        .unwrap();
    let hits = live.search("maintenancetoken", 5);

    assert!(report.scheduled);
    assert_eq!(report.merged_segments.len(), 4);
    assert_eq!(report.retained_segments.len(), 0);
    assert_eq!(report.manifest_archives, 2);
    assert_eq!(report.published_archive, Some(output_content.clone()));
    assert_eq!(keys, 1);
    assert_eq!(hits.len(), 1);

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(segments).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(initial_content).unwrap();
    fs::remove_file(output_content).unwrap();
    fs::remove_file(manifest).unwrap();
}

#[test]
fn content_index_job_spec_round_trips() {
    let path = unique_temp_path("gfm-content-job", "job");
    let spec = ContentIndexJobSpec {
        root: PathBuf::from("/tmp/root with spaces"),
        segment_dir: PathBuf::from("/tmp/segments"),
        records_path: PathBuf::from("/tmp/records.gfmidx"),
        content_path: PathBuf::from("/tmp/content.gfmcontent"),
        batch_size: 17,
    };

    spec.write(&path).unwrap();
    let read = ContentIndexJobSpec::read(&path).unwrap();

    assert_eq!(read, spec);
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
