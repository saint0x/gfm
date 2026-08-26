use super::*;
use gfm_types::{
    ContentPositions, ContentPosting, FileId, FileKind, FileRecord, GfmError, VolumeId,
};
use std::path::PathBuf;
use std::time::{Duration, UNIX_EPOCH};

#[test]
fn ranks_exact_name_above_path_component() {
    let mut index = SearchIndex::new();
    index.insert(record(1, "/Users/me/Desktop/report.pdf", "report.pdf"));
    index.insert(record(2, "/Users/me/report/archive.txt", "archive.txt"));

    let hits = index.query("report", 10);

    assert_eq!(hits[0].record.name, "report.pdf");
    assert_eq!(hits[0].reason, MatchReason::PrefixName);
    assert!(hits[0].score > hits[1].score);
}

#[test]
fn removes_reindexed_records() {
    let mut index = SearchIndex::new();
    let mut item = record(1, "/tmp/alpha.txt", "alpha.txt");
    index.insert(item.clone());
    item.path = PathBuf::from("/tmp/beta.txt");
    item.name = "beta.txt".to_string();
    index.insert(item);

    assert!(index.query("alpha", 10).is_empty());
    assert_eq!(index.query("beta", 10).len(), 1);
}

#[test]
fn name_substring_index_finds_infix_matches_without_token_prefix() {
    let mut index = SearchIndex::new();
    index.insert(record(1, "/tmp/a.pdf", "report.pdf"));
    index.insert(record(2, "/tmp/b.pdf", "notes.pdf"));

    let hits = index.query("port", 10);

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.name, "report.pdf");
    assert_eq!(hits[0].reason, MatchReason::SubstringName);
}

#[test]
fn reindexed_records_remove_stale_name_substring_postings() {
    let mut index = SearchIndex::new();
    let mut item = record(1, "/tmp/a.pdf", "report.pdf");
    index.insert(item.clone());

    item.name = "notes.pdf".to_string();
    item.path = PathBuf::from("/tmp/b.pdf");
    index.insert(item);

    assert!(index.query("port", 10).is_empty());
    assert_eq!(index.query("ote", 10).len(), 1);
}

#[test]
fn imported_substring_postings_drive_deferred_sidecar_search() {
    let mut index = SearchIndex::new();
    let item = record(1, "/tmp/report.pdf", "report.pdf");
    let columns = SearchRecordColumns {
        id: item.id,
        name: item.name.clone(),
        path: item.path.to_string_lossy().into_owned(),
        extension: item.extension().map(ToOwned::to_owned),
        tags: item.tags.clone(),
        comment: item.finder_comment.clone(),
    };
    assert!(index.insert_with_columns_deferred_sidecars(item, columns));
    assert!(index.query("port", 10).is_empty());

    assert_eq!(
        index.import_substring_postings(&[
            SearchSubstringPosting {
                gram: "por".to_string(),
                ids: vec![FileId::new(VolumeId(1), 1)],
            },
            SearchSubstringPosting {
                gram: "ort".to_string(),
                ids: vec![FileId::new(VolumeId(1), 1)],
            },
        ]),
        2
    );
    let hits = index.query("port", 10);

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.name, "report.pdf");
    assert_eq!(hits[0].reason, MatchReason::SubstringName);
}

#[test]
fn sidecar_substring_lookup_budget_caps_grams_and_reports_truncation() {
    let mut index = SearchIndex::new();
    for (id, name) in [
        (1, "report-alpha.md"),
        (2, "report-beta.md"),
        (3, "report-gamma.md"),
    ] {
        let item = record(id, &format!("/tmp/{name}"), name);
        let columns = SearchRecordColumns {
            id: item.id,
            name: item.name.clone(),
            path: item.path.to_string_lossy().into_owned(),
            extension: item.extension().map(ToOwned::to_owned),
            tags: item.tags.clone(),
            comment: item.finder_comment.clone(),
        };
        assert!(index.insert_with_columns_deferred_sidecars(item, columns));
    }
    let lookup = StaticLookup {
        prefix_ids: Vec::new(),
        substring_ids: vec![
            FileId::new(VolumeId(1), 1),
            FileId::new(VolumeId(1), 2),
            FileId::new(VolumeId(1), 3),
        ],
        fuzzy_terms: Vec::new(),
    };

    let report = index
        .query_structured_with_lookup_budget_cancellable(
            &SearchQuery::parse("port"),
            10,
            &lookup,
            SearchLookupBudget {
                max_substring_grams_per_term: 1,
                max_substring_ids_per_gram: 2,
                ..SearchLookupBudget::default()
            },
            &Cancellation::default(),
        )
        .unwrap();

    assert_eq!(report.lookup.substring_terms, 1);
    assert_eq!(report.lookup.substring_grams, 1);
    assert_eq!(report.lookup.substring_lookup_ids, 2);
    assert_eq!(report.lookup.substring_candidate_ids, 2);
    assert_eq!(report.lookup.substring_term_truncated_grams, 1);
    assert_eq!(report.lookup.substring_truncated_grams, 1);
    assert!(!report.hits.is_empty());
}

#[test]
fn hot_substring_lookup_budget_caps_local_candidates_before_archive_lookup() {
    let mut index = SearchIndex::new();
    index.insert(record(1, "/tmp/report-alpha.md", "report-alpha.md"));
    index.insert(record(2, "/tmp/report-beta.md", "report-beta.md"));
    index.insert(record(3, "/tmp/report-gamma.md", "report-gamma.md"));
    let lookup = StaticLookup {
        prefix_ids: Vec::new(),
        substring_ids: vec![FileId::new(VolumeId(1), 3)],
        fuzzy_terms: Vec::new(),
    };

    let report = index
        .query_structured_with_lookup_budget_cancellable(
            &SearchQuery::parse("port"),
            10,
            &lookup,
            SearchLookupBudget {
                max_substring_grams_per_term: 1,
                max_substring_ids_per_gram: 2,
                ..SearchLookupBudget::default()
            },
            &Cancellation::default(),
        )
        .unwrap();

    assert_eq!(report.lookup.substring_terms, 1);
    assert_eq!(report.lookup.substring_grams, 1);
    assert_eq!(report.lookup.substring_lookup_ids, 0);
    assert_eq!(report.lookup.substring_candidate_ids, 2);
    assert_eq!(report.lookup.substring_term_truncated_grams, 1);
    assert_eq!(report.lookup.substring_truncated_grams, 1);
    assert_eq!(report.hits.len(), 2);
}

#[test]
fn short_substring_query_does_not_expand_to_all_records() {
    let mut index = SearchIndex::new();
    index.insert(record(1, "/tmp/alpha.pdf", "alpha.pdf"));
    index.insert(record(2, "/tmp/report.pdf", "report.pdf"));
    let lookup = StaticLookup {
        prefix_ids: Vec::new(),
        substring_ids: Vec::new(),
        fuzzy_terms: Vec::new(),
    };

    let report = index
        .query_structured_with_lookup_budget_cancellable(
            &SearchQuery::parse("lp"),
            10,
            &lookup,
            SearchLookupBudget::default(),
            &Cancellation::default(),
        )
        .unwrap();

    assert!(report.hits.is_empty());
    assert_eq!(report.lookup.substring_cutoff_terms, 1);
    assert_eq!(report.lookup.substring_grams, 0);
    assert_eq!(report.lookup.substring_lookup_requests, 0);
    assert_eq!(report.lookup.substring_candidate_ids, 0);
}

#[test]
fn reindexed_records_refresh_cached_columns() {
    let mut index = SearchIndex::new();
    let mut item = record(1, "/tmp/alpha.md", "alpha.md");
    item.tags = vec!["Important".to_string()];
    item.finder_comment = Some("launch notes".to_string());
    index.insert(item.clone());

    item.path = PathBuf::from("/tmp/beta.pdf");
    item.name = "beta.pdf".to_string();
    item.tags = vec!["Later".to_string()];
    item.finder_comment = Some("archive notes".to_string());
    index.insert(item);

    assert!(index.query("alpha", 10).is_empty());
    assert!(index.query("tag:important", 10).is_empty());
    assert!(index.query("launch", 10).is_empty());
    assert!(index.query("ext:md", 10).is_empty());
    assert_eq!(index.query("beta tag:later archive ext:pdf", 10).len(), 1);
}

#[test]
fn applied_record_columns_drive_matching_and_filters() {
    let mut index = SearchIndex::new();
    let item = record(1, "/tmp/original.txt", "original.txt");
    index.insert(item.clone());

    assert!(index.apply_record_columns(SearchRecordColumns {
        id: item.id,
        name: "cached.md".to_string(),
        path: "/tmp/cached.md".to_string(),
        extension: Some("md".to_string()),
        tags: vec!["Important".to_string()],
        comment: Some("Launch Notes".to_string()),
    }));

    assert!(index.query("original", 10).is_empty());
    assert_eq!(
        index.query("cached tag:important launch ext:md", 10).len(),
        1
    );
}

#[test]
fn inserted_record_columns_build_terms_without_reindexing_record_fields() {
    let mut index = SearchIndex::new();
    let item = record(1, "/tmp/original.txt", "original.txt");

    assert!(index.insert_with_columns(
        item,
        SearchRecordColumns {
            id: FileId::new(VolumeId(1), 1),
            name: "cached.md".to_string(),
            path: "/tmp/cached.md".to_string(),
            extension: Some("md".to_string()),
            tags: vec!["Important".to_string()],
            comment: Some("Launch Notes".to_string()),
        },
    ));

    assert!(index.query("original", 10).is_empty());
    assert_eq!(
        index.query("cached tag:important launch ext:md", 10).len(),
        1
    );
}

#[test]
fn reinserting_record_columns_removes_stale_sidecar_terms() {
    let mut index = SearchIndex::new();
    let item = record(1, "/tmp/original.txt", "original.txt");
    assert!(index.insert_with_columns(
        item.clone(),
        SearchRecordColumns {
            id: item.id,
            name: "cached.md".to_string(),
            path: "/tmp/cached.md".to_string(),
            extension: Some("md".to_string()),
            tags: vec!["Important".to_string()],
            comment: Some("Launch Notes".to_string()),
        },
    ));
    assert!(index.insert_with_columns(
        item.clone(),
        SearchRecordColumns {
            id: item.id,
            name: "fresh.pdf".to_string(),
            path: "/tmp/fresh.pdf".to_string(),
            extension: Some("pdf".to_string()),
            tags: vec!["Later".to_string()],
            comment: Some("Archive Notes".to_string()),
        },
    ));

    assert!(index
        .query("cached tag:important launch ext:md", 10)
        .is_empty());
    assert_eq!(index.query("fresh tag:later archive ext:pdf", 10).len(), 1);
}

