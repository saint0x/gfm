use super::*;
use gfm_types::{FileId, FileKind, FileRecord, GfmError, VolumeId};
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
        created: None,
        modified: None,
        changed: None,
        hidden: false,
        tags: Vec::new(),
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