#[test]
fn mismatched_record_columns_fall_back_to_record_indexing() {
    let mut index = SearchIndex::new();
    let item = record(1, "/tmp/original.txt", "original.txt");

    assert!(!index.insert_with_columns(
        item,
        SearchRecordColumns {
            id: FileId::new(VolumeId(1), 99),
            name: "cached.md".to_string(),
            path: "/tmp/cached.md".to_string(),
            extension: Some("md".to_string()),
            tags: Vec::new(),
            comment: None,
        },
    ));

    assert_eq!(index.query("original", 10).len(), 1);
    assert!(index.query("cached ext:md", 10).is_empty());
}

#[test]
fn imported_fuzzy_postings_drive_deferred_fuzzy_search() {
    let mut index = SearchIndex::new();
    let item = record(1, "/tmp/original.txt", "original.txt");
    assert!(index.insert_with_columns_deferred_fuzzy(
        item,
        SearchRecordColumns {
            id: FileId::new(VolumeId(1), 1),
            name: "needl.md".to_string(),
            path: "/tmp/needl.md".to_string(),
            extension: Some("md".to_string()),
            tags: Vec::new(),
            comment: None,
        },
    ));
    assert!(index.query("needle", 10).is_empty());

    assert_eq!(
        index.import_fuzzy_postings(&[SearchFuzzyPosting {
            key: "needl".to_string(),
            terms: vec!["needl".to_string()],
        }]),
        1
    );
    let hits = index.query("needle", 10);

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].reason, MatchReason::FuzzyName);
}

#[test]
fn imported_fuzzy_postings_preserve_fallback_fuzzy_terms() {
    let mut index = SearchIndex::new();
    index.insert(record(1, "/tmp/plannn.md", "plannn.md"));
    assert!(index.insert_with_columns_deferred_fuzzy(
        record(2, "/tmp/original.txt", "original.txt"),
        SearchRecordColumns {
            id: FileId::new(VolumeId(1), 2),
            name: "sidecarr.md".to_string(),
            path: "/tmp/sidecarr.md".to_string(),
            extension: Some("md".to_string()),
            tags: Vec::new(),
            comment: None,
        },
    ));

    assert_eq!(index.query("planner", 10).len(), 1);
    assert!(index.query("sidecarz", 10).is_empty());
    assert!(
        index.import_fuzzy_postings(&[SearchFuzzyPosting {
            key: "sidecar".to_string(),
            terms: vec!["sidecarr".to_string()],
        }]) >= 2
    );

    assert_eq!(index.query("planner", 10).len(), 1);
    assert_eq!(index.query("sidecarz", 10).len(), 1);
}

#[test]
fn imported_prefix_postings_drive_deferred_prefix_search() {
    let mut index = SearchIndex::new();
    assert!(index.insert_with_columns_deferred_sidecars(
        record(1, "/tmp/original.txt", "original.txt"),
        SearchRecordColumns {
            id: FileId::new(VolumeId(1), 1),
            name: "project-alpha.md".to_string(),
            path: "/tmp/project-alpha.md".to_string(),
            extension: Some("md".to_string()),
            tags: Vec::new(),
            comment: None,
        },
    ));
    assert!(index.query("proj", 10).is_empty());

    assert_eq!(
        index.import_prefix_postings(&[SearchPrefixPosting {
            prefix: "proj".to_string(),
            ids: vec![FileId::new(VolumeId(1), 1)],
        }]),
        1
    );
    let hits = index.query("proj", 10);

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.name, "original.txt");
    assert_eq!(hits[0].reason, MatchReason::PrefixName);
}

#[test]
fn sidecar_prefix_lookup_budget_caps_candidates_and_reports_truncation() {
    let mut index = SearchIndex::new();
    index.insert(record(1, "/tmp/project-alpha.md", "project-alpha.md"));
    index.insert(record(2, "/tmp/project-beta.md", "project-beta.md"));
    index.insert(record(3, "/tmp/project-gamma.md", "project-gamma.md"));
    let lookup = StaticLookup {
        prefix_ids: vec![
            FileId::new(VolumeId(1), 1),
            FileId::new(VolumeId(1), 2),
            FileId::new(VolumeId(1), 3),
        ],
        substring_ids: Vec::new(),
        fuzzy_terms: Vec::new(),
    };

    let report = index
        .query_structured_with_lookup_budget_cancellable(
            &SearchQuery::parse("proj"),
            10,
            &lookup,
            SearchLookupBudget {
                max_prefix_ids_per_term: 2,
                ..SearchLookupBudget::default()
            },
            &Cancellation::default(),
        )
        .unwrap();

    assert_eq!(report.lookup.prefix_terms, 1);
    assert_eq!(report.lookup.prefix_lookup_ids, 0);
    assert_eq!(report.lookup.prefix_candidate_ids, 2);
    assert_eq!(report.lookup.prefix_cutoff_terms, 1);
    assert!(report.lookup.prefix_truncated_terms >= 1);
    assert!(!report.hits.is_empty());
}

#[test]
fn sidecar_prefix_lookup_budget_cuts_off_short_archive_prefixes() {
    let mut index = SearchIndex::new();
    index.insert(record(1, "/tmp/omega.md", "omega.md"));
    let lookup = StaticLookup {
        prefix_ids: vec![FileId::new(VolumeId(1), 1)],
        substring_ids: Vec::new(),
        fuzzy_terms: Vec::new(),
    };

    let report = index
        .query_structured_with_lookup_budget_cancellable(
            &SearchQuery::parse("p"),
            10,
            &lookup,
            SearchLookupBudget {
                min_archive_prefix_chars: 2,
                ..SearchLookupBudget::default()
            },
            &Cancellation::default(),
        )
        .unwrap();

    assert_eq!(report.lookup.prefix_terms, 1);
    assert_eq!(report.lookup.prefix_lookup_ids, 0);
    assert_eq!(report.lookup.prefix_candidate_ids, 0);
    assert_eq!(report.lookup.prefix_cutoff_terms, 1);
    assert!(report.hits.is_empty());
}

#[test]
fn imported_prefix_postings_preserve_fallback_prefix_terms() {
    let mut index = SearchIndex::new();
    index.insert(record(1, "/tmp/profile.md", "profile.md"));
    assert!(index.insert_with_columns_deferred_sidecars(
        record(2, "/tmp/original.txt", "original.txt"),
        SearchRecordColumns {
            id: FileId::new(VolumeId(1), 2),
            name: "project-alpha.md".to_string(),
            path: "/tmp/project-alpha.md".to_string(),
            extension: Some("md".to_string()),
            tags: Vec::new(),
            comment: None,
        },
    ));

    assert_eq!(index.query("prof", 10).len(), 1);
    assert!(index.query("proj", 10).is_empty());
    assert!(
        index.import_prefix_postings(&[SearchPrefixPosting {
            prefix: "proj".to_string(),
            ids: vec![FileId::new(VolumeId(1), 2), FileId::new(VolumeId(9), 9)],
        }]) >= 2
    );

    assert_eq!(index.query("prof", 10)[0].reason, MatchReason::PrefixName);
    assert_eq!(index.query("proj", 10)[0].reason, MatchReason::PrefixName);
}

#[test]
fn imported_metadata_postings_drive_deferred_tag_and_comment_search() {
    let mut index = SearchIndex::new();
    assert!(index.insert_with_columns_deferred_sidecars(
        record(1, "/tmp/original.txt", "original.txt"),
        SearchRecordColumns {
            id: FileId::new(VolumeId(1), 1),
            name: "plain.md".to_string(),
            path: "/tmp/plain.md".to_string(),
            extension: Some("md".to_string()),
            tags: vec!["Important".to_string()],
            comment: Some("launch notes".to_string()),
        },
    ));
    assert!(index.query("important", 10).is_empty());
    assert!(index.query("launch", 10).is_empty());

    assert_eq!(
        index.import_metadata_postings(&[
            SearchMetadataPosting {
                field: SearchMetadataField::Tag,
                term: "Important".to_string(),
                ids: vec![FileId::new(VolumeId(1), 1)],
            },
            SearchMetadataPosting {
                field: SearchMetadataField::Comment,
                term: "launch".to_string(),
                ids: vec![FileId::new(VolumeId(1), 1)],
            },
        ]),
        2
    );

    assert_eq!(index.query("important", 10)[0].reason, MatchReason::Tag);
    assert_eq!(index.query("launch", 10)[0].reason, MatchReason::Tag);
}

#[test]
fn imported_metadata_postings_preserve_fallback_metadata_terms() {
    let mut index = SearchIndex::new();
    let mut fallback = record(1, "/tmp/fallback.md", "fallback.md");
    fallback.tags = vec!["Existing".to_string()];
    fallback.finder_comment = Some("fallback comment".to_string());
    index.insert(fallback);
    assert!(index.insert_with_columns_deferred_sidecars(
        record(2, "/tmp/plain.txt", "plain.txt"),
        SearchRecordColumns {
            id: FileId::new(VolumeId(1), 2),
            name: "plain.md".to_string(),
            path: "/tmp/plain.md".to_string(),
            extension: Some("md".to_string()),
            tags: vec!["Important".to_string()],
            comment: Some("launch notes".to_string()),
        },
    ));

    assert_eq!(index.query("tag:Existing", 10).len(), 1);
    assert!(index.query("important", 10).is_empty());
    assert!(
        index.import_metadata_postings(&[SearchMetadataPosting {
            field: SearchMetadataField::Tag,
            term: "Important".to_string(),
            ids: vec![FileId::new(VolumeId(1), 2), FileId::new(VolumeId(9), 9)],
        }]) >= 2
    );

    assert_eq!(index.query("tag:Existing", 10).len(), 1);
    assert_eq!(index.query("important", 10)[0].reason, MatchReason::Tag);
}

#[test]
fn name_prefix_postings_drive_interactive_prefix_search() {
    let mut index = SearchIndex::new();
    index.insert(record(1, "/tmp/project-plan.md", "project-plan.md"));
    index.insert(record(2, "/tmp/profile.txt", "profile.txt"));
    index.insert(record(3, "/tmp/archive.txt", "archive.txt"));

    assert_eq!(index.name_prefix_posting_count("pro"), 2);
    assert_eq!(index.name_prefix_posting_count("proj"), 1);

    let hits = index.query("proj", 10);

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.name, "project-plan.md");
    assert_eq!(hits[0].reason, MatchReason::PrefixName);
}

#[test]
fn reindexed_records_remove_stale_name_prefix_postings() {
    let mut index = SearchIndex::new();
    let mut item = record(1, "/tmp/quartz.md", "quartz.md");
    index.insert(item.clone());

    item.path = PathBuf::from("/tmp/ledger.md");
    item.name = "ledger.md".to_string();
    index.insert(item);

    assert_eq!(index.name_prefix_posting_count("qua"), 0);
    assert!(index.query("qua", 10).is_empty());
    assert_eq!(index.query("led", 10).len(), 1);
}

#[test]
fn applied_record_columns_replace_name_prefix_postings() {
    let mut index = SearchIndex::new();
    let item = record(1, "/tmp/original.txt", "original.txt");
    index.insert(item.clone());

    assert!(index.apply_record_columns(SearchRecordColumns {
        id: item.id,
        name: "cached.md".to_string(),
        path: "/tmp/cached.md".to_string(),
        extension: Some("md".to_string()),
        tags: Vec::new(),
        comment: None,
    }));

    assert_eq!(index.name_prefix_posting_count("ori"), 0);
    assert_eq!(index.name_prefix_posting_count("cac"), 1);
    assert!(index.query("ori", 10).is_empty());
    assert_eq!(index.query("cac", 10).len(), 1);
}

#[test]
fn removes_subtree_by_path() {
    let mut index = SearchIndex::new();
    index.insert(record(1, "/tmp/folder", "folder"));
    index.insert(record(2, "/tmp/folder/child.txt", "child.txt"));
    index.insert(record(3, "/tmp/other.txt", "other.txt"));

    let removed = index.remove_subtree("/tmp/folder");

    assert_eq!(removed.len(), 2);
    assert!(index.query("child", 10).is_empty());
    assert_eq!(index.query("other", 10).len(), 1);
}

#[test]
fn finds_content_terms() {
    let mut index = SearchIndex::new();
    let item = record(1, "/tmp/notes.txt", "notes.txt");
    index.insert(item.clone());
    index.insert_content(
        item.id,
        "an elite file manager needs instant content search",
    );

    let hits = index.query("instant", 10);

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].reason, MatchReason::Content);
}

#[test]
fn content_term_scoring_uses_bounded_top_hits_without_id_materialization() {
    let mut index = SearchIndex::new();
    for node in 1..=48 {
        let item = record(
            node,
            &format!("/tmp/content-{node}.txt"),
            &format!("content-{node}.txt"),
        );
        index.insert(item.clone());
        index.insert_content(item.id, "needle body text");
    }
    let first = record(98, "/tmp/a/needle", "needle");
    let second = record(99, "/tmp/b/needle", "needle");
    index.insert(first.clone());
    index.insert(second.clone());
    index.insert_content(first.id, "needle body text");
    index.insert_content(second.id, "needle body text");

    let hits = index.query("needle", 2);
    let paths: Vec<_> = hits.into_iter().map(|hit| hit.record.path).collect();

    assert_eq!(
        paths,
        vec![
            PathBuf::from("/tmp/a/needle"),
            PathBuf::from("/tmp/b/needle")
        ]
    );
}

#[test]
fn simple_content_term_search_keeps_deterministic_top_hits() {
    let mut index = SearchIndex::new();
    for node in (1..=96).rev() {
        let item = record(
            node,
            &format!("/tmp/content/{node:03}.txt"),
            &format!("{node:03}.txt"),
        );
        index.insert(item.clone());
        index.insert_content(item.id, "needle body text");
    }

    let hits = index.query("needle", 3);
    let paths: Vec<_> = hits.into_iter().map(|hit| hit.record.path).collect();

    assert_eq!(
        paths,
        vec![
            PathBuf::from("/tmp/content/001.txt"),
            PathBuf::from("/tmp/content/002.txt"),
            PathBuf::from("/tmp/content/003.txt")
        ]
    );
}

#[test]
fn simple_content_term_search_combines_exact_and_content_scores() {
    let mut index = SearchIndex::new();
    let exact_and_content = record(1, "/tmp/a/needle", "needle");
    let exact_only = record(2, "/tmp/b/needle", "needle");
    index.insert(exact_and_content.clone());
    index.insert(exact_only);
    index.insert_content(exact_and_content.id, "needle body text");

    let hits = index.query("needle", 2);

    assert_eq!(hits[0].record.id, exact_and_content.id);
    assert_eq!(hits[0].reason, MatchReason::ExactName);
    assert!(hits[0].score > hits[1].score);
}

#[test]
fn simple_multi_term_content_search_anchors_on_bounded_candidates() {
    let mut index = SearchIndex::new();
    for node in (1..=96).rev() {
        let item = record(
            node,
            &format!("/tmp/multi/{node:03}.txt"),
            &format!("{node:03}.txt"),
        );
        index.insert(item.clone());
        index.insert_content(item.id, "alpha beta body text");
    }

    let hits = index.query("alpha beta", 3);
    let paths: Vec<_> = hits.into_iter().map(|hit| hit.record.path).collect();

    assert_eq!(
        paths,
        vec![
            PathBuf::from("/tmp/multi/001.txt"),
            PathBuf::from("/tmp/multi/002.txt"),
            PathBuf::from("/tmp/multi/003.txt")
        ]
    );
}

#[test]
fn simple_multi_term_search_combines_name_and_content_scores() {
    let mut index = SearchIndex::new();
    let name_and_content = record(1, "/tmp/a/alpha-beta.md", "alpha-beta.md");
    let name_only = record(2, "/tmp/b/alpha-beta.md", "alpha-beta.md");
    index.insert(name_and_content.clone());
    index.insert(name_only);
    index.insert_content(name_and_content.id, "alpha beta body text");

    let hits = index.query("alpha beta", 2);

    assert_eq!(hits[0].record.id, name_and_content.id);
    assert_eq!(hits[0].reason, MatchReason::SubstringName);
    assert!(hits[0].score > hits[1].score);
}

#[test]
fn content_proximity_uses_rarest_posting_candidates_without_id_sets() {
    let mut index = SearchIndex::new();
    for node in 1..=64 {
        let item = record(
            node,
            &format!("/tmp/noisy-{node}.txt"),
            &format!("noisy-{node}.txt"),
        );
        index.insert(item.clone());
        index.insert_content(item.id, "alpha far filler filler filler");
    }

    let near = record(100, "/tmp/a/near.txt", "near.txt");
    let far = record(101, "/tmp/b/far.txt", "far.txt");
    index.insert(near.clone());
    index.insert(far.clone());
    index.insert_content(near.id, "alpha beta");
    index.insert_content(far.id, "alpha filler filler filler beta");

    let hits = index.query("near:1:alpha,beta", 10);
    let paths: Vec<_> = hits.into_iter().map(|hit| hit.record.path).collect();

    assert_eq!(paths, vec![PathBuf::from("/tmp/a/near.txt")]);
}

#[test]
fn cancelled_queries_stop_before_returning_hits() {
    let mut index = SearchIndex::new();
    index.insert(record(1, "/tmp/needle.txt", "needle.txt"));
    let cancellation = Cancellation::default();
    cancellation.cancel();

    let result = index.query_cancellable("needle", 10, &cancellation);

    assert!(matches!(result, Err(GfmError::Cancelled)));
}

#[test]
fn supersession_cancels_stale_query_tokens() {
    let supersession = SearchSupersession::new();
    let first = supersession.begin();
    assert!(first.check().is_ok());

    let second = supersession.begin();

    assert!(matches!(first.check(), Err(GfmError::Cancelled)));
    assert!(second.check().is_ok());
}

#[test]
fn superseding_query_runs_latest_search() {
    let mut index = SearchIndex::new();
    index.insert(record(1, "/tmp/report.md", "report.md"));
    let supersession = SearchSupersession::new();

    let hits = supersession.query(&index, "report", 10).unwrap();

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.name, "report.md");
}

#[test]
fn superseding_query_runs_latest_sharded_search() {
    let mut index = ShardedSearchIndex::new();
    index.insert(volume_record(1, 1, "/Volumes/A/report.md", "report.md"));
    index.insert(volume_record(2, 1, "/Volumes/B/notes.md", "notes.md"));
    let supersession = SearchSupersession::new();

    let stale = supersession.begin();
    let hits = supersession.query_sharded(&index, "report", 10).unwrap();

    assert!(matches!(stale.check(), Err(GfmError::Cancelled)));
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.path, PathBuf::from("/Volumes/A/report.md"));
}

#[test]
fn superseding_stream_runs_latest_sharded_search() {
    let mut index = ShardedSearchIndex::new();
    let hot = volume_record(1, 1, "/Volumes/A/needle.md", "needle.md");
    let deep = volume_record(2, 1, "/Volumes/B/body.md", "body.md");
    index.insert(hot);
    index.insert(deep.clone());
    index.insert_content(deep.id, "needle only appears in body content");
    let supersession = SearchSupersession::new();

    let stale = supersession.begin();
    let batches = supersession.stream_sharded(&index, "needle", 10).unwrap();

    assert!(matches!(stale.check(), Err(GfmError::Cancelled)));
    assert_eq!(batches.len(), 2);
    assert_eq!(batches[0].stage, SearchStreamStage::Hot);
    assert_eq!(batches[1].stage, SearchStreamStage::Deep);
}

#[test]
fn matches_content_phrases_by_token_position() {
    let mut index = SearchIndex::new();
    let keep = record(1, "/tmp/keep.txt", "keep.txt");
    let skip = record(2, "/tmp/skip.txt", "skip.txt");
    index.insert(keep.clone());
    index.insert(skip.clone());
    index.insert_content(keep.id, "an instant content search result");
    index.insert_content(skip.id, "instant search content result");

    let hits = index.query(r#""instant content search""#, 10);

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.name, "keep.txt");
    assert_eq!(hits[0].reason, MatchReason::Content);
}

#[test]
fn reindexed_content_replaces_stale_terms_and_positions() {
    let mut index = SearchIndex::new();
    let item = record(1, "/tmp/change.txt", "change.txt");
    index.insert(item);
    index.insert_content(FileId::new(VolumeId(1), 1), "oldtoken old phrase");
    assert_eq!(
        index.content_record_term_count(FileId::new(VolumeId(1), 1)),
        3
    );
    index.insert_content(FileId::new(VolumeId(1), 1), "newtoken new phrase");

    assert_eq!(
        index.content_record_term_count(FileId::new(VolumeId(1), 1)),
        3
    );
    assert!(index.query("oldtoken", 10).is_empty());
    assert!(index.query(r#""old phrase""#, 10).is_empty());
    assert_eq!(index.query("newtoken", 10).len(), 1);
    assert_eq!(index.query(r#""new phrase""#, 10).len(), 1);
}

#[test]
fn imported_content_positions_are_sorted_once_for_phrase_lookup() {
    let mut index = SearchIndex::new();
    let item = record(1, "/tmp/imported.txt", "imported.txt");
    index.insert(item);
    index.import_content_postings(&[
        ContentPosting {
            term: "alpha".to_string(),
            ids: vec![FileId::new(VolumeId(1), 1)],
            positions: vec![ContentPositions {
                id: FileId::new(VolumeId(1), 1),
                positions: vec![4, 0, 0],
            }],
        },
        ContentPosting {
            term: "beta".to_string(),
            ids: vec![FileId::new(VolumeId(1), 1)],
            positions: vec![ContentPositions {
                id: FileId::new(VolumeId(1), 1),
                positions: vec![5, 1, 1],
            }],
        },
    ]);

    assert_eq!(
        index.content_record_term_count(FileId::new(VolumeId(1), 1)),
        2
    );
    assert_eq!(index.query(r#""alpha beta""#, 10).len(), 1);
    index.remove_content(FileId::new(VolumeId(1), 1));
    assert!(index.query(r#""alpha beta""#, 10).is_empty());
    assert_eq!(
        index.content_record_term_count(FileId::new(VolumeId(1), 1)),
        0
    );
}

#[test]
fn content_phrase_uses_rarest_posting_candidates_without_record_scan() {
    let mut index = SearchIndex::new();
    for node in 1..=64 {
        let item = record(
            node,
            &format!("/tmp/noisy-phrase-{node}.txt"),
            &format!("noisy-phrase-{node}.txt"),
        );
        index.insert(item.clone());
        index.insert_content(item.id, "instant filler filler filler");
    }

    let keep = record(100, "/tmp/a/phrase.txt", "phrase.txt");
    let skip = record(101, "/tmp/b/reordered.txt", "reordered.txt");
    index.insert(keep.clone());
    index.insert(skip.clone());
    index.insert_content(keep.id, "instant content search");
    index.insert_content(skip.id, "instant search content");

    let hits = index.query(r#""instant content search""#, 10);
    let paths: Vec<_> = hits.into_iter().map(|hit| hit.record.path).collect();

    assert_eq!(paths, vec![PathBuf::from("/tmp/a/phrase.txt")]);
}

#[test]
fn content_phrase_anchors_on_sparse_term_positions() {
    let mut index = SearchIndex::new();
    let keep = record(1, "/tmp/sparse-phrase.txt", "sparse-phrase.txt");
    let skip = record(2, "/tmp/no-phrase.txt", "no-phrase.txt");
    index.insert(keep.clone());
    index.insert(skip.clone());
    index.import_content_postings(&[
        ContentPosting {
            term: "common".to_string(),
            ids: vec![keep.id, skip.id],
            positions: vec![
                ContentPositions {
                    id: keep.id,
                    positions: (0..2_000).collect(),
                },
                ContentPositions {
                    id: skip.id,
                    positions: (0..2_000).collect(),
                },
            ],
        },
        ContentPosting {
            term: "rare".to_string(),
            ids: vec![keep.id, skip.id],
            positions: vec![
                ContentPositions {
                    id: keep.id,
                    positions: vec![1_998],
                },
                ContentPositions {
                    id: skip.id,
                    positions: vec![2_500],
                },
            ],
        },
    ]);

    let hits = index.query(r#""common rare common""#, 10);

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.name, "sparse-phrase.txt");
}

#[test]
fn content_phrase_rejects_sparse_anchor_before_phrase_start() {
    let mut index = SearchIndex::new();
    let item = record(1, "/tmp/underflow.txt", "underflow.txt");
    index.insert(item.clone());
    index.import_content_postings(&[
        ContentPosting {
            term: "alpha".to_string(),
            ids: vec![item.id],
            positions: vec![ContentPositions {
                id: item.id,
                positions: vec![1],
            }],
        },
        ContentPosting {
            term: "beta".to_string(),
            ids: vec![item.id],
            positions: vec![ContentPositions {
                id: item.id,
                positions: vec![0],
            }],
        },
    ]);

    assert!(index.query(r#""alpha beta""#, 10).is_empty());
}

#[test]
fn supports_boolean_content_phrase_queries() {
    let mut index = SearchIndex::new();
    let first = record(1, "/tmp/first.txt", "first.txt");
    let second = record(2, "/tmp/second.txt", "second.txt");
    index.insert(first.clone());
    index.insert(second.clone());
    index.insert_content(first.id, "client alpha phrase");
    index.insert_content(second.id, "client beta phrase");

    let hits = index.query(r#""client alpha" OR "client beta""#, 10);

    assert_eq!(hits.len(), 2);
}

#[test]
fn matches_content_terms_within_proximity_window() {
    let mut index = SearchIndex::new();
    let keep = record(1, "/tmp/near.txt", "near.txt");
    let skip = record(2, "/tmp/far.txt", "far.txt");
    index.insert(keep.clone());
    index.insert(skip.clone());
    index.insert_content(keep.id, "alpha one two beta");
    index.insert_content(skip.id, "alpha one two three four five beta");

    let hits = index.query("near:3:alpha,beta", 10);

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.name, "near.txt");
    assert_eq!(hits[0].reason, MatchReason::Content);
}

#[test]
fn supports_boolean_content_proximity_queries() {
    let mut index = SearchIndex::new();
    let keep = record(1, "/tmp/near.txt", "near.txt");
    let skip = record(2, "/tmp/far.txt", "far.txt");
    index.insert(keep.clone());
    index.insert(skip.clone());
    index.insert_content(keep.id, "client alpha one beta");
    index.insert_content(skip.id, "client alpha one two three beta");

    let hits = index.query("client AND near:2:alpha,beta", 10);

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.name, "near.txt");
}

#[test]
fn content_proximity_anchors_on_sparse_term_positions() {
    let mut index = SearchIndex::new();
    let keep = record(1, "/tmp/rare-anchor.txt", "rare-anchor.txt");
    let skip = record(2, "/tmp/rare-far.txt", "rare-far.txt");
    index.insert(keep.clone());
    index.insert(skip.clone());
    index.import_content_postings(&[
        ContentPosting {
            term: "common".to_string(),
            ids: vec![keep.id, skip.id],
            positions: vec![
                ContentPositions {
                    id: keep.id,
                    positions: (0..1_000).collect(),
                },
                ContentPositions {
                    id: skip.id,
                    positions: (0..1_000).collect(),
                },
            ],
        },
        ContentPosting {
            term: "rare".to_string(),
            ids: vec![keep.id, skip.id],
            positions: vec![
                ContentPositions {
                    id: keep.id,
                    positions: vec![998],
                },
                ContentPositions {
                    id: skip.id,
                    positions: vec![2_000],
                },
            ],
        },
    ]);

    let hits = index.query("near:2:common,rare", 10);

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.name, "rare-anchor.txt");
}

#[test]
fn sorted_position_range_lookup_uses_distance_bounds() {
    let positions = [4, 9, 12, 20];

    assert!(sorted_has_position_within(&positions, 10, 2));
    assert!(sorted_has_position_within(&positions, 18, 2));
    assert!(!sorted_has_position_within(&positions, 15, 2));
    assert!(sorted_has_position_within(&positions, 0, 4));
}

#[test]
fn filters_by_kind_extension_path_and_size() {
    let mut index = SearchIndex::new();
    let mut keep = record(1, "/Users/me/Desktop/PLAN.md", "PLAN.md");
    keep.len = 16 * 1024;
    let mut wrong_ext = record(2, "/Users/me/Desktop/PLAN.pdf", "PLAN.pdf");
    wrong_ext.len = 16 * 1024;
    let mut too_small = record(3, "/Users/me/Desktop/tiny.md", "tiny.md");
    too_small.len = 12;
    let mut folder = record(4, "/Users/me/Desktop/Docs", "Docs");
    folder.kind = FileKind::Directory;
    index.insert(keep);
    index.insert(wrong_ext);
    index.insert(too_small);
    index.insert(folder);

    let hits = index.query("kind:file ext:md path:desktop size:>1kb", 10);

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.name, "PLAN.md");
}

#[test]
fn filters_by_modified_created_and_changed_dates() {
    let mut index = SearchIndex::new();
    let mut recent = record(1, "/tmp/recent.md", "recent.md");
    recent.modified = Some(test_time(2026, 8, 24));
    recent.created = Some(test_time(2026, 8, 1));
    recent.changed = Some(test_time(2026, 8, 24));
    let mut old = record(2, "/tmp/old.md", "old.md");
    old.modified = Some(test_time(2024, 1, 15));
    old.created = Some(test_time(2024, 1, 1));
    old.changed = Some(test_time(2024, 1, 15));
    index.insert(recent);
    index.insert(old);

    let modified = index.query("ext:md modified:>=2026-01-01", 10);
    let created = index.query("ext:md created:<2025-01-01", 10);
    let changed = index.query("ext:md changed:2026-08-24", 10);

    assert_eq!(modified.len(), 1);
    assert_eq!(modified[0].record.name, "recent.md");
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].record.name, "old.md");
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].record.name, "recent.md");
}

#[test]
fn supports_negative_date_filters() {
    let mut index = SearchIndex::new();
    let mut recent = record(1, "/tmp/recent.md", "recent.md");
    recent.modified = Some(test_time(2026, 8, 24));
    let mut old = record(2, "/tmp/old.md", "old.md");
    old.modified = Some(test_time(2024, 1, 15));
    index.insert(recent);
    index.insert(old);

    let hits = index.query("ext:md -modified:>=2026-01-01", 10);

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.name, "old.md");
}

#[test]
fn filters_without_terms_return_matching_records() {
    let mut index = SearchIndex::new();
    index.insert(record(1, "/tmp/notes.md", "notes.md"));
    index.insert(record(2, "/tmp/archive.pdf", "archive.pdf"));

    let hits = index.query("ext:md", 10);

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.name, "notes.md");
}

#[test]
fn excludes_terms_and_matches_quoted_path_phrases() {
    let mut index = SearchIndex::new();
    index.insert(record(
        1,
        "/Users/me/Desktop/Client Work/final notes.md",
        "final notes.md",
    ));
    index.insert(record(
        2,
        "/Users/me/Desktop/Client Work/draft notes.md",
        "draft notes.md",
    ));

    let hits = index.query(r#""Client Work" notes -draft"#, 10);

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.name, "final notes.md");
}

#[test]
fn supports_boolean_or_and_not_queries() {
    let mut index = SearchIndex::new();
    index.insert(record(1, "/tmp/report.md", "report.md"));
    index.insert(record(2, "/tmp/invoice.md", "invoice.md"));
    index.insert(record(3, "/tmp/draft-report.md", "draft-report.md"));
    index.insert(record(4, "/tmp/notes.md", "notes.md"));

    let hits = index.query("(report OR invoice) NOT draft", 10);
    let names: Vec<_> = hits.iter().map(|hit| hit.record.name.as_str()).collect();

    assert_eq!(names, vec!["invoice.md", "report.md"]);
}

#[test]
fn anchored_boolean_not_expression_does_not_request_universe_scan() {
    let query = SearchQuery::parse("client AND NOT draft");
    let expression = query.expression.as_ref().unwrap();

    assert!(!expression_needs_universe(expression));
    assert!(expression_has_positive_anchor(expression));
}

#[test]
fn unanchored_boolean_branches_still_request_universe_scan() {
    let negative = SearchQuery::parse("NOT draft");
    let filter_or_term = SearchQuery::parse("ext:md OR client");
    let filter_and_negative = SearchQuery::parse("ext:md AND NOT draft");

    assert!(expression_needs_universe(
        negative.expression.as_ref().unwrap()
    ));
    assert!(expression_needs_universe(
        filter_or_term.expression.as_ref().unwrap()
    ));
    assert!(expression_needs_universe(
        filter_and_negative.expression.as_ref().unwrap()
    ));
}

#[test]
fn indexed_filter_expression_candidates_bound_boolean_not_queries() {
    let mut index = SearchIndex::new();
    index.insert(record(1, "/tmp/report.md", "report.md"));
    index.insert(record(2, "/tmp/draft.md", "draft.md"));
    index.insert(record(3, "/tmp/report.pdf", "report.pdf"));
    let query = SearchQuery::parse("ext:md AND NOT draft");
    let expression = query.expression.as_ref().unwrap();

    let candidates = index
        .expression_candidate_ids(expression, SearchPass::Full)
        .unwrap();
    let hits = index.query_structured(&query, 10);
    let names: Vec<_> = hits.iter().map(|hit| hit.record.name.as_str()).collect();

    assert_eq!(candidates.len(), 2);
    assert_eq!(names, vec!["report.md"]);
}

#[test]
fn indexed_filter_expression_candidates_bound_boolean_or_queries() {
    let mut index = SearchIndex::new();
    let mut tagged = record(1, "/tmp/tagged.txt", "tagged.txt");
    tagged.tags = vec!["Important".to_string()];
    index.insert(tagged);
    index.insert(record(2, "/tmp/report.md", "report.md"));
    index.insert(record(3, "/tmp/client.pdf", "client.pdf"));
    let query = SearchQuery::parse("tag:important OR ext:md OR kind:symlink OR client");
    let expression = query.expression.as_ref().unwrap();

    let candidates = index
        .expression_candidate_ids(expression, SearchPass::Full)
        .unwrap();
    let hits = index.query_structured(&query, 10);
    let names: Vec<_> = hits.iter().map(|hit| hit.record.name.as_str()).collect();

    assert_eq!(candidates.len(), 3);
    assert_eq!(names, vec!["client.pdf", "report.md", "tagged.txt"]);
}

#[test]
fn exact_boolean_and_candidates_intersect_indexed_filters() {
    let mut index = SearchIndex::new();
    let mut tagged_report = record(1, "/tmp/report.md", "report.md");
    tagged_report.tags = vec!["Important".to_string()];
    let mut tagged_pdf = record(2, "/tmp/report.pdf", "report.pdf");
    tagged_pdf.tags = vec!["Important".to_string()];
    index.insert(tagged_report);
    index.insert(tagged_pdf);
    index.insert(record(3, "/tmp/notes.md", "notes.md"));
    let query = SearchQuery::parse("ext:md AND tag:important");
    let expression = query.expression.as_ref().unwrap();

    let candidates = index
        .expression_candidate_ids(expression, SearchPass::Full)
        .unwrap();
    let hits = index.query_structured(&query, 10);
    let names: Vec<_> = hits.iter().map(|hit| hit.record.name.as_str()).collect();

    assert_eq!(candidates.len(), 1);
    assert_eq!(names, vec!["report.md"]);
}

#[test]
fn exact_boolean_and_empty_intersection_skips_candidate_seed() {
    let mut index = SearchIndex::new();
    let mut tagged_pdf = record(1, "/tmp/report.pdf", "report.pdf");
    tagged_pdf.tags = vec!["Important".to_string()];
    index.insert(tagged_pdf);
    index.insert(record(2, "/tmp/notes.md", "notes.md"));
    let query = SearchQuery::parse("ext:md AND tag:important");
    let expression = query.expression.as_ref().unwrap();

    let candidates = index
        .expression_candidate_ids(expression, SearchPass::Full)
        .unwrap();
    let hits = index.query_structured(&query, 10);

    assert!(candidates.is_empty());
    assert!(hits.is_empty());
}

#[test]
fn candidate_set_intersection_uses_exact_smallest_shared_ids() {
    let large = [1_u64, 2, 3, 4, 5, 6]
        .into_iter()
        .map(|node| FileId::new(VolumeId(1), node))
        .collect();
    let small = [3_u64, 5]
        .into_iter()
        .map(|node| FileId::new(VolumeId(1), node))
        .collect();
    let medium = [2_u64, 3, 4, 5]
        .into_iter()
        .map(|node| FileId::new(VolumeId(1), node))
        .collect();

    let ids = intersect_candidate_sets([large, small, medium]).unwrap();

    assert_eq!(
        ids,
        [3_u64, 5]
            .into_iter()
            .map(|node| FileId::new(VolumeId(1), node))
            .collect()
    );
}

#[test]
fn nested_exact_boolean_and_candidates_remain_intersected() {
    let mut index = SearchIndex::new();
    let mut keep = record(1, "/tmp/report.md", "report.md");
    keep.tags = vec!["Important".to_string()];
    let mut wrong_extension = record(2, "/tmp/report.pdf", "report.pdf");
    wrong_extension.tags = vec!["Important".to_string()];
    let wrong_tag = record(3, "/tmp/report.md", "report.md");
    index.insert(keep);
    index.insert(wrong_extension);
    index.insert(wrong_tag);
    let query = SearchQuery::parse("(ext:md AND tag:important) AND name:report");
    let expression = query.expression.as_ref().unwrap();

    let candidates = index
        .expression_candidate_ids(expression, SearchPass::Full)
        .unwrap();
    let hits = index.query_structured(&query, 10);

    assert_eq!(
        candidates,
        [FileId::new(VolumeId(1), 1)].into_iter().collect()
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.id, FileId::new(VolumeId(1), 1));
}

#[test]
fn exact_boolean_candidates_do_not_treat_term_branches_as_exact() {
    let index = SearchIndex::new();
    let query = SearchQuery::parse("(client AND ext:md) OR tag:important");
    let expression = query.expression.as_ref().unwrap();

    assert!(index
        .exact_expression_candidate_ids(expression, SearchPass::Full)
        .is_none());
}

#[test]
fn indexed_filter_only_query_with_no_candidates_skips_universe_seed() {
    let mut index = SearchIndex::new();
    index.insert(record(1, "/tmp/report.md", "report.md"));
    index.insert(record(2, "/tmp/invoice.pdf", "invoice.pdf"));
    let query = SearchQuery::parse("ext:zip");
    let expression = query.expression.as_ref().unwrap();

    let candidates = index
        .expression_candidate_ids(expression, SearchPass::Full)
        .unwrap();
    let hits = index.query_structured(&query, 10);

    assert!(candidates.is_empty());
    assert!(hits.is_empty());
}

#[test]
fn unsupported_filter_only_query_still_uses_universe_seed() {
    let mut index = SearchIndex::new();
    index.insert(record(1, "/tmp/report.md", "report.md"));
    index.insert(record(2, "/tmp/invoice.pdf", "invoice.pdf"));
    let query = SearchQuery::parse("scope:/tmp");
    let expression = query.expression.as_ref().unwrap();

    assert!(index
        .expression_candidate_ids(expression, SearchPass::Full)
        .is_none());
    assert_eq!(index.query_structured(&query, 10).len(), 2);
}

#[test]
fn supports_boolean_or_between_filters() {
    let mut index = SearchIndex::new();
    index.insert(record(1, "/tmp/report.md", "report.md"));
    index.insert(record(2, "/tmp/invoice.pdf", "invoice.pdf"));
    index.insert(record(3, "/tmp/image.png", "image.png"));

    let hits = index.query("ext:md OR ext:pdf", 10);
    let names: Vec<_> = hits.iter().map(|hit| hit.record.name.as_str()).collect();

    assert_eq!(names, vec!["invoice.pdf", "report.md"]);
}

#[test]
fn searches_and_filters_finder_tags() {
    let mut index = SearchIndex::new();
    let mut keep = record(1, "/tmp/report.md", "report.md");
    keep.tags = vec!["Important".to_string(), "Client".to_string()];
    let mut skip = record(2, "/tmp/draft.md", "draft.md");
    skip.tags = vec!["Later".to_string()];
    index.insert(keep);
    index.insert(skip);

    let filtered = index.query("tag:important", 10);
    let plain = index.query("client", 10);

    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].record.name, "report.md");
    assert_eq!(plain.len(), 1);
    assert_eq!(plain[0].reason, MatchReason::Tag);
}

#[test]
fn removes_reindexed_tag_postings() {
    let mut index = SearchIndex::new();
    let mut item = record(1, "/tmp/report.md", "report.md");
    item.tags = vec!["Important".to_string()];
    index.insert(item.clone());
    item.tags = vec!["Later".to_string()];
    index.insert(item);

    assert!(index.query("tag:important", 10).is_empty());
    assert_eq!(index.query("tag:later", 10).len(), 1);
}

#[test]
fn searches_and_reindexes_finder_comments() {
    let mut index = SearchIndex::new();
    let mut item = record(1, "/tmp/report.md", "report.md");
    item.finder_comment = Some("client handoff notes".to_string());
    index.insert(item.clone());

    let hits = index.query("handoff", 10);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.name, "report.md");

    item.finder_comment = Some("archived later".to_string());
    index.insert(item);

    assert!(index.query("handoff", 10).is_empty());
    assert_eq!(index.query("archived", 10).len(), 1);
}

#[test]
fn filters_named_and_absolute_scopes() {
    let mut index = SearchIndex::new();
    index.insert(record(1, "/Users/me/Desktop/report.md", "report.md"));
    index.insert(record(2, "/Users/me/Downloads/report.md", "report.md"));
    index.insert(record(3, "/Users/me/Documents/report.md", "report.md"));

    let desktop = index.query("report @desktop", 10);
    let downloads = index.query("report scope:downloads", 10);
    let subtree = index.query("report scope:/Users/me/Documents", 10);

    assert_eq!(desktop.len(), 1);
    assert!(desktop[0].record.path.ends_with("Desktop/report.md"));
    assert_eq!(downloads.len(), 1);
    assert!(downloads[0].record.path.ends_with("Downloads/report.md"));
    assert_eq!(subtree.len(), 1);
    assert!(subtree[0].record.path.ends_with("Documents/report.md"));
}

#[test]
fn supports_negative_scope_filters() {
    let mut index = SearchIndex::new();
    index.insert(record(1, "/Users/me/Desktop/report.md", "report.md"));
    index.insert(record(2, "/Users/me/Downloads/report.md", "report.md"));

    let hits = index.query("report -@desktop", 10);

    assert_eq!(hits.len(), 1);
    assert!(hits[0].record.path.ends_with("Downloads/report.md"));
}

#[test]
fn intent_ranking_surfaces_applications_for_app_queries() {
    let mut index = SearchIndex::new();
    let mut app = record(1, "/Applications/Notes.app", "Notes.app");
    app.kind = FileKind::Directory;
    index.insert(app);
    index.insert(record(
        2,
        "/Users/me/Documents/app-notes.txt",
        "app-notes.txt",
    ));

    let hits = index.query("app", 10);

    assert_eq!(
        hits[0].record.path,
        PathBuf::from("/Applications/Notes.app")
    );
}

#[test]
fn intent_ranking_prefers_requested_downloads_and_desktop_locations() {
    let mut index = SearchIndex::new();
    index.insert(record(1, "/Users/me/Documents/report.pdf", "report.pdf"));
    index.insert(record(2, "/Users/me/Downloads/report.pdf", "report.pdf"));
    index.insert(record(3, "/Users/me/Desktop/report.pdf", "report.pdf"));

    let downloads = index.query("download report", 10);
    let desktop = index.query("desktop report", 10);

    assert_eq!(
        downloads[0].record.path,
        PathBuf::from("/Users/me/Downloads/report.pdf")
    );
    assert_eq!(
        desktop[0].record.path,
        PathBuf::from("/Users/me/Desktop/report.pdf")
    );
}

#[test]
fn intent_ranking_surfaces_screenshots() {
    let mut index = SearchIndex::new();
    index.insert(record(
        1,
        "/Users/me/Desktop/Screenshot 2026-08-24 at 6.59.43 PM.png",
        "Screenshot 2026-08-24 at 6.59.43 PM.png",
    ));
    index.insert(record(
        2,
        "/Users/me/Desktop/screen-notes.md",
        "screen-notes.md",
    ));

    let hits = index.query("screen shot", 10);

    assert_eq!(
        hits[0].record.name,
        "Screenshot 2026-08-24 at 6.59.43 PM.png"
    );
}

#[test]
fn intent_ranking_surfaces_project_folders() {
    let mut index = SearchIndex::new();
    let mut project = record(1, "/Users/me/work/gfm", "gfm");
    project.kind = FileKind::Directory;
    index.insert(project);
    index.insert(record(2, "/Users/me/Documents/gfm.txt", "gfm.txt"));

    let hits = index.query("project gfm", 10);

    assert_eq!(hits[0].record.path, PathBuf::from("/Users/me/work/gfm"));
}

#[test]
fn intent_ranking_surfaces_recently_touched_files() {
    let mut index = SearchIndex::new();
    let mut recent = record(1, "/Users/me/Documents/report.md", "report.md");
    recent.modified = Some(std::time::SystemTime::now());
    let mut old = record(2, "/Users/me/Desktop/recent-report.md", "recent-report.md");
    old.modified = Some(UNIX_EPOCH);
    index.insert(old);
    index.insert(recent);

    let hits = index.query("recent report", 10);

    assert_eq!(
        hits[0].record.path,
        PathBuf::from("/Users/me/Documents/report.md")
    );
}

#[test]
fn ranking_boosts_user_pinned_relevant_results() {
    let mut index = SearchIndex::new();
    let first = record(1, "/tmp/a/report.md", "report.md");
    let second = record(2, "/tmp/b/report.md", "report.md");
    let second_id = second.id;
    index.insert(first);
    index.insert(second);

    assert!(index.pin(second_id));
    let hits = index.query("report", 10);

    assert_eq!(hits[0].record.id, second_id);
    assert!(index.is_pinned(second_id));
    assert!(index.unpin(second_id));
}

#[test]
fn ranking_composes_capped_term_frequency() {
    let mut index = SearchIndex::new();
    index.insert(record(1, "/tmp/alpha.md", "alpha.md"));
    index.insert(record(
        2,
        "/tmp/alpha-alpha-alpha.md",
        "alpha-alpha-alpha.md",
    ));

    let hits = index.query("alpha", 10);

    assert_eq!(hits[0].record.name, "alpha-alpha-alpha.md");
    assert!(hits[0].score > hits[1].score);
}

#[test]
fn ranking_scores_kind_filter_matches() {
    let mut index = SearchIndex::new();
    let mut folder = record(1, "/tmp/report", "report");
    folder.kind = FileKind::Directory;
    index.insert(folder);

    let hits = index.query("report kind:directory", 10);

    assert_eq!(hits.len(), 1);
    assert!(hits[0].score >= 90);
}

#[test]
fn ranking_keeps_strongest_primary_reason() {
    let mut index = SearchIndex::new();
    let mut item = record(1, "/tmp/client", "client");
    item.tags = vec!["client".to_string()];
    index.insert(item);

    let hits = index.query("client", 10);

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].reason, MatchReason::ExactName);
}

#[test]
fn stream_returns_hot_results_before_deep_content_results() {
    let mut index = SearchIndex::new();
    let hot = record(1, "/tmp/needle.md", "needle.md");
    let deep = record(2, "/tmp/deep.md", "deep.md");
    index.insert(hot);
    index.insert(deep.clone());
    index.insert_content(deep.id, "needle exists only in content");

    let batches = index.stream("needle", 10).unwrap();

    assert_eq!(batches.len(), 2);
    assert_eq!(batches[0].stage, SearchStreamStage::Hot);
    assert_eq!(batches[0].hits[0].record.name, "needle.md");
    assert_eq!(batches[1].stage, SearchStreamStage::Deep);
    assert_eq!(batches[1].hits[0].record.name, "deep.md");
    assert_eq!(batches[1].hits[0].reason, MatchReason::Content);
}

#[test]
fn stream_omits_deep_batch_when_full_results_do_not_change() {
    let mut index = SearchIndex::new();
    index.insert(record(1, "/tmp/report.md", "report.md"));

    let batches = index.stream("report", 10).unwrap();

    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].stage, SearchStreamStage::Hot);
}

#[test]
fn stream_honors_cancellation() {
    let mut index = SearchIndex::new();
    index.insert(record(1, "/tmp/report.md", "report.md"));
    let cancellation = Cancellation::default();
    cancellation.cancel();

    let result = index.stream_cancellable("report", 10, &cancellation);

    assert!(matches!(result, Err(GfmError::Cancelled)));
}

#[test]
fn fuzzy_term_candidates_survive_structured_expression_filtering() {
    let mut index = SearchIndex::new();
    index.insert(record(1, "/tmp/needl", "needl"));

    let hits = index.query("needle", 10);

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].reason, MatchReason::FuzzyName);
}

#[test]
fn fuzzy_retrieval_uses_indexed_name_tokens() {
    let mut index = SearchIndex::new();
    index.insert(record(1, "/tmp/quartely-plan.md", "quartely-plan.md"));

    let hits = index.query("quarterly", 10);

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.name, "quartely-plan.md");
    assert_eq!(hits[0].reason, MatchReason::FuzzyName);
}

#[test]
fn sidecar_fuzzy_lookup_budget_caps_keys_terms_and_verified_candidates() {
    let mut index = SearchIndex::new();
    for (id, name) in [(1, "needl.md"), (2, "needle.md")] {
        let item = record(id, &format!("/tmp/{name}"), name);
        let columns = SearchRecordColumns {
            id: item.id,
            name: item.name.clone(),
            path: item.path.to_string_lossy().into_owned(),
            extension: item.extension().map(ToOwned::to_owned),
            tags: item.tags.clone(),
            comment: item.finder_comment.clone(),
        };
        assert!(index.insert_with_columns_deferred_fuzzy(item, columns));
    }
    let lookup = StaticLookup {
        prefix_ids: Vec::new(),
        substring_ids: Vec::new(),
        fuzzy_terms: vec![
            "needl".to_string(),
            "needle".to_string(),
            "neatly".to_string(),
        ],
    };

    let report = index
        .query_structured_with_lookup_budget_cancellable(
            &SearchQuery::parse("needle"),
            10,
            &lookup,
            SearchLookupBudget {
                max_fuzzy_keys_per_term: 1,
                max_fuzzy_terms_per_key: 1,
                max_fuzzy_candidates_per_term: 1,
                ..SearchLookupBudget::default()
            },
            &Cancellation::default(),
        )
        .unwrap();

    assert_eq!(report.lookup.fuzzy_terms, 1);
    assert_eq!(report.lookup.fuzzy_keys, 1);
    assert_eq!(report.lookup.fuzzy_key_truncated_terms, 1);
    assert_eq!(report.lookup.fuzzy_term_truncated_keys, 1);
    assert_eq!(report.lookup.fuzzy_candidate_truncated_terms, 0);
    assert_eq!(report.lookup.fuzzy_verified_candidates, 1);
}

#[test]
fn hot_fuzzy_lookup_budget_caps_local_candidates_before_archive_lookup() {
    let mut index = SearchIndex::new();
    for (id, name) in [(1, "needl.md"), (2, "needle.md")] {
        let item = record(id, &format!("/tmp/{name}"), name);
        let columns = SearchRecordColumns {
            id: item.id,
            name: item.name.clone(),
            path: item.path.to_string_lossy().into_owned(),
            extension: item.extension().map(ToOwned::to_owned),
            tags: item.tags.clone(),
            comment: item.finder_comment.clone(),
        };
        assert!(index.insert_with_columns_deferred_fuzzy(item, columns));
    }
    let first_key = SearchQuery::parse("needle")
        .fuzzy_candidate_keys()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(
        index.import_fuzzy_postings(&[SearchFuzzyPosting {
            key: first_key,
            terms: vec!["needl".to_string(), "needle".to_string()],
        }]),
        1
    );
    let lookup = StaticLookup {
        prefix_ids: Vec::new(),
        substring_ids: Vec::new(),
        fuzzy_terms: vec!["neatly".to_string()],
    };

    let report = index
        .query_structured_with_lookup_budget_cancellable(
            &SearchQuery::parse("needle"),
            10,
            &lookup,
            SearchLookupBudget {
                max_fuzzy_keys_per_term: 1,
                max_fuzzy_terms_per_key: 1,
                max_fuzzy_candidates_per_term: 1,
                ..SearchLookupBudget::default()
            },
            &Cancellation::default(),
        )
        .unwrap();

    assert_eq!(report.lookup.fuzzy_terms, 1);
    assert_eq!(report.lookup.fuzzy_keys, 1);
    assert_eq!(report.lookup.fuzzy_lookup_terms, 0);
    assert_eq!(report.lookup.fuzzy_candidate_terms, 1);
    assert_eq!(report.lookup.fuzzy_candidate_truncated_terms, 1);
    assert_eq!(report.lookup.fuzzy_verified_candidates, 1);
}

#[test]
fn fuzzy_lookup_skips_numeric_only_and_digit_run_terms() {
    let mut index = SearchIndex::new();
    index.insert(record(
        1,
        "/tmp/project-PackageProject00012345.md",
        "project-PackageProject00012345.md",
    ));
    let lookup = StaticLookup {
        prefix_ids: Vec::new(),
        substring_ids: Vec::new(),
        fuzzy_terms: vec!["packageproject00012345".to_string()],
    };

    let report = index
        .query_structured_with_lookup_budget_cancellable(
            &SearchQuery::parse("PackageProject00012346"),
            10,
            &lookup,
            SearchLookupBudget::default(),
            &Cancellation::default(),
        )
        .unwrap();

    assert_eq!(report.lookup.fuzzy_terms, 0);
    assert_eq!(report.lookup.fuzzy_keys, 0);
    assert_eq!(report.lookup.fuzzy_lookup_terms, 0);
}

#[test]
fn removes_reindexed_fuzzy_postings() {
    let mut index = SearchIndex::new();
    let mut item = record(1, "/tmp/needl", "needl");
    index.insert(item.clone());
    item.path = PathBuf::from("/tmp/unrelated");
    item.name = "unrelated".to_string();
    index.insert(item);

    assert!(index.query("needle", 10).is_empty());
}

#[test]
fn fuzzy_index_ignores_unbounded_long_terms() {
    let mut index = SearchIndex::new();
    let long_name = format!("{}{}", "a".repeat(96), ".md");
    index.insert(record(1, &format!("/tmp/{long_name}"), &long_name));

    let hits = index.query(&"b".repeat(95), 10);

    assert!(hits.is_empty());
}

#[test]
fn sharded_search_merges_volume_results_deterministically() {
    let mut index = ShardedSearchIndex::new();
    index.insert(volume_record(2, 2, "/Volumes/B/report.md", "report.md"));
    index.insert(volume_record(1, 1, "/Volumes/A/report.md", "report.md"));

    let hits = index.query("report", 10);
    let paths: Vec<_> = hits
        .iter()
        .map(|hit| hit.record.path.to_string_lossy().into_owned())
        .collect();

    assert_eq!(index.shard_count(), 2);
    assert_eq!(paths, vec!["/Volumes/A/report.md", "/Volumes/B/report.md"]);
}

#[test]
fn single_shard_search_dispatches_directly_with_query_report() {
    let mut index = ShardedSearchIndex::new();
    let record = volume_record(1, 1, "/Volumes/A/report.md", "report.md");
    index.insert(record.clone());

    let report = index
        .query_structured_with_lookup_budget_cancellable(
            &SearchQuery::parse("report"),
            10,
            &StaticLookup {
                prefix_ids: Vec::new(),
                substring_ids: Vec::new(),
                fuzzy_terms: Vec::new(),
            },
            SearchLookupBudget::default(),
            &Cancellation::default(),
        )
        .unwrap();

    assert_eq!(index.shard_count(), 1);
    assert_eq!(report.hits.len(), 1);
    assert_eq!(report.hits[0].record.path, record.path);
    assert_eq!(report.hits[0].reason, MatchReason::PrefixName);
}

#[test]
fn sharded_prefix_sidecar_import_partitions_ids_by_volume() {
    let mut index = ShardedSearchIndex::new();
    index.insert_with_columns_deferred_sidecars(
        volume_record(1, 1, "/Volumes/A/original.md", "original.md"),
        SearchRecordColumns {
            id: FileId::new(VolumeId(1), 1),
            name: "project-alpha.md".to_string(),
            path: "/Volumes/A/project-alpha.md".to_string(),
            extension: Some("md".to_string()),
            tags: Vec::new(),
            comment: None,
        },
    );
    index.insert_with_columns_deferred_sidecars(
        volume_record(2, 2, "/Volumes/B/original.md", "original.md"),
        SearchRecordColumns {
            id: FileId::new(VolumeId(2), 2),
            name: "project-beta.md".to_string(),
            path: "/Volumes/B/project-beta.md".to_string(),
            extension: Some("md".to_string()),
            tags: Vec::new(),
            comment: None,
        },
    );

    assert!(
        index.import_prefix_postings(&[SearchPrefixPosting {
            prefix: "proj".to_string(),
            ids: vec![
                FileId::new(VolumeId(1), 1),
                FileId::new(VolumeId(2), 2),
                FileId::new(VolumeId(9), 9),
            ],
        }]) >= 2
    );
    let paths = index
        .query("proj", 10)
        .into_iter()
        .filter(|hit| hit.reason == MatchReason::PrefixName)
        .map(|hit| hit.record.path)
        .collect::<Vec<_>>();

    assert_eq!(
        paths,
        vec![
            PathBuf::from("/Volumes/A/original.md"),
            PathBuf::from("/Volumes/B/original.md")
        ]
    );
}

#[test]
fn sharded_metadata_sidecar_import_partitions_ids_by_volume() {
    let mut index = ShardedSearchIndex::new();
    index.insert_with_columns_deferred_sidecars(
        volume_record(1, 1, "/Volumes/A/original.md", "original.md"),
        SearchRecordColumns {
            id: FileId::new(VolumeId(1), 1),
            name: "alpha.md".to_string(),
            path: "/Volumes/A/alpha.md".to_string(),
            extension: Some("md".to_string()),
            tags: vec!["Important".to_string()],
            comment: None,
        },
    );
    index.insert_with_columns_deferred_sidecars(
        volume_record(2, 2, "/Volumes/B/original.md", "original.md"),
        SearchRecordColumns {
            id: FileId::new(VolumeId(2), 2),
            name: "beta.md".to_string(),
            path: "/Volumes/B/beta.md".to_string(),
            extension: Some("md".to_string()),
            tags: vec!["Important".to_string()],
            comment: None,
        },
    );

    assert!(
        index.import_metadata_postings(&[SearchMetadataPosting {
            field: SearchMetadataField::Tag,
            term: "Important".to_string(),
            ids: vec![
                FileId::new(VolumeId(1), 1),
                FileId::new(VolumeId(2), 2),
                FileId::new(VolumeId(9), 9),
            ],
        }]) >= 2
    );
    let paths = index
        .query("tag:Important", 10)
        .into_iter()
        .map(|hit| hit.record.path)
        .collect::<Vec<_>>();

    assert_eq!(
        paths,
        vec![
            PathBuf::from("/Volumes/A/original.md"),
            PathBuf::from("/Volumes/B/original.md")
        ]
    );
}

#[test]
fn sharded_content_sidecar_import_partitions_ids_and_positions_by_volume() {
    let mut index = ShardedSearchIndex::new();
    index.insert(volume_record(1, 1, "/Volumes/A/alpha.md", "alpha.md"));
    index.insert(volume_record(2, 2, "/Volumes/B/beta.md", "beta.md"));

    index.import_content_postings(&[ContentPosting {
        term: "bodymarker".to_string(),
        ids: vec![
            FileId::new(VolumeId(1), 1),
            FileId::new(VolumeId(2), 2),
            FileId::new(VolumeId(9), 9),
        ],
        positions: vec![
            ContentPositions {
                id: FileId::new(VolumeId(1), 1),
                positions: vec![0],
            },
            ContentPositions {
                id: FileId::new(VolumeId(2), 2),
                positions: vec![0],
            },
            ContentPositions {
                id: FileId::new(VolumeId(9), 9),
                positions: vec![0],
            },
        ],
    }]);
    let paths = index
        .query("bodymarker", 10)
        .into_iter()
        .map(|hit| hit.record.path)
        .collect::<Vec<_>>();

    assert_eq!(
        paths,
        vec![
            PathBuf::from("/Volumes/A/alpha.md"),
            PathBuf::from("/Volumes/B/beta.md")
        ]
    );
}

#[test]
fn sharded_search_truncates_after_global_merge() {
    let mut index = ShardedSearchIndex::new();
    let mut exact = volume_record(2, 2, "/Volumes/B/report.md", "report.md");
    exact.modified = Some(std::time::SystemTime::now());
    index.insert(volume_record(
        1,
        1,
        "/Volumes/A/archive-report.md",
        "archive-report.md",
    ));
    index.insert(exact);

    let hits = index.query("report", 1);

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.path, PathBuf::from("/Volumes/B/report.md"));
}

#[test]
fn sharded_search_uses_bounded_global_top_hits() {
    let mut index = ShardedSearchIndex::new();
    for volume in 1..=4 {
        for node in 1..=12 {
            let name = format!("archive-report-{volume}-{node}.md");
            let path = format!("/Volumes/{volume}/{name}");
            index.insert(volume_record(volume, node, &path, &name));
        }
    }
    index.insert(volume_record(4, 99, "/Volumes/4/report", "report"));
    index.insert(volume_record(3, 99, "/Volumes/3/report", "report"));

    let hits = index.query("report", 2);
    let paths: Vec<_> = hits.into_iter().map(|hit| hit.record.path).collect();

    assert_eq!(
        paths,
        vec![
            PathBuf::from("/Volumes/3/report"),
            PathBuf::from("/Volumes/4/report")
        ]
    );
}

#[test]
fn search_index_uses_bounded_top_hits_for_single_shard() {
    let mut index = SearchIndex::new();
    for node in 1..=48 {
        let name = format!("archive-report-{node}.md");
        let path = format!("/tmp/{name}");
        index.insert(record(node, &path, &name));
    }
    index.insert(record(98, "/tmp/b/report", "report"));
    index.insert(record(99, "/tmp/a/report", "report"));

    let hits = index.query("report", 2);
    let paths: Vec<_> = hits.into_iter().map(|hit| hit.record.path).collect();

    assert_eq!(
        paths,
        vec![
            PathBuf::from("/tmp/a/report"),
            PathBuf::from("/tmp/b/report")
        ]
    );
}

#[test]
fn simple_single_term_stream_keeps_name_hits_hot_and_content_hits_deep() {
    let mut index = SearchIndex::new();
    let hot = record(1, "/tmp/report.md", "report.md");
    let deep = record(2, "/tmp/body.md", "body.md");
    index.insert(hot.clone());
    index.insert(deep.clone());
    index.insert_content(deep.id, "report appears only in content");

    let batches = index.stream("report", 10).unwrap();

    assert_eq!(batches.len(), 2);
    assert_eq!(batches[0].stage, SearchStreamStage::Hot);
    assert_eq!(batches[0].hits.len(), 1);
    assert_eq!(batches[0].hits[0].record.path, hot.path);
    assert_eq!(batches[1].stage, SearchStreamStage::Deep);
    assert_eq!(batches[1].hits.len(), 1);
    assert_eq!(batches[1].hits[0].record.path, deep.path);
}

#[test]
fn simple_multi_term_stream_keeps_name_hits_hot_and_content_hits_deep() {
    let mut index = SearchIndex::new();
    let hot = record(1, "/tmp/alpha-beta.md", "alpha-beta.md");
    let deep = record(2, "/tmp/body.md", "body.md");
    index.insert(hot.clone());
    index.insert(deep.clone());
    index.insert_content(deep.id, "alpha beta appears only in content");

    let batches = index.stream("alpha beta", 10).unwrap();

    assert_eq!(batches.len(), 2);
    assert_eq!(batches[0].stage, SearchStreamStage::Hot);
    assert_eq!(batches[0].hits.len(), 1);
    assert_eq!(batches[0].hits[0].record.path, hot.path);
    assert_eq!(batches[1].stage, SearchStreamStage::Deep);
    assert_eq!(batches[1].hits.len(), 1);
    assert_eq!(batches[1].hits[0].record.path, deep.path);
}

#[test]
fn hit_sorting_caches_keys_without_changing_tie_break_order() {
    let mut hits = vec![
        SearchHit {
            record: record(3, "/tmp/b/report.md", "Report.md"),
            score: 100,
            reason: MatchReason::PrefixName,
            snippet: None,
        },
        SearchHit {
            record: record(2, "/tmp/a/report.md", "report.md"),
            score: 100,
            reason: MatchReason::PrefixName,
            snippet: None,
        },
        SearchHit {
            record: record(1, "/tmp/z/archive.md", "archive.md"),
            score: 200,
            reason: MatchReason::ExactName,
            snippet: None,
        },
    ];

    sort_hits(&mut hits);
    let paths: Vec<_> = hits.into_iter().map(|hit| hit.record.path).collect();

    assert_eq!(
        paths,
        vec![
            PathBuf::from("/tmp/z/archive.md"),
            PathBuf::from("/tmp/a/report.md"),
            PathBuf::from("/tmp/b/report.md"),
        ]
    );
}

#[test]
fn sharded_search_removes_records_by_volume_and_path() {
    let mut index = ShardedSearchIndex::new();
    let first = volume_record(1, 1, "/Volumes/A/report.md", "report.md");
    let second = volume_record(2, 1, "/Volumes/B/report.md", "report.md");
    index.insert(first.clone());
    index.insert(second.clone());

    assert_eq!(index.remove(first.id).unwrap().path, first.path);
    assert_eq!(index.remove_path(&second.path).unwrap().path, second.path);

    assert!(index.query("report", 10).is_empty());
    assert!(index.is_empty());
}

#[test]
fn sharded_search_honors_cancellation() {
    let mut index = ShardedSearchIndex::new();
    index.insert(record(1, "/tmp/report.md", "report.md"));
    let cancellation = Cancellation::default();
    cancellation.cancel();

    let result = index.query_cancellable("report", 10, &cancellation);

    assert!(matches!(result, Err(GfmError::Cancelled)));
}

#[test]
fn sharded_stream_merges_stages_across_volumes() {
    let mut index = ShardedSearchIndex::new();
    let hot = volume_record(1, 1, "/Volumes/A/needle.md", "needle.md");
    let deep = volume_record(2, 1, "/Volumes/B/deep.md", "deep.md");
    index.insert(hot);
    index.insert(deep.clone());
    index.insert_content(deep.id, "needle exists only in volume b content");

    let batches = index.stream("needle", 10).unwrap();

    assert_eq!(batches.len(), 2);
    assert_eq!(batches[0].stage, SearchStreamStage::Hot);
    assert_eq!(
        batches[0].hits[0].record.path,
        PathBuf::from("/Volumes/A/needle.md")
    );
    assert_eq!(batches[1].stage, SearchStreamStage::Deep);
    assert_eq!(
        batches[1].hits[0].record.path,
        PathBuf::from("/Volumes/B/deep.md")
    );
}

#[test]
fn single_shard_stream_dispatches_directly_with_hot_and_deep_batches() {
    let mut index = ShardedSearchIndex::new();
    let hot = volume_record(1, 1, "/Volumes/A/needle.md", "needle.md");
    let deep = volume_record(1, 2, "/Volumes/A/deep.md", "deep.md");
    index.insert(hot.clone());
    index.insert(deep.clone());
    index.insert_content(deep.id, "needle exists in content");

    let batches = index.stream("needle", 10).unwrap();

    assert_eq!(index.shard_count(), 1);
    assert_eq!(batches.len(), 2);
    assert_eq!(batches[0].stage, SearchStreamStage::Hot);
    assert_eq!(batches[0].hits[0].record.path, hot.path);
    assert_eq!(batches[1].stage, SearchStreamStage::Deep);
    assert_eq!(batches[1].hits[0].record.path, deep.path);
}

#[test]
fn sharded_stream_bounds_stage_merges_across_many_volumes() {
    let mut index = ShardedSearchIndex::new();
    for volume in 1..=24 {
        let hot = volume_record(
            volume,
            1,
            &format!("/Volumes/{volume:02}/needle.md"),
            "needle.md",
        );
        let deep = volume_record(
            volume,
            2,
            &format!("/Volumes/{volume:02}/deep-{volume:02}.md"),
            &format!("deep-{volume:02}.md"),
        );
        index.insert(hot);
        index.insert(deep.clone());
        index.insert_content(deep.id, "needle exists only in content");
    }

    let batches = index.stream("needle", 5).unwrap();

    assert_eq!(batches.len(), 2);
    assert_eq!(batches[0].stage, SearchStreamStage::Hot);
    assert_eq!(batches[0].hits.len(), 5);
    assert_eq!(batches[1].stage, SearchStreamStage::Deep);
    assert_eq!(batches[1].hits.len(), 5);
    assert_eq!(
        batches[0].hits[0].record.path,
        PathBuf::from("/Volumes/01/needle.md")
    );
    assert_eq!(
        batches[1].hits[0].record.path,
        PathBuf::from("/Volumes/01/deep-01.md")
    );
}

fn record(node: u64, path: &str, name: &str) -> FileRecord {
    volume_record(1, node, path, name)
}

fn volume_record(volume: u64, node: u64, path: &str, name: &str) -> FileRecord {
    FileRecord {
        id: FileId::new(VolumeId(volume), node),
        parent: None,
        path: PathBuf::from(path),
        name: name.to_string(),
        kind: FileKind::File,
        len: 0,
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
    }
}

struct StaticLookup {
    prefix_ids: Vec<FileId>,
    substring_ids: Vec<FileId>,
    fuzzy_terms: Vec<String>,
}

impl SearchLookup for StaticLookup {
    fn prefix_ids(&self, _prefix: &str) -> gfm_types::Result<Vec<FileId>> {
        Ok(self.prefix_ids.clone())
    }

    fn substring_ids(&self, _gram: &str) -> gfm_types::Result<Vec<FileId>> {
        Ok(self.substring_ids.clone())
    }

    fn fuzzy_terms(&self, _key: &str) -> gfm_types::Result<Vec<String>> {
        Ok(self.fuzzy_terms.clone())
    }
}

fn test_time(year: i32, month: u32, day: u32) -> std::time::SystemTime {
    UNIX_EPOCH + Duration::from_secs(test_days_from_civil(year, month, day) as u64 * 86_400)
}

fn test_days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let (year, month) = if month <= 2 {
        (year - 1, month + 12)
    } else {
        (year, month)
    };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month as i32 - 3) + 2) / 5 + day as i32 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era as i64 * 146_097 + day_of_era as i64 - 719_468
}
