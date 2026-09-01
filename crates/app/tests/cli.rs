use flate2::{write::GzEncoder, Compression};
use gfm_content::extractor_version_for_path;
use gfm_store::{
    content_manifest_promotion_journal_path, read_records, write_content_postings,
    ContentArchiveManifest, ContentArchiveManifestEntry, ContentManifestPromotionJournal,
    ContentMergeTier, MetadataField, MetadataPosting,
};
use gfm_types::{ContentPosting, FileId, VolumeId};
use std::fs;
use std::io::{Cursor, Write};
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use zip::write::SimpleFileOptions;

#[test]
fn indexes_and_searches_real_files_from_binary() {
    let root = unique_temp_dir("gfm-cli-root");
    let index = unique_temp_path("gfm-cli-index", "gfmidx");
    fs::create_dir_all(root.join("Reports")).unwrap();
    fs::write(root.join("Reports").join("QuarterlyPlan.md"), "alpha").unwrap();
    fs::write(root.join("notes.txt"), "beta").unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), index.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );
    let index_stderr = String::from_utf8_lossy(&index_output.stderr);
    assert!(index_stderr.contains("security-scope\t"), "{index_stderr}");
    assert!(index_stderr.contains("\tintent=index\t"), "{index_stderr}");
    assert!(
        index_stderr.contains(&format!(
            "security-worker-admission\tworker=index\tpath={}",
            root.display()
        )),
        "{index_stderr}"
    );

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["search-index", index.to_str().unwrap(), "quarterly"])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );
    let search_stderr = String::from_utf8_lossy(&search_output.stderr);
    assert!(
        search_stderr.contains(&format!(
            "security-worker-admission\tworker=search index\tpath={}",
            index.display()
        )),
        "{search_stderr}"
    );

    let stdout = String::from_utf8(search_output.stdout).unwrap();
    assert!(stdout.contains("QuarterlyPlan.md"), "{stdout}");

    let mmap_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["search-index-mmap", index.to_str().unwrap(), "quarterly"])
        .output()
        .unwrap();
    assert!(
        mmap_output.status.success(),
        "{}",
        String::from_utf8_lossy(&mmap_output.stderr)
    );
    let mmap_stderr = String::from_utf8_lossy(&mmap_output.stderr);
    assert!(
        mmap_stderr.contains(&format!(
            "security-worker-admission\tworker=search index mmap\tpath={}",
            index.display()
        )),
        "{mmap_stderr}"
    );
    assert_eq!(String::from_utf8(mmap_output.stdout).unwrap(), stdout);

    let verify_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["records-verify", index.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        verify_output.status.success(),
        "{}",
        String::from_utf8_lossy(&verify_output.stderr)
    );
    let verify_stderr = String::from_utf8_lossy(&verify_output.stderr);
    assert!(
        verify_stderr.contains(&format!(
            "security-worker-admission\tworker=records verify\tpath={}",
            index.display()
        )),
        "{verify_stderr}"
    );
    let verify_stdout = String::from_utf8(verify_output.stdout).unwrap();
    assert!(
        verify_stdout.contains("\tchecksum=verified"),
        "{verify_stdout}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(index).unwrap();
}

#[test]
fn index_retries_transient_failure_from_binary() {
    let root = unique_temp_dir("gfm-cli-index-retry-root");
    let index = unique_temp_path("gfm-cli-index-retry", "gfmidx");
    let journal = unique_temp_path("gfm-cli-index-retry", "journal");
    let catalog = unique_temp_path("gfm-cli-index-retry", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-index-retry", "gfmprogress");
    let retry_probe = unique_temp_path("gfm-cli-index-retry", "state");
    fs::create_dir_all(root.join("Reports")).unwrap();
    fs::write(root.join("Reports").join("RetryPlan.md"), "metadata retry").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "index-retry-probe",
            root.to_str().unwrap(),
            index.to_str().unwrap(),
            retry_probe.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_to_string(&retry_probe).unwrap(), "2");
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(
        journal_text.contains("1\t1\tstarted\tindex"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t1\tfailed:temporary index retry probe busy\tindex"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tstarted\tindex"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tcompleted\tindex"),
        "{journal_text}"
    );

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["search-index", index.to_str().unwrap(), "retryplan"])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );
    let stdout = String::from_utf8(search_output.stdout).unwrap();
    assert!(stdout.contains("RetryPlan.md"), "{stdout}");

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(index).unwrap();
    fs::remove_file(journal).unwrap();
    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
    fs::remove_file(retry_probe).unwrap();
}

#[test]
fn search_index_retries_transient_archive_read_failure_from_binary() {
    let root = unique_temp_dir("gfm-cli-search-index-retry-root");
    let index = unique_temp_path("gfm-cli-search-index-retry", "gfmidx");
    let journal = unique_temp_path("gfm-cli-search-index-retry", "journal");
    let catalog = unique_temp_path("gfm-cli-search-index-retry", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-search-index-retry", "gfmprogress");
    let retry_probe = unique_temp_path("gfm-cli-search-index-retry", "state");
    fs::create_dir_all(root.join("Reports")).unwrap();
    fs::write(
        root.join("Reports").join("RetrySearch.md"),
        "persisted search retry",
    )
    .unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), index.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "search-index-retry-probe",
            index.to_str().unwrap(),
            "retrysearch",
            retry_probe.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );
    let stdout = String::from_utf8(search_output.stdout).unwrap();
    assert!(stdout.contains("RetrySearch.md"), "{stdout}");
    assert_eq!(fs::read_to_string(&retry_probe).unwrap(), "2");
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(
        journal_text.contains("1\t1\tstarted\tsearch index"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t1\tfailed:temporary search index retry probe busy\tsearch index"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tstarted\tsearch index"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tcompleted\tsearch index"),
        "{journal_text}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(index).unwrap();
    fs::remove_file(journal).unwrap();
    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
    fs::remove_file(retry_probe).unwrap();
}

#[test]
fn search_index_mmap_retries_transient_archive_read_failure_from_binary() {
    let root = unique_temp_dir("gfm-cli-search-index-mmap-retry-root");
    let index = unique_temp_path("gfm-cli-search-index-mmap-retry", "gfmidx");
    let journal = unique_temp_path("gfm-cli-search-index-mmap-retry", "journal");
    let catalog = unique_temp_path("gfm-cli-search-index-mmap-retry", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-search-index-mmap-retry", "gfmprogress");
    let retry_probe = unique_temp_path("gfm-cli-search-index-mmap-retry", "state");
    fs::create_dir_all(root.join("Reports")).unwrap();
    fs::write(
        root.join("Reports").join("RetryMmapSearch.md"),
        "persisted mmap search retry",
    )
    .unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), index.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "search-index-mmap-retry-probe",
            index.to_str().unwrap(),
            "retrymmapsearch",
            retry_probe.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );
    let stdout = String::from_utf8(search_output.stdout).unwrap();
    assert!(stdout.contains("RetryMmapSearch.md"), "{stdout}");
    assert_eq!(fs::read_to_string(&retry_probe).unwrap(), "2");
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(
        journal_text.contains("1\t1\tstarted\tsearch index mmap"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains(
            "1\t1\tfailed:temporary search index mmap retry probe busy\tsearch index mmap"
        ),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tcompleted\tsearch index mmap"),
        "{journal_text}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(index).unwrap();
    fs::remove_file(journal).unwrap();
    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
    fs::remove_file(retry_probe).unwrap();
}

#[test]
fn live_search_routes_retry_transient_scan_failure_from_binary() {
    let root = unique_temp_dir("gfm-cli-live-search-retry-root");
    let search_journal = unique_temp_path("gfm-cli-live-search-retry", "journal");
    let search_catalog = unique_temp_path("gfm-cli-live-search-retry", "gfmjobs");
    let search_progress = unique_temp_path("gfm-cli-live-search-retry", "gfmprogress");
    let search_probe = unique_temp_path("gfm-cli-live-search-retry", "state");
    let stream_journal = unique_temp_path("gfm-cli-live-search-stream-retry", "journal");
    let stream_catalog = unique_temp_path("gfm-cli-live-search-stream-retry", "gfmjobs");
    let stream_progress = unique_temp_path("gfm-cli-live-search-stream-retry", "gfmprogress");
    let stream_probe = unique_temp_path("gfm-cli-live-search-stream-retry", "state");
    fs::create_dir_all(root.join("Reports")).unwrap();
    fs::write(
        root.join("Reports").join("LiveRetrySearch.md"),
        "live retry search marker",
    )
    .unwrap();

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &search_journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &search_catalog)
        .env("GFM_JOB_PROGRESS_STORE", &search_progress)
        .args([
            "search-retry-probe",
            root.to_str().unwrap(),
            "liveretrysearch",
            search_probe.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );
    let search_stdout = String::from_utf8(search_output.stdout).unwrap();
    assert!(
        search_stdout.contains("LiveRetrySearch.md"),
        "{search_stdout}"
    );
    assert_eq!(fs::read_to_string(&search_probe).unwrap(), "2");
    let search_journal_text = fs::read_to_string(&search_journal).unwrap();
    assert!(
        search_journal_text.contains("1\t1\tstarted\tsearch"),
        "{search_journal_text}"
    );
    assert!(
        search_journal_text.contains("1\t1\tfailed:temporary search retry probe busy\tsearch"),
        "{search_journal_text}"
    );
    assert!(
        search_journal_text.contains("1\t2\tcompleted\tsearch"),
        "{search_journal_text}"
    );

    let stream_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &stream_journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &stream_catalog)
        .env("GFM_JOB_PROGRESS_STORE", &stream_progress)
        .args([
            "search-stream-retry-probe",
            root.to_str().unwrap(),
            "liveretrysearch",
            stream_probe.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        stream_output.status.success(),
        "{}",
        String::from_utf8_lossy(&stream_output.stderr)
    );
    let stream_stdout = String::from_utf8(stream_output.stdout).unwrap();
    assert!(
        stream_stdout.contains("batch\t") && stream_stdout.contains("LiveRetrySearch.md"),
        "{stream_stdout}"
    );
    assert_eq!(fs::read_to_string(&stream_probe).unwrap(), "2");
    let stream_journal_text = fs::read_to_string(&stream_journal).unwrap();
    assert!(
        stream_journal_text.contains("1\t1\tstarted\tsearch stream"),
        "{stream_journal_text}"
    );
    assert!(
        stream_journal_text
            .contains("1\t1\tfailed:temporary search stream retry probe busy\tsearch stream"),
        "{stream_journal_text}"
    );
    assert!(
        stream_journal_text.contains("1\t2\tcompleted\tsearch stream"),
        "{stream_journal_text}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(search_journal).unwrap();
    fs::remove_file(search_catalog).unwrap();
    fs::remove_file(search_progress).unwrap();
    fs::remove_file(search_probe).unwrap();
    fs::remove_file(stream_journal).unwrap();
    fs::remove_file(stream_catalog).unwrap();
    fs::remove_file(stream_progress).unwrap();
    fs::remove_file(stream_probe).unwrap();
}

#[test]
fn index_preflight_refreshes_permission_state_from_binary() {
    let root = unique_temp_dir("gfm-cli-permission-worker-refresh");
    let index = root.join("records.gfmidx");
    let state = root.join("permission-state.tsv");
    fs::write(root.join("note.md"), "alpha").unwrap();
    seed_stale_permission_state(&state);

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_PERMISSION_STATE", &state)
        .args(["index", root.to_str().unwrap(), index.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("permission-refresh\taudience=workers\tsubject=index records\t"),
        "{stderr}"
    );
    assert!(stderr.contains("refresh-workers=true"), "{stderr}");
    assert_worker_admitted(&stderr, "index", &root);
    assert!(stderr.contains("security-scope\t"), "{stderr}");

    let quiet = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_PERMISSION_STATE", &state)
        .args(["index", root.to_str().unwrap(), index.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        quiet.status.success(),
        "{}",
        String::from_utf8_lossy(&quiet.stderr)
    );
    let quiet_stderr = String::from_utf8_lossy(&quiet.stderr);
    assert!(
        !quiet_stderr.contains("permission-refresh\t"),
        "{quiet_stderr}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn index_preflight_skips_default_permission_snapshot_refresh_from_binary() {
    let scratch = unique_temp_dir("gfm-cli-permission-worker-no-default-refresh");
    let root = scratch.join("root");
    let index = scratch.join("records.gfmidx");
    let default_state = scratch.join("gfm-permission-state.tsv");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("note.md"), "alpha").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("TMPDIR", &scratch)
        .env_remove("GFM_PERMISSION_STATE")
        .args(["index", root.to_str().unwrap(), index.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("permission-refresh\t"), "{stderr}");
    assert_worker_admitted(&stderr, "index", &root);
    assert!(
        !default_state.exists(),
        "worker preflight should not materialize default permission snapshot at {}",
        default_state.display()
    );

    fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn lists_directory_entries_through_visible_worker_from_binary() {
    let root = unique_temp_dir("gfm-cli-list-visible-worker");
    fs::create_dir_all(root.join("Folder")).unwrap();
    fs::write(root.join("Visible.txt"), "listed").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("list")
        .arg(&root)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("Visible.txt"), "{stdout}");
    assert!(stdout.contains("Folder"), "{stdout}");
    assert!(
        stderr.contains(&format!(
            "security-worker-admission\tworker=directory listing\tpath={}",
            root.display()
        )),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn list_refuses_unreachable_network_volume_before_reading_from_binary() {
    let root = unique_temp_dir("gfm-cli-list-unreachable-volume");
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(root.join("Visible.txt"), "should not be listed").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("list")
        .arg(&root)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("Visible.txt"), "{stdout}");
    assert!(
        stderr.contains("directory listing volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=directory listing\tpath={}",
            root.display()
        )),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn watch_once_refuses_unreachable_network_volume_before_subscribing_from_binary() {
    let root = unique_temp_dir("gfm-cli-watch-unreachable-volume");
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("watch-once")
        .arg(&root)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("modify\t"), "{stdout}");
    assert!(
        stderr.contains("index volume access blocked: unreachable volume network"),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn index_refuses_unreachable_network_volume_before_worker_admission_from_binary() {
    let root = unique_temp_dir("gfm-cli-index-unreachable-volume");
    let index = unique_temp_path("gfm-cli-index-unreachable-volume", "gfmidx");
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(root.join("Visible.txt"), "should not be indexed").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), index.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("index volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=index\tpath={}",
            root.display()
        )),
        "{stderr}"
    );
    assert!(!index.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn index_routes_refuse_unreachable_outputs_before_writing_from_binary() {
    let root = unique_temp_dir("gfm-cli-index-route-root");
    let offline = unique_temp_dir("gfm-cli-index-route-output-unreachable");
    fs::write(root.join("Visible.txt"), "alpha").unwrap();
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();

    let index = offline.join("records.gfmidx");
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), index.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("index records volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=index\tpath={}",
            root.display()
        )),
        "{stderr}"
    );
    assert!(!index.exists());

    let records = root.join("records.gfmidx");
    let state = offline.join("state.gfmstate");
    let state_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-state",
            root.to_str().unwrap(),
            records.to_str().unwrap(),
            state.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!state_output.status.success());
    let state_stderr = String::from_utf8_lossy(&state_output.stderr);
    assert!(
        state_stderr.contains("index state volume access blocked: unreachable volume network"),
        "{state_stderr}"
    );
    assert!(!records.exists());
    assert!(!state.exists());

    let progress = offline.join("scan.gfmprogress");
    let progress_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "scan-progress",
            root.to_str().unwrap(),
            records.to_str().unwrap(),
            progress.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!progress_output.status.success());
    let progress_stderr = String::from_utf8_lossy(&progress_output.stderr);
    assert!(
        progress_stderr
            .contains("scan progress checkpoint volume access blocked: unreachable volume network"),
        "{progress_stderr}"
    );
    assert!(!records.exists());
    assert!(!progress.exists());

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn content_index_reports_output_path_probe_failure_before_scanning_from_binary() {
    let root = unique_temp_dir("gfm-cli-content-index-output-probe-root");
    let records = unique_temp_path("gfm-cli-content-index-output-probe", "gfmidx");
    let content = root.join(format!("{}.gfmcontent", "content-unavailable".repeat(16)));
    fs::write(root.join("Visible.txt"), "must not be content indexed").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-content",
            root.to_str().unwrap(),
            records.to_str().unwrap(),
            content.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("content write"), "{stderr}");
    assert!(stderr.contains("content-unavailable"), "{stderr}");
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=content index\tpath={}",
            root.display()
        )),
        "{stderr}"
    );
    assert!(!records.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn index_reports_output_path_probe_failure_before_scanning_from_binary() {
    let root = unique_temp_dir("gfm-cli-index-output-probe-root");
    let index = root.join(format!("{}.gfmidx", "records-unavailable".repeat(16)));
    fs::write(root.join("Visible.txt"), "must not be indexed").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), index.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains("index write path metadata unavailable"),
        "{stderr}"
    );
    assert!(stderr.contains("records-unavailable"), "{stderr}");
    assert!(
        !stderr.contains("security-worker-admission\tworker=index records\t"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=index\tpath={}",
            root.display()
        )),
        "{stderr}"
    );
    assert!(!index.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn index_routes_refuse_unreachable_state_inputs_before_reading_from_binary() {
    let offline = unique_temp_dir("gfm-cli-index-route-input-unreachable");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let state = offline.join("state.gfmstate");
    let progress = offline.join("scan.gfmprogress");
    let cursor = offline.join("cursor.gfmfsevents");
    fs::write(&state, "not parsed after access denial\n").unwrap();
    fs::write(&progress, "not parsed after access denial\n").unwrap();
    fs::write(&cursor, "not parsed after access denial\n").unwrap();

    let cases = [
        (
            vec!["index-state-inspect", state.to_str().unwrap()],
            "index state inspect volume access blocked: unreachable volume network",
            "index state inspect",
            &state,
        ),
        (
            vec!["scan-progress-inspect", progress.to_str().unwrap()],
            "scan progress checkpoint inspect volume access blocked: unreachable volume network",
            "scan progress checkpoint inspect",
            &progress,
        ),
        (
            vec!["fsevents-cursor-inspect", cursor.to_str().unwrap()],
            "fsevents cursor inspect volume access blocked: unreachable volume network",
            "fsevents cursor inspect",
            &cursor,
        ),
    ];

    for (args, expected, worker, path) in cases {
        let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stdout.is_empty(), "{stdout}");
        assert!(stderr.contains(expected), "{stderr}");
        assert!(
            !stderr.contains(&format!(
                "security-worker-admission\tworker={worker}\tpath={}",
                path.display()
            )),
            "{stderr}"
        );
    }

    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn fsevents_cursor_resume_refuses_unreachable_cursor_before_reading_state_from_binary() {
    let local = unique_temp_dir("gfm-cli-fsevents-resume-local");
    let offline = unique_temp_dir("gfm-cli-fsevents-resume-cursor-unreachable");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let state = local.join("state.gfmstate");
    let cursor = offline.join("cursor.gfmfsevents");
    fs::write(&state, "state is not parsed after cursor access denial\n").unwrap();
    fs::write(&cursor, "cursor is not parsed after access denial\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "fsevents-cursor-resume",
            state.to_str().unwrap(),
            cursor.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains("fsevents cursor resume volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=fsevents cursor resume state\tpath={}",
            state.display()
        )),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=fsevents cursor resume\tpath={}",
            cursor.display()
        )),
        "{stderr}"
    );

    fs::remove_dir_all(local).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn operation_preflight_refreshes_permission_state_from_binary() {
    let root = unique_temp_dir("gfm-cli-permission-operation-refresh");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    let state = root.join("permission-state.tsv");
    fs::write(&source, "alpha").unwrap();
    seed_stale_permission_state(&state);

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_PERMISSION_STATE", &state)
        .args([
            "copy",
            source.to_str().unwrap(),
            destination.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("permission-refresh\taudience=operations\tsubject=copy\t"),
        "{stderr}"
    );
    assert!(
        stderr.contains("security-worker-admission\tworker=copy source\t")
            && stderr.contains("\tworker-action=start\t")
            && stderr.contains("\tcan-touch-filesystem=true\t"),
        "{stderr}"
    );
    assert!(
        stderr.contains("security-worker-admission\tworker=copy destination-parent\t")
            && stderr.contains("\tworker-action=start\t")
            && stderr.contains("\tcan-touch-filesystem=true\t"),
        "{stderr}"
    );
    assert!(stderr.contains("\trefresh-operations=true"), "{stderr}");
    assert!(destination.is_file());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn operation_preflight_emits_denied_worker_admission_for_unreachable_destination_from_binary() {
    let root = unique_temp_dir("gfm-cli-operation-unreachable-admission");
    let source_root = root.join("source");
    let offline = root.join("offline-share");
    let journal = root.join("ops.journal");
    let trash_metadata = root.join("trash.tsv");
    let bookmarks = root.join("bookmarks.tsv");
    let permission_state = root.join("permission-state.tsv");
    fs::create_dir_all(&source_root).unwrap();
    fs::create_dir_all(&offline).unwrap();
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(&journal, "").unwrap();
    fs::write(&trash_metadata, "gfm-trash-metadata-v1\n").unwrap();
    fs::write(&bookmarks, "gfm-security-bookmarks-v1\n").unwrap();
    fs::write(&permission_state, "gfm-permission-state-v1\n").unwrap();
    let source = source_root.join("source.txt");
    let destination = offline.join("destination.txt");
    fs::write(&source, "alpha").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPS_JOURNAL", &journal)
        .env("GFM_TRASH_METADATA", &trash_metadata)
        .env("GFM_SECURITY_BOOKMARKS", &bookmarks)
        .env("GFM_PERMISSION_STATE", &permission_state)
        .args([
            "copy",
            source.to_str().unwrap(),
            destination.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains("security-worker-admission\tworker=copy destination-parent\t"),
        "{stderr}"
    );
    assert!(stderr.contains("\tworker-action=deny\t"), "{stderr}");
    assert!(
        stderr.contains("\tcan-touch-filesystem=false\t"),
        "{stderr}"
    );
    assert!(
        stderr.contains("\trefresh-on-permission-change=true\t"),
        "{stderr}"
    );
    assert!(
        stderr
            .contains("copy destination-parent volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        stderr.contains("refresh-on-permission-change=true"),
        "{stderr}"
    );
    assert!(!destination.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extraction_preflight_retains_security_scoped_bookmark_from_binary() {
    let root = unique_temp_dir("gfm-cli-extraction-bookmark");
    let home = root.join("home");
    let documents = home.join("Documents");
    let protected = documents.join("Plan.md");
    let bookmarks = root.join("bookmarks.tsv");
    fs::create_dir_all(&documents).unwrap();
    fs::write(&protected, "alpha protected content").unwrap();

    let create = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("HOME", &home)
        .env("GFM_SECURITY_BOOKMARKS", &bookmarks)
        .args([
            "security-bookmark-create",
            protected.to_str().unwrap(),
            "read",
        ])
        .output()
        .unwrap();
    assert!(
        create.status.success(),
        "{}",
        String::from_utf8_lossy(&create.stderr)
    );
    let create_stderr = String::from_utf8_lossy(&create.stderr);
    assert!(
        create_stderr.contains(&format!(
            "security-worker-admission\tworker=security bookmark store\tpath={}",
            root.display()
        )),
        "{create_stderr}"
    );
    let create_stdout = String::from_utf8(create.stdout).unwrap();
    assert!(
        create_stdout.contains("security-bookmark\t")
            && create_stdout.contains("\tstatus=created\t"),
        "{create_stdout}"
    );
    assert!(
        create_stdout.contains("security-bookmark-store\t")
            && create_stdout.contains("\trecords=1\t"),
        "{create_stdout}"
    );

    let extract = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("HOME", &home)
        .env("GFM_SECURITY_BOOKMARKS", &bookmarks)
        .args(["extract-report", protected.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        extract.status.success(),
        "{}",
        String::from_utf8_lossy(&extract.stderr)
    );
    let stderr = String::from_utf8_lossy(&extract.stderr);
    assert!(stderr.contains("security-scope\t"), "{stderr}");
    assert!(stderr.contains("\tscope=documents\t"), "{stderr}");
    assert!(stderr.contains("\tbookmark-required=true\t"), "{stderr}");
    assert!(
        stderr.contains("security-scope-access\t")
            && stderr.contains("\tstatus=resolved\t")
            && stderr.contains("\taccess-started=true\t"),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn worker_preflights_refresh_permission_state_before_content_and_preview_from_binary() {
    let root = unique_temp_dir("gfm-cli-permission-content-preview-refresh");
    let document = root.join("Report.md");
    let preview = root.join("Preview.pdf");
    let thumbnail = root.join("Thumbnail.png");
    fs::write(&document, "alpha protected content").unwrap();
    fs::write(&preview, "%PDF-1.7\nalpha preview").unwrap();
    fs::write(&thumbnail, b"\x89PNG\r\n\x1a\nalpha thumbnail").unwrap();

    for (state_name, args, subject) in [
        (
            "extract",
            vec![
                "extract-report".to_string(),
                document.to_string_lossy().into_owned(),
            ],
            "content extraction",
        ),
        (
            "quicklook",
            vec![
                "quicklook-session".to_string(),
                preview.to_string_lossy().into_owned(),
            ],
            "quicklook preview",
        ),
        (
            "thumbnail",
            vec![
                "thumbnail-generation".to_string(),
                thumbnail.to_string_lossy().into_owned(),
            ],
            "thumbnail generation",
        ),
    ] {
        let state = root.join(format!("{state_name}-permission-state.tsv"));
        seed_stale_permission_state(&state);

        let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
            .env("GFM_PERMISSION_STATE", &state)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(&format!(
                "permission-refresh\taudience=workers\tsubject={subject}\t"
            )),
            "{stderr}"
        );
        assert!(stderr.contains("\trefresh-workers=true\t"), "{stderr}");
        assert!(stderr.contains("security-scope\t"), "{stderr}");
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn security_bookmark_create_refuses_unreachable_store_before_persisting_from_binary() {
    let root = unique_temp_dir("gfm-cli-security-bookmark-store-create-root");
    let offline = unique_temp_dir("gfm-cli-security-bookmark-store-create-offline");
    let home = root.join("home");
    let documents = home.join("Documents");
    let protected = documents.join("Plan.md");
    let bookmarks = offline.join("bookmarks.tsv");
    fs::create_dir_all(&documents).unwrap();
    fs::write(&protected, "alpha protected content").unwrap();
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("HOME", &home)
        .env("GFM_SECURITY_BOOKMARKS", &bookmarks)
        .args([
            "security-bookmark-create",
            protected.to_str().unwrap(),
            "read",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("security-bookmark\t"), "{stdout}");
    assert!(
        stderr
            .contains("security bookmark store volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("security-worker-admission\tworker=security bookmark store\t"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("security-worker-admission\tworker=security bookmark create\t"),
        "{stderr}"
    );
    assert!(!bookmarks.exists());

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn security_bookmark_create_refuses_unreachable_target_before_bookmarking_from_binary() {
    let root = unique_temp_dir("gfm-cli-security-bookmark-target-create-root");
    let offline = unique_temp_dir("gfm-cli-security-bookmark-target-create-offline");
    let home = offline.join("home");
    let documents = home.join("Documents");
    let protected = documents.join("Plan.md");
    let bookmarks = root.join("bookmarks.tsv");
    fs::create_dir_all(&documents).unwrap();
    fs::write(&protected, "alpha protected content").unwrap();
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("HOME", &home)
        .env("GFM_SECURITY_BOOKMARKS", &bookmarks)
        .args([
            "security-bookmark-create",
            protected.to_str().unwrap(),
            "read",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("security-bookmark\t"), "{stdout}");
    assert!(
        stderr
            .contains("security bookmark create volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("security-worker-admission\tworker=security bookmark store\t"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("security-worker-admission\tworker=security bookmark create\t"),
        "{stderr}"
    );
    assert!(!bookmarks.exists());

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn bookmark_required_preview_refuses_unreachable_store_before_reading_from_binary() {
    let root = unique_temp_dir("gfm-cli-security-bookmark-store-read-root");
    let offline = unique_temp_dir("gfm-cli-security-bookmark-store-read-offline");
    let home = root.join("home");
    let documents = home.join("Documents");
    let protected = documents.join("Plan.pdf");
    let bookmarks = offline.join("bookmarks.tsv");
    fs::create_dir_all(&documents).unwrap();
    fs::write(&protected, "%PDF-1.7\nalpha protected preview").unwrap();
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(&bookmarks, "gfm-security-bookmarks-v1\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("HOME", &home)
        .env("GFM_SECURITY_BOOKMARKS", &bookmarks)
        .args(["quicklook-session", protected.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("quicklook-session\t"), "{stdout}");
    assert!(
        stderr
            .contains("security bookmark store volume access blocked: unreachable volume network"),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn security_bookmark_reconcile_reports_store_state_from_binary() {
    let root = unique_temp_dir("gfm-cli-security-bookmark-reconcile");
    let bookmarks = root.join("bookmarks.tsv");
    fs::write(&bookmarks, "gfm-security-bookmarks-v1\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_SECURITY_BOOKMARKS", &bookmarks)
        .arg("security-bookmark-reconcile")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout,
        format!(
            "security-bookmark-store\t{}\trecords=0\trepaired=0\tunavailable=0\n",
            bookmarks.display()
        )
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn security_bookmark_reconcile_refuses_unreachable_store_before_reading_from_binary() {
    let offline = unique_temp_dir("gfm-cli-security-bookmark-reconcile-offline");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let bookmarks = offline.join("bookmarks.tsv");
    fs::write(&bookmarks, "not a bookmark store\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_SECURITY_BOOKMARKS", &bookmarks)
        .arg("security-bookmark-reconcile")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("security-bookmark-store\t"), "{stdout}");
    assert!(
        stderr.contains(
            "security bookmark reconcile volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert!(
        !stderr.contains("unsupported security bookmark store"),
        "{stderr}"
    );

    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn quicklook_preflight_retains_security_scoped_bookmark_from_binary() {
    let root = unique_temp_dir("gfm-cli-quicklook-bookmark");
    let home = root.join("home");
    let documents = home.join("Documents");
    let protected = documents.join("Plan.pdf");
    let bookmarks = root.join("bookmarks.tsv");
    fs::create_dir_all(&documents).unwrap();
    fs::write(&protected, "%PDF-1.7\nalpha protected preview").unwrap();

    let create = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("HOME", &home)
        .env("GFM_SECURITY_BOOKMARKS", &bookmarks)
        .args([
            "security-bookmark-create",
            protected.to_str().unwrap(),
            "preview",
        ])
        .output()
        .unwrap();
    assert!(
        create.status.success(),
        "{}",
        String::from_utf8_lossy(&create.stderr)
    );
    let create_stderr = String::from_utf8_lossy(&create.stderr);
    assert_worker_admitted(&create_stderr, "security bookmark store", &root);
    let create_stdout = String::from_utf8(create.stdout).unwrap();
    assert!(
        create_stdout.contains("\tstatus=created\t"),
        "{create_stdout}"
    );
    assert!(create_stdout.contains("\trecords=1\t"), "{create_stdout}");

    let quicklook = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("HOME", &home)
        .env("GFM_SECURITY_BOOKMARKS", &bookmarks)
        .args(["quicklook-session", protected.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        quicklook.status.success(),
        "{}",
        String::from_utf8_lossy(&quicklook.stderr)
    );
    let stderr = String::from_utf8_lossy(&quicklook.stderr);
    assert!(stderr.contains("security-scope\t"), "{stderr}");
    assert!(stderr.contains("\tintent=preview\t"), "{stderr}");
    assert!(stderr.contains("\tscope=documents\t"), "{stderr}");
    assert!(stderr.contains("\tbookmark-required=true\t"), "{stderr}");
    assert!(
        stderr.contains("security-scope-access\t")
            && stderr.contains("\tstatus=resolved\t")
            && stderr.contains("\taccess-started=true\t"),
        "{stderr}"
    );
    let stdout = String::from_utf8(quicklook.stdout).unwrap();
    assert!(
        stdout.starts_with("quicklook-session\tquick-look\t"),
        "{stdout}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn quicklook_preflight_accepts_retained_parent_folder_bookmark_from_binary() {
    let root = unique_temp_dir("gfm-cli-quicklook-parent-bookmark");
    let home = root.join("home");
    let documents = home.join("Documents");
    let protected = documents.join("Project").join("Plan.pdf");
    let bookmarks = root.join("bookmarks.tsv");
    fs::create_dir_all(protected.parent().unwrap()).unwrap();
    fs::write(&protected, "%PDF-1.7\nalpha protected preview").unwrap();

    let create = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("HOME", &home)
        .env("GFM_SECURITY_BOOKMARKS", &bookmarks)
        .args([
            "security-bookmark-create",
            documents.to_str().unwrap(),
            "preview",
        ])
        .output()
        .unwrap();
    assert!(
        create.status.success(),
        "{}",
        String::from_utf8_lossy(&create.stderr)
    );
    let create_stderr = String::from_utf8_lossy(&create.stderr);
    assert_worker_admitted(&create_stderr, "security bookmark store", &root);
    let create_stdout = String::from_utf8(create.stdout).unwrap();
    let canonical_documents = fs::canonicalize(&documents).unwrap();
    assert!(
        create_stdout.contains("\tstatus=created\t"),
        "{create_stdout}"
    );
    assert!(
        create_stdout.contains(&format!(
            "security-bookmark\t{}",
            canonical_documents.display()
        )),
        "{create_stdout}"
    );

    let quicklook = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("HOME", &home)
        .env("GFM_SECURITY_BOOKMARKS", &bookmarks)
        .args(["quicklook-session", protected.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        quicklook.status.success(),
        "{}",
        String::from_utf8_lossy(&quicklook.stderr)
    );
    let stderr = String::from_utf8_lossy(&quicklook.stderr);
    assert!(stderr.contains("security-scope\t"), "{stderr}");
    assert!(stderr.contains("\tintent=preview\t"), "{stderr}");
    assert!(stderr.contains("\tscope=documents\t"), "{stderr}");
    assert!(stderr.contains("\tbookmark-required=true\t"), "{stderr}");
    assert!(
        stderr.contains("security-scope-access\t")
            && stderr.contains("\tstatus=resolved\t")
            && stderr.contains("\taccess-started=true\t"),
        "{stderr}"
    );
    let stdout = String::from_utf8(quicklook.stdout).unwrap();
    assert!(
        stdout.starts_with("quicklook-session\tquick-look\t"),
        "{stdout}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn thumbnail_preflight_retains_security_scoped_bookmark_from_binary() {
    let root = unique_temp_dir("gfm-cli-thumbnail-bookmark");
    let home = root.join("home");
    let documents = home.join("Documents");
    let protected = documents.join("Image.png");
    let bookmarks = root.join("bookmarks.tsv");
    fs::create_dir_all(&documents).unwrap();
    fs::write(&protected, b"\x89PNG\r\n\x1a\nprotected thumbnail").unwrap();

    let create = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("HOME", &home)
        .env("GFM_SECURITY_BOOKMARKS", &bookmarks)
        .args([
            "security-bookmark-create",
            protected.to_str().unwrap(),
            "preview",
        ])
        .output()
        .unwrap();
    assert!(
        create.status.success(),
        "{}",
        String::from_utf8_lossy(&create.stderr)
    );
    let create_stderr = String::from_utf8_lossy(&create.stderr);
    assert_worker_admitted(&create_stderr, "security bookmark store", &root);
    let create_stdout = String::from_utf8(create.stdout).unwrap();
    assert!(
        create_stdout.contains("\tstatus=created\t"),
        "{create_stdout}"
    );
    assert!(create_stdout.contains("\trecords=1\t"), "{create_stdout}");

    let thumbnail = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("HOME", &home)
        .env("GFM_SECURITY_BOOKMARKS", &bookmarks)
        .args(["thumbnail-generation", protected.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        thumbnail.status.success(),
        "{}",
        String::from_utf8_lossy(&thumbnail.stderr)
    );
    let stderr = String::from_utf8_lossy(&thumbnail.stderr);
    assert!(stderr.contains("security-scope\t"), "{stderr}");
    assert!(stderr.contains("\tintent=preview\t"), "{stderr}");
    assert!(stderr.contains("\tscope=documents\t"), "{stderr}");
    assert!(stderr.contains("\tbookmark-required=true\t"), "{stderr}");
    assert!(
        stderr.contains("security-scope-access\t")
            && stderr.contains("\tstatus=resolved\t")
            && stderr.contains("\taccess-started=true\t"),
        "{stderr}"
    );
    let stdout = String::from_utf8(thumbnail.stdout).unwrap();
    assert!(stdout.starts_with("thumbnail-generation\t"), "{stdout}");
    assert!(
        stdout.contains("\tallow-native\tcloud=native-eligible\tquicklook-thumbnailing\t"),
        "{stdout}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn preview_cache_invalidation_refuses_unreachable_cache_root_before_disk_touch_from_binary() {
    let root = unique_temp_dir("gfm-cli-preview-cache-root");
    let offline = unique_temp_dir("gfm-cli-preview-cache-unreachable");
    let cache_root = offline.join("cache");
    let previewed = root.join("Document.icloud");
    fs::write(&previewed, "downloaded preview").unwrap();
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "preview-cache-fileprovider-invalidation",
            cache_root.to_str().unwrap(),
            "downloaded",
            previewed.to_str().unwrap(),
            "thumbnail",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("preview-cache-invalidation\t"), "{stdout}");
    assert!(
        stderr.contains("preview cache root volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(!cache_root.exists());

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn index_refuses_missing_root_before_scan_from_binary() {
    let root = unique_temp_path("gfm-cli-index-missing-root", "missing");
    let index = unique_temp_path("gfm-cli-index-missing-output", "gfmidx");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), index.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("security-scope\t"), "{stderr}");
    assert!(stderr.contains("\tintent=index\t"), "{stderr}");
    assert!(stderr.contains("\taction=deny\t"), "{stderr}");
    assert!(
        stderr.contains("index access blocked: path is not present on this host"),
        "{stderr}"
    );
    assert!(!index.exists());
}

#[test]
fn parity_gate_and_review_use_governed_masks_from_binary() {
    let root = unique_temp_dir("gfm-cli-parity-gate-root");
    let expected = root.join("finder.rgba");
    let actual = root.join("gfm.rgba");
    let mask = root.join("mask.tsv");
    let manifest = root.join("gate.tsv");
    let review = root.join("review");
    fs::write(&expected, [0, 0, 0, 255, 10, 10, 10, 255]).unwrap();
    fs::write(&actual, [0, 0, 0, 255, 9, 10, 10, 255]).unwrap();
    fs::write(&mask, "1\t0\t1\t1\tOS-owned sidebar clock repaint\n").unwrap();
    fs::write(
        &manifest,
        format!(
            "manifest-version\t1\nprofile\tmacos-build=25A354\thardware-profile=macbookpro18,3\tdisplay-profile=studio-display-p3\tapp-version=0.1.0\tfixture-manifest=fixtures/manifest.tsv\tcaptured-at=2026-08-27T00:00:00Z\tcapture-command=screencapture:-x\treviewer=codex\tsigner=codex\tapproved-mask-set=macos-25A354-default\tappearance=dark\tscale=2x\tcolor-profile=display-p3\nentry\ttoolbar\t{}\t{}\t2\t1\t{}\t1040\t720\tactive\ticon\tfixtures/toolbar\n",
            expected.display(),
            actual.display(),
            mask.display()
        ),
    )
    .unwrap();

    let gate = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["parity-gate", manifest.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        gate.status.success(),
        "{}",
        String::from_utf8_lossy(&gate.stderr)
    );
    let gate_stdout = String::from_utf8(gate.stdout).unwrap();
    assert!(gate_stdout.contains("passed=true"), "{gate_stdout}");
    assert!(gate_stdout.contains("masked=1"), "{gate_stdout}");

    let bundle = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "parity-review",
            manifest.to_str().unwrap(),
            review.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        bundle.status.success(),
        "{}",
        String::from_utf8_lossy(&bundle.stderr)
    );
    let bundle_stdout = String::from_utf8(bundle.stdout).unwrap();
    assert!(bundle_stdout.contains("passed=true"), "{bundle_stdout}");
    assert!(review
        .join("visual-diffs")
        .join("000-toolbar-diff.png")
        .exists());
    assert!(review
        .join("source-artifacts")
        .join("000-toolbar-finder.rgba")
        .exists());
    assert!(fs::read_to_string(review.join("mask-justifications.tsv"))
        .unwrap()
        .contains("OS-owned sidebar clock repaint"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn parity_gate_rejects_unprovenanced_manifest_from_binary() {
    let root = unique_temp_dir("gfm-cli-parity-gate-missing-provenance");
    let expected = root.join("finder.rgba");
    let actual = root.join("gfm.rgba");
    let manifest = root.join("gate.tsv");
    fs::write(&expected, [0, 0, 0, 255]).unwrap();
    fs::write(&actual, [0, 0, 0, 255]).unwrap();
    fs::write(
        &manifest,
        format!(
            "toolbar\t{}\t{}\t1\t1\n",
            expected.display(),
            actual.display()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["parity-gate", manifest.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("missing capture provenance"), "{stderr}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn search_typing_benchmark_reports_hot_path_latency_from_binary() {
    let root = unique_temp_dir("gfm-cli-search-typing-benchmark");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-typing-benchmark",
            root.to_str().unwrap(),
            "256",
            "1",
            "PackageProject00000006",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("search-typing-benchmark\t"), "{stdout}");
    assert!(stdout.contains("\trecords=256\t"), "{stdout}");
    assert!(stdout.contains("\trepetitions=1\t"), "{stdout}");
    assert!(stdout.contains("\tprefix-candidates="), "{stdout}");
    assert!(stdout.contains("\tprefix-cache-hits="), "{stdout}");
    assert!(stdout.contains("\tviolations=0\tpassed=true"), "{stdout}");
    assert!(root.join("gfm-search-typing-history.tsv").exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn search_typing_session_benchmark_reports_cache_reuse_from_binary() {
    let root = unique_temp_dir("gfm-cli-search-typing-session-benchmark");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-typing-session-benchmark",
            root.to_str().unwrap(),
            "256",
            "3",
            "PackageProject00000006",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(
            "security-worker-admission\tworker=search typing session benchmark workspace\t"
        ) && stderr.contains("\tworker-action=start\t")
            && stderr.contains("\tcan-touch-filesystem=true\t"),
        "{stderr}"
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.starts_with("search-typing-session-benchmark\t"),
        "{stdout}"
    );
    assert!(stdout.contains("\trecords=256\t"), "{stdout}");
    assert!(stdout.contains("\tindexed-records=256\t"), "{stdout}");
    assert!(stdout.contains("\trepetitions=3\t"), "{stdout}");
    assert!(stdout.contains("\tprefix-cache-hits="), "{stdout}");
    assert!(stdout.contains("\tcontent-cache-hits="), "{stdout}");
    assert!(stdout.contains("\trecord-cache-hits="), "{stdout}");
    assert!(stdout.contains("\tresult-cache-hits="), "{stdout}");
    assert!(stdout.contains("\tresult-cache-misses="), "{stdout}");
    assert!(stdout.contains("\tviolations=0\tpassed=true"), "{stdout}");
    assert!(root.join("gfm-search-typing-session-history.tsv").exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn inspects_archive_schema_from_binary() {
    let root = unique_temp_dir("gfm-cli-archive-schema-root");
    let index = unique_temp_path("gfm-cli-archive-schema", "gfmidx");
    let prefixes = unique_temp_path("gfm-cli-archive-schema", "gfmprefix");
    let unsupported = unique_temp_path("gfm-cli-archive-schema", "gfmidx");
    fs::write(root.join("InstantSearch.md"), "alpha").unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), index.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );

    let records_schema = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["archive-schema", "records", index.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        records_schema.status.success(),
        "{}",
        String::from_utf8_lossy(&records_schema.stderr)
    );
    let records_stderr = String::from_utf8_lossy(&records_schema.stderr);
    assert!(
        records_stderr.contains(&format!(
            "security-worker-admission\tworker=archive schema\tpath={}",
            index.display()
        )),
        "{records_stderr}"
    );
    let records_stdout = String::from_utf8(records_schema.stdout).unwrap();
    assert!(
        records_stdout
            .contains("archive-schema\tkind=records\tstatus=current\tschema=gfm-store-v3"),
        "{records_stdout}"
    );

    let prefixes_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-prefixes",
            index.to_str().unwrap(),
            prefixes.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        prefixes_output.status.success(),
        "{}",
        String::from_utf8_lossy(&prefixes_output.stderr)
    );

    let prefixes_schema = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["archive-schema", "prefixes", prefixes.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        prefixes_schema.status.success(),
        "{}",
        String::from_utf8_lossy(&prefixes_schema.stderr)
    );
    let prefixes_stderr = String::from_utf8_lossy(&prefixes_schema.stderr);
    assert!(
        prefixes_stderr.contains(&format!(
            "security-worker-admission\tworker=archive schema\tpath={}",
            prefixes.display()
        )),
        "{prefixes_stderr}"
    );
    let prefixes_stdout = String::from_utf8(prefixes_schema.stdout).unwrap();
    assert!(
        prefixes_stdout
            .contains("archive-schema\tkind=prefixes\tstatus=current\tschema=gfm-prefix-v1"),
        "{prefixes_stdout}"
    );

    fs::write(&unsupported, "not-gfm\n").unwrap();
    let unsupported_schema = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["archive-schema", "records", unsupported.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(unsupported_schema.status.success());
    let unsupported_stdout = String::from_utf8(unsupported_schema.stdout).unwrap();
    assert!(
        unsupported_stdout.contains("\tstatus=unsupported\t"),
        "{unsupported_stdout}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(index).unwrap();
    fs::remove_file(prefixes).unwrap();
    fs::remove_file(unsupported).unwrap();
}

#[test]
fn migrates_legacy_record_archive_from_binary() {
    let records = unique_temp_path("gfm-cli-records-migrate", "gfmidx");
    let backup = unique_temp_dir("gfm-cli-records-migrate-backup");
    fs::write(
        &records,
        "gfm-store-v1\n1\t2\t0\tf\t1\t0\t0\t0\t0\t/tmp/legacy.txt\n",
    )
    .unwrap();

    let plan = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["records-migration-plan", records.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        plan.status.success(),
        "{}",
        String::from_utf8_lossy(&plan.stderr)
    );
    let plan_stdout = String::from_utf8(plan.stdout).unwrap();
    let plan_stderr = String::from_utf8_lossy(&plan.stderr);
    assert!(
        plan_stderr.contains(&format!(
            "security-worker-admission\tworker=records migration plan\tpath={}",
            records.display()
        )),
        "{plan_stderr}"
    );
    assert!(
        plan_stdout.contains("record-archive-migration-plan\taction=migrate"),
        "{plan_stdout}"
    );

    let migration = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "records-migrate",
            records.to_str().unwrap(),
            backup.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        migration.status.success(),
        "{}",
        String::from_utf8_lossy(&migration.stderr)
    );
    let migration_stdout = String::from_utf8(migration.stdout).unwrap();
    let migration_stderr = String::from_utf8_lossy(&migration.stderr);
    assert_worker_admitted(&migration_stderr, "records migrate archive", &records);
    assert_worker_admitted(&migration_stderr, "records migrate backup", &backup);
    assert!(
        migration_stdout.contains(
            "record-archive-migration\tmigrated-records=1\tbefore-status=legacy\tafter-status=current"
        ),
        "{migration_stdout}"
    );

    let schema = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["archive-schema", "records", records.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(schema.status.success());
    let schema_stdout = String::from_utf8(schema.stdout).unwrap();
    assert!(
        schema_stdout.contains("\tstatus=current\tschema=gfm-store-v3"),
        "{schema_stdout}"
    );
    assert!(backup.read_dir().unwrap().next().is_some());

    fs::remove_file(records).unwrap();
    fs::remove_dir_all(backup).unwrap();
}

#[test]
fn migrates_legacy_content_archive_from_binary() {
    let content = unique_temp_path("gfm-cli-content-migrate", "gfmcontent");
    let backup = unique_temp_dir("gfm-cli-content-migrate-backup");
    write_legacy_content_archive(
        &content,
        &[ContentPosting {
            term: "legacyneedle".to_string(),
            ids: vec![FileId::new(VolumeId(7), 11)],
            positions: Vec::new(),
        }],
    );

    let plan = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["content-migration-plan", content.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        plan.status.success(),
        "{}",
        String::from_utf8_lossy(&plan.stderr)
    );
    let plan_stdout = String::from_utf8(plan.stdout).unwrap();
    let plan_stderr = String::from_utf8_lossy(&plan.stderr);
    assert!(
        plan_stderr.contains(&format!(
            "security-worker-admission\tworker=content migration plan\tpath={}",
            content.display()
        )),
        "{plan_stderr}"
    );
    assert!(
        plan_stdout.contains("content-archive-migration-plan\taction=migrate"),
        "{plan_stdout}"
    );

    let migration = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-migrate",
            content.to_str().unwrap(),
            backup.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        migration.status.success(),
        "{}",
        String::from_utf8_lossy(&migration.stderr)
    );
    let migration_stdout = String::from_utf8(migration.stdout).unwrap();
    let migration_stderr = String::from_utf8_lossy(&migration.stderr);
    assert_worker_admitted(&migration_stderr, "content migrate archive", &content);
    assert_worker_admitted(&migration_stderr, "content migrate backup", &backup);
    assert!(
        migration_stdout.contains(
            "content-archive-migration\tmigrated-postings=1\tbefore-status=legacy\tafter-status=current"
        ),
        "{migration_stdout}"
    );

    let verify = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["content-verify", content.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        verify.status.success(),
        "{}",
        String::from_utf8_lossy(&verify.stderr)
    );
    let verify_stdout = String::from_utf8(verify.stdout).unwrap();
    assert!(
        verify_stdout.contains("\tchecksum=verified"),
        "{verify_stdout}"
    );
    assert!(backup.read_dir().unwrap().next().is_some());

    fs::remove_file(content).unwrap();
    fs::remove_dir_all(backup).unwrap();
}

#[test]
fn migrates_legacy_metadata_archive_from_binary() {
    let metadata = unique_temp_path("gfm-cli-metadata-migrate", "gfmmeta");
    let backup = unique_temp_dir("gfm-cli-metadata-migrate-backup");
    write_legacy_metadata_archive(
        &metadata,
        &[MetadataPosting {
            field: MetadataField::Tag,
            term: "important".to_string(),
            ids: vec![FileId::new(VolumeId(7), 11)],
        }],
    );

    let plan = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["metadata-migration-plan", metadata.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        plan.status.success(),
        "{}",
        String::from_utf8_lossy(&plan.stderr)
    );
    let plan_stdout = String::from_utf8(plan.stdout).unwrap();
    let plan_stderr = String::from_utf8_lossy(&plan.stderr);
    assert!(
        plan_stderr.contains(&format!(
            "security-worker-admission\tworker=metadata migration plan\tpath={}",
            metadata.display()
        )),
        "{plan_stderr}"
    );
    assert!(
        plan_stdout.contains("metadata-archive-migration-plan\taction=migrate"),
        "{plan_stdout}"
    );

    let migration = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "metadata-migrate",
            metadata.to_str().unwrap(),
            backup.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        migration.status.success(),
        "{}",
        String::from_utf8_lossy(&migration.stderr)
    );
    let migration_stdout = String::from_utf8(migration.stdout).unwrap();
    let migration_stderr = String::from_utf8_lossy(&migration.stderr);
    assert_worker_admitted(&migration_stderr, "metadata migrate archive", &metadata);
    assert_worker_admitted(&migration_stderr, "metadata migrate backup", &backup);
    assert!(
        migration_stdout.contains(
            "metadata-archive-migration\tmigrated-postings=1\tbefore-status=legacy\tafter-status=current"
        ),
        "{migration_stdout}"
    );

    let verify = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["metadata-verify", metadata.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        verify.status.success(),
        "{}",
        String::from_utf8_lossy(&verify.stderr)
    );
    let verify_stdout = String::from_utf8(verify.stdout).unwrap();
    assert!(
        verify_stdout.contains("\tchecksum=verified"),
        "{verify_stdout}"
    );
    assert!(backup.read_dir().unwrap().next().is_some());

    fs::remove_file(metadata).unwrap();
    fs::remove_dir_all(backup).unwrap();
}

#[test]
fn archive_schema_refuses_unreachable_archive_before_inspection_from_binary() {
    let offline = unique_temp_dir("gfm-cli-archive-schema-preflight-offline");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let records = offline.join("records.gfmidx");
    fs::write(&records, "not inspected").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["archive-schema", "records", records.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("archive-schema\t"), "{stdout}");
    assert!(
        stderr.contains("archive schema volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=archive schema\tpath={}",
            records.display()
        )),
        "{stderr}"
    );
    assert!(!stderr.contains("invalid magic"), "{stderr}");

    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn archive_migration_routes_refuse_unreachable_backup_before_mutating_from_binary() {
    let root = unique_temp_dir("gfm-cli-archive-migrate-preflight-root");
    let offline = unique_temp_dir("gfm-cli-archive-migrate-preflight-offline");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();

    let cases = [
        ("records-migrate", "records.gfmidx", "records migrate"),
        ("content-migrate", "content.gfmcontent", "content migrate"),
        ("metadata-migrate", "metadata.gfmmeta", "metadata migrate"),
    ];

    for (route, archive_name, worker) in cases {
        let archive = root.join(archive_name);
        fs::write(&archive, "not parsed").unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
            .args([route, archive.to_str().unwrap(), offline.to_str().unwrap()])
            .output()
            .unwrap();

        assert!(!output.status.success(), "{route}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stdout.is_empty(), "{route}: {stdout}");
        assert!(
            stderr.contains(&format!(
                "{worker} backup volume access blocked: unreachable volume network"
            )),
            "{route}: {stderr}"
        );
        assert!(!stderr.contains("invalid magic"), "{route}: {stderr}");
    }

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn archive_migration_routes_report_backup_probe_failures_before_mutating_from_binary() {
    let root = unique_temp_dir("gfm-cli-archive-migrate-backup-probe");
    let backup = root.join("archive-backup-unavailable".repeat(16));

    let cases = [
        ("records-migrate", "records.gfmidx", "records migrate"),
        ("content-migrate", "content.gfmcontent", "content migrate"),
        ("metadata-migrate", "metadata.gfmmeta", "metadata migrate"),
    ];

    for (route, archive_name, worker) in cases {
        let archive = root.join(archive_name);
        fs::write(&archive, "not parsed").unwrap();
        let original = fs::read_to_string(&archive).unwrap();

        let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
            .args([route, archive.to_str().unwrap(), backup.to_str().unwrap()])
            .output()
            .unwrap();

        assert!(!output.status.success(), "{route}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stdout.is_empty(), "{route}: {stdout}");
        assert!(
            stderr.contains("archive write path metadata unavailable"),
            "{route}: {stderr}"
        );
        assert!(
            stderr.contains("archive-backup-unavailable"),
            "{route}: {stderr}"
        );
        assert!(
            !stderr.contains(&format!(
                "security-worker-admission\tworker={worker} archive\t"
            )),
            "{route}: {stderr}"
        );
        assert!(!stderr.contains("invalid magic"), "{route}: {stderr}");
        assert_eq!(fs::read_to_string(&archive).unwrap(), original, "{route}");
        assert!(!backup.exists(), "{route}");
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn archive_rebuild_plans_refuse_unreachable_inputs_before_classifying_from_binary() {
    let root = unique_temp_dir("gfm-cli-archive-rebuild-plan-preflight-root");
    let offline = unique_temp_dir("gfm-cli-archive-rebuild-plan-preflight-offline");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let records = root.join("records.gfmidx");
    let columns = offline.join("columns.gfmcols");
    fs::write(&records, "not parsed").unwrap();
    fs::write(&columns, "not classified").unwrap();

    let columns_plan = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "columns-rebuild-plan",
            records.to_str().unwrap(),
            columns.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!columns_plan.status.success());
    let columns_stdout = String::from_utf8_lossy(&columns_plan.stdout);
    let columns_stderr = String::from_utf8_lossy(&columns_plan.stderr);
    assert!(
        !columns_stdout.contains("columns-archive-rebuild-plan"),
        "{columns_stdout}"
    );
    assert!(
        columns_stderr.contains(
            "columns rebuild plan columns volume access blocked: unreachable volume network"
        ),
        "{columns_stderr}"
    );
    assert!(
        !columns_stderr.contains("invalid magic"),
        "{columns_stderr}"
    );

    let derived_plan = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "derived-sidecar-rebuild-plan",
            records.to_str().unwrap(),
            "metadata",
            columns.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!derived_plan.status.success());
    let derived_stdout = String::from_utf8_lossy(&derived_plan.stdout);
    let derived_stderr = String::from_utf8_lossy(&derived_plan.stderr);
    assert!(
        !derived_stdout.contains("derived-sidecar-rebuild-plan"),
        "{derived_stdout}"
    );
    assert!(
        derived_stderr.contains(
            "derived sidecar rebuild plan sidecar volume access blocked: unreachable volume network"
        ),
        "{derived_stderr}"
    );
    assert!(
        !derived_stderr.contains("invalid magic"),
        "{derived_stderr}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn rebuilds_columns_archive_from_binary() {
    let root = unique_temp_dir("gfm-cli-columns-rebuild-root");
    let records = unique_temp_path("gfm-cli-columns-rebuild-records", "gfmidx");
    let columns = unique_temp_path("gfm-cli-columns-rebuild-columns", "gfmcols");
    let backup = unique_temp_dir("gfm-cli-columns-rebuild-backup");
    fs::write(root.join("ColumnNeedle.md"), "alpha").unwrap();

    let index = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), records.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        index.status.success(),
        "{}",
        String::from_utf8_lossy(&index.stderr)
    );

    let plan = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "columns-rebuild-plan",
            records.to_str().unwrap(),
            columns.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        plan.status.success(),
        "{}",
        String::from_utf8_lossy(&plan.stderr)
    );
    let plan_stdout = String::from_utf8(plan.stdout).unwrap();
    assert!(
        plan_stdout.contains(
            "columns-archive-rebuild-plan\taction=rebuild\trecords-status=current\tcolumns-status=missing",
        ),
        "{plan_stdout}"
    );

    let rebuild = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "columns-rebuild",
            records.to_str().unwrap(),
            columns.to_str().unwrap(),
            backup.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        rebuild.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuild.stderr)
    );
    let rebuild_stdout = String::from_utf8(rebuild.stdout).unwrap();
    let rebuild_stderr = String::from_utf8_lossy(&rebuild.stderr);
    assert_worker_admitted(&rebuild_stderr, "columns rebuild records", &records);
    assert_worker_admitted(
        &rebuild_stderr,
        "columns rebuild columns",
        columns.parent().unwrap(),
    );
    assert_worker_admitted(
        &rebuild_stderr,
        "columns rebuild output",
        columns.parent().unwrap(),
    );
    assert_worker_admitted(&rebuild_stderr, "columns rebuild backup", &backup);
    assert!(
        rebuild_stdout.contains(
            "columns-archive-rebuild\trebuilt-records=2\trecords-status=current\tbefore-status=missing\tafter-status=current",
        ),
        "{rebuild_stdout}"
    );

    let verify = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["columns-verify", columns.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        verify.status.success(),
        "{}",
        String::from_utf8_lossy(&verify.stderr)
    );
    let verify_stdout = String::from_utf8(verify.stdout).unwrap();
    assert!(
        verify_stdout.contains("\tchecksum=verified"),
        "{verify_stdout}"
    );
    assert!(backup.read_dir().unwrap().next().is_none());

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(columns).unwrap();
    fs::remove_dir_all(backup).unwrap();
}

#[test]
fn archive_read_routes_refuse_unreachable_volume_before_mapping_from_binary() {
    let offline = unique_temp_dir("gfm-cli-archive-read-preflight-offline");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();

    let cases = [
        (
            "records-verify",
            "gfmidx",
            vec![],
            "records verify",
            "records-verify\t",
        ),
        (
            "columns-verify",
            "gfmcols",
            vec![],
            "columns verify",
            "columns-verify\t",
        ),
        (
            "columns-lookup",
            "gfmcols",
            vec!["1", "1"],
            "columns lookup",
            "columns\t",
        ),
    ];

    for (route, extension, tail_args, worker, forbidden_stdout) in cases {
        let archive = offline.join(format!("{route}.{extension}"));
        fs::write(&archive, "not opened").unwrap();
        let mut args = vec![route.to_string(), archive.to_string_lossy().into_owned()];
        args.extend(tail_args.into_iter().map(str::to_string));

        let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
            .args(args)
            .output()
            .unwrap();

        assert!(!output.status.success(), "{route}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stdout.contains(forbidden_stdout), "{route}: {stdout}");
        assert!(
            stderr.contains(&format!(
                "{worker} volume access blocked: unreachable volume network"
            )),
            "{route}: {stderr}"
        );
        assert!(
            !stderr.contains(&format!(
                "security-worker-admission\tworker={worker}\tpath={}",
                archive.display()
            )),
            "{route}: {stderr}"
        );
        assert!(!stderr.contains("invalid magic"), "{route}: {stderr}");
    }

    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn record_sidecar_build_routes_refuse_unreachable_output_before_mapping_from_binary() {
    let root = unique_temp_dir("gfm-cli-record-sidecar-build-preflight-root");
    let offline = unique_temp_dir("gfm-cli-record-sidecar-build-preflight-offline");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let records = root.join("records.gfmidx");
    fs::write(&records, "not opened").unwrap();

    let cases = [
        ("index-columns", "columns.gfmcols", "index columns"),
        ("index-metadata", "metadata.gfmmeta", "index metadata"),
        ("index-dictionary", "dictionary.gfmdict", "index dictionary"),
        ("index-prefixes", "prefixes.gfmprefix", "index prefixes"),
        (
            "index-substrings",
            "substrings.gfmsubstr",
            "index substrings",
        ),
        ("index-fuzzy", "fuzzy.gfmfuzzy", "index fuzzy"),
    ];

    for (route, output_name, worker) in cases {
        let output_path = offline.join(format!(
            "{}-{output_name}",
            "record-sidecar-output-unavailable".repeat(8)
        ));
        let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
            .args([
                route,
                records.to_str().unwrap(),
                output_path.to_str().unwrap(),
            ])
            .output()
            .unwrap();

        assert!(!output.status.success(), "{route}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stdout.is_empty(), "{route}: {stdout}");
        assert!(
            stderr.contains(&format!(
                "{worker} output volume access blocked: unreachable volume network"
            )),
            "{route}: {stderr}"
        );
        assert!(
            !stderr.contains("archive write path metadata unavailable"),
            "{route}: {stderr}"
        );
        assert!(
            !stderr.contains(&format!(
                "security-worker-admission\tworker={worker} records\t"
            )),
            "{route}: {stderr}"
        );
        assert!(!stderr.contains("invalid magic"), "{route}: {stderr}");
        assert!(!output_path.exists(), "{route}: {}", output_path.display());
    }

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn record_sidecar_build_routes_report_output_probe_failure_before_mapping_from_binary() {
    let root = unique_temp_dir("gfm-cli-record-sidecar-output-probe");
    let records = root.join("records.gfmidx");
    fs::write(&records, "not opened").unwrap();

    let cases = [
        ("index-columns", "gfmcols", "index columns"),
        ("index-metadata", "gfmmeta", "index metadata"),
        ("index-dictionary", "gfmdict", "index dictionary"),
        ("index-prefixes", "gfmprefix", "index prefixes"),
        ("index-substrings", "gfmsubstr", "index substrings"),
        ("index-fuzzy", "gfmfuzzy", "index fuzzy"),
    ];

    for (route, extension, worker) in cases {
        let output_path = root.join(format!(
            "{}.{extension}",
            format!("{route}-unavailable").repeat(16)
        ));
        let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
            .args([
                route,
                records.to_str().unwrap(),
                output_path.to_str().unwrap(),
            ])
            .output()
            .unwrap();

        assert!(!output.status.success(), "{route}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stdout.is_empty(), "{route}: {stdout}");
        assert!(
            stderr.contains("archive write path metadata unavailable"),
            "{route}: {stderr}"
        );
        assert!(
            stderr.contains(&format!("{route}-unavailable")),
            "{route}: {stderr}"
        );
        assert!(
            !stderr.contains(&format!(
                "security-worker-admission\tworker={worker} records\t"
            )),
            "{route}: {stderr}"
        );
        assert!(!stderr.contains("invalid magic"), "{route}: {stderr}");
        assert!(!output_path.exists(), "{route}");
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rebuilds_derived_sidecars_from_binary() {
    let root = unique_temp_dir("gfm-cli-derived-rebuild-root");
    let records = unique_temp_path("gfm-cli-derived-rebuild-records", "gfmidx");
    let columns = unique_temp_path("gfm-cli-derived-rebuild-columns", "gfmcols");
    let metadata = unique_temp_path("gfm-cli-derived-rebuild-metadata", "gfmmeta");
    let prefixes = unique_temp_path("gfm-cli-derived-rebuild-prefixes", "gfmprefix");
    let substrings = unique_temp_path("gfm-cli-derived-rebuild-substrings", "gfmsubstr");
    let fuzzy = unique_temp_path("gfm-cli-derived-rebuild-fuzzy", "gfmfuzzy");
    let dictionary = unique_temp_path("gfm-cli-derived-rebuild-dictionary", "gfmdict");
    let content = unique_temp_path("gfm-cli-derived-rebuild-content", "gfmcontent");
    let manifest = unique_temp_path("gfm-cli-derived-rebuild-content", "gfmmanifest");
    let backup = unique_temp_dir("gfm-cli-derived-rebuild-backup");
    fs::write(root.join("DerivedNeedle.md"), "alpha").unwrap();

    let index = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), records.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        index.status.success(),
        "{}",
        String::from_utf8_lossy(&index.stderr)
    );
    write_content_postings(&content, &[]).unwrap();

    let archive_plan = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "archive-rebuild-plan",
            records.to_str().unwrap(),
            columns.to_str().unwrap(),
            metadata.to_str().unwrap(),
            prefixes.to_str().unwrap(),
            substrings.to_str().unwrap(),
            fuzzy.to_str().unwrap(),
            dictionary.to_str().unwrap(),
            content.to_str().unwrap(),
            manifest.to_str().unwrap(),
            &format!("hot:{}", content.display()),
        ])
        .output()
        .unwrap();
    assert!(
        archive_plan.status.success(),
        "{}",
        String::from_utf8_lossy(&archive_plan.stderr)
    );
    let archive_plan_stdout = String::from_utf8(archive_plan.stdout).unwrap();
    assert!(
        archive_plan_stdout.contains(
            "archive-rebuild-plan\tentries=9\tready=2\tmigrate=0\trebuild=6\trecover=1\tblocked=0"
        ) && archive_plan_stdout.contains(
            "archive-rebuild-entry\tkind=content-manifest\troute=recover\tstatus=write-discovered-manifest"
        ) && archive_plan_stdout.contains(
            "archive-rebuild-entry\tkind=prefixes\troute=rebuild\tstatus=missing\tsource=durable-records"
        ) && archive_plan_stdout.contains(
            "archive-rebuild-entry\tkind=substrings\troute=rebuild\tstatus=missing\tsource=durable-records"
        ),
        "{archive_plan_stdout}"
    );

    let plan = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "derived-sidecar-rebuild-plan",
            records.to_str().unwrap(),
            "prefixes",
            prefixes.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        plan.status.success(),
        "{}",
        String::from_utf8_lossy(&plan.stderr)
    );
    let plan_stdout = String::from_utf8(plan.stdout).unwrap();
    assert!(
        plan_stdout.contains(
            "derived-sidecar-rebuild-plan\taction=rebuild\tkind=prefixes\trecords-status=current\tsidecar-status=missing",
        ),
        "{plan_stdout}"
    );

    let rebuild_prefixes = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "derived-sidecar-rebuild",
            records.to_str().unwrap(),
            "prefixes",
            prefixes.to_str().unwrap(),
            backup.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        rebuild_prefixes.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuild_prefixes.stderr)
    );
    let rebuild_prefixes_stdout = String::from_utf8(rebuild_prefixes.stdout).unwrap();
    assert!(
        rebuild_prefixes_stdout.contains(
            "derived-sidecar-rebuild\trebuilt-records=2\tkind=prefixes\trecords-status=current\tbefore-status=missing\tafter-status=current",
        ),
        "{rebuild_prefixes_stdout}"
    );
    let prefix_verify = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["prefix-verify", prefixes.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(prefix_verify.status.success());

    fs::write(&dictionary, "not-a-dictionary").unwrap();
    let rebuild_dictionary = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "derived-sidecar-rebuild",
            records.to_str().unwrap(),
            "dictionary",
            dictionary.to_str().unwrap(),
            backup.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        rebuild_dictionary.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuild_dictionary.stderr)
    );
    let rebuild_dictionary_stdout = String::from_utf8(rebuild_dictionary.stdout).unwrap();
    assert!(
        rebuild_dictionary_stdout.contains(
            "derived-sidecar-rebuild\trebuilt-records=2\tkind=dictionary\trecords-status=current\tbefore-status=unsupported\tafter-status=current",
        ),
        "{rebuild_dictionary_stdout}"
    );
    assert!(backup.read_dir().unwrap().next().is_some());
    let dictionary_verify = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["dictionary-verify", dictionary.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(dictionary_verify.status.success());

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(content).unwrap();
    if manifest.exists() {
        fs::remove_file(manifest).unwrap();
    }
    fs::remove_file(prefixes).unwrap();
    if substrings.exists() {
        fs::remove_file(substrings).unwrap();
    }
    fs::remove_file(dictionary).unwrap();
    fs::remove_dir_all(backup).unwrap();
}

#[test]
fn derived_sidecar_rebuild_refuses_unreachable_volume_before_repair_from_binary() {
    let root = unique_temp_dir("gfm-cli-derived-rebuild-unreachable-root");
    let records = root.join("records.gfmidx");
    let prefixes = root.join("prefixes.gfmprefix");
    let backup = root.join("backup");
    fs::create_dir_all(&backup).unwrap();
    fs::write(root.join("DerivedNeedle.md"), "alpha").unwrap();

    let index = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), records.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        index.status.success(),
        "{}",
        String::from_utf8_lossy(&index.stderr)
    );
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "derived-sidecar-rebuild",
            records.to_str().unwrap(),
            "prefixes",
            prefixes.to_str().unwrap(),
            backup.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("derived-sidecar-rebuild\t"), "{stdout}");
    assert!(
        stderr.contains(
            "derived sidecar rebuild records volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=derived sidecar rebuild records\tpath={}",
            records.display()
        )),
        "{stderr}"
    );
    assert!(!prefixes.exists());
    assert!(fs::read_dir(&backup).unwrap().next().is_none());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn derived_sidecar_rebuild_retry_probe_recovers_transient_failure_from_binary() {
    let root = unique_temp_dir("gfm-cli-derived-rebuild-retry-root");
    let records = unique_temp_path("gfm-cli-derived-rebuild-retry-records", "gfmidx");
    let prefixes = unique_temp_path("gfm-cli-derived-rebuild-retry-prefixes", "gfmprefix");
    let backup = unique_temp_dir("gfm-cli-derived-rebuild-retry-backup");
    let journal = unique_temp_path("gfm-cli-derived-rebuild-retry", "journal");
    let catalog = unique_temp_path("gfm-cli-derived-rebuild-retry", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-derived-rebuild-retry", "gfmprogress");
    let retry_probe = unique_temp_path("gfm-cli-derived-rebuild-retry", "state");
    fs::write(root.join("RetrySidecar.md"), "derived retry").unwrap();

    let index = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), records.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        index.status.success(),
        "{}",
        String::from_utf8_lossy(&index.stderr)
    );

    let rebuild = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "derived-sidecar-rebuild-retry-probe",
            records.to_str().unwrap(),
            "prefixes",
            prefixes.to_str().unwrap(),
            backup.to_str().unwrap(),
            retry_probe.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        rebuild.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuild.stderr)
    );
    let stdout = String::from_utf8(rebuild.stdout).unwrap();
    assert!(
        stdout.contains(
            "derived-sidecar-rebuild\trebuilt-records=2\tkind=prefixes\trecords-status=current\tbefore-status=missing\tafter-status=current",
        ),
        "{stdout}"
    );
    assert_eq!(fs::read_to_string(&retry_probe).unwrap(), "2");
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(
        journal_text.contains("1\t1\tstarted\tderived sidecar rebuild"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains(
            "1\t1\tfailed:temporary derived sidecar rebuild retry probe busy\tderived sidecar rebuild"
        ),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tstarted\tderived sidecar rebuild"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tcompleted\tderived sidecar rebuild"),
        "{journal_text}"
    );
    let verify = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["prefix-verify", prefixes.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        verify.status.success(),
        "{}",
        String::from_utf8_lossy(&verify.stderr)
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(prefixes).unwrap();
    fs::remove_dir_all(backup).unwrap();
    fs::remove_file(journal).unwrap();
    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
    fs::remove_file(retry_probe).unwrap();
}

#[test]
fn derived_sidecar_rebuild_reports_output_probe_failure_before_repair_from_binary() {
    let root = unique_temp_dir("gfm-cli-derived-rebuild-output-probe");
    let records = root.join("records.gfmidx");
    let sidecar = root.join("derived-sidecar-unavailable".repeat(16));
    let backup = root.join("backup");
    fs::create_dir_all(&backup).unwrap();
    fs::write(&records, "not opened").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "derived-sidecar-rebuild",
            records.to_str().unwrap(),
            "prefixes",
            sidecar.to_str().unwrap(),
            backup.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("derived-sidecar-rebuild\t"), "{stdout}");
    assert!(
        stderr.contains("archive write path metadata unavailable"),
        "{stderr}"
    );
    assert!(stderr.contains("derived-sidecar-unavailable"), "{stderr}");
    assert!(
        !stderr.contains("security-worker-admission\tworker=derived sidecar rebuild records\t"),
        "{stderr}"
    );
    assert!(!stderr.contains("invalid magic"), "{stderr}");
    assert!(!sidecar.exists());
    assert!(fs::read_dir(&backup).unwrap().next().is_none());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn persists_volume_index_state_from_binary() {
    let root = unique_temp_dir("gfm-cli-index-state-root");
    let index = unique_temp_path("gfm-cli-index-state-records", "gfmidx");
    let state = unique_temp_path("gfm-cli-index-state", "gfmstate");
    fs::create_dir_all(root.join("Projects")).unwrap();
    fs::write(root.join("Projects").join("StatefulSearch.md"), "alpha").unwrap();

    let first = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-state",
            root.to_str().unwrap(),
            index.to_str().unwrap(),
            state.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first_stdout = String::from_utf8(first.stdout).unwrap();
    assert_index_security_preflight(&first.stderr);
    assert!(first_stdout.starts_with("index-state\t"), "{first_stdout}");
    assert!(first_stdout.contains("\tschema=1\t"), "{first_stdout}");
    assert!(first_stdout.contains("\tscan-epoch=1\t"), "{first_stdout}");
    assert!(
        first_stdout.contains("\trecord-count=3\t"),
        "{first_stdout}"
    );

    let second = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-state",
            root.to_str().unwrap(),
            index.to_str().unwrap(),
            state.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let second_stdout = String::from_utf8(second.stdout).unwrap();
    assert_index_security_preflight(&second.stderr);
    assert!(
        second_stdout.contains("\tscan-epoch=2\t"),
        "{second_stdout}"
    );

    let inspect = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index-state-inspect", state.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        inspect.status.success(),
        "{}",
        String::from_utf8_lossy(&inspect.stderr)
    );
    let inspect_stderr = String::from_utf8_lossy(&inspect.stderr);
    assert_worker_admitted(&inspect_stderr, "index state inspect", &state);
    let inspect_stdout = String::from_utf8(inspect.stdout).unwrap();
    assert_eq!(inspect_stdout, second_stdout);

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["search-index", index.to_str().unwrap(), "stateful"])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );
    let search_stdout = String::from_utf8(search_output.stdout).unwrap();
    assert!(
        search_stdout.contains("StatefulSearch.md"),
        "{search_stdout}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(index).unwrap();
    fs::remove_file(state).unwrap();
}

#[test]
fn writes_index_admission_state_from_binary_without_records_crawl() {
    let root = unique_temp_dir("gfm-cli-index-admission-root");
    let index = unique_temp_path("gfm-cli-index-admission-records", "gfmidx");
    let state = unique_temp_path("gfm-cli-index-admission-state", "gfmstate");
    fs::write(root.join("Admission.md"), "stateful admission").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-admission-state",
            "enabled",
            "enabled",
            root.to_str().unwrap(),
            index.to_str().unwrap(),
            state.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!index.exists());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("index-state\t"), "{stdout}");
    assert!(stdout.contains("\trecord-count=0\t"), "{stdout}");
    assert!(stdout.contains("\tinaccessible-count=0\t"), "{stdout}");
    assert!(stdout.contains("\tindex-action="), "{stdout}");
    assert!(stdout.contains("\tnative-status="), "{stdout}");

    let inspect = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index-state-inspect", state.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        inspect.status.success(),
        "{}",
        String::from_utf8_lossy(&inspect.stderr)
    );
    assert_eq!(String::from_utf8(inspect.stdout).unwrap(), stdout);

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(state).unwrap();
}

#[test]
fn index_state_inspect_rejects_duplicate_state_fields_from_binary() {
    let state = unique_temp_path("gfm-cli-index-state-duplicate-field", "gfmstate");
    fs::write(
        &state,
        "gfm-index-state-v1\nschema_version\t1\nroot\t/tmp/root\nrecords_path\t/tmp/index.gfmidx\nvolume_id\t1\nmount_id\tdev:1:root:/tmp/root\nscan_epoch\t1\nscan_epoch\t2\nrecord_count\t1\ninaccessible_count\t0\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index-state-inspect", state.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("index-state\t"), "{stdout}");
    assert!(
        stderr.contains("duplicate index state field `scan_epoch`"),
        "{stderr}"
    );
    assert!(stderr.contains("line 8"), "{stderr}");
    assert_worker_admitted(&stderr, "index state inspect", &state);

    fs::remove_file(state).unwrap();
}

#[test]
fn reports_scan_progress_from_binary() {
    let root = unique_temp_dir("gfm-cli-scan-progress-root");
    let records = unique_temp_path("gfm-cli-scan-progress-records", "gfmidx");
    let progress = unique_temp_path("gfm-cli-scan-progress", "gfmprogress");
    fs::write(root.join("Progress.md"), "alpha").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "scan-progress",
            root.to_str().unwrap(),
            records.to_str().unwrap(),
            progress.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_index_security_preflight(&output.stderr);
    assert!(stdout.starts_with("scan-progress\t"), "{stdout}");
    assert!(stdout.contains("\tcompleted=true"), "{stdout}");

    let inspect = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["scan-progress-inspect", progress.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        inspect.status.success(),
        "{}",
        String::from_utf8_lossy(&inspect.stderr)
    );
    let inspect_stderr = String::from_utf8_lossy(&inspect.stderr);
    assert_worker_admitted(
        &inspect_stderr,
        "scan progress checkpoint inspect",
        &progress,
    );
    assert_eq!(String::from_utf8(inspect.stdout).unwrap(), stdout);

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(progress).unwrap();
}

#[test]
fn reports_fair_scan_from_binary() {
    let root = unique_temp_dir("gfm-cli-fair-scan-root");
    let visible = root.join("Visible");
    let background = root.join("Background");
    fs::create_dir_all(&visible).unwrap();
    fs::create_dir_all(&background).unwrap();
    fs::write(visible.join("Needle.md"), "visible").unwrap();
    fs::write(background.join("Bulk.md"), "background").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "fair-scan",
            root.to_str().unwrap(),
            "2",
            visible.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_index_security_preflight(&output.stderr);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!(
            "security-worker-admission\tworker=index\tpath={}",
            root.display()
        )),
        "{stderr}"
    );
    assert!(stdout.starts_with("fair-scan\t"), "{stdout}");
    assert!(stdout.contains("\tvisible-records="), "{stdout}");
    assert!(stdout.contains("\tbackground-records="), "{stdout}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn fair_scan_refuses_unreachable_visible_root_before_worker_from_binary() {
    let root = unique_temp_dir("gfm-cli-fair-scan-visible-root");
    let offline = unique_temp_dir("gfm-cli-fair-scan-visible-offline");
    fs::write(root.join("Background.md"), "background").unwrap();
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(offline.join("Visible.md"), "visible").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "fair-scan",
            root.to_str().unwrap(),
            "2",
            offline.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("fair-scan\t"), "{stdout}");
    assert!(
        stderr.contains("index volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=index\tpath={}",
            root.display()
        )),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn reports_rename_correlation_from_binary() {
    let root = unique_temp_dir("gfm-cli-rename-root");
    let from = root.join("RenameOld.md");
    let to = root.join("RenameNew.md");
    fs::write(&from, "rename identity").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "rename-correlation",
            from.to_str().unwrap(),
            to.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_index_security_preflight(&output.stderr);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!(
            "security-worker-admission\tworker=index\tpath={}",
            root.display()
        )),
        "{stderr}"
    );
    assert!(stdout.starts_with("rename-correlation\t"), "{stdout}");
    assert!(stdout.contains("\tremoved=1\t"), "{stdout}");
    assert!(stdout.contains("\tinserted=1\t"), "{stdout}");
    assert!(stdout.contains("\tpreserved=1"), "{stdout}");
    assert!(!from.exists());
    assert!(to.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rename_correlation_refuses_unreachable_destination_before_indexing_from_binary() {
    let root = unique_temp_dir("gfm-cli-rename-preflight-root");
    let offline = unique_temp_dir("gfm-cli-rename-preflight-offline");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let from = root.join("RenameOld.md");
    let to = offline.join("RenameNew.md");
    fs::write(&from, "rename identity").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "rename-correlation",
            from.to_str().unwrap(),
            to.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("rename-correlation\t"), "{stdout}");
    assert!(
        stderr.contains(
            "rename correlation destination volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=index\tpath={}",
            root.display()
        )),
        "{stderr}"
    );
    assert_eq!(fs::read_to_string(&from).unwrap(), "rename identity");
    assert!(!to.exists());

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn reports_metadata_update_from_binary() {
    let root = unique_temp_dir("gfm-cli-metadata-root");
    let path = root.join("Metadata.md");
    fs::write(&path, "metadata").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "metadata-update",
            path.to_str().unwrap(),
            " appended content",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_index_security_preflight(&output.stderr);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!(
            "security-worker-admission\tworker=index\tpath={}",
            root.display()
        )),
        "{stderr}"
    );
    assert!(stdout.starts_with("metadata-update\t"), "{stdout}");
    assert!(stdout.contains("\texisted=true\t"), "{stdout}");
    assert!(stdout.contains("size"), "{stdout}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn metadata_update_refuses_unreachable_write_before_appending_from_binary() {
    let root = unique_temp_dir("gfm-cli-metadata-preflight-root");
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let path = root.join("Metadata.md");
    fs::write(&path, "metadata").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "metadata-update",
            path.to_str().unwrap(),
            " appended content",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("metadata-update\t"), "{stdout}");
    assert!(
        stderr.contains("metadata update volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=index\tpath={}",
            root.display()
        )),
        "{stderr}"
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "metadata");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reports_event_backpressure_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["event-backpressure", "5", "2", "8", "2"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("event-backpressure\t"), "{stdout}");
    assert!(stdout.contains("\tvisible=2\t"), "{stdout}");
    assert!(stdout.contains("repair-required=true"), "{stdout}");
}

#[test]
fn persists_fsevents_cursor_from_binary() {
    let root = unique_temp_dir("gfm-cli-fsevents-root");
    let index = unique_temp_path("gfm-cli-fsevents-records", "gfmidx");
    let state = unique_temp_path("gfm-cli-fsevents-state", "gfmstate");
    let cursor = unique_temp_path("gfm-cli-fsevents-cursor", "gfmcursor");
    fs::write(root.join("CursorSearch.md"), "alpha").unwrap();

    let index_state = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-state",
            root.to_str().unwrap(),
            index.to_str().unwrap(),
            state.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        index_state.status.success(),
        "{}",
        String::from_utf8_lossy(&index_state.stderr)
    );

    let checkpoint = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "fsevents-cursor-checkpoint",
            state.to_str().unwrap(),
            cursor.to_str().unwrap(),
            "123",
        ])
        .output()
        .unwrap();
    assert!(
        checkpoint.status.success(),
        "{}",
        String::from_utf8_lossy(&checkpoint.stderr)
    );
    let checkpoint_stdout = String::from_utf8(checkpoint.stdout).unwrap();
    assert!(
        checkpoint_stdout.starts_with("fsevents-cursor\t"),
        "{checkpoint_stdout}"
    );
    assert!(
        checkpoint_stdout.contains("\tlast-event-id=123\thealth=clean"),
        "{checkpoint_stdout}"
    );

    let inspect = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["fsevents-cursor-inspect", cursor.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        inspect.status.success(),
        "{}",
        String::from_utf8_lossy(&inspect.stderr)
    );
    let inspect_stderr = String::from_utf8_lossy(&inspect.stderr);
    assert_worker_admitted(&inspect_stderr, "fsevents cursor inspect", &cursor);
    let inspect_stdout = String::from_utf8(inspect.stdout).unwrap();
    assert_eq!(inspect_stdout, checkpoint_stdout);

    let resume = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "fsevents-cursor-resume",
            state.to_str().unwrap(),
            cursor.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        resume.status.success(),
        "{}",
        String::from_utf8_lossy(&resume.stderr)
    );
    let resume_stderr = String::from_utf8_lossy(&resume.stderr);
    assert_worker_admitted(&resume_stderr, "fsevents cursor resume state", &state);
    assert_worker_admitted(&resume_stderr, "fsevents cursor resume", &cursor);
    let resume_stdout = String::from_utf8(resume.stdout).unwrap();
    assert_eq!(
        resume_stdout.trim(),
        "fsevents-resume\taction=continue\tfrom-event-id=124\treason=cursor-clean"
    );

    let reindex = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-state",
            root.to_str().unwrap(),
            index.to_str().unwrap(),
            state.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        reindex.status.success(),
        "{}",
        String::from_utf8_lossy(&reindex.stderr)
    );
    let stale = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "fsevents-cursor-resume",
            state.to_str().unwrap(),
            cursor.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        stale.status.success(),
        "{}",
        String::from_utf8_lossy(&stale.stderr)
    );
    let stale_stdout = String::from_utf8(stale.stdout).unwrap();
    assert_eq!(
        stale_stdout.trim(),
        "fsevents-resume\taction=rescan\tfrom-event-id=-\treason=scan-epoch-changed"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(index).unwrap();
    fs::remove_file(state).unwrap();
    fs::remove_file(cursor).unwrap();
}

#[test]
fn fsevents_cursor_checkpoint_surfaces_cursor_probe_failure_before_state_read_from_binary() {
    let root = unique_temp_dir("gfm-cli-fsevents-cursor-probe-root");
    let state = root.join("state.gfmstate");
    let cursor = root.join(format!("{}.gfmcursor", "cursor-unavailable".repeat(16)));
    fs::write(&state, "state is not parsed after cursor probe failure\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "fsevents-cursor-checkpoint",
            state.to_str().unwrap(),
            cursor.to_str().unwrap(),
            "123",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains("index write path metadata unavailable"),
        "{stderr}"
    );
    assert!(stderr.contains("cursor-unavailable"), "{stderr}");
    assert!(
        !stderr.contains("security-worker-admission\tworker=fsevents cursor checkpoint state\t"),
        "{stderr}"
    );
    assert_eq!(
        fs::read_to_string(&state).unwrap(),
        "state is not parsed after cursor probe failure\n"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn schedules_fsevents_repair_from_binary() {
    let root = unique_temp_dir("gfm-cli-repair-root");
    let index = unique_temp_path("gfm-cli-repair-records", "gfmidx");
    let state = unique_temp_path("gfm-cli-repair-state", "gfmstate");
    let cursor = unique_temp_path("gfm-cli-repair-cursor", "gfmcursor");
    fs::create_dir_all(root.join("Projects").join("Nested")).unwrap();
    fs::write(
        root.join("Projects").join("Nested").join("Repair.md"),
        "alpha",
    )
    .unwrap();

    let index_state = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-state",
            root.to_str().unwrap(),
            index.to_str().unwrap(),
            state.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        index_state.status.success(),
        "{}",
        String::from_utf8_lossy(&index_state.stderr)
    );

    let checkpoint = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "fsevents-cursor-checkpoint",
            state.to_str().unwrap(),
            cursor.to_str().unwrap(),
            "200",
        ])
        .output()
        .unwrap();
    assert!(
        checkpoint.status.success(),
        "{}",
        String::from_utf8_lossy(&checkpoint.stderr)
    );

    let gap = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "fsevents-repair-schedule",
            state.to_str().unwrap(),
            cursor.to_str().unwrap(),
            "201,204",
            "-",
        ])
        .output()
        .unwrap();
    assert!(
        gap.status.success(),
        "{}",
        String::from_utf8_lossy(&gap.stderr)
    );
    let gap_stderr = String::from_utf8_lossy(&gap.stderr);
    assert_worker_admitted(&gap_stderr, "fsevents repair schedule state", &state);
    assert_worker_admitted(&gap_stderr, "fsevents repair schedule cursor", &cursor);
    let gap_stdout = String::from_utf8(gap.stdout).unwrap();
    assert!(gap_stdout.starts_with("repair-schedule\t"), "{gap_stdout}");
    assert!(gap_stdout.contains("\tjobs=1\t"), "{gap_stdout}");
    assert!(
        gap_stdout.contains("reason=event-id-gap:202-204"),
        "{gap_stdout}"
    );

    let explicit = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "fsevents-repair-schedule",
            state.to_str().unwrap(),
            cursor.to_str().unwrap(),
            "201",
            "kernel-dropped",
            root.join("Projects").to_str().unwrap(),
            root.join("Projects").join("Nested").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        explicit.status.success(),
        "{}",
        String::from_utf8_lossy(&explicit.stderr)
    );
    let explicit_stderr = String::from_utf8_lossy(&explicit.stderr);
    assert_worker_admitted(&explicit_stderr, "fsevents repair schedule state", &state);
    assert_worker_admitted(&explicit_stderr, "fsevents repair schedule cursor", &cursor);
    assert_worker_admitted(
        &explicit_stderr,
        "fsevents repair schedule dropped root",
        &root.join("Projects"),
    );
    assert_worker_admitted(
        &explicit_stderr,
        "fsevents repair schedule dropped root",
        &root.join("Projects").join("Nested"),
    );
    let explicit_stdout = String::from_utf8(explicit.stdout).unwrap();
    assert!(explicit_stdout.contains("\tjobs=1\t"), "{explicit_stdout}");
    assert!(
        explicit_stdout.contains("reason=explicit-drop:kernel-dropped"),
        "{explicit_stdout}"
    );
    assert!(explicit_stdout.contains("Projects"), "{explicit_stdout}");

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(index).unwrap();
    fs::remove_file(state).unwrap();
    fs::remove_file(cursor).unwrap();
}

#[test]
fn fsevents_repair_schedule_retries_transient_failure_from_binary() {
    let root = unique_temp_dir("gfm-cli-repair-retry-root");
    let index = unique_temp_path("gfm-cli-repair-retry-records", "gfmidx");
    let state = unique_temp_path("gfm-cli-repair-retry-state", "gfmstate");
    let cursor = unique_temp_path("gfm-cli-repair-retry-cursor", "gfmcursor");
    let journal = unique_temp_path("gfm-cli-repair-retry", "journal");
    let catalog = unique_temp_path("gfm-cli-repair-retry", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-repair-retry", "gfmprogress");
    let retry_probe = unique_temp_path("gfm-cli-repair-retry", "state");
    fs::write(root.join("RepairRetry.md"), "repair retry").unwrap();

    let index_state = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-state",
            root.to_str().unwrap(),
            index.to_str().unwrap(),
            state.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        index_state.status.success(),
        "{}",
        String::from_utf8_lossy(&index_state.stderr)
    );

    let checkpoint = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "fsevents-cursor-checkpoint",
            state.to_str().unwrap(),
            cursor.to_str().unwrap(),
            "200",
        ])
        .output()
        .unwrap();
    assert!(
        checkpoint.status.success(),
        "{}",
        String::from_utf8_lossy(&checkpoint.stderr)
    );

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "fsevents-repair-schedule-retry-probe",
            state.to_str().unwrap(),
            cursor.to_str().unwrap(),
            "201,204",
            "-",
            retry_probe.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("repair-schedule\t"), "{stdout}");
    assert!(stdout.contains("\tjobs=1\t"), "{stdout}");
    assert!(stdout.contains("reason=event-id-gap:202-204"), "{stdout}");
    assert_eq!(fs::read_to_string(&retry_probe).unwrap(), "2");
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(
        journal_text.contains("1\t1\tstarted\tfsevents repair schedule"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains(
            "1\t1\tfailed:temporary fsevents repair schedule retry probe busy\tfsevents repair schedule"
        ),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tstarted\tfsevents repair schedule"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tcompleted\tfsevents repair schedule"),
        "{journal_text}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(index).unwrap();
    fs::remove_file(state).unwrap();
    fs::remove_file(cursor).unwrap();
    fs::remove_file(journal).unwrap();
    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
    fs::remove_file(retry_probe).unwrap();
}

#[test]
fn fsevents_repair_schedule_refuses_unreachable_dropped_root_before_reading_state_from_binary() {
    let local = unique_temp_dir("gfm-cli-fsevents-repair-local");
    let offline = unique_temp_dir("gfm-cli-fsevents-repair-dropped-unreachable");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let state = local.join("state.gfmstate");
    let cursor = local.join("cursor.gfmcursor");
    fs::write(
        &state,
        "state is not parsed after dropped root access denial\n",
    )
    .unwrap();
    fs::write(
        &cursor,
        "cursor is not parsed after dropped root access denial\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "fsevents-repair-schedule",
            state.to_str().unwrap(),
            cursor.to_str().unwrap(),
            "201",
            "kernel-dropped",
            offline.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains(
            "fsevents repair schedule dropped root volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=fsevents repair schedule state\tpath={}",
            state.display()
        )),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=fsevents repair schedule dropped root\tpath={}",
            offline.display()
        )),
        "{stderr}"
    );

    fs::remove_dir_all(local).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn fsevents_repair_schedule_surfaces_dropped_root_probe_failure_before_reading_state_from_binary() {
    let local = unique_temp_dir("gfm-cli-fsevents-repair-dropped-probe-local");
    let state = local.join("state.gfmstate");
    let cursor = local.join("cursor.gfmcursor");
    let dropped = local.join("dropped-root-unavailable".repeat(16));
    fs::write(
        &state,
        "state is not parsed after dropped root probe failure\n",
    )
    .unwrap();
    fs::write(
        &cursor,
        "cursor is not parsed after dropped root probe failure\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "fsevents-repair-schedule",
            state.to_str().unwrap(),
            cursor.to_str().unwrap(),
            "201",
            "kernel-dropped",
            dropped.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains("fsevents repair dropped root existence unavailable"),
        "{stderr}"
    );
    assert!(stderr.contains("dropped-root-unavailable"), "{stderr}");
    assert!(
        !stderr.contains("security-worker-admission\tworker=fsevents repair schedule state\t"),
        "{stderr}"
    );
    assert_eq!(
        fs::read_to_string(&state).unwrap(),
        "state is not parsed after dropped root probe failure\n"
    );
    assert_eq!(
        fs::read_to_string(&cursor).unwrap(),
        "cursor is not parsed after dropped root probe failure\n"
    );

    fs::remove_dir_all(local).unwrap();
}

#[test]
fn fsevents_repair_schedule_ignores_vanished_dropped_root_after_volume_gate() {
    let root = unique_temp_dir("gfm-cli-fsevents-repair-vanished-root");
    let index = unique_temp_path("gfm-cli-fsevents-repair-vanished-records", "gfmidx");
    let state = unique_temp_path("gfm-cli-fsevents-repair-vanished-state", "gfmstate");
    let cursor = unique_temp_path("gfm-cli-fsevents-repair-vanished-cursor", "gfmcursor");
    fs::write(root.join("Repair.md"), "alpha").unwrap();
    let vanished = root.join("AlreadyGone");

    let index_state = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-state",
            root.to_str().unwrap(),
            index.to_str().unwrap(),
            state.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        index_state.status.success(),
        "{}",
        String::from_utf8_lossy(&index_state.stderr)
    );

    let checkpoint = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "fsevents-cursor-checkpoint",
            state.to_str().unwrap(),
            cursor.to_str().unwrap(),
            "200",
        ])
        .output()
        .unwrap();
    assert!(
        checkpoint.status.success(),
        "{}",
        String::from_utf8_lossy(&checkpoint.stderr)
    );

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "fsevents-repair-schedule",
            state.to_str().unwrap(),
            cursor.to_str().unwrap(),
            "201",
            "kernel-dropped",
            vanished.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("repair-schedule\t"), "{stdout}");
    assert!(stdout.contains("\tjobs=0\t"), "{stdout}");
    assert!(!stdout.contains("explicit-drop"), "{stdout}");
    assert!(!vanished.exists());

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(index).unwrap();
    fs::remove_file(state).unwrap();
    fs::remove_file(cursor).unwrap();
}

#[test]
fn searches_with_structured_filters_from_binary() {
    let root = unique_temp_dir("gfm-cli-filter-root");
    fs::create_dir_all(root.join("Desktop").join("Client Work")).unwrap();
    fs::write(
        root.join("Desktop")
            .join("Client Work")
            .join("final notes.md"),
        "approved",
    )
    .unwrap();
    fs::write(
        root.join("Desktop")
            .join("Client Work")
            .join("draft notes.md"),
        "draft",
    )
    .unwrap();
    fs::write(
        root.join("Desktop").join("Client Work").join("notes.pdf"),
        "pdf",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search",
            root.to_str().unwrap(),
            r#""Client Work" notes kind:file ext:md -draft size:>1b"#,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("final notes.md"), "{stdout}");
    assert!(!stdout.contains("draft notes.md"), "{stdout}");
    assert!(!stdout.contains("notes.pdf"), "{stdout}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn searches_with_date_filters_from_binary() {
    let root = unique_temp_dir("gfm-cli-date-filter-root");
    fs::write(root.join("current.md"), "fresh").unwrap();

    let after_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search",
            root.to_str().unwrap(),
            "current ext:md modified:>=2020-01-01",
        ])
        .output()
        .unwrap();
    assert!(
        after_output.status.success(),
        "{}",
        String::from_utf8_lossy(&after_output.stderr)
    );
    let after_stdout = String::from_utf8(after_output.stdout).unwrap();
    assert!(after_stdout.contains("current.md"), "{after_stdout}");

    let before_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search",
            root.to_str().unwrap(),
            "current ext:md modified:<2020-01-01",
        ])
        .output()
        .unwrap();
    assert!(
        before_output.status.success(),
        "{}",
        String::from_utf8_lossy(&before_output.stderr)
    );
    let before_stdout = String::from_utf8(before_output.stdout).unwrap();
    assert!(!before_stdout.contains("current.md"), "{before_stdout}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn searches_with_boolean_groups_from_binary() {
    let root = unique_temp_dir("gfm-cli-boolean-filter-root");
    fs::write(root.join("report.md"), "report").unwrap();
    fs::write(root.join("invoice.md"), "invoice").unwrap();
    fs::write(root.join("draft-report.md"), "draft").unwrap();
    fs::write(root.join("image.png"), "image").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search",
            root.to_str().unwrap(),
            "(report OR invoice) NOT draft ext:md",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("report.md"), "{stdout}");
    assert!(stdout.contains("invoice.md"), "{stdout}");
    assert!(!stdout.contains("draft-report.md"), "{stdout}");
    assert!(!stdout.contains("image.png"), "{stdout}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn streams_hot_then_deep_search_batches_from_binary() {
    let root = unique_temp_dir("gfm-cli-stream-root");
    fs::write(root.join("needle.md"), "hot").unwrap();
    fs::write(root.join("needl"), "fuzzy").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["search-stream", root.to_str().unwrap(), "needle"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let hot = stdout.find("batch\thot").expect(&stdout);
    let deep = stdout.find("batch\tdeep").expect(&stdout);
    assert!(hot < deep, "{stdout}");
    assert!(stdout.contains("needle.md"), "{stdout}");
    assert!(stdout.contains("needl"), "{stdout}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn search_refuses_unreachable_network_volume_before_indexing_from_binary() {
    let root = unique_temp_dir("gfm-cli-search-unreachable-volume");
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(root.join("Needle.md"), "needle should not be indexed").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["search", root.to_str().unwrap(), "needle"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("Needle.md"), "{stdout}");
    assert!(
        stderr.contains("search volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=search\tpath={}",
            root.display()
        )),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn search_stream_refuses_unreachable_network_volume_before_indexing_from_binary() {
    let root = unique_temp_dir("gfm-cli-search-stream-unreachable-volume");
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(root.join("Needle.md"), "needle should not be streamed").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["search-stream", root.to_str().unwrap(), "needle"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("batch\t"), "{stdout}");
    assert!(
        stderr.contains("search stream volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=search stream\tpath={}",
            root.display()
        )),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ui_search_results_contract_runs_through_visible_worker_from_binary() {
    let root = unique_temp_dir("gfm-cli-ui-search-root");
    fs::write(root.join("Needle.md"), "needle").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "ui-search-results-contract",
            root.to_str().unwrap(),
            "Needle",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("Needle.md"), "{stdout}");
    assert!(
        stderr.contains(&format!(
            "security-worker-admission\tworker=ui search\tpath={}\tintent=index",
            root.display()
        )),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ui_search_results_contract_refuses_unreachable_root_before_worker_from_binary() {
    let root = unique_temp_dir("gfm-cli-ui-search-unreachable-volume");
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(root.join("Needle.md"), "needle should not reach ui results").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "ui-search-results-contract",
            root.to_str().unwrap(),
            "Needle",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("Needle.md"), "{stdout}");
    assert!(
        stderr.contains("ui search volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=ui search\tpath={}",
            root.display()
        )),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn search_content_refuses_unreachable_network_volume_before_extracting_from_binary() {
    let root = unique_temp_dir("gfm-cli-search-content-unreachable-volume");
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(root.join("Needle.md"), "needle should not be extracted").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["search-content", root.to_str().unwrap(), "needle"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("Needle.md"), "{stdout}");
    assert!(
        stderr.contains("content search volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=content search\tpath={}",
            root.display()
        )),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn search_content_adaptive_refuses_unreachable_network_volume_before_extracting_from_binary() {
    let root = unique_temp_dir("gfm-cli-search-content-adaptive-unreachable-volume");
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(root.join("Needle.md"), "needle should not be extracted").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-adaptive",
            root.to_str().unwrap(),
            "needle",
            "nominal",
            "nominal",
            "ac",
            "idle",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("Needle.md"), "{stdout}");
    assert!(
        stderr.contains("content search volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=content search\tpath={}",
            root.display()
        )),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reports_package_traversal_policy_from_binary() {
    let root = unique_temp_dir("gfm-cli-package-root");
    fs::create_dir_all(root.join("GFMFixture.app").join("Contents")).unwrap();
    fs::write(
        root.join("GFMFixture.app")
            .join("Contents")
            .join("Info.plist"),
        "plist",
    )
    .unwrap();
    fs::create_dir_all(root.join("Proposal.pages").join("Data")).unwrap();
    fs::write(
        root.join("Proposal.pages").join("Data").join("Index.zip"),
        "zip",
    )
    .unwrap();

    let opaque = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["package-traversal", root.to_str().unwrap(), "opaque"])
        .output()
        .unwrap();
    assert!(
        opaque.status.success(),
        "{}",
        String::from_utf8_lossy(&opaque.stderr)
    );
    let opaque_stdout = String::from_utf8(opaque.stdout).unwrap();
    let opaque_stderr = String::from_utf8_lossy(&opaque.stderr);
    assert!(
        opaque_stderr.contains(&format!(
            "security-worker-admission\tworker=package traversal\tpath={}",
            root.display()
        )),
        "{opaque_stderr}"
    );
    assert!(
        opaque_stdout.contains("package-traversal\tmode=opaque"),
        "{opaque_stdout}"
    );
    assert!(
        opaque_stdout.contains("package\tapplication\tfalse\tGFMFixture.app"),
        "{opaque_stdout}"
    );
    assert!(
        opaque_stdout.contains("package\tdocument-package\tfalse\tProposal.pages"),
        "{opaque_stdout}"
    );

    let traverse = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["package-traversal", root.to_str().unwrap(), "traverse"])
        .output()
        .unwrap();
    assert!(
        traverse.status.success(),
        "{}",
        String::from_utf8_lossy(&traverse.stderr)
    );
    let traverse_stdout = String::from_utf8(traverse.stdout).unwrap();
    let traverse_stderr = String::from_utf8_lossy(&traverse.stderr);
    assert!(
        traverse_stderr.contains(&format!(
            "security-worker-admission\tworker=package traversal\tpath={}",
            root.display()
        )),
        "{traverse_stderr}"
    );
    assert!(
        traverse_stdout.contains("package-traversal\tmode=traverse"),
        "{traverse_stdout}"
    );
    assert!(
        traverse_stdout.contains("package\tapplication\ttrue\tGFMFixture.app"),
        "{traverse_stdout}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn package_traversal_refuses_unreachable_volume_before_worker_from_binary() {
    let root = unique_temp_dir("gfm-cli-package-unreachable-root");
    fs::create_dir_all(root.join("GFMFixture.app").join("Contents")).unwrap();
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(
        root.join("GFMFixture.app")
            .join("Contents")
            .join("Info.plist"),
        "plist",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["package-traversal", root.to_str().unwrap(), "opaque"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("package-traversal\t"), "{stdout}");
    assert!(
        stderr.contains("package traversal volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=package traversal\tpath={}",
            root.display()
        )),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reports_finder_metadata_from_binary() {
    let root = unique_temp_dir("gfm-cli-finder-metadata");
    let app = root.join("GFMFixture.app");
    fs::create_dir_all(&app).unwrap();

    let app_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("finder-metadata")
        .arg(&app)
        .output()
        .unwrap();
    assert!(
        app_output.status.success(),
        "{}",
        String::from_utf8_lossy(&app_output.stderr)
    );
    let app_stderr = String::from_utf8_lossy(&app_output.stderr);
    assert_worker_admitted(&app_stderr, "finder metadata", &app);
    let app_stdout = String::from_utf8(app_output.stdout).unwrap();
    assert!(app_stdout.starts_with("finder-metadata\t"), "{app_stdout}");
    assert!(
        app_stdout.contains("\tdisplay=GFMFixture\t"),
        "{app_stdout}"
    );
    assert!(app_stdout.contains("\tkind=Application\t"), "{app_stdout}");
    assert!(app_stdout.contains("\ttype=application\t"), "{app_stdout}");
    assert!(app_stdout.contains("\text-hidden=true\t"), "{app_stdout}");

    let target = root.join("target.txt");
    let link = root.join("target link");
    fs::write(&target, "target").unwrap();
    make_symlink(&target, &link);
    let link_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("finder-metadata")
        .arg(&link)
        .output()
        .unwrap();
    assert!(
        link_output.status.success(),
        "{}",
        String::from_utf8_lossy(&link_output.stderr)
    );
    let link_stderr = String::from_utf8_lossy(&link_output.stderr);
    assert_worker_admitted(&link_stderr, "finder metadata", &link);
    let link_stdout = String::from_utf8(link_output.stdout).unwrap();
    assert!(link_stdout.contains("\tkind=Alias\t"), "{link_stdout}");
    assert!(link_stdout.contains("\ttype=symlink\t"), "{link_stdout}");
    assert!(link_stdout.contains("\tlink=symlink\t"), "{link_stdout}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn finder_metadata_refuses_unreachable_volume_before_native_read_from_binary() {
    let root = unique_temp_dir("gfm-cli-finder-metadata-unreachable");
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let file = root.join("Report.md");
    fs::write(&file, "metadata blocked").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("finder-metadata")
        .arg(&file)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("finder-metadata\t"), "{stdout}");
    assert!(
        stderr.contains("finder metadata volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=finder metadata\tpath={}",
            file.display()
        )),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn searches_persisted_tags_from_binary() {
    let index = unique_temp_path("gfm-cli-tags", "gfmidx");
    let metadata = unique_temp_path("gfm-cli-tags", "gfmmeta");
    let dictionary = unique_temp_path("gfm-cli-tags", "gfmdict");
    let columns = unique_temp_path("gfm-cli-tags", "gfmcols");
    let column_retry_journal = unique_temp_path("gfm-cli-column-search-retry", "journal");
    let column_retry_catalog = unique_temp_path("gfm-cli-column-search-retry", "gfmjobs");
    let column_retry_progress = unique_temp_path("gfm-cli-column-search-retry", "gfmprogress");
    let column_retry_probe = unique_temp_path("gfm-cli-column-search-retry", "state");
    let prefixes = unique_temp_path("gfm-cli-tags", "gfmprefix");
    let substrings = unique_temp_path("gfm-cli-tags", "gfmsubstr");
    let fuzzy = unique_temp_path("gfm-cli-tags", "gfmfuzzy");
    let content = unique_temp_path("gfm-cli-tags", "gfmcontent");
    let retry_journal = unique_temp_path("gfm-cli-sidecar-search-retry", "journal");
    let retry_catalog = unique_temp_path("gfm-cli-sidecar-search-retry", "gfmjobs");
    let retry_progress = unique_temp_path("gfm-cli-sidecar-search-retry", "gfmprogress");
    let retry_probe = unique_temp_path("gfm-cli-sidecar-search-retry", "state");
    let session_retry_journal = unique_temp_path("gfm-cli-sidecar-search-session-retry", "journal");
    let session_retry_catalog = unique_temp_path("gfm-cli-sidecar-search-session-retry", "gfmjobs");
    let session_retry_progress =
        unique_temp_path("gfm-cli-sidecar-search-session-retry", "gfmprogress");
    let session_retry_probe = unique_temp_path("gfm-cli-sidecar-search-session-retry", "state");
    let budget_retry_journal = unique_temp_path("gfm-cli-sidecar-budget-retry", "journal");
    let budget_retry_catalog = unique_temp_path("gfm-cli-sidecar-budget-retry", "gfmjobs");
    let budget_retry_progress = unique_temp_path("gfm-cli-sidecar-budget-retry", "gfmprogress");
    let budget_retry_probe = unique_temp_path("gfm-cli-sidecar-budget-retry", "state");
    let scope_retry_journal = unique_temp_path("gfm-cli-sidecar-volume-scope-retry", "journal");
    let scope_retry_catalog = unique_temp_path("gfm-cli-sidecar-volume-scope-retry", "gfmjobs");
    let scope_retry_progress =
        unique_temp_path("gfm-cli-sidecar-volume-scope-retry", "gfmprogress");
    let scope_retry_probe = unique_temp_path("gfm-cli-sidecar-volume-scope-retry", "state");
    let metadata_ids_retry_journal = unique_temp_path("gfm-cli-metadata-ids-retry", "journal");
    let metadata_ids_retry_catalog = unique_temp_path("gfm-cli-metadata-ids-retry", "gfmjobs");
    let metadata_ids_retry_progress = unique_temp_path("gfm-cli-metadata-ids-retry", "gfmprogress");
    let metadata_ids_retry_probe = unique_temp_path("gfm-cli-metadata-ids-retry", "state");
    let metadata_block_retry_journal =
        unique_temp_path("gfm-cli-metadata-id-block-retry", "journal");
    let metadata_block_retry_catalog =
        unique_temp_path("gfm-cli-metadata-id-block-retry", "gfmjobs");
    let metadata_block_retry_progress =
        unique_temp_path("gfm-cli-metadata-id-block-retry", "gfmprogress");
    let metadata_block_retry_probe = unique_temp_path("gfm-cli-metadata-id-block-retry", "state");
    let prefix_ids_retry_journal = unique_temp_path("gfm-cli-prefix-ids-retry", "journal");
    let prefix_ids_retry_catalog = unique_temp_path("gfm-cli-prefix-ids-retry", "gfmjobs");
    let prefix_ids_retry_progress = unique_temp_path("gfm-cli-prefix-ids-retry", "gfmprogress");
    let prefix_ids_retry_probe = unique_temp_path("gfm-cli-prefix-ids-retry", "state");
    let prefix_block_retry_journal = unique_temp_path("gfm-cli-prefix-id-block-retry", "journal");
    let prefix_block_retry_catalog = unique_temp_path("gfm-cli-prefix-id-block-retry", "gfmjobs");
    let prefix_block_retry_progress =
        unique_temp_path("gfm-cli-prefix-id-block-retry", "gfmprogress");
    let prefix_block_retry_probe = unique_temp_path("gfm-cli-prefix-id-block-retry", "state");
    let substring_ids_retry_journal = unique_temp_path("gfm-cli-substring-ids-retry", "journal");
    let substring_ids_retry_catalog = unique_temp_path("gfm-cli-substring-ids-retry", "gfmjobs");
    let substring_ids_retry_progress =
        unique_temp_path("gfm-cli-substring-ids-retry", "gfmprogress");
    let substring_ids_retry_probe = unique_temp_path("gfm-cli-substring-ids-retry", "state");
    let substring_block_retry_journal =
        unique_temp_path("gfm-cli-substring-id-block-retry", "journal");
    let substring_block_retry_catalog =
        unique_temp_path("gfm-cli-substring-id-block-retry", "gfmjobs");
    let substring_block_retry_progress =
        unique_temp_path("gfm-cli-substring-id-block-retry", "gfmprogress");
    let substring_block_retry_probe = unique_temp_path("gfm-cli-substring-id-block-retry", "state");
    let dictionary_lookup_retry_journal =
        unique_temp_path("gfm-cli-dictionary-lookup-retry", "journal");
    let dictionary_lookup_retry_catalog =
        unique_temp_path("gfm-cli-dictionary-lookup-retry", "gfmjobs");
    let dictionary_lookup_retry_progress =
        unique_temp_path("gfm-cli-dictionary-lookup-retry", "gfmprogress");
    let dictionary_lookup_retry_probe =
        unique_temp_path("gfm-cli-dictionary-lookup-retry", "state");
    let fuzzy_terms_retry_journal = unique_temp_path("gfm-cli-fuzzy-terms-retry", "journal");
    let fuzzy_terms_retry_catalog = unique_temp_path("gfm-cli-fuzzy-terms-retry", "gfmjobs");
    let fuzzy_terms_retry_progress = unique_temp_path("gfm-cli-fuzzy-terms-retry", "gfmprogress");
    let fuzzy_terms_retry_probe = unique_temp_path("gfm-cli-fuzzy-terms-retry", "state");
    let assert_worker_admitted = |stderr: &[u8], worker: &str, path: &Path| {
        let stderr = String::from_utf8_lossy(stderr);
        assert!(
            stderr.contains(&format!(
                "security-worker-admission\tworker={worker}\tpath={}",
                path.display()
            )),
            "{stderr}"
        );
    };
    let assert_retry_probe = |command: &str,
                              archive: &Path,
                              tail_args: &[&str],
                              worker: &str,
                              expected_stdout: &str,
                              journal: &Path,
                              catalog: &Path,
                              progress: &Path,
                              probe: &Path| {
        let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
            .env("GFM_JOB_JOURNAL", journal)
            .env("GFM_JOB_PAYLOAD_CATALOG", catalog)
            .env("GFM_JOB_PROGRESS_STORE", progress)
            .arg(command)
            .arg(archive)
            .args(tail_args)
            .arg(probe)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8(output.stdout).unwrap(), expected_stdout);
        assert_eq!(fs::read_to_string(probe).unwrap(), "2");
        let journal_text = fs::read_to_string(journal).unwrap();
        assert!(
            journal_text.contains(&format!("1\t1\tstarted\t{worker}")),
            "{journal_text}"
        );
        assert!(
            journal_text.contains(&format!(
                "1\t1\tfailed:temporary {worker} retry probe busy\t{worker}"
            )),
            "{journal_text}"
        );
        assert!(
            journal_text.contains(&format!("1\t2\tcompleted\t{worker}")),
            "{journal_text}"
        );
    };
    fs::write(
        &index,
        "gfm-store-v2\n1\t1\t0\tf\t5\t0\t0\t0\t0\tImportant,Client\t/tmp/tagged.md\n2\t2\t0\tf\t5\t0\t0\t0\t0\tLater\t/tmp/other.md\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["search-index", index.to_str().unwrap(), "tag:Important"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("tagged.md"), "{stdout}");
    assert!(!stdout.contains("other.md"), "{stdout}");

    let metadata_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-metadata",
            index.to_str().unwrap(),
            metadata.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        metadata_output.status.success(),
        "{}",
        String::from_utf8_lossy(&metadata_output.stderr)
    );

    let ids_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "metadata-ids-mmap",
            metadata.to_str().unwrap(),
            "tag",
            "Important",
        ])
        .output()
        .unwrap();
    assert!(
        ids_output.status.success(),
        "{}",
        String::from_utf8_lossy(&ids_output.stderr)
    );
    assert_worker_admitted(&ids_output.stderr, "metadata ids mmap", &metadata);
    assert_eq!(String::from_utf8(ids_output.stdout).unwrap(), "1\t1\n");
    assert_retry_probe(
        "metadata-ids-mmap-retry-probe",
        &metadata,
        &["tag", "Important"],
        "metadata ids mmap",
        "1\t1\n",
        &metadata_ids_retry_journal,
        &metadata_ids_retry_catalog,
        &metadata_ids_retry_progress,
        &metadata_ids_retry_probe,
    );

    let block_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "metadata-id-block-mmap",
            metadata.to_str().unwrap(),
            "tag",
            "Important",
            "0",
        ])
        .output()
        .unwrap();
    assert!(
        block_output.status.success(),
        "{}",
        String::from_utf8_lossy(&block_output.stderr)
    );
    assert_worker_admitted(&block_output.stderr, "metadata id block mmap", &metadata);
    assert_eq!(String::from_utf8(block_output.stdout).unwrap(), "1\t1\n");
    assert_retry_probe(
        "metadata-id-block-mmap-retry-probe",
        &metadata,
        &["tag", "Important", "0"],
        "metadata id block mmap",
        "1\t1\n",
        &metadata_block_retry_journal,
        &metadata_block_retry_catalog,
        &metadata_block_retry_progress,
        &metadata_block_retry_probe,
    );

    let verify_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["metadata-verify", metadata.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        verify_output.status.success(),
        "{}",
        String::from_utf8_lossy(&verify_output.stderr)
    );
    assert_worker_admitted(&verify_output.stderr, "metadata verify", &metadata);
    let verify_stdout = String::from_utf8(verify_output.stdout).unwrap();
    assert!(
        verify_stdout.contains("\tchecksum=verified"),
        "{verify_stdout}"
    );

    let dictionary_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-dictionary",
            index.to_str().unwrap(),
            dictionary.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        dictionary_output.status.success(),
        "{}",
        String::from_utf8_lossy(&dictionary_output.stderr)
    );
    let dictionary_stderr = String::from_utf8(dictionary_output.stderr).unwrap();
    assert!(
        dictionary_stderr.contains("dictionary-indexed\tterms=")
            && dictionary_stderr.contains("\tpaths=")
            && dictionary_stderr.contains("\tpath-prefixes=1")
            && dictionary_stderr.contains("\textensions=1")
            && dictionary_stderr.contains("\ttags=3")
            && dictionary_stderr.contains("\tkinds=1")
            && dictionary_stderr.contains("\tmetadata-keys=6"),
        "{dictionary_stderr}"
    );

    let lookup_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "dictionary-lookup",
            dictionary.to_str().unwrap(),
            "Important",
        ])
        .output()
        .unwrap();
    assert!(
        lookup_output.status.success(),
        "{}",
        String::from_utf8_lossy(&lookup_output.stderr)
    );
    assert_worker_admitted(&lookup_output.stderr, "dictionary lookup", &dictionary);
    let lookup_stdout = String::from_utf8(lookup_output.stdout).unwrap();
    assert!(
        lookup_stdout.starts_with("dictionary\tfound\t"),
        "{lookup_stdout}"
    );
    assert_retry_probe(
        "dictionary-lookup-retry-probe",
        &dictionary,
        &["Important"],
        "dictionary lookup",
        &lookup_stdout,
        &dictionary_lookup_retry_journal,
        &dictionary_lookup_retry_catalog,
        &dictionary_lookup_retry_progress,
        &dictionary_lookup_retry_probe,
    );

    let prefix_lookup_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["dictionary-lookup", dictionary.to_str().unwrap(), "/tmp"])
        .output()
        .unwrap();
    assert!(
        prefix_lookup_output.status.success(),
        "{}",
        String::from_utf8_lossy(&prefix_lookup_output.stderr)
    );
    assert_worker_admitted(
        &prefix_lookup_output.stderr,
        "dictionary lookup",
        &dictionary,
    );
    let prefix_lookup_stdout = String::from_utf8(prefix_lookup_output.stdout).unwrap();
    assert!(
        prefix_lookup_stdout.starts_with("dictionary\tfound\t"),
        "{prefix_lookup_stdout}"
    );

    let dictionary_verify = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["dictionary-verify", dictionary.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        dictionary_verify.status.success(),
        "{}",
        String::from_utf8_lossy(&dictionary_verify.stderr)
    );
    assert_worker_admitted(&dictionary_verify.stderr, "dictionary verify", &dictionary);
    let dictionary_verify_stdout = String::from_utf8(dictionary_verify.stdout).unwrap();
    assert!(
        dictionary_verify_stdout.contains("\tchecksum=verified"),
        "{dictionary_verify_stdout}"
    );

    let prefixes_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-prefixes",
            index.to_str().unwrap(),
            prefixes.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        prefixes_output.status.success(),
        "{}",
        String::from_utf8_lossy(&prefixes_output.stderr)
    );

    let prefix_ids = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["prefix-ids-mmap", prefixes.to_str().unwrap(), "tag"])
        .output()
        .unwrap();
    assert!(
        prefix_ids.status.success(),
        "{}",
        String::from_utf8_lossy(&prefix_ids.stderr)
    );
    assert_worker_admitted(&prefix_ids.stderr, "prefix ids mmap", &prefixes);
    assert_eq!(String::from_utf8(prefix_ids.stdout).unwrap(), "1\t1\n");
    assert_retry_probe(
        "prefix-ids-mmap-retry-probe",
        &prefixes,
        &["tag"],
        "prefix ids mmap",
        "1\t1\n",
        &prefix_ids_retry_journal,
        &prefix_ids_retry_catalog,
        &prefix_ids_retry_progress,
        &prefix_ids_retry_probe,
    );

    let prefix_block = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "prefix-id-block-mmap",
            prefixes.to_str().unwrap(),
            "tag",
            "0",
        ])
        .output()
        .unwrap();
    assert!(
        prefix_block.status.success(),
        "{}",
        String::from_utf8_lossy(&prefix_block.stderr)
    );
    assert_worker_admitted(&prefix_block.stderr, "prefix id block mmap", &prefixes);
    assert_eq!(String::from_utf8(prefix_block.stdout).unwrap(), "1\t1\n");
    assert_retry_probe(
        "prefix-id-block-mmap-retry-probe",
        &prefixes,
        &["tag", "0"],
        "prefix id block mmap",
        "1\t1\n",
        &prefix_block_retry_journal,
        &prefix_block_retry_catalog,
        &prefix_block_retry_progress,
        &prefix_block_retry_probe,
    );

    let prefix_verify = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["prefix-verify", prefixes.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        prefix_verify.status.success(),
        "{}",
        String::from_utf8_lossy(&prefix_verify.stderr)
    );
    assert_worker_admitted(&prefix_verify.stderr, "prefix verify", &prefixes);
    let prefix_verify_stdout = String::from_utf8(prefix_verify.stdout).unwrap();
    assert!(
        prefix_verify_stdout.contains("\tchecksum=true"),
        "{prefix_verify_stdout}"
    );

    let substrings_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-substrings",
            index.to_str().unwrap(),
            substrings.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        substrings_output.status.success(),
        "{}",
        String::from_utf8_lossy(&substrings_output.stderr)
    );

    let substring_ids = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["substring-ids-mmap", substrings.to_str().unwrap(), "agg"])
        .output()
        .unwrap();
    assert!(
        substring_ids.status.success(),
        "{}",
        String::from_utf8_lossy(&substring_ids.stderr)
    );
    assert_worker_admitted(&substring_ids.stderr, "substring ids mmap", &substrings);
    assert_eq!(String::from_utf8(substring_ids.stdout).unwrap(), "1\t1\n");
    assert_retry_probe(
        "substring-ids-mmap-retry-probe",
        &substrings,
        &["agg"],
        "substring ids mmap",
        "1\t1\n",
        &substring_ids_retry_journal,
        &substring_ids_retry_catalog,
        &substring_ids_retry_progress,
        &substring_ids_retry_probe,
    );

    let substring_block = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "substring-id-block-mmap",
            substrings.to_str().unwrap(),
            "agg",
            "0",
        ])
        .output()
        .unwrap();
    assert!(
        substring_block.status.success(),
        "{}",
        String::from_utf8_lossy(&substring_block.stderr)
    );
    assert_worker_admitted(
        &substring_block.stderr,
        "substring id block mmap",
        &substrings,
    );
    assert_eq!(String::from_utf8(substring_block.stdout).unwrap(), "1\t1\n");
    assert_retry_probe(
        "substring-id-block-mmap-retry-probe",
        &substrings,
        &["agg", "0"],
        "substring id block mmap",
        "1\t1\n",
        &substring_block_retry_journal,
        &substring_block_retry_catalog,
        &substring_block_retry_progress,
        &substring_block_retry_probe,
    );

    let substring_verify = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["substring-verify", substrings.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        substring_verify.status.success(),
        "{}",
        String::from_utf8_lossy(&substring_verify.stderr)
    );
    assert_worker_admitted(&substring_verify.stderr, "substring verify", &substrings);
    let substring_verify_stdout = String::from_utf8(substring_verify.stdout).unwrap();
    assert!(
        substring_verify_stdout.contains("\tchecksum=true"),
        "{substring_verify_stdout}"
    );

    let fuzzy_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-fuzzy",
            index.to_str().unwrap(),
            fuzzy.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        fuzzy_output.status.success(),
        "{}",
        String::from_utf8_lossy(&fuzzy_output.stderr)
    );

    let fuzzy_terms = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["fuzzy-terms-mmap", fuzzy.to_str().unwrap(), "tagge"])
        .output()
        .unwrap();
    assert!(
        fuzzy_terms.status.success(),
        "{}",
        String::from_utf8_lossy(&fuzzy_terms.stderr)
    );
    assert_worker_admitted(&fuzzy_terms.stderr, "fuzzy terms mmap", &fuzzy);
    assert_eq!(String::from_utf8(fuzzy_terms.stdout).unwrap(), "tagged\n");
    assert_retry_probe(
        "fuzzy-terms-mmap-retry-probe",
        &fuzzy,
        &["tagge"],
        "fuzzy terms mmap",
        "tagged\n",
        &fuzzy_terms_retry_journal,
        &fuzzy_terms_retry_catalog,
        &fuzzy_terms_retry_progress,
        &fuzzy_terms_retry_probe,
    );

    let fuzzy_verify = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["fuzzy-verify", fuzzy.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        fuzzy_verify.status.success(),
        "{}",
        String::from_utf8_lossy(&fuzzy_verify.stderr)
    );
    assert_worker_admitted(&fuzzy_verify.stderr, "fuzzy verify", &fuzzy);
    let fuzzy_verify_stdout = String::from_utf8(fuzzy_verify.stdout).unwrap();
    assert!(
        fuzzy_verify_stdout.contains("\tchecksum=true"),
        "{fuzzy_verify_stdout}"
    );

    let columns_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-columns",
            index.to_str().unwrap(),
            columns.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        columns_output.status.success(),
        "{}",
        String::from_utf8_lossy(&columns_output.stderr)
    );

    let columns_verify = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["columns-verify", columns.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        columns_verify.status.success(),
        "{}",
        String::from_utf8_lossy(&columns_verify.stderr)
    );
    let columns_verify_stdout = String::from_utf8(columns_verify.stdout).unwrap();
    assert!(
        columns_verify_stdout.contains("\tchecksum=verified"),
        "{columns_verify_stdout}"
    );

    let columns_lookup = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["columns-lookup", columns.to_str().unwrap(), "1", "1"])
        .output()
        .unwrap();
    assert!(
        columns_lookup.status.success(),
        "{}",
        String::from_utf8_lossy(&columns_lookup.stderr)
    );
    let columns_lookup_stdout = String::from_utf8(columns_lookup.stdout).unwrap();
    assert!(
        columns_lookup_stdout.contains("\tname=tagged.md\t"),
        "{columns_lookup_stdout}"
    );
    assert!(
        columns_lookup_stdout.contains("\ttags=client,important\t"),
        "{columns_lookup_stdout}"
    );
    write_content_postings(
        &content,
        &[
            ContentPosting {
                term: "bodymarker".to_string(),
                ids: vec![FileId::new(VolumeId(1), 1)],
                positions: Vec::new(),
            },
            ContentPosting {
                term: "coldmarker".to_string(),
                ids: vec![FileId::new(VolumeId(1), 2)],
                positions: Vec::new(),
            },
        ],
    )
    .unwrap();

    let column_search = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-index-columns",
            index.to_str().unwrap(),
            columns.to_str().unwrap(),
            "tag:Important",
        ])
        .output()
        .unwrap();
    assert!(
        column_search.status.success(),
        "{}",
        String::from_utf8_lossy(&column_search.stderr)
    );
    let column_search_stderr = String::from_utf8_lossy(&column_search.stderr);
    assert!(
        column_search_stderr.contains(&format!(
            "security-worker-admission\tworker=search index columns records\tpath={}",
            index.display()
        )) && column_search_stderr.contains(&format!(
            "security-worker-admission\tworker=search index columns columns\tpath={}",
            columns.display()
        )),
        "{column_search_stderr}"
    );
    let column_search_stdout = String::from_utf8(column_search.stdout).unwrap();
    assert!(
        column_search_stdout.contains("tagged.md"),
        "{column_search_stdout}"
    );
    assert!(
        !column_search_stdout.contains("other.md"),
        "{column_search_stdout}"
    );

    let column_retry_search = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &column_retry_journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &column_retry_catalog)
        .env("GFM_JOB_PROGRESS_STORE", &column_retry_progress)
        .args([
            "search-index-columns-retry-probe",
            index.to_str().unwrap(),
            columns.to_str().unwrap(),
            "tag:Important",
            column_retry_probe.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        column_retry_search.status.success(),
        "{}",
        String::from_utf8_lossy(&column_retry_search.stderr)
    );
    let column_retry_stdout = String::from_utf8(column_retry_search.stdout).unwrap();
    assert!(
        column_retry_stdout.contains("tagged.md"),
        "{column_retry_stdout}"
    );
    assert!(
        !column_retry_stdout.contains("other.md"),
        "{column_retry_stdout}"
    );
    assert_eq!(fs::read_to_string(&column_retry_probe).unwrap(), "2");
    let column_retry_journal_text = fs::read_to_string(&column_retry_journal).unwrap();
    assert!(
        column_retry_journal_text.contains("1\t1\tstarted\tsearch index columns"),
        "{column_retry_journal_text}"
    );
    assert!(
        column_retry_journal_text.contains(
            "1\t1\tfailed:temporary search index columns retry probe busy\tsearch index columns"
        ),
        "{column_retry_journal_text}"
    );
    assert!(
        column_retry_journal_text.contains("1\t2\tcompleted\tsearch index columns"),
        "{column_retry_journal_text}"
    );

    let sidecar_search = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-index-sidecars",
            index.to_str().unwrap(),
            columns.to_str().unwrap(),
            metadata.to_str().unwrap(),
            prefixes.to_str().unwrap(),
            substrings.to_str().unwrap(),
            fuzzy.to_str().unwrap(),
            content.to_str().unwrap(),
            "tag:Important",
        ])
        .output()
        .unwrap();
    assert!(
        sidecar_search.status.success(),
        "{}",
        String::from_utf8_lossy(&sidecar_search.stderr)
    );
    let sidecar_search_stdout = String::from_utf8(sidecar_search.stdout).unwrap();
    assert!(
        sidecar_search_stdout.contains("tagged.md"),
        "{sidecar_search_stdout}"
    );
    let sidecar_search_stderr = String::from_utf8(sidecar_search.stderr).unwrap();
    assert!(
        sidecar_search_stderr.contains(&format!(
            "security-worker-admission\tworker=sidecar search records\tpath={}",
            index.display()
        )) && sidecar_search_stderr.contains(&format!(
            "security-worker-admission\tworker=sidecar search content\tpath={}",
            content.display()
        )) && sidecar_search_stderr.contains("metadata-keys 1")
            && sidecar_search_stderr.contains("prefix-keys 0")
            && sidecar_search_stderr.contains("substring-keys 0")
            && sidecar_search_stderr.contains("fuzzy-keys 0")
            && sidecar_search_stderr.contains("prefix-archive-keys")
            && sidecar_search_stderr.contains("substring-archive-keys")
            && sidecar_search_stderr.contains("fuzzy-archive-keys")
            && sidecar_search_stderr.contains("content-keys 0")
            && sidecar_search_stderr.contains("content-cache-hits 0")
            && sidecar_search_stderr.contains("content-cache-misses 0")
            && sidecar_search_stderr.contains("metadata-budget 4096")
            && sidecar_search_stderr.contains("substring-budget 4096")
            && sidecar_search_stderr.contains("content-budget 4096"),
        "{sidecar_search_stderr}"
    );

    let sidecar_prefix_search = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-index-sidecars",
            index.to_str().unwrap(),
            columns.to_str().unwrap(),
            metadata.to_str().unwrap(),
            prefixes.to_str().unwrap(),
            substrings.to_str().unwrap(),
            fuzzy.to_str().unwrap(),
            content.to_str().unwrap(),
            "tag",
        ])
        .output()
        .unwrap();
    assert!(
        sidecar_prefix_search.status.success(),
        "{}",
        String::from_utf8_lossy(&sidecar_prefix_search.stderr)
    );
    let sidecar_prefix_stdout = String::from_utf8(sidecar_prefix_search.stdout).unwrap();
    assert!(
        sidecar_prefix_stdout.contains("tagged.md"),
        "{sidecar_prefix_stdout}"
    );
    let sidecar_prefix_stderr = String::from_utf8(sidecar_prefix_search.stderr).unwrap();
    assert!(
        sidecar_prefix_stderr.contains("prefix-keys 1")
            && sidecar_prefix_stderr.contains("records-loaded 1")
            && sidecar_prefix_stderr.contains("candidate-ids 1")
            && sidecar_prefix_stderr.contains("full-hydration false")
            && sidecar_prefix_stderr.contains("prefix-archive-keys")
            && sidecar_prefix_stderr.contains("substring-archive-keys"),
        "{sidecar_prefix_stderr}"
    );
    let sidecar_budget_search = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-index-sidecars-budget",
            index.to_str().unwrap(),
            columns.to_str().unwrap(),
            metadata.to_str().unwrap(),
            prefixes.to_str().unwrap(),
            substrings.to_str().unwrap(),
            fuzzy.to_str().unwrap(),
            content.to_str().unwrap(),
            "1",
            "1",
            "1",
            "1",
            "1",
            "1",
            "1",
            "tag",
        ])
        .output()
        .unwrap();
    assert!(
        sidecar_budget_search.status.success(),
        "{}",
        String::from_utf8_lossy(&sidecar_budget_search.stderr)
    );
    let sidecar_budget_stderr = String::from_utf8(sidecar_budget_search.stderr).unwrap();
    assert!(
        sidecar_budget_stderr.contains(&format!(
            "security-worker-admission\tworker=sidecar budget records\tpath={}",
            index.display()
        )) && sidecar_budget_stderr.contains(&format!(
            "security-worker-admission\tworker=sidecar budget content\tpath={}",
            content.display()
        )) && sidecar_budget_stderr.contains("sidecar-budget")
            && sidecar_budget_stderr.contains("\trecords-loaded=1")
            && sidecar_budget_stderr.contains("\tcandidate-ids=1")
            && sidecar_budget_stderr.contains("\tfull-hydration=false")
            && sidecar_budget_stderr.contains("\tprefix-keys=1")
            && sidecar_budget_stderr.contains("\tsubstring-keys=1")
            && sidecar_budget_stderr.contains("\tcontent-cache-hits=0")
            && sidecar_budget_stderr.contains("\tcontent-cache-misses=1")
            && sidecar_budget_stderr.contains("\tprefix-terms=1")
            && sidecar_budget_stderr.contains("\tprefix-lookup-requests=")
            && sidecar_budget_stderr.contains("\tprefix-lookup-ids=")
            && sidecar_budget_stderr.contains("\tprefix-candidate-ids=")
            && sidecar_budget_stderr.contains("\tprefix-cache-misses=")
            && sidecar_budget_stderr.contains("\tprefix-cutoff-terms=")
            && sidecar_budget_stderr.contains("\tsubstring-terms=")
            && sidecar_budget_stderr.contains("\tsubstring-lookup-requests=")
            && sidecar_budget_stderr.contains("\tsubstring-cache-misses=")
            && sidecar_budget_stderr.contains("\tfuzzy-terms=1")
            && sidecar_budget_stderr.contains("\tfuzzy-lookup-requests=")
            && sidecar_budget_stderr.contains("\tfuzzy-cache-misses=")
            && sidecar_budget_stderr.contains("\tmetadata-budget=1")
            && sidecar_budget_stderr.contains("\tcontent-budget=1")
            && sidecar_budget_stderr.contains("\tfuzzy-term-truncated-keys=1"),
        "{sidecar_budget_stderr}"
    );

    let sidecar_budget_retry_search = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &budget_retry_journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &budget_retry_catalog)
        .env("GFM_JOB_PROGRESS_STORE", &budget_retry_progress)
        .args([
            "search-index-sidecars-budget-retry-probe",
            index.to_str().unwrap(),
            columns.to_str().unwrap(),
            metadata.to_str().unwrap(),
            prefixes.to_str().unwrap(),
            substrings.to_str().unwrap(),
            fuzzy.to_str().unwrap(),
            content.to_str().unwrap(),
            "1",
            "1",
            "1",
            "1",
            "1",
            "1",
            "1",
            "tag",
            budget_retry_probe.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        sidecar_budget_retry_search.status.success(),
        "{}",
        String::from_utf8_lossy(&sidecar_budget_retry_search.stderr)
    );
    let sidecar_budget_retry_stdout =
        String::from_utf8(sidecar_budget_retry_search.stdout).unwrap();
    assert!(
        sidecar_budget_retry_stdout.contains("tagged.md"),
        "{sidecar_budget_retry_stdout}"
    );
    assert_eq!(fs::read_to_string(&budget_retry_probe).unwrap(), "2");
    let budget_retry_journal_text = fs::read_to_string(&budget_retry_journal).unwrap();
    assert!(
        budget_retry_journal_text.contains("1\t1\tstarted\tsidecar budget"),
        "{budget_retry_journal_text}"
    );
    assert!(
        budget_retry_journal_text
            .contains("1\t1\tfailed:temporary sidecar budget retry probe busy\tsidecar budget"),
        "{budget_retry_journal_text}"
    );
    assert!(
        budget_retry_journal_text.contains("1\t2\tcompleted\tsidecar budget"),
        "{budget_retry_journal_text}"
    );

    let sidecar_content_search = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-index-sidecars",
            index.to_str().unwrap(),
            columns.to_str().unwrap(),
            metadata.to_str().unwrap(),
            prefixes.to_str().unwrap(),
            substrings.to_str().unwrap(),
            fuzzy.to_str().unwrap(),
            content.to_str().unwrap(),
            "bodymarker",
        ])
        .output()
        .unwrap();
    assert!(
        sidecar_content_search.status.success(),
        "{}",
        String::from_utf8_lossy(&sidecar_content_search.stderr)
    );
    let sidecar_content_stdout = String::from_utf8(sidecar_content_search.stdout).unwrap();
    assert!(
        sidecar_content_stdout.contains("tagged.md"),
        "{sidecar_content_stdout}"
    );
    let sidecar_content_stderr = String::from_utf8(sidecar_content_search.stderr).unwrap();
    assert!(
        sidecar_content_stderr.contains("content-keys 1")
            && sidecar_content_stderr.contains("content-cache-hits 0")
            && sidecar_content_stderr.contains("content-cache-misses 1"),
        "{sidecar_content_stderr}"
    );

    let sidecar_retry_search = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &retry_journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &retry_catalog)
        .env("GFM_JOB_PROGRESS_STORE", &retry_progress)
        .args([
            "search-index-sidecars-retry-probe",
            index.to_str().unwrap(),
            columns.to_str().unwrap(),
            metadata.to_str().unwrap(),
            prefixes.to_str().unwrap(),
            substrings.to_str().unwrap(),
            fuzzy.to_str().unwrap(),
            content.to_str().unwrap(),
            "bodymarker",
            retry_probe.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        sidecar_retry_search.status.success(),
        "{}",
        String::from_utf8_lossy(&sidecar_retry_search.stderr)
    );
    let sidecar_retry_stdout = String::from_utf8(sidecar_retry_search.stdout).unwrap();
    assert!(
        sidecar_retry_stdout.contains("tagged.md"),
        "{sidecar_retry_stdout}"
    );
    assert_eq!(fs::read_to_string(&retry_probe).unwrap(), "2");
    let retry_journal_text = fs::read_to_string(&retry_journal).unwrap();
    assert!(
        retry_journal_text.contains("1\t1\tstarted\tsidecar search"),
        "{retry_journal_text}"
    );
    assert!(
        retry_journal_text
            .contains("1\t1\tfailed:temporary sidecar search retry probe busy\tsidecar search"),
        "{retry_journal_text}"
    );
    assert!(
        retry_journal_text.contains("1\t2\tcompleted\tsidecar search"),
        "{retry_journal_text}"
    );

    let sidecar_session_search = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-index-sidecars-session",
            index.to_str().unwrap(),
            columns.to_str().unwrap(),
            metadata.to_str().unwrap(),
            prefixes.to_str().unwrap(),
            substrings.to_str().unwrap(),
            fuzzy.to_str().unwrap(),
            content.to_str().unwrap(),
            "bodymarker",
        ])
        .output()
        .unwrap();
    assert!(
        sidecar_session_search.status.success(),
        "{}",
        String::from_utf8_lossy(&sidecar_session_search.stderr)
    );
    let sidecar_session_stdout = String::from_utf8(sidecar_session_search.stdout).unwrap();
    assert!(
        sidecar_session_stdout.contains("tagged.md"),
        "{sidecar_session_stdout}"
    );
    let sidecar_session_stderr = String::from_utf8(sidecar_session_search.stderr).unwrap();
    assert!(
        sidecar_session_stderr.contains(&format!(
            "security-worker-admission\tworker=sidecar session records\tpath={}",
            index.display()
        )) && sidecar_session_stderr.contains(&format!(
            "security-worker-admission\tworker=sidecar session content\tpath={}",
            content.display()
        )) && sidecar_session_stderr.contains("sidecar-session-first")
            && sidecar_session_stderr.contains("\tcontent-keys=1")
            && sidecar_session_stderr.contains("\tcontent-cache-hits=0")
            && sidecar_session_stderr.contains("\tcontent-cache-misses=1")
            && sidecar_session_stderr.contains("\trecord-cache-hits=0")
            && sidecar_session_stderr.contains("\trecord-cache-misses=1")
            && sidecar_session_stderr.contains("\tresult-cache-hits=0")
            && sidecar_session_stderr.contains("\tresult-cache-misses=1")
            && sidecar_session_stderr.contains("sidecar-session-second")
            && sidecar_session_stderr.contains("\tcontent-cache-hits=0")
            && sidecar_session_stderr.contains("\tcontent-cache-misses=0")
            && sidecar_session_stderr.contains("\trecord-cache-hits=0")
            && sidecar_session_stderr.contains("\trecord-cache-misses=0")
            && sidecar_session_stderr.contains("\tresult-cache-hits=1")
            && sidecar_session_stderr.contains("\tresult-cache-misses=0"),
        "{sidecar_session_stderr}"
    );

    let sidecar_session_retry_search = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &session_retry_journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &session_retry_catalog)
        .env("GFM_JOB_PROGRESS_STORE", &session_retry_progress)
        .args([
            "search-index-sidecars-session-retry-probe",
            index.to_str().unwrap(),
            columns.to_str().unwrap(),
            metadata.to_str().unwrap(),
            prefixes.to_str().unwrap(),
            substrings.to_str().unwrap(),
            fuzzy.to_str().unwrap(),
            content.to_str().unwrap(),
            "bodymarker",
            session_retry_probe.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        sidecar_session_retry_search.status.success(),
        "{}",
        String::from_utf8_lossy(&sidecar_session_retry_search.stderr)
    );
    let sidecar_session_retry_stdout =
        String::from_utf8(sidecar_session_retry_search.stdout).unwrap();
    assert!(
        sidecar_session_retry_stdout.contains("tagged.md"),
        "{sidecar_session_retry_stdout}"
    );
    assert_eq!(fs::read_to_string(&session_retry_probe).unwrap(), "2");
    let session_retry_journal_text = fs::read_to_string(&session_retry_journal).unwrap();
    assert!(
        session_retry_journal_text.contains("1\t1\tstarted\tsidecar session"),
        "{session_retry_journal_text}"
    );
    assert!(
        session_retry_journal_text
            .contains("1\t1\tfailed:temporary sidecar session retry probe busy\tsidecar session"),
        "{session_retry_journal_text}"
    );
    assert!(
        session_retry_journal_text.contains("1\t2\tcompleted\tsidecar session"),
        "{session_retry_journal_text}"
    );

    let sidecar_scoped_search = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-index-sidecars-volume-scope",
            index.to_str().unwrap(),
            columns.to_str().unwrap(),
            metadata.to_str().unwrap(),
            prefixes.to_str().unwrap(),
            substrings.to_str().unwrap(),
            fuzzy.to_str().unwrap(),
            content.to_str().unwrap(),
            "1",
            "other",
        ])
        .output()
        .unwrap();
    assert!(
        sidecar_scoped_search.status.success(),
        "{}",
        String::from_utf8_lossy(&sidecar_scoped_search.stderr)
    );
    let sidecar_scoped_stdout = String::from_utf8(sidecar_scoped_search.stdout).unwrap();
    assert!(sidecar_scoped_stdout.is_empty(), "{sidecar_scoped_stdout}");
    let sidecar_scoped_stderr = String::from_utf8(sidecar_scoped_search.stderr).unwrap();
    assert!(
        sidecar_scoped_stderr.contains(&format!(
            "security-worker-admission\tworker=sidecar volume scope records\tpath={}",
            index.display()
        )) && sidecar_scoped_stderr.contains(&format!(
            "security-worker-admission\tworker=sidecar volume scope content\tpath={}",
            content.display()
        )) && sidecar_scoped_stderr.contains("sidecar-volume-scope")
            && sidecar_scoped_stderr.contains("\trecords-loaded=0")
            && sidecar_scoped_stderr.contains("\tcandidate-ids=0")
            && sidecar_scoped_stderr.contains("\tfull-hydration=false")
            && sidecar_scoped_stderr.contains("\tcontent-cache-misses=1")
            && sidecar_scoped_stderr.contains("\trecord-cache-misses=0"),
        "{sidecar_scoped_stderr}"
    );

    let sidecar_scoped_retry_search = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &scope_retry_journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &scope_retry_catalog)
        .env("GFM_JOB_PROGRESS_STORE", &scope_retry_progress)
        .args([
            "search-index-sidecars-volume-scope-retry-probe",
            index.to_str().unwrap(),
            columns.to_str().unwrap(),
            metadata.to_str().unwrap(),
            prefixes.to_str().unwrap(),
            substrings.to_str().unwrap(),
            fuzzy.to_str().unwrap(),
            content.to_str().unwrap(),
            "1",
            "bodymarker",
            scope_retry_probe.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        sidecar_scoped_retry_search.status.success(),
        "{}",
        String::from_utf8_lossy(&sidecar_scoped_retry_search.stderr)
    );
    let sidecar_scoped_retry_stdout =
        String::from_utf8(sidecar_scoped_retry_search.stdout).unwrap();
    assert!(
        sidecar_scoped_retry_stdout.contains("tagged.md"),
        "{sidecar_scoped_retry_stdout}"
    );
    assert_eq!(fs::read_to_string(&scope_retry_probe).unwrap(), "2");
    let scope_retry_journal_text = fs::read_to_string(&scope_retry_journal).unwrap();
    assert!(
        scope_retry_journal_text.contains("1\t1\tstarted\tsidecar volume scope"),
        "{scope_retry_journal_text}"
    );
    assert!(
        scope_retry_journal_text.contains(
            "1\t1\tfailed:temporary sidecar volume scope retry probe busy\tsidecar volume scope"
        ),
        "{scope_retry_journal_text}"
    );
    assert!(
        scope_retry_journal_text.contains("1\t2\tcompleted\tsidecar volume scope"),
        "{scope_retry_journal_text}"
    );

    let sidecar_empty_scope = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-index-sidecars-volume-scope",
            index.to_str().unwrap(),
            columns.to_str().unwrap(),
            metadata.to_str().unwrap(),
            prefixes.to_str().unwrap(),
            substrings.to_str().unwrap(),
            fuzzy.to_str().unwrap(),
            content.to_str().unwrap(),
            "-",
            "bodymarker",
        ])
        .output()
        .unwrap();
    assert!(
        sidecar_empty_scope.status.success(),
        "{}",
        String::from_utf8_lossy(&sidecar_empty_scope.stderr)
    );
    let sidecar_empty_scope_stdout = String::from_utf8(sidecar_empty_scope.stdout).unwrap();
    assert!(
        sidecar_empty_scope_stdout.is_empty(),
        "{sidecar_empty_scope_stdout}"
    );
    let sidecar_empty_scope_stderr = String::from_utf8(sidecar_empty_scope.stderr).unwrap();
    assert!(
        sidecar_empty_scope_stderr.contains("sidecar-volume-scope")
            && sidecar_empty_scope_stderr.contains("\trecords-loaded=0")
            && sidecar_empty_scope_stderr.contains("\tcandidate-ids=0")
            && sidecar_empty_scope_stderr.contains("\tcontent-cache-misses=0")
            && sidecar_empty_scope_stderr.contains("\tprefix-lookup-requests=0")
            && sidecar_empty_scope_stderr.contains("\tsubstring-lookup-requests=0"),
        "{sidecar_empty_scope_stderr}"
    );

    fs::remove_file(index).unwrap();
    fs::remove_file(metadata).unwrap();
    fs::remove_file(dictionary).unwrap();
    fs::remove_file(columns).unwrap();
    fs::remove_file(column_retry_journal).unwrap();
    fs::remove_file(column_retry_catalog).unwrap();
    fs::remove_file(column_retry_progress).unwrap();
    fs::remove_file(column_retry_probe).unwrap();
    fs::remove_file(prefixes).unwrap();
    fs::remove_file(substrings).unwrap();
    fs::remove_file(fuzzy).unwrap();
    fs::remove_file(content).unwrap();
    fs::remove_file(retry_journal).unwrap();
    fs::remove_file(retry_catalog).unwrap();
    fs::remove_file(retry_progress).unwrap();
    fs::remove_file(retry_probe).unwrap();
    fs::remove_file(session_retry_journal).unwrap();
    fs::remove_file(session_retry_catalog).unwrap();
    fs::remove_file(session_retry_progress).unwrap();
    fs::remove_file(session_retry_probe).unwrap();
    fs::remove_file(budget_retry_journal).unwrap();
    fs::remove_file(budget_retry_catalog).unwrap();
    fs::remove_file(budget_retry_progress).unwrap();
    fs::remove_file(budget_retry_probe).unwrap();
    fs::remove_file(scope_retry_journal).unwrap();
    fs::remove_file(scope_retry_catalog).unwrap();
    fs::remove_file(scope_retry_progress).unwrap();
    fs::remove_file(scope_retry_probe).unwrap();
    for path in [
        metadata_ids_retry_journal,
        metadata_ids_retry_catalog,
        metadata_ids_retry_progress,
        metadata_ids_retry_probe,
        metadata_block_retry_journal,
        metadata_block_retry_catalog,
        metadata_block_retry_progress,
        metadata_block_retry_probe,
        prefix_ids_retry_journal,
        prefix_ids_retry_catalog,
        prefix_ids_retry_progress,
        prefix_ids_retry_probe,
        prefix_block_retry_journal,
        prefix_block_retry_catalog,
        prefix_block_retry_progress,
        prefix_block_retry_probe,
        substring_ids_retry_journal,
        substring_ids_retry_catalog,
        substring_ids_retry_progress,
        substring_ids_retry_probe,
        substring_block_retry_journal,
        substring_block_retry_catalog,
        substring_block_retry_progress,
        substring_block_retry_probe,
        dictionary_lookup_retry_journal,
        dictionary_lookup_retry_catalog,
        dictionary_lookup_retry_progress,
        dictionary_lookup_retry_probe,
        fuzzy_terms_retry_journal,
        fuzzy_terms_retry_catalog,
        fuzzy_terms_retry_progress,
        fuzzy_terms_retry_probe,
    ] {
        fs::remove_file(path).unwrap();
    }
}

#[test]
fn reports_cancelled_sidecar_candidate_expansion_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("search-index-sidecars-cancel-candidates")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout,
        "search-candidate-expansion\tstatus=cancelled\treason=cancelled-before-candidate-expansion\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn reports_cancelled_search_query_parse_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("search-query-cancel-parse")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout,
        "search-query-parse\tstatus=cancelled\treason=cancelled-before-parse\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn archive_sidecar_write_reports_output_probe_failure_before_indexing_from_binary() {
    let root = unique_temp_dir("gfm-cli-archive-sidecar-write-probe");
    let records = root.join("records.gfmidx");
    let output = root.join(format!(
        "{}.gfmmeta",
        "archive-output-unavailable".repeat(16)
    ));
    fs::write(
        &records,
        "gfm-store-v2\n1\t1\t0\tf\t5\t0\t0\t0\t0\tImportant\t/tmp/tagged.md\n",
    )
    .unwrap();

    let output_result = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-metadata",
            records.to_str().unwrap(),
            output.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output_result.status.success());
    let stdout = String::from_utf8_lossy(&output_result.stdout);
    let stderr = String::from_utf8_lossy(&output_result.stderr);
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains("archive write path metadata unavailable"),
        "{stderr}"
    );
    assert!(stderr.contains("archive-output-unavailable"), "{stderr}");
    assert!(
        !stderr.contains("security-worker-admission\tworker=index metadata output\t"),
        "{stderr}"
    );
    assert!(!output.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn empty_volume_scope_sidecar_search_skips_archive_access_from_binary() {
    let root = unique_temp_dir("gfm-empty-scope-sidecar-access");

    let records = root.join("missing.records");
    let columns = root.join("missing.columns");
    let metadata = root.join("missing.metadata");
    let prefixes = root.join("missing.prefixes");
    let substrings = root.join("missing.substrings");
    let fuzzy = root.join("missing.fuzzy");
    let content = root.join("missing.content");
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-index-sidecars-volume-scope",
            records.to_str().unwrap(),
            columns.to_str().unwrap(),
            metadata.to_str().unwrap(),
            prefixes.to_str().unwrap(),
            substrings.to_str().unwrap(),
            fuzzy.to_str().unwrap(),
            content.to_str().unwrap(),
            "-",
            "bodymarker",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.is_empty(), "{stdout}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("sidecar-volume-scope"), "{stderr}");
    assert!(!stderr.contains("security-worker-admission\t"), "{stderr}");
    assert!(stderr.contains("\trecords-indexed=0\t"), "{stderr}");
    assert!(stderr.contains("\tcontent-cache-misses=0\t"), "{stderr}");
    assert!(stderr.contains("\tfuzzy-lookup-requests=0\t"), "{stderr}");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn direct_search_archives_refuse_unreachable_volume_before_mapping_from_binary() {
    let offline = unique_temp_dir("gfm-cli-direct-search-preflight-offline");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();

    let cases = [
        (
            "search-index",
            "gfmidx",
            vec!["needle"],
            "search index",
            "hit\t",
        ),
        (
            "search-index-mmap",
            "gfmidx",
            vec!["needle"],
            "search index mmap",
            "hit\t",
        ),
        (
            "fuzzy-terms-mmap",
            "gfmfuzzy",
            vec!["key"],
            "fuzzy terms mmap",
            "fuzzy-verify\t",
        ),
        (
            "fuzzy-verify",
            "gfmfuzzy",
            vec![],
            "fuzzy verify",
            "fuzzy-verify\t",
        ),
        (
            "prefix-ids-mmap",
            "gfmprefix",
            vec!["tag"],
            "prefix ids mmap",
            "1\t",
        ),
        (
            "prefix-id-block-mmap",
            "gfmprefix",
            vec!["tag", "0"],
            "prefix id block mmap",
            "1\t",
        ),
        (
            "prefix-verify",
            "gfmprefix",
            vec![],
            "prefix verify",
            "prefix-verify\t",
        ),
        (
            "substring-ids-mmap",
            "gfmsubstr",
            vec!["tag"],
            "substring ids mmap",
            "1\t",
        ),
        (
            "substring-id-block-mmap",
            "gfmsubstr",
            vec!["tag", "0"],
            "substring id block mmap",
            "1\t",
        ),
        (
            "substring-verify",
            "gfmsubstr",
            vec![],
            "substring verify",
            "substring-verify\t",
        ),
        (
            "dictionary-lookup",
            "gfmdict",
            vec!["tag"],
            "dictionary lookup",
            "dictionary\t",
        ),
        (
            "dictionary-verify",
            "gfmdict",
            vec![],
            "dictionary verify",
            "dictionary-verify\t",
        ),
        (
            "metadata-ids-mmap",
            "gfmmeta",
            vec!["tag", "Important"],
            "metadata ids mmap",
            "1\t",
        ),
        (
            "metadata-id-block-mmap",
            "gfmmeta",
            vec!["tag", "Important", "0"],
            "metadata id block mmap",
            "1\t",
        ),
        (
            "metadata-verify",
            "gfmmeta",
            vec![],
            "metadata verify",
            "metadata-verify\t",
        ),
    ];

    for (route, extension, tail_args, worker, forbidden_stdout) in cases {
        let archive = offline.join(format!("{route}.{extension}"));
        fs::write(&archive, "not opened").unwrap();
        let mut args = vec![route.to_string(), archive.to_string_lossy().into_owned()];
        args.extend(tail_args.into_iter().map(str::to_string));

        let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
            .args(args)
            .output()
            .unwrap();

        assert!(!output.status.success(), "{route}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stdout.contains(forbidden_stdout), "{route}: {stdout}");
        assert!(
            stderr.contains(&format!(
                "{worker} volume access blocked: unreachable volume network"
            )),
            "{route}: {stderr}"
        );
        assert!(
            !stderr.contains(&format!(
                "security-worker-admission\tworker={worker}\tpath={}",
                archive.display()
            )),
            "{route}: {stderr}"
        );
        assert!(!stderr.contains("invalid magic"), "{route}: {stderr}");
    }

    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn search_index_columns_refuses_unreachable_columns_before_open_from_binary() {
    let root = unique_temp_dir("gfm-cli-search-columns-preflight-root");
    let offline = unique_temp_dir("gfm-cli-search-columns-preflight-offline");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let records = root.join("records.gfmidx");
    let columns = offline.join("columns.gfmcols");
    fs::write(&records, "not opened").unwrap();
    fs::write(&columns, "not opened").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-index-columns",
            records.to_str().unwrap(),
            columns.to_str().unwrap(),
            "needle",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("hit\t"), "{stdout}");
    assert!(
        stderr.contains(
            "search index columns columns volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=search index columns columns\tpath={}",
            columns.display()
        )),
        "{stderr}"
    );
    assert!(!stderr.contains("invalid magic"), "{stderr}");

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn archive_read_helpers_refuse_unreachable_volume_before_mapping_from_binary() {
    let offline = unique_temp_dir("gfm-cli-archive-read-preflight-offline");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let cases = [
        (
            "records-verify",
            "gfmidx",
            Vec::<&str>::new(),
            "records verify",
            "records-verify\t",
        ),
        (
            "columns-verify",
            "gfmcols",
            Vec::<&str>::new(),
            "columns verify",
            "columns-verify\t",
        ),
        (
            "columns-lookup",
            "gfmcols",
            vec!["1", "1"],
            "columns lookup",
            "columns\t",
        ),
    ];

    for (route, extension, tail_args, worker, forbidden_stdout) in cases {
        let archive = offline.join(format!("{route}.{extension}"));
        fs::write(&archive, "not opened").unwrap();
        let mut args = vec![route.to_string(), archive.to_string_lossy().into_owned()];
        args.extend(tail_args.into_iter().map(str::to_string));

        let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
            .args(args)
            .output()
            .unwrap();

        assert!(!output.status.success(), "{route}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stdout.contains(forbidden_stdout), "{route}: {stdout}");
        assert!(
            stderr.contains(&format!(
                "{worker} volume access blocked: unreachable volume network"
            )),
            "{route}: {stderr}"
        );
        assert!(
            !stderr.contains(&format!(
                "security-worker-admission\tworker={worker}\tpath={}",
                archive.display()
            )),
            "{route}: {stderr}"
        );
        assert!(!stderr.contains("invalid magic"), "{route}: {stderr}");
    }

    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn archive_plan_routes_refuse_unreachable_inputs_before_inspection_from_binary() {
    let root = unique_temp_dir("gfm-cli-archive-plan-preflight-root");
    let offline = unique_temp_dir("gfm-cli-archive-plan-preflight-offline");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let local_records = root.join("records.gfmidx");
    let local_columns = root.join("columns.gfmcols");
    let local_metadata = root.join("metadata.gfmmeta");
    let local_prefixes = root.join("prefixes.gfmprefix");
    let local_substrings = root.join("substrings.gfmsubstr");
    let local_fuzzy = root.join("fuzzy.gfmfuzzy");
    let local_dictionary = root.join("dictionary.gfmdict");
    let local_manifest = root.join("content.gfmmanifest");
    for path in [
        &local_records,
        &local_columns,
        &local_metadata,
        &local_prefixes,
        &local_substrings,
        &local_fuzzy,
        &local_dictionary,
        &local_manifest,
    ] {
        fs::write(path, "not opened").unwrap();
    }
    let offline_archive = offline.join("archive.gfmidx");
    let offline_content = offline.join("content.gfmcontent");
    fs::write(&offline_archive, "not opened").unwrap();
    fs::write(&offline_content, "not opened").unwrap();

    let cases = [
        (
            vec![
                "archive-schema".to_string(),
                "records".to_string(),
                offline_archive.to_string_lossy().into_owned(),
            ],
            "archive schema",
            "archive-schema\t",
        ),
        (
            vec![
                "records-migration-plan".to_string(),
                offline_archive.to_string_lossy().into_owned(),
            ],
            "records migration plan",
            "record-archive-migration-plan\t",
        ),
        (
            vec![
                "content-migration-plan".to_string(),
                offline_archive.to_string_lossy().into_owned(),
            ],
            "content migration plan",
            "content-archive-migration-plan\t",
        ),
        (
            vec![
                "metadata-migration-plan".to_string(),
                offline_archive.to_string_lossy().into_owned(),
            ],
            "metadata migration plan",
            "metadata-archive-migration-plan\t",
        ),
        (
            vec![
                "columns-rebuild-plan".to_string(),
                local_records.to_string_lossy().into_owned(),
                offline_archive.to_string_lossy().into_owned(),
            ],
            "columns rebuild plan columns",
            "columns-archive-rebuild-plan\t",
        ),
        (
            vec![
                "derived-sidecar-rebuild-plan".to_string(),
                local_records.to_string_lossy().into_owned(),
                "prefixes".to_string(),
                offline_archive.to_string_lossy().into_owned(),
            ],
            "derived sidecar rebuild plan sidecar",
            "derived-sidecar-rebuild-plan\t",
        ),
        (
            vec![
                "archive-rebuild-plan".to_string(),
                local_records.to_string_lossy().into_owned(),
                local_columns.to_string_lossy().into_owned(),
                local_metadata.to_string_lossy().into_owned(),
                local_prefixes.to_string_lossy().into_owned(),
                local_substrings.to_string_lossy().into_owned(),
                local_fuzzy.to_string_lossy().into_owned(),
                local_dictionary.to_string_lossy().into_owned(),
                offline_content.to_string_lossy().into_owned(),
                local_manifest.to_string_lossy().into_owned(),
            ],
            "archive rebuild plan content",
            "archive-rebuild-plan\t",
        ),
    ];

    for (args, worker, forbidden_stdout) in cases {
        let route = args[0].clone();
        let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
            .args(args)
            .output()
            .unwrap();

        assert!(!output.status.success(), "{route}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stdout.contains(forbidden_stdout), "{route}: {stdout}");
        assert!(
            stderr.contains(&format!(
                "{worker} volume access blocked: unreachable volume network"
            )),
            "{route}: {stderr}"
        );
        assert!(
            !stderr.contains(&format!("security-worker-admission\tworker={worker}\t")),
            "{route}: {stderr}"
        );
        assert!(!stderr.contains("invalid magic"), "{route}: {stderr}");
    }

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn archive_mutation_routes_refuse_unreachable_backup_before_parsing_from_binary() {
    let root = unique_temp_dir("gfm-cli-archive-mutation-preflight-root");
    let offline = unique_temp_dir("gfm-cli-archive-mutation-preflight-offline");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let records = root.join("records.gfmidx");
    let content = root.join("content.gfmcontent");
    let metadata = root.join("metadata.gfmmeta");
    let columns = root.join("columns.gfmcols");
    for path in [&records, &content, &metadata, &columns] {
        fs::write(path, "not opened").unwrap();
    }
    let backup = offline.join("backup");
    let cases = [
        (
            vec![
                "records-migrate".to_string(),
                records.to_string_lossy().into_owned(),
                backup.to_string_lossy().into_owned(),
            ],
            "records migrate backup",
            "record-archive-migration\t",
        ),
        (
            vec![
                "content-migrate".to_string(),
                content.to_string_lossy().into_owned(),
                backup.to_string_lossy().into_owned(),
            ],
            "content migrate backup",
            "content-archive-migration\t",
        ),
        (
            vec![
                "metadata-migrate".to_string(),
                metadata.to_string_lossy().into_owned(),
                backup.to_string_lossy().into_owned(),
            ],
            "metadata migrate backup",
            "metadata-archive-migration\t",
        ),
        (
            vec![
                "columns-rebuild".to_string(),
                records.to_string_lossy().into_owned(),
                columns.to_string_lossy().into_owned(),
                backup.to_string_lossy().into_owned(),
            ],
            "columns rebuild backup",
            "columns-archive-rebuild\t",
        ),
    ];

    for (args, worker, forbidden_stdout) in cases {
        let route = args[0].clone();
        let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
            .args(args)
            .output()
            .unwrap();

        assert!(!output.status.success(), "{route}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stdout.contains(forbidden_stdout), "{route}: {stdout}");
        assert!(
            stderr.contains(&format!(
                "{worker} volume access blocked: unreachable volume network"
            )),
            "{route}: {stderr}"
        );
        assert!(!stderr.contains("invalid magic"), "{route}: {stderr}");
        assert!(!backup.exists(), "{route}");
    }

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn record_sidecar_builders_refuse_unreachable_records_before_mapping_from_binary() {
    let root = unique_temp_dir("gfm-cli-record-sidecar-preflight-root");
    let offline = unique_temp_dir("gfm-cli-record-sidecar-preflight-offline");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let records = offline.join("records.gfmidx");
    fs::write(&records, "not opened").unwrap();
    let cases = [
        ("index-columns", "gfmcols", "index columns"),
        ("index-metadata", "gfmmeta", "index metadata"),
        ("index-dictionary", "gfmdict", "index dictionary"),
        ("index-prefixes", "gfmprefix", "index prefixes"),
        ("index-substrings", "gfmsubstr", "index substrings"),
        ("index-fuzzy", "gfmfuzzy", "index fuzzy"),
    ];

    for (route, extension, worker) in cases {
        let output_path = root.join(format!("{route}.{extension}"));
        let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
            .args([
                route,
                records.to_str().unwrap(),
                output_path.to_str().unwrap(),
            ])
            .output()
            .unwrap();

        assert!(!output.status.success(), "{route}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(&format!(
                "{worker} records volume access blocked: unreachable volume network"
            )),
            "{route}: {stderr}"
        );
        assert!(
            !stderr.contains(&format!(
                "security-worker-admission\tworker={worker} records\t"
            )),
            "{route}: {stderr}"
        );
        assert!(!stderr.contains("invalid magic"), "{route}: {stderr}");
        assert!(!output_path.exists(), "{route}");
    }

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn record_sidecar_builders_refuse_unreachable_outputs_before_mapping_from_binary() {
    let root = unique_temp_dir("gfm-cli-record-sidecar-output-root");
    let offline = unique_temp_dir("gfm-cli-record-sidecar-output-offline");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let records = root.join("records.gfmidx");
    fs::write(&records, "not opened").unwrap();
    let cases = [
        ("index-columns", "gfmcols", "index columns"),
        ("index-metadata", "gfmmeta", "index metadata"),
        ("index-dictionary", "gfmdict", "index dictionary"),
        ("index-prefixes", "gfmprefix", "index prefixes"),
        ("index-substrings", "gfmsubstr", "index substrings"),
        ("index-fuzzy", "gfmfuzzy", "index fuzzy"),
    ];

    for (route, extension, worker) in cases {
        let output_path = offline.join(format!("{route}.{extension}"));
        let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
            .args([
                route,
                records.to_str().unwrap(),
                output_path.to_str().unwrap(),
            ])
            .output()
            .unwrap();

        assert!(!output.status.success(), "{route}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(&format!(
                "{worker} output volume access blocked: unreachable volume network"
            )),
            "{route}: {stderr}"
        );
        assert!(
            !stderr.contains(&format!(
                "security-worker-admission\tworker={worker} records\t"
            )),
            "{route}: {stderr}"
        );
        assert!(!stderr.contains("invalid magic"), "{route}: {stderr}");
        assert!(!output_path.exists(), "{route}");
    }

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn search_index_sidecars_refuses_unreachable_content_before_open_from_binary() {
    let root = unique_temp_dir("gfm-cli-sidecar-preflight-root");
    let offline = unique_temp_dir("gfm-cli-sidecar-preflight-offline");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let records = root.join("records.gfmidx");
    let columns = root.join("columns.gfmcols");
    let metadata = root.join("metadata.gfmmeta");
    let prefixes = root.join("prefixes.gfmprefix");
    let substrings = root.join("substrings.gfmsubstr");
    let fuzzy = root.join("fuzzy.gfmfuzzy");
    let content = offline.join("content.gfmcontent");
    for path in [
        &records,
        &columns,
        &metadata,
        &prefixes,
        &substrings,
        &fuzzy,
    ] {
        fs::write(path, "not opened").unwrap();
    }
    fs::write(&content, "not opened").unwrap();

    let cases: [(&str, &[&str], &str); 4] = [
        ("search-index-sidecars", &["needle"], "sidecar search"),
        (
            "search-index-sidecars-session",
            &["needle"],
            "sidecar session",
        ),
        (
            "search-index-sidecars-budget",
            &["1", "1", "1", "1", "1", "1", "1", "needle"],
            "sidecar budget",
        ),
        (
            "search-index-sidecars-volume-scope",
            &["1", "needle"],
            "sidecar volume scope",
        ),
    ];

    for (route, tail_args, worker) in cases {
        let mut args = vec![
            route,
            records.to_str().unwrap(),
            columns.to_str().unwrap(),
            metadata.to_str().unwrap(),
            prefixes.to_str().unwrap(),
            substrings.to_str().unwrap(),
            fuzzy.to_str().unwrap(),
            content.to_str().unwrap(),
        ];
        args.extend_from_slice(tail_args);
        let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
            .args(args)
            .output()
            .unwrap();

        assert!(!output.status.success(), "{route}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stdout.contains("hit\t"), "{route}: {stdout}");
        assert!(
            stderr.contains(&format!(
                "{worker} content volume access blocked: unreachable volume network"
            )),
            "{route}: {stderr}"
        );
        assert!(
            !stderr.contains(&format!(
                "security-worker-admission\tworker={worker} content\tpath={}",
                content.display()
            )),
            "{route}: {stderr}"
        );
        assert!(!stderr.contains("invalid magic"), "{route}: {stderr}");
    }

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn recovers_missing_and_corrupt_sidecars_from_binary() {
    let root = unique_temp_dir("gfm-cli-sidecar-recovery-root");
    let records = unique_temp_path("gfm-cli-sidecar-recovery", "gfmidx");
    let prefixes = unique_temp_path("gfm-cli-sidecar-recovery", "gfmprefix");
    let dictionary = unique_temp_path("gfm-cli-sidecar-recovery", "gfmdict");
    let quarantine = unique_temp_dir("gfm-cli-sidecar-recovery-quarantine");
    fs::write(root.join("ProjectPlan.md"), "sidecar").unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), records.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );
    fs::write(&dictionary, "not-a-dictionary").unwrap();

    let plan_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "sidecar-recovery-plan",
            records.to_str().unwrap(),
            "-",
            "-",
            prefixes.to_str().unwrap(),
            "-",
            "-",
            dictionary.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        plan_output.status.success(),
        "{}",
        String::from_utf8_lossy(&plan_output.stderr)
    );
    let plan_stderr = String::from_utf8_lossy(&plan_output.stderr);
    assert!(
        plan_stderr.contains(&format!(
            "security-worker-admission\tworker=sidecar repair plan records\tpath={}",
            records.display()
        )) && plan_stderr.contains(&format!(
            "security-worker-admission\tworker=sidecar repair plan sidecar\tpath={}",
            prefixes.parent().unwrap().display()
        )) && plan_stderr.contains(&format!(
            "security-worker-admission\tworker=sidecar repair plan sidecar\tpath={}",
            dictionary.parent().unwrap().display()
        )),
        "{plan_stderr}"
    );
    let plan_stdout = String::from_utf8(plan_output.stdout).unwrap();
    assert!(
        plan_stdout.contains("action=rebuild")
            && plan_stdout.contains("reason=missing-sidecar")
            && plan_stdout.contains("invalid=2"),
        "{plan_stdout}"
    );

    let deferred_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "sidecar-recover-adaptive",
            records.to_str().unwrap(),
            quarantine.to_str().unwrap(),
            "saturated",
            "nominal",
            "ac",
            "idle",
            "-",
            "-",
            prefixes.to_str().unwrap(),
            "-",
            "-",
            dictionary.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        deferred_output.status.success(),
        "{}",
        String::from_utf8_lossy(&deferred_output.stderr)
    );
    let deferred_stderr = String::from_utf8(deferred_output.stderr).unwrap();
    assert!(
        deferred_stderr.contains("sidecar-recovery-deferred")
            && deferred_stderr.contains("action=Defer"),
        "{deferred_stderr}"
    );
    assert!(!prefixes.exists());
    assert_eq!(fs::read_to_string(&dictionary).unwrap(), "not-a-dictionary");
    assert!(fs::read_dir(&quarantine).unwrap().next().is_none());

    let catalog = unique_temp_path("gfm-cli-sidecar-recovery-runtime", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-sidecar-recovery-runtime", "gfmprogress");
    let journal = unique_temp_path("gfm-cli-sidecar-recovery-runtime", "journal");
    let recover_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .env("GFM_JOB_JOURNAL", &journal)
        .args([
            "sidecar-recover-adaptive",
            records.to_str().unwrap(),
            quarantine.to_str().unwrap(),
            "nominal",
            "nominal",
            "ac",
            "idle",
            "-",
            "-",
            prefixes.to_str().unwrap(),
            "-",
            "-",
            dictionary.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        recover_output.status.success(),
        "{}",
        String::from_utf8_lossy(&recover_output.stderr)
    );
    let recover_stdout = String::from_utf8(recover_output.stdout).unwrap();
    assert!(
        recover_stdout.contains("sidecar-recovery\trebuilt=2\tquarantined=1")
            && recover_stdout.contains("action=ready"),
        "{recover_stdout}"
    );
    let recover_stderr = String::from_utf8(recover_output.stderr).unwrap();
    assert!(
        recover_stderr.contains("sidecar-recovery-action"),
        "{recover_stderr}"
    );
    assert_eq!(
        recover_stderr
            .matches(&format!(
                "security-worker-admission\tworker=sidecar repair records\tpath={}\tintent=read",
                records.display()
            ))
            .count(),
        1,
        "{recover_stderr}"
    );
    let catalog_text = fs::read_to_string(&catalog).unwrap();
    assert!(catalog_text.contains("\trepair\t"), "{catalog_text}");
    assert!(catalog_text.contains("sidecar repair"), "{catalog_text}");
    assert!(
        catalog_text.contains(&records.display().to_string()),
        "{catalog_text}"
    );
    let progress_text = fs::read_to_string(&progress).unwrap();
    assert!(
        progress_text.contains("progress\t1\tbackground\tbackground\tsidecar repair"),
        "{progress_text}"
    );
    assert!(
        progress_text.contains("\tcompleted\t1\t1\tcompleted\t"),
        "{progress_text}"
    );
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(
        journal_text.contains("1\t1\tstarted\tsidecar repair"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t1\tcompleted\tsidecar repair"),
        "{journal_text}"
    );

    let prefix_verify = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["prefix-verify", prefixes.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(prefix_verify.status.success());
    let dictionary_verify = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["dictionary-verify", dictionary.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(dictionary_verify.status.success());

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(prefixes).unwrap();
    fs::remove_file(dictionary).unwrap();
    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
    fs::remove_file(journal).unwrap();
    fs::remove_dir_all(quarantine).unwrap();
}

#[test]
fn sidecar_recover_retries_transient_corrupt_sidecar_from_binary() {
    let root = unique_temp_dir("gfm-cli-sidecar-recovery-retry-root");
    let records = unique_temp_path("gfm-cli-sidecar-recovery-retry", "gfmidx");
    let prefixes = unique_temp_path("gfm-cli-sidecar-recovery-retry", "gfmprefix");
    let quarantine = unique_temp_dir("gfm-cli-sidecar-recovery-retry-quarantine");
    let journal = unique_temp_path("gfm-cli-sidecar-recovery-retry", "journal");
    let catalog = unique_temp_path("gfm-cli-sidecar-recovery-retry", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-sidecar-recovery-retry", "gfmprogress");
    let retry_probe = unique_temp_path("gfm-cli-sidecar-recovery-retry", "state");
    fs::write(root.join("RepairPrefix.md"), "sidecar retry").unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), records.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );

    let recover_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "sidecar-recover-retry-probe",
            records.to_str().unwrap(),
            quarantine.to_str().unwrap(),
            retry_probe.to_str().unwrap(),
            "-",
            "-",
            prefixes.to_str().unwrap(),
            "-",
            "-",
            "-",
        ])
        .output()
        .unwrap();
    assert!(
        recover_output.status.success(),
        "{}",
        String::from_utf8_lossy(&recover_output.stderr)
    );
    let stdout = String::from_utf8(recover_output.stdout).unwrap();
    assert!(
        stdout.contains("sidecar-recovery\trebuilt=1\tquarantined=0"),
        "{stdout}"
    );
    assert_eq!(fs::read_to_string(&retry_probe).unwrap(), "2");
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(
        journal_text.contains("1\t1\tstarted\tsidecar repair"),
        "{journal_text}"
    );
    assert!(
        journal_text
            .contains("1\t1\tfailed:temporary sidecar repair retry probe busy\tsidecar repair"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tstarted\tsidecar repair"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tcompleted\tsidecar repair"),
        "{journal_text}"
    );
    let prefix_verify = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["prefix-verify", prefixes.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(prefix_verify.status.success());

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(prefixes).unwrap();
    fs::remove_file(journal).unwrap();
    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
    fs::remove_file(retry_probe).unwrap();
    fs::remove_dir_all(quarantine).unwrap();
}

#[test]
fn sidecar_recovery_plan_refuses_unreachable_sidecar_before_inspection_from_binary() {
    let root = unique_temp_dir("gfm-cli-sidecar-recovery-plan-root");
    let offline = unique_temp_dir("gfm-cli-sidecar-recovery-plan-offline");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let records = root.join("records.gfmidx");
    let prefixes = offline.join("prefixes.gfmprefix");
    fs::write(
        &records,
        "gfm-store-v2\n1\t1\t0\tf\t5\t0\t0\t0\t0\t\t/tmp/tagged.md\n",
    )
    .unwrap();
    fs::write(&prefixes, "not inspected").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "sidecar-recovery-plan",
            records.to_str().unwrap(),
            "-",
            "-",
            prefixes.to_str().unwrap(),
            "-",
            "-",
            "-",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("sidecar-recovery-plan\t"), "{stdout}");
    assert!(
        stderr.contains(
            "sidecar repair plan sidecar volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert!(
        !stderr.contains("security-worker-admission\tworker=sidecar repair plan sidecar\t"),
        "{stderr}"
    );
    assert!(!stderr.contains("invalid magic"), "{stderr}");

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn sidecar_recover_refuses_unreachable_volume_before_repair_from_binary() {
    let root = unique_temp_dir("gfm-cli-sidecar-recovery-unreachable-root");
    let records = root.join("records.gfmidx");
    let prefixes = root.join("prefixes.gfmprefix");
    let dictionary = root.join("dictionary.gfmdict");
    let quarantine = root.join("quarantine");
    fs::create_dir_all(&quarantine).unwrap();
    fs::write(root.join("ProjectPlan.md"), "sidecar").unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), records.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );
    fs::write(&dictionary, "not-a-dictionary").unwrap();
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "sidecar-recover",
            records.to_str().unwrap(),
            quarantine.to_str().unwrap(),
            "-",
            "-",
            prefixes.to_str().unwrap(),
            "-",
            "-",
            dictionary.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("sidecar-recovery\t"), "{stdout}");
    assert!(
        stderr.contains("sidecar repair records volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=sidecar repair records\tpath={}",
            records.display()
        )),
        "{stderr}"
    );
    assert!(!prefixes.exists());
    assert_eq!(fs::read_to_string(&dictionary).unwrap(), "not-a-dictionary");
    assert!(fs::read_dir(&quarantine).unwrap().next().is_none());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn deferred_sidecar_recover_adaptive_does_not_touch_unreachable_records_from_binary() {
    let root = unique_temp_dir("gfm-cli-sidecar-recovery-deferred-unreachable-root");
    let records = root.join("records.gfmidx");
    let prefixes = root.join("prefixes.gfmprefix");
    let dictionary = root.join("dictionary.gfmdict");
    let quarantine = root.join("quarantine");
    let catalog = unique_temp_path("gfm-cli-sidecar-recovery-deferred-unreachable", "gfmjobs");
    let progress = unique_temp_path(
        "gfm-cli-sidecar-recovery-deferred-unreachable",
        "gfmprogress",
    );
    fs::create_dir_all(&quarantine).unwrap();
    fs::write(&records, "not-a-record-archive").unwrap();
    fs::write(&dictionary, "not-a-dictionary").unwrap();
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "sidecar-recover-adaptive",
            records.to_str().unwrap(),
            quarantine.to_str().unwrap(),
            "saturated",
            "critical",
            "low",
            "active",
            "-",
            "-",
            prefixes.to_str().unwrap(),
            "-",
            "-",
            dictionary.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("sidecar-recovery\t"), "{stdout}");
    assert!(
        stderr.contains("sidecar-recovery-deferred") && stderr.contains("action=Defer"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "\tworker=sidecar repair records\tpath={}",
            records.display()
        )),
        "{stderr}"
    );
    assert_eq!(
        fs::read_to_string(&records).unwrap(),
        "not-a-record-archive"
    );
    assert_eq!(fs::read_to_string(&dictionary).unwrap(), "not-a-dictionary");
    assert!(!prefixes.exists());
    assert!(fs::read_dir(&quarantine).unwrap().next().is_none());
    let progress_text = fs::read_to_string(&progress).unwrap();
    assert!(
        progress_text.contains("progress\t1\tbackground\tbackground\tsidecar repair"),
        "{progress_text}"
    );
    assert!(
        progress_text.contains("\tpaused\t0\t1\tdeferred:Defer\t"),
        "{progress_text}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
}

#[test]
fn adaptive_sidecar_recover_refuses_unreachable_outputs_before_worker_from_binary() {
    let root = unique_temp_dir("gfm-cli-sidecar-recovery-output-root");
    let offline = unique_temp_dir("gfm-cli-sidecar-recovery-output-unreachable");
    let records = root.join("records.gfmidx");
    let prefixes = offline.join("prefixes.gfmprefix");
    let dictionary = root.join("dictionary.gfmdict");
    let quarantine = root.join("quarantine");
    fs::create_dir_all(&quarantine).unwrap();
    fs::write(root.join("ProjectPlan.md"), "sidecar output").unwrap();
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(&dictionary, "not-a-dictionary").unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), records.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "sidecar-recover-adaptive",
            records.to_str().unwrap(),
            quarantine.to_str().unwrap(),
            "nominal",
            "nominal",
            "ac",
            "idle",
            "-",
            "-",
            prefixes.to_str().unwrap(),
            "-",
            "-",
            dictionary.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("sidecar-recovery\t"), "{stdout}");
    assert!(
        stderr.contains("sidecar repair output volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "\tworker=sidecar repair records\tpath={}",
            records.display()
        )),
        "{stderr}"
    );
    assert!(!prefixes.exists());
    assert_eq!(fs::read_to_string(&dictionary).unwrap(), "not-a-dictionary");
    assert!(fs::read_dir(&quarantine).unwrap().next().is_none());

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn adaptive_sidecar_recover_refuses_unreachable_quarantine_before_worker_from_binary() {
    let root = unique_temp_dir("gfm-cli-sidecar-recovery-quarantine-root");
    let offline = unique_temp_dir("gfm-cli-sidecar-recovery-quarantine-unreachable");
    let records = root.join("records.gfmidx");
    let prefixes = root.join("prefixes.gfmprefix");
    let dictionary = root.join("dictionary.gfmdict");
    let quarantine = offline.join("sidecar-quarantine-unavailable".repeat(16));
    fs::write(root.join("ProjectPlan.md"), "sidecar quarantine").unwrap();
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(&dictionary, "not-a-dictionary").unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), records.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "sidecar-recover-adaptive",
            records.to_str().unwrap(),
            quarantine.to_str().unwrap(),
            "nominal",
            "nominal",
            "ac",
            "idle",
            "-",
            "-",
            prefixes.to_str().unwrap(),
            "-",
            "-",
            dictionary.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("sidecar-recovery\t"), "{stdout}");
    assert!(
        stderr.contains(
            "sidecar repair quarantine volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert!(
        !stderr.contains("archive write path metadata unavailable"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "\tworker=sidecar repair records\tpath={}",
            records.display()
        )),
        "{stderr}"
    );
    assert!(!prefixes.exists());
    assert_eq!(fs::read_to_string(&dictionary).unwrap(), "not-a-dictionary");
    assert!(!quarantine.exists());

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn sidecar_recover_reports_output_probe_failure_before_repair_from_binary() {
    let root = unique_temp_dir("gfm-cli-sidecar-recovery-output-probe");
    let records = root.join("records.gfmidx");
    let prefixes = root.join("sidecar-recovery-output-unavailable".repeat(16));
    let dictionary = root.join("dictionary.gfmdict");
    let quarantine = root.join("quarantine");
    fs::create_dir_all(&quarantine).unwrap();
    fs::write(&records, "not opened").unwrap();
    fs::write(&dictionary, "not-a-dictionary").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "sidecar-recover",
            records.to_str().unwrap(),
            quarantine.to_str().unwrap(),
            "-",
            "-",
            prefixes.to_str().unwrap(),
            "-",
            "-",
            dictionary.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("sidecar-recovery\t"), "{stdout}");
    assert!(
        stderr.contains("archive write path metadata unavailable"),
        "{stderr}"
    );
    assert!(
        stderr.contains("sidecar-recovery-output-unavailable"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("security-worker-admission\tworker=sidecar repair records\t"),
        "{stderr}"
    );
    assert!(!stderr.contains("invalid magic"), "{stderr}");
    assert!(!prefixes.exists());
    assert_eq!(fs::read_to_string(&dictionary).unwrap(), "not-a-dictionary");
    assert!(fs::read_dir(&quarantine).unwrap().next().is_none());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn sidecar_recover_retries_transient_failure_from_binary() {
    let root = unique_temp_dir("gfm-cli-sidecar-recovery-retry-root");
    let records = unique_temp_path("gfm-cli-sidecar-recovery-retry-records", "gfmidx");
    let prefixes = unique_temp_path("gfm-cli-sidecar-recovery-retry-prefixes", "gfmprefix");
    let quarantine = unique_temp_dir("gfm-cli-sidecar-recovery-retry-quarantine");
    let journal = unique_temp_path("gfm-cli-sidecar-recovery-retry", "journal");
    let catalog = unique_temp_path("gfm-cli-sidecar-recovery-retry", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-sidecar-recovery-retry", "gfmprogress");
    let retry_probe = unique_temp_path("gfm-cli-sidecar-recovery-retry", "state");
    fs::write(root.join("RecoverRetry.md"), "recover retry").unwrap();

    let index = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), records.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        index.status.success(),
        "{}",
        String::from_utf8_lossy(&index.stderr)
    );
    fs::write(&prefixes, "not-a-prefix-archive").unwrap();

    let recover = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "sidecar-recover-retry-probe",
            records.to_str().unwrap(),
            quarantine.to_str().unwrap(),
            retry_probe.to_str().unwrap(),
            "-",
            "-",
            prefixes.to_str().unwrap(),
            "-",
            "-",
            "-",
        ])
        .output()
        .unwrap();
    assert!(
        recover.status.success(),
        "{}",
        String::from_utf8_lossy(&recover.stderr)
    );
    let stdout = String::from_utf8(recover.stdout).unwrap();
    assert!(
        stdout.contains("sidecar-recovery\trebuilt=1\tquarantined=1"),
        "{stdout}"
    );
    assert!(stdout.contains("quarantined\t"), "{stdout}");
    assert_eq!(fs::read_to_string(&retry_probe).unwrap(), "2");
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(
        journal_text.contains("1\t1\tstarted\tsidecar repair"),
        "{journal_text}"
    );
    assert!(
        journal_text
            .contains("1\t1\tfailed:temporary sidecar repair retry probe busy\tsidecar repair"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tstarted\tsidecar repair"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tcompleted\tsidecar repair"),
        "{journal_text}"
    );

    let verify = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["prefix-verify", prefixes.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        verify.status.success(),
        "{}",
        String::from_utf8_lossy(&verify.stderr)
    );
    assert!(fs::read_dir(&quarantine).unwrap().next().is_some());

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(prefixes).unwrap();
    fs::remove_dir_all(quarantine).unwrap();
    fs::remove_file(journal).unwrap();
    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
    fs::remove_file(retry_probe).unwrap();
}

#[test]
fn sidecar_recover_reports_quarantine_probe_failure_before_repair_from_binary() {
    let root = unique_temp_dir("gfm-cli-sidecar-recovery-quarantine-probe");
    let records = root.join("records.gfmidx");
    let prefixes = root.join("prefixes.gfmprefix");
    let dictionary = root.join("dictionary.gfmdict");
    let quarantine = root.join("sidecar-quarantine-unavailable".repeat(16));
    fs::write(&records, "not opened").unwrap();
    fs::write(&dictionary, "not-a-dictionary").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "sidecar-recover",
            records.to_str().unwrap(),
            quarantine.to_str().unwrap(),
            "-",
            "-",
            prefixes.to_str().unwrap(),
            "-",
            "-",
            dictionary.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("sidecar-recovery\t"), "{stdout}");
    assert!(
        stderr.contains("archive write path metadata unavailable"),
        "{stderr}"
    );
    assert!(
        stderr.contains("sidecar-quarantine-unavailable"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("security-worker-admission\tworker=sidecar repair records\t"),
        "{stderr}"
    );
    assert!(!stderr.contains("invalid magic"), "{stderr}");
    assert!(!prefixes.exists());
    assert_eq!(fs::read_to_string(&dictionary).unwrap(), "not-a-dictionary");
    assert!(!quarantine.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn searches_with_scope_prefixes_from_binary() {
    let root = unique_temp_dir("gfm-cli-scope-root");
    fs::create_dir_all(root.join("Desktop")).unwrap();
    fs::create_dir_all(root.join("Downloads")).unwrap();
    fs::write(root.join("Desktop").join("report.md"), "desktop").unwrap();
    fs::write(root.join("Downloads").join("report.md"), "downloads").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["search", root.to_str().unwrap(), "report @desktop"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Desktop/report.md"), "{stdout}");
    assert!(!stdout.contains("Downloads/report.md"), "{stdout}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn performs_journaled_copy_move_and_delete_from_binary() {
    let root = unique_temp_dir("gfm-cli-ops-root");
    let journal = root.join("ops.journal");
    let source = root.join("source.txt");
    let copy = root.join("copy.txt");
    let moved = root.join("moved.txt");
    let link = root.join("source-link");
    let link_copy = root.join("copy-link");
    fs::write(&source, "hello ops").unwrap();
    make_symlink(&source, &link);
    let modified = filetime::FileTime::from_unix_time(1_700_000_001, 456_000_000);
    filetime::set_file_mtime(&source, modified).unwrap();
    let xattr_supported = match xattr::set(&source, "user.gfm.cli-test", b"preserved") {
        Ok(()) => true,
        Err(err)
            if matches!(
                err.kind(),
                std::io::ErrorKind::Unsupported | std::io::ErrorKind::PermissionDenied
            ) =>
        {
            false
        }
        Err(err) => panic!("unexpected xattr setup failure: {err}"),
    };

    run_gfm(
        &journal,
        ["copy", source.to_str().unwrap(), copy.to_str().unwrap()],
    );
    assert_eq!(fs::read_to_string(&copy).unwrap(), "hello ops");
    assert_eq!(
        filetime::FileTime::from_last_modification_time(&fs::metadata(&copy).unwrap()),
        modified
    );
    if xattr_supported {
        assert_eq!(
            xattr::get(&copy, "user.gfm.cli-test").unwrap().as_deref(),
            Some(b"preserved".as_slice())
        );
    }

    run_gfm(
        &journal,
        ["copy", link.to_str().unwrap(), link_copy.to_str().unwrap()],
    );
    #[cfg(unix)]
    {
        assert!(fs::symlink_metadata(&link_copy)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_link(&link_copy).unwrap(), source);
    }
    #[cfg(not(unix))]
    assert_eq!(fs::read_to_string(&link_copy).unwrap(), "link");

    run_gfm(
        &journal,
        ["move", copy.to_str().unwrap(), moved.to_str().unwrap()],
    );
    assert!(!copy.exists());
    assert_eq!(fs::read_to_string(&moved).unwrap(), "hello ops");

    run_gfm(&journal, ["delete", moved.to_str().unwrap()]);
    assert!(!moved.exists());

    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(journal_text.contains("copy"));
    assert!(journal_text.contains("move"));
    assert!(journal_text.contains("delete"));
    assert!(journal_text.contains("completed"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_operation_from_binary_still_journals_failure() {
    let root = unique_temp_dir("gfm-cli-ops-failure-root");
    let journal = root.join("ops.journal");
    let missing = root.join("missing.txt");
    let destination = root.join("destination.txt");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPS_JOURNAL", &journal)
        .args([
            "copy",
            missing.to_str().unwrap(),
            destination.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(journal_text.contains("copy"), "{journal_text}");
    assert!(journal_text.contains("failed"), "{journal_text}");
    assert!(
        journal_text.contains(&missing.to_string_lossy().to_string()),
        "{journal_text}"
    );
    assert!(!destination.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn live_copy_operation_retries_transient_failure_from_binary() {
    let root = unique_temp_dir("gfm-cli-ops-live-retry-root");
    let journal = root.join("ops.journal");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    let state = root.join("retry.state");
    fs::write(&source, "arrived during backoff").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPS_JOURNAL", &journal)
        .args([
            "copy-retry-probe",
            source.to_str().unwrap(),
            destination.to_str().unwrap(),
            state.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(&destination).unwrap(),
        "arrived during backoff"
    );
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert_eq!(journal_text.matches("\tstarted\t").count(), 2);
    assert_eq!(journal_text.matches("\tfailed\t").count(), 1);
    assert_eq!(journal_text.matches("\tcompleted\t").count(), 1);
    assert_eq!(fs::read_to_string(&state).unwrap(), "2");
    assert!(
        journal_text.contains("temporary operation retry probe busy"),
        "{journal_text}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn operation_refuses_unreachable_journal_before_mutating_from_binary() {
    let root = unique_temp_dir("gfm-cli-ops-journal-preflight-root");
    let offline = unique_temp_dir("gfm-cli-ops-journal-preflight-offline");
    let journal = offline.join(format!(
        "{}.journal",
        "operation-journal-unavailable".repeat(16)
    ));
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(&source, "do not copy without durable journal").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPS_JOURNAL", &journal)
        .args([
            "copy",
            source.to_str().unwrap(),
            destination.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("\tcompleted"), "{stdout}");
    assert!(
        stderr.contains("operation journal volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("operation write path metadata unavailable"),
        "{stderr}"
    );
    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        "do not copy without durable journal"
    );
    assert!(!destination.exists());
    assert!(!journal.exists());

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn operation_reports_journal_probe_failure_before_mutating_from_binary() {
    let root = unique_temp_dir("gfm-cli-ops-journal-probe-root");
    let journal = root.join(format!("{}.journal", "ops-journal-unavailable".repeat(16)));
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    fs::write(&source, "do not copy without probeable journal").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPS_JOURNAL", &journal)
        .args([
            "copy",
            source.to_str().unwrap(),
            destination.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("\tcompleted"), "{stdout}");
    assert!(
        stderr.contains("operation write path metadata unavailable"),
        "{stderr}"
    );
    assert!(stderr.contains("ops-journal-unavailable"), "{stderr}");
    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        "do not copy without probeable journal"
    );
    assert!(!destination.exists());
    assert!(!journal.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn copy_operation_reports_destination_parent_probe_failure_before_mutating_from_binary() {
    let root = unique_temp_dir("gfm-cli-ops-destination-parent-probe-root");
    let journal = root.join("ops.journal");
    let source = root.join("source.txt");
    let destination = root
        .join("destination-parent-unavailable".repeat(16))
        .join("copy.txt");
    fs::write(&source, "do not copy to an unprobeable destination").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPS_JOURNAL", &journal)
        .args([
            "copy",
            source.to_str().unwrap(),
            destination.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("\tcompleted"), "{stdout}");
    assert!(
        stderr.contains("security-worker-admission\tworker=copy destination-parent\t"),
        "{stderr}"
    );
    assert!(
        stderr.contains("operation target path existence unavailable"),
        "{stderr}"
    );
    assert!(
        stderr.contains("destination-parent-unavailable"),
        "{stderr}"
    );
    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        "do not copy to an unprobeable destination"
    );
    assert!(!destination.exists());
    assert!(!journal.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn operation_refuses_unreachable_destination_volume_before_copying_from_binary() {
    let root = unique_temp_dir("gfm-cli-ops-unreachable-destination-root");
    let journal = root.join("ops.journal");
    let source_root = root.join("Source");
    let destination_volume = root.join("Team Share");
    fs::create_dir_all(&source_root).unwrap();
    fs::create_dir_all(&destination_volume).unwrap();
    fs::write(
        destination_volume.join(".gfm-volume-kind"),
        "network-unreachable\n",
    )
    .unwrap();
    let source = source_root.join("source.txt");
    let destination = destination_volume.join("copy.txt");
    fs::write(&source, "do not copy onto unreachable storage").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPS_JOURNAL", &journal)
        .args([
            "copy",
            source.to_str().unwrap(),
            destination.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("\tcompleted"), "{stdout}");
    assert!(
        stderr.contains("destination-parent is not accessible for mutation"),
        "{stderr}"
    );
    assert!(
        stderr.contains("unreachable volume network") && stderr.contains("role=destination-parent"),
        "{stderr}"
    );
    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        "do not copy onto unreachable storage"
    );
    assert!(!destination.exists());
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(journal_text.contains("\tstarted\t"), "{journal_text}");
    assert!(journal_text.contains("\tfailed\t"), "{journal_text}");
    assert!(
        journal_text.contains("unreachable volume network"),
        "{journal_text}"
    );
    assert!(!journal_text.contains("\tcompleted\t"), "{journal_text}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn operation_volume_copy_policy_reports_descriptor_classes_from_binary() {
    let root = unique_temp_dir("gfm-cli-operation-copy-policy");
    let network = root.join("TeamShare");
    let external = root.join("Backup");
    fs::create_dir_all(&network).unwrap();
    fs::create_dir_all(&external).unwrap();
    fs::write(network.join(".gfm-volume-kind"), "network-smb\n").unwrap();
    fs::write(external.join(".gfm-volume-kind"), "external-removable\n").unwrap();
    let source = network.join("source.bin");
    let destination = external.join("destination.bin");
    fs::write(&source, "policy only").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "operation-volume-copy-policy",
            source.to_str().unwrap(),
            destination.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.starts_with("operation-volume-copy-policy\t"),
        "{stdout}"
    );
    assert!(stdout.contains("\tsource-class=network\t"), "{stdout}");
    assert!(
        stdout.contains("\tdestination-class=external\t"),
        "{stdout}"
    );
    assert!(stdout.contains("\tbuffer-bytes=65536\t"), "{stdout}");
    assert!(stdout.contains("\tvolumes=2"), "{stdout}");
    assert!(
        !stderr.contains("security-access\t") && !stderr.contains("security-worker-admission\t"),
        "{stderr}"
    );
    assert!(!destination.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn operation_volume_copy_policy_reports_disk_image_as_slow_from_binary() {
    let root = unique_temp_dir("gfm-cli-operation-copy-policy-disk-image");
    let image = root.join("Installer");
    let local = root.join("Local");
    fs::create_dir_all(&image).unwrap();
    fs::create_dir_all(&local).unwrap();
    fs::write(image.join(".gfm-volume-kind"), "disk-image\n").unwrap();
    let source = image.join("source.bin");
    let destination = local.join("destination.bin");
    fs::write(&source, "policy only").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "operation-volume-copy-policy",
            source.to_str().unwrap(),
            destination.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.starts_with("operation-volume-copy-policy\t"),
        "{stdout}"
    );
    assert!(stdout.contains("\tsource-class=slow\t"), "{stdout}");
    assert!(stdout.contains("\tdestination-class=local\t"), "{stdout}");
    assert!(stdout.contains("\tbuffer-bytes=65536\t"), "{stdout}");
    assert!(stdout.contains("\tvolumes=2"), "{stdout}");
    assert!(!destination.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn operation_volume_copy_policy_refuses_unreachable_destination_from_binary() {
    let root = unique_temp_dir("gfm-cli-operation-copy-policy-unreachable");
    let source_root = root.join("Source");
    let offline = root.join("Offline");
    fs::create_dir_all(&source_root).unwrap();
    fs::create_dir_all(&offline).unwrap();
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let source = source_root.join("source.bin");
    let destination = offline.join(format!(
        "{}.bin",
        "operation-copy-policy-destination-unavailable".repeat(8)
    ));
    fs::write(&source, "policy only").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "operation-volume-copy-policy",
            source.to_str().unwrap(),
            destination.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("operation-volume-copy-policy\t"),
        "{stdout}"
    );
    assert!(
        stderr.contains(
            "operation volume copy policy destination volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert!(
        !stderr.contains("operation write path metadata unavailable"),
        "{stderr}"
    );
    assert!(!destination.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn operation_volume_copy_policy_refuses_read_only_destination_from_binary() {
    let root = unique_temp_dir("gfm-cli-operation-copy-policy-read-only");
    let source_root = root.join("Source");
    let read_only = root.join("Camera Card");
    fs::create_dir_all(&source_root).unwrap();
    fs::create_dir_all(&read_only).unwrap();
    fs::write(
        read_only.join(".gfm-volume-kind"),
        "external-removable-read-only\n",
    )
    .unwrap();
    let source = source_root.join("source.bin");
    let destination = read_only.join("destination.bin");
    fs::write(&source, "policy only").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "operation-volume-copy-policy",
            source.to_str().unwrap(),
            destination.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("operation-volume-copy-policy\t"),
        "{stdout}"
    );
    assert!(
        stderr.contains(
            "operation volume copy policy destination volume access blocked: read-only volume external"
        ),
        "{stderr}"
    );
    assert!(!destination.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn operation_refuses_read_only_destination_volume_before_copying_from_binary() {
    let root = unique_temp_dir("gfm-cli-ops-readonly-destination-root");
    let journal = root.join("ops.journal");
    let source_root = root.join("Source");
    let destination_volume = root.join("Camera Card");
    fs::create_dir_all(&source_root).unwrap();
    fs::create_dir_all(&destination_volume).unwrap();
    fs::write(
        destination_volume.join(".gfm-volume-kind"),
        "external-removable-read-only\n",
    )
    .unwrap();
    let source = source_root.join("source.txt");
    let destination = destination_volume.join("copy.txt");
    fs::write(&source, "do not copy onto read-only storage").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPS_JOURNAL", &journal)
        .args([
            "copy",
            source.to_str().unwrap(),
            destination.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("\tcompleted"), "{stdout}");
    assert!(
        stderr.contains("destination-parent is not accessible for mutation"),
        "{stderr}"
    );
    assert!(
        stderr.contains("read-only volume external") && stderr.contains("role=destination-parent"),
        "{stderr}"
    );
    assert!(
        stderr.contains("refresh-on-permission-change=true"),
        "{stderr}"
    );
    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        "do not copy onto read-only storage"
    );
    assert!(!destination.exists());
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(journal_text.contains("\tstarted\t"), "{journal_text}");
    assert!(journal_text.contains("\tfailed\t"), "{journal_text}");
    assert!(journal_text.contains("\tcopy\t"), "{journal_text}");
    assert!(
        journal_text.contains("read-only volume external"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("refresh-on-permission-change=true"),
        "{journal_text}"
    );
    assert!(!journal_text.contains("\tcompleted\t"), "{journal_text}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn copy_skip_from_binary_journals_skipped_without_overwrite() {
    let root = unique_temp_dir("gfm-cli-ops-skip-root");
    let journal = root.join("ops.journal");
    let source = root.join("report.md");
    let destination = root.join("destination.md");
    fs::write(&source, "new report").unwrap();
    fs::write(&destination, "old report").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPS_JOURNAL", &journal)
        .args([
            "copy",
            source.to_str().unwrap(),
            destination.to_str().unwrap(),
            "--skip",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("\tskipped"), "{stdout}");
    assert_eq!(fs::read_to_string(&source).unwrap(), "new report");
    assert_eq!(fs::read_to_string(&destination).unwrap(), "old report");
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(journal_text.contains("\tskipped\t"), "{journal_text}");
    assert!(!journal_text.contains("\tcompleted\t"), "{journal_text}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn copy_keep_both_from_binary_uses_actual_journal_destination() {
    let root = unique_temp_dir("gfm-cli-ops-keep-both-root");
    let journal = root.join("ops.journal");
    let source = root.join("report.md");
    let destination = root.join("destination.md");
    let copied = root.join("destination copy.md");
    fs::write(&source, "new report").unwrap();
    fs::write(&destination, "old report").unwrap();

    run_gfm(
        &journal,
        [
            "copy",
            source.to_str().unwrap(),
            destination.to_str().unwrap(),
            "--keep-both",
        ],
    );

    assert_eq!(fs::read_to_string(&source).unwrap(), "new report");
    assert_eq!(fs::read_to_string(&destination).unwrap(), "old report");
    assert_eq!(fs::read_to_string(&copied).unwrap(), "new report");
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(
        journal_text.contains("destination copy.md"),
        "{journal_text}"
    );
    assert!(!journal_text.contains("failed"), "{journal_text}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn operation_conflict_apply_executes_resolved_copy_from_binary() {
    let root = unique_temp_dir("gfm-cli-operation-conflict-apply-root");
    let journal = root.join("ops.journal");
    let conflicts = root.join("operation-conflicts.tsv");
    let source = root.join("report.md");
    let destination = root.join("destination.md");
    let copied = root.join("destination copy.md");
    fs::write(&source, "new report").unwrap();
    fs::write(&destination, "old report").unwrap();

    let failed_copy = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPS_JOURNAL", &journal)
        .env("GFM_OPERATION_CONFLICT_STORE", &conflicts)
        .args([
            "copy",
            source.to_str().unwrap(),
            destination.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!failed_copy.status.success());
    let conflict_store = fs::read_to_string(&conflicts).unwrap();
    assert!(
        conflict_store.contains(&format!("\tsource={}\t", source.display())),
        "{conflict_store}"
    );
    assert!(
        conflict_store.contains("\tblocks-operation=true\t"),
        "{conflict_store}"
    );

    let apply = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPS_JOURNAL", &journal)
        .args([
            "operation-conflict-apply",
            conflicts.to_str().unwrap(),
            destination.to_str().unwrap(),
            "keep-both",
        ])
        .output()
        .unwrap();
    assert!(
        apply.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    let stdout = String::from_utf8(apply.stdout).unwrap();
    assert!(stdout.contains("\tcompleted"), "{stdout}");
    assert!(
        stdout.contains("operation-conflict-control\tapply\t"),
        "{stdout}"
    );
    let stderr = String::from_utf8_lossy(&apply.stderr);
    assert!(
        stderr.contains("security-worker-admission\tworker=operation conflict store\t")
            && stderr.contains("\tworker-action=start\t")
            && stderr.contains("\tcan-touch-filesystem=true\t"),
        "{stderr}"
    );
    assert_eq!(fs::read_to_string(&source).unwrap(), "new report");
    assert_eq!(fs::read_to_string(&destination).unwrap(), "old report");
    assert_eq!(fs::read_to_string(&copied).unwrap(), "new report");
    let resolved_store = fs::read_to_string(&conflicts).unwrap();
    assert!(
        resolved_store.contains("\tpolicy=keep-both\t"),
        "{resolved_store}"
    );
    assert!(
        resolved_store.contains("\tblocks-operation=false\t"),
        "{resolved_store}"
    );
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(
        journal_text.contains("destination copy.md"),
        "{journal_text}"
    );
    assert!(journal_text.contains("\tcompleted\t"), "{journal_text}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ui_operation_conflict_resolve_persists_on_visible_worker_from_binary() {
    let root = unique_temp_dir("gfm-cli-ui-operation-conflict-resolve-root");
    let conflicts = root.join("operation-conflicts.tsv");
    let source = root.join("report.md");
    let destination = root.join("destination.md");
    fs::write(&source, "new report").unwrap();
    fs::write(&destination, "old report").unwrap();
    fs::write(
        &conflicts,
        format!(
            "operation-conflict\toperation=copy\tsource={}\ttarget={}\texists=true\tkind=file\tpolicy=fail\tavailable=replace,keep-both,skip\tblocks-operation=true\treason=destination-conflict-requires-user-resolution\n",
            source.display(),
            destination.display()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "ui-operation-conflict-resolve",
            conflicts.to_str().unwrap(),
            destination.to_str().unwrap(),
            "skip",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(
        &stderr,
        "ui operation conflict resolve",
        conflicts.parent().unwrap(),
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("operation-conflict-control\tresolve\t"),
        "{stdout}"
    );
    assert!(stdout.contains("\tpolicy=skip\t"), "{stdout}");
    assert!(stdout.contains("\tblocks-operation=false\t"), "{stdout}");
    assert!(stdout.contains("dialog\tsurface=conflict\t"), "{stdout}");
    assert!(stdout.contains("operation-conflict-ui\t"), "{stdout}");
    assert!(stdout.contains("operation-conflict-row\t"), "{stdout}");

    let stored = fs::read_to_string(&conflicts).unwrap();
    assert!(stored.contains("\tpolicy=skip\t"), "{stored}");
    assert!(stored.contains("\tblocks-operation=false\t"), "{stored}");
    assert_eq!(fs::read_to_string(&source).unwrap(), "new report");
    assert_eq!(fs::read_to_string(&destination).unwrap(), "old report");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ui_operation_conflict_resolve_writes_relative_store_on_visible_worker_from_binary() {
    let root = unique_temp_dir("gfm-cli-ui-operation-conflict-relative-root");
    let source = root.join("report.md");
    let destination = root.join("destination.md");
    fs::write(&source, "new report").unwrap();
    fs::write(&destination, "old report").unwrap();
    fs::write(
        root.join("operation-conflicts.tsv"),
        format!(
            "operation-conflict\toperation=copy\tsource={}\ttarget={}\texists=true\tkind=file\tpolicy=fail\tavailable=replace,keep-both,skip\tblocks-operation=true\treason=destination-conflict-requires-user-resolution\n",
            source.display(),
            destination.display()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .current_dir(&root)
        .args([
            "ui-operation-conflict-resolve",
            "operation-conflicts.tsv",
            destination.to_str().unwrap(),
            "skip",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(&stderr, "ui operation conflict resolve", &root);
    assert!(
        !stderr
            .contains("security-worker-admission\tworker=ui operation conflict resolve\tpath=\t"),
        "{stderr}"
    );
    let stored = fs::read_to_string(root.join("operation-conflicts.tsv")).unwrap();
    assert!(stored.contains("\tpolicy=skip\t"), "{stored}");
    assert!(stored.contains("\tblocks-operation=false\t"), "{stored}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ui_operation_conflict_resolve_refuses_unreachable_store_before_mutating_from_binary() {
    let root = unique_temp_dir("gfm-cli-ui-operation-conflict-resolve-unreachable");
    let conflicts = root.join("operation-conflicts.tsv");
    let source = root.join("report.md");
    let destination = root.join("destination.md");
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(
        &conflicts,
        format!(
            "operation-conflict\toperation=copy\tsource={}\ttarget={}\texists=true\tkind=file\tpolicy=fail\tavailable=replace,keep-both,skip\tblocks-operation=true\treason=destination-conflict-requires-user-resolution\n",
            source.display(),
            destination.display()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "ui-operation-conflict-resolve",
            conflicts.to_str().unwrap(),
            destination.to_str().unwrap(),
            "skip",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("operation-conflict-control\tresolve\t"),
        "{stdout}"
    );
    assert!(
        stderr.contains(
            "ui operation conflict resolve volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert!(
        !stderr.contains("security-worker-admission\tworker=ui operation conflict resolve\t"),
        "{stderr}"
    );
    let stored = fs::read_to_string(&conflicts).unwrap();
    assert!(stored.contains("\tpolicy=fail\t"), "{stored}");
    assert!(stored.contains("\tblocks-operation=true\t"), "{stored}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ui_operation_conflict_resolve_reports_store_probe_failure_before_mutating_from_binary() {
    let root = unique_temp_dir("gfm-cli-ui-operation-conflict-resolve-probe");
    let conflicts = root.join(format!(
        "{}.tsv",
        "interface-conflicts-unavailable".repeat(16)
    ));
    let destination = root.join("destination.md");
    fs::write(&destination, "old report").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "ui-operation-conflict-resolve",
            conflicts.to_str().unwrap(),
            destination.to_str().unwrap(),
            "skip",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("operation-conflict-control\tresolve\t"),
        "{stdout}"
    );
    assert!(
        stderr.contains("interface write path metadata unavailable"),
        "{stderr}"
    );
    assert!(
        stderr.contains("interface-conflicts-unavailable"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("security-worker-admission\tworker=ui operation conflict resolve\t"),
        "{stderr}"
    );
    assert_eq!(fs::read_to_string(&destination).unwrap(), "old report");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn operation_conflict_store_refuses_unreachable_writes_before_recording_from_binary() {
    let root = unique_temp_dir("gfm-cli-operation-conflict-store-root");
    let offline = unique_temp_dir("gfm-cli-operation-conflict-store-unreachable");
    let journal = root.join("ops.journal");
    let conflicts = offline.join(format!(
        "{}.tsv",
        "operation-conflicts-unavailable".repeat(16)
    ));
    let source = root.join("report.md");
    let destination = root.join("destination.md");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(&source, "new report").unwrap();
    fs::write(&destination, "old report").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPS_JOURNAL", &journal)
        .env("GFM_OPERATION_CONFLICT_STORE", &conflicts)
        .args([
            "copy",
            source.to_str().unwrap(),
            destination.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr
            .contains("operation conflict store volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("operation write path metadata unavailable"),
        "{stderr}"
    );
    assert!(!conflicts.exists());
    assert_eq!(fs::read_to_string(&source).unwrap(), "new report");
    assert_eq!(fs::read_to_string(&destination).unwrap(), "old report");

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn operation_conflict_store_reports_write_probe_failure_before_recording_from_binary() {
    let root = unique_temp_dir("gfm-cli-operation-conflict-store-probe-root");
    let journal = root.join("ops.journal");
    let conflicts = root.join(format!(
        "{}.tsv",
        "operation-conflicts-unavailable".repeat(16)
    ));
    let source = root.join("report.md");
    let destination = root.join("destination.md");
    fs::write(&source, "new report").unwrap();
    fs::write(&destination, "old report").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPS_JOURNAL", &journal)
        .env("GFM_OPERATION_CONFLICT_STORE", &conflicts)
        .args([
            "copy",
            source.to_str().unwrap(),
            destination.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("\tcompleted"), "{stdout}");
    assert!(
        stderr.contains("runtime write path metadata unavailable")
            || stderr.contains("operation write path metadata unavailable"),
        "{stderr}"
    );
    assert!(
        stderr.contains("operation-conflicts-unavailable"),
        "{stderr}"
    );
    assert!(!conflicts.exists());
    assert_eq!(fs::read_to_string(&source).unwrap(), "new report");
    assert_eq!(fs::read_to_string(&destination).unwrap(), "old report");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn operation_conflict_store_refuses_unreachable_reads_before_applying_from_binary() {
    let root = unique_temp_dir("gfm-cli-operation-conflict-read-root");
    let offline = unique_temp_dir("gfm-cli-operation-conflict-read-unreachable");
    let journal = root.join("ops.journal");
    let conflicts = offline.join("operation-conflicts.tsv");
    let source = root.join("report.md");
    let destination = root.join("destination.md");
    let copied = root.join("destination copy.md");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(&source, "new report").unwrap();
    fs::write(&destination, "old report").unwrap();
    fs::write(
        &conflicts,
        format!(
            "operation-conflict\toperation=copy\tsource={}\ttarget={}\texists=true\tkind=file\tpolicy=fail\tavailable=replace,keep-both,skip\tblocks-operation=true\treason=destination-conflict-requires-user-resolution\n",
            source.display(),
            destination.display()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPS_JOURNAL", &journal)
        .args([
            "operation-conflict-apply",
            conflicts.to_str().unwrap(),
            destination.to_str().unwrap(),
            "keep-both",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("operation-conflict-control\tapply\t"),
        "{stdout}"
    );
    assert!(
        stderr
            .contains("operation conflict store volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert_eq!(fs::read_to_string(&source).unwrap(), "new report");
    assert_eq!(fs::read_to_string(&destination).unwrap(), "old report");
    assert!(!copied.exists());
    let stored = fs::read_to_string(&conflicts).unwrap();
    assert!(stored.contains("\tpolicy=fail\t"), "{stored}");
    assert!(stored.contains("\tblocks-operation=true\t"), "{stored}");

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn operation_conflict_apply_refuses_unreachable_operation_before_resolving_from_binary() {
    let root = unique_temp_dir("gfm-cli-operation-conflict-apply-unreachable-root");
    let offline = unique_temp_dir("gfm-cli-operation-conflict-apply-unreachable-volume");
    let journal = root.join("ops.journal");
    let conflicts = root.join("operation-conflicts.tsv");
    let source = offline.join("report.md");
    let destination = offline.join("destination.md");
    let copied = offline.join("destination copy.md");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(&source, "new report").unwrap();
    fs::write(&destination, "old report").unwrap();
    fs::write(
        &conflicts,
        format!(
            "operation-conflict\toperation=copy\tsource={}\ttarget={}\texists=true\tkind=file\tpolicy=fail\tavailable=replace,keep-both,skip\tblocks-operation=true\treason=destination-conflict-requires-user-resolution\n",
            source.display(),
            destination.display()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPS_JOURNAL", &journal)
        .args([
            "operation-conflict-apply",
            conflicts.to_str().unwrap(),
            destination.to_str().unwrap(),
            "keep-both",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("operation-conflict-control\tapply\t"),
        "{stdout}"
    );
    assert!(stderr.contains("unreachable volume network"), "{stderr}");
    assert_eq!(fs::read_to_string(&source).unwrap(), "new report");
    assert_eq!(fs::read_to_string(&destination).unwrap(), "old report");
    assert!(!copied.exists());
    let stored = fs::read_to_string(&conflicts).unwrap();
    assert!(stored.contains("\tpolicy=fail\t"), "{stored}");
    assert!(stored.contains("\tblocks-operation=true\t"), "{stored}");
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(journal_text.contains("\tstarted\t"), "{journal_text}");
    assert!(journal_text.contains("\tfailed\t"), "{journal_text}");
    assert!(
        journal_text.contains("unreachable volume network"),
        "{journal_text}"
    );
    assert!(!journal_text.contains("\tcompleted\t"), "{journal_text}");

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn operation_conflict_apply_keeps_store_blocking_when_execution_fails() {
    let root = unique_temp_dir("gfm-cli-operation-conflict-apply-failed-root");
    let journal = root.join("ops.journal");
    let conflicts = root.join("operation-conflicts.tsv");
    let missing_source = root.join("missing-report.md");
    let destination = root.join("destination.md");
    fs::write(&destination, "old report").unwrap();
    fs::write(
        &conflicts,
        format!(
            "operation-conflict\toperation=copy\tsource={}\ttarget={}\texists=true\tkind=file\tpolicy=fail\tavailable=replace,keep-both,skip\tblocks-operation=true\treason=destination-conflict-requires-user-resolution\n",
            missing_source.display(),
            destination.display()
        ),
    )
    .unwrap();

    let apply = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPS_JOURNAL", &journal)
        .args([
            "operation-conflict-apply",
            conflicts.to_str().unwrap(),
            destination.to_str().unwrap(),
            "keep-both",
        ])
        .output()
        .unwrap();
    assert!(
        !apply.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    let unresolved_store = fs::read_to_string(&conflicts).unwrap();
    assert!(
        unresolved_store.contains("\tpolicy=fail\t"),
        "{unresolved_store}"
    );
    assert!(
        unresolved_store.contains("\tblocks-operation=true\t"),
        "{unresolved_store}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn operation_conflict_apply_all_executes_blocking_batch_from_binary() {
    let root = unique_temp_dir("gfm-cli-operation-conflict-apply-all-root");
    let journal = root.join("ops.journal");
    let conflicts = root.join("operation-conflicts.tsv");
    let copy_source = root.join("copy-source.md");
    let copy_target = root.join("copy-target.md");
    let copy_keep_both = root.join("copy-target copy.md");
    let move_source = root.join("move-source");
    let move_target = root.join("move-target");
    let move_keep_both = root.join("move-target copy");
    fs::write(&copy_source, "incoming copy").unwrap();
    fs::write(&copy_target, "existing copy").unwrap();
    fs::create_dir_all(&move_source).unwrap();
    fs::create_dir_all(&move_target).unwrap();
    fs::write(move_source.join("new.txt"), "incoming move").unwrap();
    fs::write(move_target.join("old.txt"), "existing move").unwrap();
    fs::write(
        &conflicts,
        format!(
            "operation-conflict\toperation=copy\tsource={}\ttarget={}\texists=true\tkind=file\tpolicy=fail\tavailable=replace,keep-both,skip\tblocks-operation=true\treason=destination-conflict-requires-user-resolution\noperation-conflict\toperation=move\tsource={}\ttarget={}\texists=true\tkind=directory\tpolicy=fail\tavailable=replace,keep-both,merge,skip\tblocks-operation=true\treason=destination-conflict-requires-user-resolution\n",
            copy_source.display(),
            copy_target.display(),
            move_source.display(),
            move_target.display()
        ),
    )
    .unwrap();

    let apply = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPS_JOURNAL", &journal)
        .args([
            "operation-conflict-apply-all",
            conflicts.to_str().unwrap(),
            "keep-both",
        ])
        .output()
        .unwrap();
    assert!(
        apply.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    let stdout = String::from_utf8(apply.stdout).unwrap();
    assert!(
        stdout.contains("operation-conflict-control\tapply-all\tpolicy=keep-both\tresolved=2"),
        "{stdout}"
    );
    assert_eq!(fs::read_to_string(&copy_source).unwrap(), "incoming copy");
    assert_eq!(fs::read_to_string(&copy_target).unwrap(), "existing copy");
    assert_eq!(
        fs::read_to_string(&copy_keep_both).unwrap(),
        "incoming copy"
    );
    assert!(!move_source.exists());
    assert!(move_target.join("old.txt").exists());
    assert_eq!(
        fs::read_to_string(move_keep_both.join("new.txt")).unwrap(),
        "incoming move"
    );
    let resolved_store = fs::read_to_string(&conflicts).unwrap();
    assert_eq!(
        resolved_store.matches("\tblocks-operation=false\t").count(),
        2
    );
    assert_eq!(resolved_store.matches("\tpolicy=keep-both\t").count(), 2);
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(
        journal_text.contains("copy-target copy.md"),
        "{journal_text}"
    );
    assert!(journal_text.contains("move-target copy"), "{journal_text}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn operation_conflict_apply_all_rejects_unavailable_policy_before_mutation() {
    let root = unique_temp_dir("gfm-cli-operation-conflict-apply-all-reject-root");
    let journal = root.join("ops.journal");
    let conflicts = root.join("operation-conflicts.tsv");
    let source = root.join("copy-source.md");
    let target = root.join("copy-target.md");
    fs::write(&source, "incoming copy").unwrap();
    fs::write(&target, "existing copy").unwrap();
    fs::write(
        &conflicts,
        format!(
            "operation-conflict\toperation=copy\tsource={}\ttarget={}\texists=true\tkind=file\tpolicy=fail\tavailable=replace,keep-both,skip\tblocks-operation=true\treason=destination-conflict-requires-user-resolution\n",
            source.display(),
            target.display()
        ),
    )
    .unwrap();

    let apply = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPS_JOURNAL", &journal)
        .args([
            "operation-conflict-apply-all",
            conflicts.to_str().unwrap(),
            "merge",
        ])
        .output()
        .unwrap();
    assert!(
        !apply.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    assert_eq!(fs::read_to_string(&source).unwrap(), "incoming copy");
    assert_eq!(fs::read_to_string(&target).unwrap(), "existing copy");
    assert!(!root.join("copy-target copy.md").exists());
    let unresolved_store = fs::read_to_string(&conflicts).unwrap();
    assert!(
        unresolved_store.contains("\tblocks-operation=true\t"),
        "{unresolved_store}"
    );
    assert!(
        unresolved_store.contains("\tpolicy=fail\t"),
        "{unresolved_store}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn operation_conflict_apply_all_resolves_successful_prefix_on_later_failure() {
    let root = unique_temp_dir("gfm-cli-operation-conflict-apply-all-partial-root");
    let journal = root.join("ops.journal");
    let conflicts = root.join("operation-conflicts.tsv");
    let copy_source = root.join("copy-source.md");
    let copy_target = root.join("copy-target.md");
    let copy_keep_both = root.join("copy-target copy.md");
    let missing_source = root.join("missing-source.md");
    let failing_target = root.join("failing-target.md");
    fs::write(&copy_source, "incoming copy").unwrap();
    fs::write(&copy_target, "existing copy").unwrap();
    fs::write(&failing_target, "existing failing target").unwrap();
    fs::write(
        &conflicts,
        format!(
            "operation-conflict\toperation=copy\tsource={}\ttarget={}\texists=true\tkind=file\tpolicy=fail\tavailable=replace,keep-both,skip\tblocks-operation=true\treason=destination-conflict-requires-user-resolution\noperation-conflict\toperation=copy\tsource={}\ttarget={}\texists=true\tkind=file\tpolicy=fail\tavailable=replace,keep-both,skip\tblocks-operation=true\treason=destination-conflict-requires-user-resolution\n",
            copy_source.display(),
            copy_target.display(),
            missing_source.display(),
            failing_target.display()
        ),
    )
    .unwrap();

    let apply = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPS_JOURNAL", &journal)
        .args([
            "operation-conflict-apply-all",
            conflicts.to_str().unwrap(),
            "keep-both",
        ])
        .output()
        .unwrap();
    assert!(
        !apply.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    assert_eq!(
        fs::read_to_string(&copy_keep_both).unwrap(),
        "incoming copy"
    );
    assert_eq!(
        fs::read_to_string(&failing_target).unwrap(),
        "existing failing target"
    );
    let store_text = fs::read_to_string(&conflicts).unwrap();
    assert_eq!(store_text.matches("\tpolicy=keep-both\t").count(), 1);
    assert_eq!(store_text.matches("\tblocks-operation=false\t").count(), 1);
    assert_eq!(store_text.matches("\tblocks-operation=true\t").count(), 1);
    assert!(
        store_text.contains(&format!(
            "\tsource={}\ttarget={}\t",
            missing_source.display(),
            failing_target.display()
        )),
        "{store_text}"
    );
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(
        journal_text.contains("copy-target copy.md"),
        "{journal_text}"
    );
    assert!(journal_text.contains("\tfailed\t"), "{journal_text}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn copy_merge_from_binary_combines_directories_without_overwrite() {
    let root = unique_temp_dir("gfm-cli-ops-merge-root");
    let journal = root.join("ops.journal");
    let source = root.join("source");
    let destination = root.join("destination");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::create_dir_all(destination.join("nested")).unwrap();
    fs::write(source.join("nested").join("new.txt"), "new").unwrap();
    fs::write(destination.join("nested").join("old.txt"), "old").unwrap();

    run_gfm(
        &journal,
        [
            "copy",
            source.to_str().unwrap(),
            destination.to_str().unwrap(),
            "--merge",
        ],
    );

    assert_eq!(
        fs::read_to_string(destination.join("nested").join("new.txt")).unwrap(),
        "new"
    );
    assert_eq!(
        fs::read_to_string(destination.join("nested").join("old.txt")).unwrap(),
        "old"
    );
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(journal_text.contains("completed"), "{journal_text}");
    assert!(!journal_text.contains("failed"), "{journal_text}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn recovers_interrupted_operation_from_binary() {
    let root = unique_temp_dir("gfm-cli-ops-recover-root");
    let journal = root.join("ops.journal");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    fs::write(&source, "recoverable bytes").unwrap();
    fs::write(
        &journal,
        format!(
            "987\tstarted\t1\tcopy\t{}\t{}\t\n",
            source.to_string_lossy(),
            destination.to_string_lossy()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPS_JOURNAL", &journal)
        .args(["ops-recover", journal.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("987\tcompleted\tcopy\t"), "{stdout}");
    assert_eq!(
        fs::read_to_string(&destination).unwrap(),
        "recoverable bytes"
    );
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(journal_text.contains("987\tstarted"));
    assert!(journal_text.contains("987\tcompleted"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn recovers_paused_operation_from_binary() {
    let root = unique_temp_dir("gfm-cli-ops-recover-paused-root");
    let journal = root.join("ops.journal");
    let source = root.join("source");
    let destination = root.join("destination");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("first.txt"), "first").unwrap();
    fs::write(source.join("nested").join("second.txt"), "second").unwrap();
    fs::create_dir_all(&destination).unwrap();
    fs::write(
        &journal,
        format!(
            "989\tstarted\t1\tcopy\t{}\t{}\t\n989\tpaused\t2\tcopy\t{}\t{}\t\n",
            source.to_string_lossy(),
            destination.to_string_lossy(),
            source.to_string_lossy(),
            destination.to_string_lossy()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPS_JOURNAL", &journal)
        .args(["ops-recover", journal.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("989\tcompleted\tcopy\t"), "{stdout}");
    assert_eq!(
        fs::read_to_string(destination.join("first.txt")).unwrap(),
        "first"
    );
    assert_eq!(
        fs::read_to_string(destination.join("nested").join("second.txt")).unwrap(),
        "second"
    );
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(journal_text.contains("989\tpaused"), "{journal_text}");
    assert_eq!(journal_text.matches("989\tstarted").count(), 2);
    assert!(journal_text.contains("989\tcompleted"), "{journal_text}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ops_recover_refuses_unreachable_journal_before_reading_from_binary() {
    let root = unique_temp_dir("gfm-cli-ops-recover-journal-preflight-root");
    let offline = unique_temp_dir("gfm-cli-ops-recover-journal-preflight-offline");
    let journal = offline.join("ops.journal");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(&source, "recoverable bytes").unwrap();
    fs::write(
        &journal,
        format!(
            "991\tstarted\t1\tcopy\t{}\t{}\t\n",
            source.to_string_lossy(),
            destination.to_string_lossy()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["ops-recover", journal.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("991\tcompleted\tcopy\t"), "{stdout}");
    assert!(
        stderr.contains("operation journal volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert_eq!(fs::read_to_string(&source).unwrap(), "recoverable bytes");
    assert!(!destination.exists());

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn restores_trash_entry_from_binary_using_metadata_destination() {
    let root = unique_temp_dir("gfm-cli-ops-restore-root");
    let journal = root.join("ops.journal");
    let metadata = root.join("trash.tsv");
    let trash_dir = root.join("Trash");
    let original_dir = root.join("Documents");
    let trashed = trash_dir.join("report.md");
    let original = original_dir.join("report.md");
    fs::create_dir_all(&trash_dir).unwrap();
    fs::create_dir_all(&original_dir).unwrap();
    fs::write(&trashed, "restored bytes").unwrap();
    fs::write(
        &metadata,
        format!(
            "report.md\t{}\t7\ttrue\ttrue\t\n",
            original.to_string_lossy()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPS_JOURNAL", &journal)
        .env("GFM_TRASH_METADATA", &metadata)
        .args(["restore", trashed.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("\tcompleted"), "{stdout}");
    assert!(!trashed.exists());
    assert_eq!(fs::read_to_string(&original).unwrap(), "restored bytes");
    assert!(fs::read_to_string(&metadata).unwrap().trim().is_empty());
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(journal_text.contains("\trestore\t"), "{journal_text}");
    assert!(journal_text.contains("\tcompleted\t"), "{journal_text}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn restores_trash_entry_from_binary_with_metadata_destination_and_replace() {
    let root = unique_temp_dir("gfm-cli-ops-restore-replace-root");
    let journal = root.join("ops.journal");
    let metadata = root.join("trash.tsv");
    let trash_dir = root.join("Trash");
    let original_dir = root.join("Documents");
    let trashed = trash_dir.join("report.md");
    let original = original_dir.join("report.md");
    fs::create_dir_all(&trash_dir).unwrap();
    fs::create_dir_all(&original_dir).unwrap();
    fs::write(&trashed, "replacement").unwrap();
    fs::write(&original, "existing").unwrap();
    fs::write(
        &metadata,
        format!(
            "report.md\t{}\t8\ttrue\ttrue\t\n",
            original.to_string_lossy()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPS_JOURNAL", &journal)
        .env("GFM_TRASH_METADATA", &metadata)
        .args(["restore", trashed.to_str().unwrap(), "--replace"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(!trashed.exists());
    assert_eq!(fs::read_to_string(&original).unwrap(), "replacement");
    assert!(fs::read_to_string(&metadata).unwrap().trim().is_empty());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn trash_refuses_unreachable_metadata_before_mutating_from_binary() {
    let root = unique_temp_dir("gfm-cli-ops-trash-metadata-preflight-root");
    let offline = unique_temp_dir("gfm-cli-ops-trash-metadata-preflight-offline");
    let journal = root.join("ops.journal");
    let metadata = offline.join(format!("{}.tsv", "trash-metadata-unavailable".repeat(16)));
    let file = root.join("report.md");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(&file, "do not trash without metadata access").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPS_JOURNAL", &journal)
        .env("GFM_TRASH_METADATA", &metadata)
        .args(["trash", file.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("\tcompleted"), "{stdout}");
    assert!(
        stderr.contains("trash metadata volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("operation write path metadata unavailable"),
        "{stderr}"
    );
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "do not trash without metadata access"
    );
    assert!(!journal.exists());
    assert!(!metadata.exists());

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn restore_refuses_unreachable_metadata_before_resolving_default_destination_from_binary() {
    let root = unique_temp_dir("gfm-cli-ops-restore-metadata-preflight-root");
    let offline = unique_temp_dir("gfm-cli-ops-restore-metadata-preflight-offline");
    let metadata = offline.join("trash.tsv");
    let trash_dir = root.join("Trash");
    let trashed = trash_dir.join("report.md");
    let original = root.join("Documents").join("report.md");
    fs::create_dir_all(&trash_dir).unwrap();
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(&trashed, "restore only after metadata read is allowed").unwrap();
    fs::write(
        &metadata,
        format!(
            "report.md\t{}\t7\ttrue\ttrue\t\n",
            original.to_string_lossy()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_TRASH_METADATA", &metadata)
        .args(["restore", trashed.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("\tcompleted"), "{stdout}");
    assert!(
        stderr.contains("trash metadata volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert_eq!(
        fs::read_to_string(&trashed).unwrap(),
        "restore only after metadata read is allowed"
    );
    assert!(!original.exists());

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn permanently_deletes_trash_entry_from_binary_and_removes_metadata() {
    let root = unique_temp_dir("gfm-cli-ops-permanent-delete-root");
    let journal = root.join("ops.journal");
    let metadata = root.join("trash.tsv");
    let trash_dir = root.join("Trash");
    let trashed = trash_dir.join("report.md");
    let original = root.join("Documents").join("report.md");
    fs::create_dir_all(&trash_dir).unwrap();
    fs::write(&trashed, "delete forever").unwrap();
    fs::write(
        &metadata,
        format!(
            "report.md\t{}\t9\ttrue\ttrue\t\n",
            original.to_string_lossy()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPS_JOURNAL", &journal)
        .env("GFM_TRASH_METADATA", &metadata)
        .args(["delete", trashed.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("\tcompleted"), "{stdout}");
    assert!(!trashed.exists());
    assert!(fs::read_to_string(&metadata).unwrap().trim().is_empty());
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(journal_text.contains("\tdelete\t"), "{journal_text}");
    assert!(journal_text.contains("\tcompleted\t"), "{journal_text}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn empties_trash_from_binary_and_removes_metadata() {
    let root = unique_temp_dir("gfm-cli-ops-empty-trash-root");
    let journal = root.join("ops.journal");
    let metadata = root.join("trash.tsv");
    let trash_dir = root.join("Trash");
    let trashed_file = trash_dir.join("report.md");
    let trashed_dir = trash_dir.join("Old Folder");
    fs::create_dir_all(trashed_dir.join("nested")).unwrap();
    fs::write(&trashed_file, "delete file").unwrap();
    fs::write(trashed_dir.join("nested").join("note.txt"), "delete folder").unwrap();
    fs::write(
        &metadata,
        format!(
            "report.md\t{}\t11\ttrue\ttrue\t\nOld Folder\t{}\t12\ttrue\ttrue\t\n",
            root.join("Documents").join("report.md").to_string_lossy(),
            root.join("Documents").join("Old Folder").to_string_lossy()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPS_JOURNAL", &journal)
        .env("GFM_TRASH_METADATA", &metadata)
        .args(["empty-trash", trash_dir.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("\tcompleted"), "{stdout}");
    assert!(trash_dir.exists());
    assert!(fs::read_dir(&trash_dir).unwrap().next().is_none());
    assert!(fs::read_to_string(&metadata).unwrap().trim().is_empty());
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(journal_text.contains("\tempty-trash\t"), "{journal_text}");
    assert!(journal_text.contains("\tcompleted\t"), "{journal_text}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn empty_trash_from_binary_reconciles_stale_metadata() {
    let root = unique_temp_dir("gfm-cli-ops-empty-trash-stale-root");
    let journal = root.join("ops.journal");
    let metadata = root.join("trash.tsv");
    let trash_dir = root.join("Trash");
    fs::create_dir_all(&trash_dir).unwrap();
    fs::write(
        &metadata,
        format!(
            "already-deleted.md\t{}\t14\ttrue\ttrue\t\n",
            root.join("Documents")
                .join("already-deleted.md")
                .to_string_lossy()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPS_JOURNAL", &journal)
        .env("GFM_TRASH_METADATA", &metadata)
        .args(["empty-trash", trash_dir.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(trash_dir.exists());
    assert!(fs::read_dir(&trash_dir).unwrap().next().is_none());
    assert!(fs::read_to_string(&metadata).unwrap().trim().is_empty());
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(journal_text.contains("\tempty-trash\t"), "{journal_text}");
    assert!(journal_text.contains("\tcompleted\t"), "{journal_text}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ui_trash_view_refuses_unreachable_restore_metadata_before_rendering_from_binary() {
    let root = unique_temp_dir("gfm-cli-ui-trash-view-root");
    let offline = unique_temp_dir("gfm-cli-ui-trash-metadata-unreachable");
    let trash_dir = root.join("Trash");
    let trashed = trash_dir.join("report.md");
    let metadata = offline.join("trash.tsv");
    fs::create_dir_all(&trash_dir).unwrap();
    fs::write(&trashed, "trashed bytes").unwrap();
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(
        &metadata,
        format!(
            "{}\t{}\t2026-08-27T04:00:00Z\ttrue\ttrue\t\n",
            trashed.file_name().unwrap().to_string_lossy(),
            root.join("report.md").display()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "ui-trash-view-contract",
            trash_dir.to_str().unwrap(),
            metadata.to_str().unwrap(),
            "6",
            "0",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("trash-view\t"), "{stdout}");
    assert!(
        stderr.contains("ui trash metadata volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=ui trash metadata\tpath={}",
            metadata.display()
        )),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn retries_failed_operation_from_binary_when_policy_allows_it() {
    let root = unique_temp_dir("gfm-cli-ops-retry-root");
    let journal = root.join("ops.journal");
    let source = root.join("late-source.txt");
    let destination = root.join("destination.txt");
    fs::write(
        &journal,
        format!(
            "988\tstarted\t1\tcopy\t{}\t{}\t\n988\tfailed\t2\tcopy\t{}\t{}\t{}: source does not exist\n",
            source.to_string_lossy(),
            destination.to_string_lossy(),
            source.to_string_lossy(),
            destination.to_string_lossy(),
            source.to_string_lossy()
        ),
    )
    .unwrap();
    fs::write(&source, "late bytes").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "ops-recover",
            journal.to_str().unwrap(),
            "--retry-failed",
            "--max-attempts",
            "2",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("988\tcompleted\tcopy\t"), "{stdout}");
    assert_eq!(fs::read_to_string(&destination).unwrap(), "late bytes");
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert_eq!(journal_text.matches("988\tstarted").count(), 2);
    assert!(journal_text.contains("988\tcompleted"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn searches_text_content_from_binary() {
    let root = unique_temp_dir("gfm-cli-content-root");
    fs::write(root.join("journal.md"), "the body contains superneedle").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["search-content", root.to_str().unwrap(), "superneedle"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("journal.md"), "{stdout}");
    assert!(stdout.contains("[[superneedle]]"), "{stdout}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn adaptive_search_content_applies_pressure_budget_from_binary() {
    let root = unique_temp_dir("gfm-cli-content-pressure-root");
    let mut body = "x".repeat(1024 * 1024 + 1);
    body.push_str(" directpressuremarker");
    fs::write(root.join("large.md"), body).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-adaptive",
            root.to_str().unwrap(),
            "directpressuremarker",
            "elevated",
            "serious",
            "low",
            "active",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("content-indexed 0 files"), "{stderr}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains("large.md"), "{stdout}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn searches_multipart_email_content_from_binary() {
    let root = unique_temp_dir("gfm-cli-email-content-root");
    fs::write(
        root.join("message.eml"),
        br#"From: Ada <ada@example.com>
To: Team
Subject: Multipart Search
Content-Type: multipart/mixed; boundary="outer"

--outer
Content-Type: text/plain; charset=utf-8
Content-Transfer-Encoding: quoted-printable

Plain mail has emailmultipartneedle=20inside
--outer
Content-Type: application/octet-stream
Content-Disposition: attachment; filename="secret.txt"

attachmentneedle should not appear
--outer--
"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content",
            root.to_str().unwrap(),
            "emailmultipartneedle",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("message.eml"), "{stdout}");
    assert!(stdout.contains("[[emailmultipartneedle]]"), "{stdout}");
    assert!(!stdout.contains("attachmentneedle"), "{stdout}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn searches_pdf_content_from_binary() {
    let root = unique_temp_dir("gfm-cli-pdf-content-root");
    fs::write(root.join("brief.pdf"), minimal_pdf("pdfneedle lives here")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["search-content", root.to_str().unwrap(), "pdfneedle"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("brief.pdf"), "{stdout}");
    assert!(stdout.contains("[[pdfneedle]]"), "{stdout}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn searches_docx_content_from_binary() {
    let root = unique_temp_dir("gfm-cli-docx-content-root");
    fs::write(
        root.join("brief.docx"),
        ooxml_package(&[(
            "word/document.xml",
            "<w:document><w:body><w:p><w:r><w:t>docxneedle lives here</w:t></w:r></w:p></w:body></w:document>",
        )]),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["search-content", root.to_str().unwrap(), "docxneedle"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("brief.docx"), "{stdout}");
    assert!(stdout.contains("[[docxneedle]]"), "{stdout}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn searches_zip_archive_metadata_from_binary() {
    let root = unique_temp_dir("gfm-cli-zip-content-root");
    fs::write(
        root.join("bundle.zip"),
        ooxml_package(&[("docs/zipneedle.txt", "payload")]),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["search-content", root.to_str().unwrap(), "zipneedle"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("bundle.zip"), "{stdout}");
    assert!(stdout.contains("[[zipneedle]]"), "{stdout}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn searches_tar_archive_metadata_from_binary() {
    let root = unique_temp_dir("gfm-cli-tar-content-root");
    fs::write(
        root.join("bundle.tar"),
        tar_package(&[("docs/tarneedle.txt", "payload")]),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["search-content", root.to_str().unwrap(), "tarneedle"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("bundle.tar"), "{stdout}");
    assert!(stdout.contains("[[tarneedle]]"), "{stdout}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn searches_pax_tar_archive_metadata_from_binary() {
    let root = unique_temp_dir("gfm-cli-pax-tar-content-root");
    fs::write(
        root.join("bundle.tar"),
        tar_pax_package("deep/archive/path/paxbinaryneedle.txt", "payload"),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["search-content", root.to_str().unwrap(), "paxbinaryneedle"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("bundle.tar"), "{stdout}");
    assert!(stdout.contains("[[paxbinaryneedle]]"), "{stdout}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn searches_compressed_tar_archive_metadata_from_binary() {
    let root = unique_temp_dir("gfm-cli-targz-content-root");
    fs::write(
        root.join("bundle.tar.gz"),
        tar_gz_package(&[("docs/targzneedle.txt", "payload")]),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["search-content", root.to_str().unwrap(), "targzneedle"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("bundle.tar.gz"), "{stdout}");
    assert!(stdout.contains("[[targzneedle]]"), "{stdout}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn searches_json_content_from_binary() {
    let root = unique_temp_dir("gfm-cli-json-content-root");
    fs::write(
        root.join("data.json"),
        br#"{"client":"Aperture","marker":"jsonneedle"}"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["search-content", root.to_str().unwrap(), "jsonneedle"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("data.json"), "{stdout}");
    assert!(stdout.contains("[[jsonneedle]]"), "{stdout}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn search_content_skips_disguised_binary_from_binary() {
    let root = unique_temp_dir("gfm-cli-disguised-binary-root");
    fs::write(
        root.join("fake.txt"),
        b"\x89PNG\r\n\x1a\nsuperneedle hidden in binary payload",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["search-content", root.to_str().unwrap(), "superneedle"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.is_empty(), "{stdout}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reports_compressed_pdf_extraction_from_binary() {
    let root = unique_temp_dir("gfm-cli-extract-report-root");
    let path = root.join("Compressed.pdf");
    fs::write(&path, compressed_pdf("compressed report needle")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["extract-report", path.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("security-scope\t"), "{stderr}");
    assert_eq!(
        stderr
            .matches(&format!(
                "security-worker-admission\tworker=content extraction\tpath={}",
                path.display()
            ))
            .count(),
        1,
        "{stderr}"
    );
    assert!(stderr.contains("\tintent=read\t"), "{stderr}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("extract\tpath="), "{stdout}");
    assert!(
        stdout.contains("\tformat=pdf\tstatus=extracted\t"),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "\tversion={}\t",
            extractor_version_for_path(&path)
        )),
        "{stdout}"
    );
    assert!(stdout.contains("quarantine\tallow"), "{stdout}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extract_report_retries_transient_failure_from_binary() {
    let root = unique_temp_dir("gfm-cli-extract-report-retry-root");
    let path = root.join("Retry.txt");
    let journal = unique_temp_path("gfm-cli-extract-report-retry", "journal");
    let catalog = unique_temp_path("gfm-cli-extract-report-retry", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-extract-report-retry", "gfmprogress");
    let retry_probe = unique_temp_path("gfm-cli-extract-report-retry", "state");
    fs::write(&path, "visible extraction retry marker").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "extract-report-retry-probe",
            path.to_str().unwrap(),
            retry_probe.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("extract\tpath="), "{stdout}");
    assert!(
        stdout.contains("\tformat=text\tstatus=extracted\t"),
        "{stdout}"
    );
    assert_eq!(fs::read_to_string(&retry_probe).unwrap(), "2");
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(
        journal_text.contains("1\t1\tstarted\tcontent extraction"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains(
            "1\t1\tfailed:temporary content extraction retry probe busy\tcontent extraction"
        ),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tstarted\tcontent extraction"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tcompleted\tcontent extraction"),
        "{journal_text}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(journal).unwrap();
    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
    fs::remove_file(retry_probe).unwrap();
}

#[test]
fn extract_report_refuses_unreachable_volume_before_extraction_from_binary() {
    let root = unique_temp_dir("gfm-cli-extract-report-unreachable");
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let path = root.join("Offline.pdf");
    fs::write(&path, minimal_pdf("offline extraction")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["extract-report", path.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("extract\t"), "{stdout}");
    assert!(
        stderr.contains("content extraction volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=content extraction\tpath={}",
            path.display()
        )),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extract_report_refuses_missing_path_before_extraction_from_binary() {
    let path = unique_temp_path("gfm-cli-extract-missing-path", "txt");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["extract-report", path.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("security-scope\t"), "{stderr}");
    assert_eq!(stderr.matches("security-scope\t").count(), 1, "{stderr}");
    assert!(stderr.contains("\tintent=read\t"), "{stderr}");
    assert!(stderr.contains("\taction=deny\t"), "{stderr}");
    assert!(
        stderr.contains("content extraction access blocked: path is not present on this host"),
        "{stderr}"
    );
}

#[test]
fn adaptive_extraction_worker_applies_pressure_budget_from_binary() {
    let root = unique_temp_dir("gfm-cli-extract-worker-budget-root");
    let path = root.join("large.txt");
    let catalog = unique_temp_path("gfm-cli-extract-worker-runtime", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-extract-worker-runtime", "gfmprogress");
    fs::write(&path, "x".repeat(1024 * 1024 + 1)).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "extract-worker-adaptive",
            path.to_str().unwrap(),
            "elevated",
            "serious",
            "low",
            "active",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("security-scope\t"), "{stderr}");
    assert!(stderr.contains("\tintent=read\t"), "{stderr}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("\tformat=text\t"), "{stdout}");
    assert!(stdout.contains("\tstatus=skipped\t"), "{stdout}");
    assert!(stdout.contains("\treason=too-large\t"), "{stdout}");
    assert!(stdout.contains("\tbytes-read=0\t"), "{stdout}");
    let catalog_text = fs::read_to_string(&catalog).unwrap();
    assert!(catalog_text.contains("\textraction\t"), "{catalog_text}");
    assert!(
        catalog_text.contains("adaptive extraction"),
        "{catalog_text}"
    );
    assert!(
        catalog_text.contains(&path.display().to_string()),
        "{catalog_text}"
    );
    let progress_text = fs::read_to_string(&progress).unwrap();
    assert!(
        progress_text.contains("progress\t1\tbackground\tbackground\tadaptive extraction"),
        "{progress_text}"
    );
    assert!(
        progress_text.contains("\tcompleted\t1\t1\tcompleted\t"),
        "{progress_text}"
    );

    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn adaptive_extraction_worker_applies_network_volume_descriptor_budget_from_binary() {
    let root = unique_temp_dir("gfm-cli-extract-worker-network-budget-root");
    fs::write(root.join(".gfm-volume-kind"), "network-smb\n").unwrap();
    let path = root.join("network-large.txt");
    let catalog = unique_temp_path("gfm-cli-extract-worker-network-runtime", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-extract-worker-network-runtime", "gfmprogress");
    fs::write(&path, "x".repeat(1024 * 1024 + 1)).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "extract-worker-adaptive",
            path.to_str().unwrap(),
            "nominal",
            "nominal",
            "ac",
            "idle",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("security-scope\t"), "{stderr}");
    assert!(
        stderr.contains(&format!(
            "security-worker-admission\tworker=adaptive extraction worker\tpath={}",
            path.display()
        )),
        "{stderr}"
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("\tformat=text\t"), "{stdout}");
    assert!(stdout.contains("\tstatus=skipped\t"), "{stdout}");
    assert!(stdout.contains("\treason=too-large\t"), "{stdout}");
    assert!(stdout.contains("\tbytes-read=0\t"), "{stdout}");
    let catalog_text = fs::read_to_string(&catalog).unwrap();
    assert!(
        catalog_text.contains("adaptive extraction"),
        "{catalog_text}"
    );
    let progress_text = fs::read_to_string(&progress).unwrap();
    assert!(
        progress_text.contains("\tcompleted\t1\t1\tcompleted\t"),
        "{progress_text}"
    );

    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn deferred_adaptive_extraction_worker_does_not_touch_unreachable_target_from_binary() {
    let root = unique_temp_dir("gfm-cli-extract-worker-deferred-unreachable");
    let path = root.join("document.txt");
    let catalog = unique_temp_path("gfm-cli-extract-worker-deferred-runtime", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-extract-worker-deferred-runtime", "gfmprogress");
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(&path, "deferred worker marker").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "extract-worker-adaptive",
            path.to_str().unwrap(),
            "saturated",
            "nominal",
            "ac",
            "idle",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("extract\t"), "{stdout}");
    assert!(
        stderr.contains("adaptive-extraction-deferred\taction=Defer"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "\tworker=adaptive extraction worker\tpath={}",
            path.display()
        )),
        "{stderr}"
    );
    let catalog_text = fs::read_to_string(&catalog).unwrap();
    assert!(catalog_text.contains("\textraction\t"), "{catalog_text}");
    assert!(
        catalog_text.contains("adaptive extraction"),
        "{catalog_text}"
    );
    assert!(
        catalog_text.contains(&path.display().to_string()),
        "{catalog_text}"
    );
    let progress_text = fs::read_to_string(&progress).unwrap();
    assert!(
        progress_text.contains("progress\t1\tbackground\tbackground\tadaptive extraction"),
        "{progress_text}"
    );
    assert!(
        progress_text.contains("\tpaused\t0\t1\tdeferred:Defer\t"),
        "{progress_text}"
    );

    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn adaptive_extraction_worker_refuses_unreachable_scratch_before_launch_from_binary() {
    let root = unique_temp_dir("gfm-cli-extract-worker-scratch-root");
    let scratch = unique_temp_dir("gfm-cli-extract-worker-scratch-unreachable");
    let path = root.join("document.txt");
    let permission_state = root.join("permission-state.tsv");
    fs::write(&path, "scratch preflight marker").unwrap();
    fs::write(scratch.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("TMPDIR", &scratch)
        .env("GFM_PERMISSION_STATE", &permission_state)
        .args([
            "extract-worker-adaptive",
            path.to_str().unwrap(),
            "nominal",
            "nominal",
            "ac",
            "idle",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("extract\t"), "{stdout}");
    assert!(
        stderr.contains("adaptive extraction volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "\tworker=adaptive extraction worker\tpath={}",
            path.display()
        )),
        "{stderr}"
    );
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "scratch preflight marker"
    );
    let scratch_entries = fs::read_dir(&scratch)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        scratch_entries,
        vec![".gfm-volume-kind"],
        "{scratch_entries:?}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn adaptive_extraction_worker_refuses_unprobeable_scratch_volume_before_launch_from_binary() {
    let root = unique_temp_dir("gfm-cli-extract-worker-scratch-probe-root");
    let path = root.join("document.txt");
    let unavailable_scratch = root.join("scratch-unavailable".repeat(16));
    fs::write(&path, "scratch probe marker").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("TMPDIR", &unavailable_scratch)
        .args([
            "extract-worker-adaptive",
            path.to_str().unwrap(),
            "nominal",
            "nominal",
            "ac",
            "idle",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("extract\t"), "{stdout}");
    assert!(
        stderr.contains("adaptive extraction volume access blocked"),
        "{stderr}"
    );
    assert!(stderr.contains("read-only volume system"), "{stderr}");
    assert!(stderr.contains("scratch-unavailable"), "{stderr}");
    assert!(
        !stderr.contains("security-worker-admission\tworker=adaptive extraction stdout\t"),
        "{stderr}"
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "scratch probe marker");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn fileprovider_progress_job_persists_runtime_payload_and_progress_from_binary() {
    let root = unique_temp_dir("gfm-cli-fileprovider-progress-runtime-root");
    let item = root.join("Remote.icloud-downloading");
    fs::write(&item, "downloading").unwrap();
    xattr::set(&item, "com.apple.fileprovider.state", b"downloading").unwrap();
    let catalog = unique_temp_path("gfm-cli-fileprovider-progress-runtime", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-fileprovider-progress-runtime", "gfmprogress");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args(["fileprovider-progress-job", item.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("fileprovider-progress\t"), "{stdout}");
    assert!(
        stdout.contains("\tstate=downloading\tprogress-direction=download\t"),
        "{stdout}"
    );
    assert!(
        stdout.contains("\tprogress-indeterminate=true\t"),
        "{stdout}"
    );

    let catalog_text = fs::read_to_string(&catalog).unwrap();
    assert!(
        catalog_text.contains("\toperation\tfileprovider download\t"),
        "{catalog_text}"
    );
    assert!(
        catalog_text.contains(&item.display().to_string()),
        "{catalog_text}"
    );
    assert!(
        catalog_text.contains(
            "fileprovider:icloud-drive:downloading:download:state:-:provider-progress-unavailable"
        ),
        "{catalog_text}"
    );

    let progress_text = fs::read_to_string(&progress).unwrap();
    assert!(
        progress_text.contains("progress\t1\tvisible\tvisible\tfileprovider download\t"),
        "{progress_text}"
    );
    assert!(
        progress_text.contains(
            "\trunning\t0\t1\tfileprovider:icloud-drive:downloading:download:state:-:provider-progress-unavailable\t"
        ),
        "{progress_text}"
    );

    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ui_fileprovider_sidebar_state_reads_on_visible_worker_from_binary() {
    let root = unique_temp_dir("gfm-cli-ui-fileprovider-sidebar-state-root");
    let current = root.join("Desktop");
    let item = root.join("Remote.icloud-placeholder");
    fs::create_dir_all(&current).unwrap();
    fs::write(&item, "remote placeholder").unwrap();
    xattr::set(&item, "com.apple.icloud.placeholder", b"1").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "ui-sidebar-fileprovider-contract",
            current.to_str().unwrap(),
            item.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(&stderr, "ui fileprovider sidebar state", &item);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("sidebar\t"), "{stdout}");
    assert!(stdout.contains("icloud"), "{stdout}");
    assert!(stdout.contains("cloud"), "{stdout}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ui_fileprovider_sidebar_invalidation_reads_on_visible_worker_from_binary() {
    let root = unique_temp_dir("gfm-cli-ui-fileprovider-sidebar-invalidation-root");
    let item = root.join("Remote.icloud-placeholder");
    fs::write(&item, "remote placeholder").unwrap();
    xattr::set(&item, "com.apple.icloud.placeholder", b"1").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "ui-sidebar-fileprovider-invalidation",
            "downloaded",
            item.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(&stderr, "ui fileprovider sidebar invalidation", &item);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("sidebar-cloud-invalidation\t"), "{stdout}");
    assert!(
        stdout.contains("\tprevious=available-offline\t"),
        "{stdout}"
    );
    assert!(stdout.contains("\tcurrent=cloud-only\t"), "{stdout}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ui_fileprovider_sidebar_routes_refuse_unreachable_provider_before_reading_from_binary() {
    let root = unique_temp_dir("gfm-cli-ui-fileprovider-sidebar-unreachable");
    let current = root.join("Desktop");
    let item = root.join("Remote.icloud-placeholder");
    fs::create_dir_all(&current).unwrap();
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(&item, "remote placeholder").unwrap();

    for args in [
        vec![
            "ui-sidebar-fileprovider-contract".to_string(),
            current.display().to_string(),
            item.display().to_string(),
        ],
        vec![
            "ui-sidebar-fileprovider-invalidation".to_string(),
            "downloaded".to_string(),
            item.display().to_string(),
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
            .args(&args)
            .output()
            .unwrap();

        assert!(!output.status.success(), "{args:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stdout.is_empty(), "{args:?}: {stdout}");
        assert!(
            stderr.contains("volume access blocked: unreachable volume network"),
            "{args:?}: {stderr}"
        );
        assert!(
            !stderr.contains("security-worker-admission\tworker=ui fileprovider sidebar"),
            "{args:?}: {stderr}"
        );
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ui_fileprovider_observed_invalidation_persists_snapshot_on_visible_worker_from_binary() {
    let root = unique_temp_dir("gfm-cli-ui-fileprovider-observed-root");
    let state = root.join("fileprovider-state.tsv");
    let item = root.join("Remote.icloud-placeholder");
    fs::write(&item, "remote placeholder").unwrap();
    xattr::set(&item, "com.apple.icloud.placeholder", b"1").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "ui-sidebar-fileprovider-observed-invalidation",
            state.to_str().unwrap(),
            "metadata",
            item.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(
        &stderr,
        "ui fileprovider sidebar observed invalidation",
        &root,
    );
    assert_worker_admitted(
        &stderr,
        "ui fileprovider sidebar observed invalidation",
        &item,
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains(
            "fileprovider-observed-invalidation\tevents=1\tevent-kinds=metadata\tpaths=1"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains("fileprovider-state-invalidation\t"),
        "{stdout}"
    );
    assert!(stdout.contains("sidebar-cloud-invalidation\t"), "{stdout}");

    let stored = fs::read_to_string(&state).unwrap();
    assert!(
        stored.starts_with("gfm-fileprovider-state-v1\n"),
        "{stored}"
    );
    assert!(stored.contains(&item.display().to_string()), "{stored}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ui_fileprovider_observed_invalidation_refuses_unreachable_state_before_snapshot_io_from_binary()
{
    let root = unique_temp_dir("gfm-cli-ui-fileprovider-observed-unreachable");
    let state = root.join("fileprovider-state.tsv");
    let item = root.join("Remote.icloud-placeholder");
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(&item, "remote placeholder").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "ui-sidebar-fileprovider-observed-invalidation",
            state.to_str().unwrap(),
            "metadata",
            item.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("fileprovider-observed-invalidation\t"),
        "{stdout}"
    );
    assert!(
        stderr.contains(
            "ui fileprovider sidebar observed invalidation volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert!(
        !stderr.contains(
            "security-worker-admission\tworker=ui fileprovider sidebar observed invalidation\t"
        ),
        "{stderr}"
    );
    assert!(!state.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn fileprovider_invalidation_scan_persists_snapshot_on_visible_worker_from_binary() {
    let root = unique_temp_dir("gfm-cli-fileprovider-scan-root");
    let state = root.join("fileprovider-state.tsv");
    let item = root.join("Remote.icloud-placeholder");
    fs::write(&item, "remote placeholder").unwrap();
    xattr::set(&item, "com.apple.icloud.placeholder", b"1").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "fileprovider-invalidation-scan",
            state.to_str().unwrap(),
            item.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(&stderr, "fileprovider invalidation scan", &root);
    assert_worker_admitted(&stderr, "fileprovider invalidation scan", &item);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("fileprovider-state-invalidation\t"),
        "{stdout}"
    );
    assert!(stdout.contains(&item.display().to_string()), "{stdout}");

    let stored = fs::read_to_string(&state).unwrap();
    assert!(
        stored.starts_with("gfm-fileprovider-state-v1\n"),
        "{stored}"
    );
    assert!(stored.contains(&item.display().to_string()), "{stored}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn fileprovider_invalidation_event_persists_snapshot_on_visible_worker_from_binary() {
    let root = unique_temp_dir("gfm-cli-fileprovider-event-root");
    let state = root.join("fileprovider-state.tsv");
    let item = root.join("Remote.icloud-placeholder");
    fs::write(&item, "remote placeholder").unwrap();
    xattr::set(&item, "com.apple.icloud.placeholder", b"1").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "fileprovider-invalidation-event",
            state.to_str().unwrap(),
            "metadata",
            item.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(&stderr, "fileprovider invalidation event", &root);
    assert_worker_admitted(&stderr, "fileprovider invalidation event", &item);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains(
            "fileprovider-observed-invalidation\tevents=1\tevent-kinds=metadata\tpaths=1"
        ),
        "{stdout}"
    );

    let stored = fs::read_to_string(&state).unwrap();
    assert!(
        stored.starts_with("gfm-fileprovider-state-v1\n"),
        "{stored}"
    );
    assert!(stored.contains(&item.display().to_string()), "{stored}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn fileprovider_observed_metadata_invalidation_persists_snapshot_on_visible_worker_from_binary() {
    let root = unique_temp_dir("gfm-cli-fileprovider-metadata-root");
    let state = root.join("fileprovider-state.tsv");
    let item = root.join("Remote.icloud-placeholder");
    fs::write(&item, "remote placeholder").unwrap();
    xattr::set(&item, "com.apple.icloud.placeholder", b"1").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "fileprovider-observed-metadata-invalidation",
            state.to_str().unwrap(),
            "metadata",
            item.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(
        &stderr,
        "fileprovider observed metadata invalidation",
        &root,
    );
    assert_worker_admitted(
        &stderr,
        "fileprovider observed metadata invalidation",
        &item,
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains(
            "fileprovider-observed-invalidation\tevents=1\tevent-kinds=metadata\tpaths=1"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains("provider-metadata-invalidation\t"),
        "{stdout}"
    );

    let stored = fs::read_to_string(&state).unwrap();
    assert!(
        stored.starts_with("gfm-fileprovider-state-v1\n"),
        "{stored}"
    );
    assert!(stored.contains(&item.display().to_string()), "{stored}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn preview_cache_fileprovider_observed_invalidation_runs_on_visible_worker_from_binary() {
    let root = unique_temp_dir("gfm-cli-preview-fileprovider-observed-root");
    let cache = root.join("preview-cache");
    let state = root.join("fileprovider-state.tsv");
    let item = root.join("Remote.icloud-placeholder");
    fs::create_dir_all(&cache).unwrap();
    fs::write(&item, "remote placeholder").unwrap();
    xattr::set(&item, "com.apple.icloud.placeholder", b"1").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "preview-cache-fileprovider-observed-invalidation",
            cache.to_str().unwrap(),
            state.to_str().unwrap(),
            "thumbnail",
            "metadata",
            item.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(&stderr, "preview cache root", &cache);
    assert_worker_admitted(
        &stderr,
        "preview cache fileprovider observed invalidation",
        &root,
    );
    assert_worker_admitted(
        &stderr,
        "preview cache fileprovider observed invalidation",
        &item,
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains(
            "fileprovider-observed-invalidation\tevents=1\tevent-kinds=metadata\tpaths=1"
        ),
        "{stdout}"
    );
    assert!(stdout.contains("preview-cache-invalidation\t"), "{stdout}");

    let stored = fs::read_to_string(&state).unwrap();
    assert!(
        stored.starts_with("gfm-fileprovider-state-v1\n"),
        "{stored}"
    );
    assert!(stored.contains(&item.display().to_string()), "{stored}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn platform_fileprovider_invalidation_routes_refuse_unreachable_state_before_worker_io_from_binary()
{
    let root = unique_temp_dir("gfm-cli-fileprovider-routes-unreachable");
    let cache = root.join("preview-cache");
    let state = root.join("fileprovider-state.tsv");
    let item = root.join("Remote.icloud-placeholder");
    fs::create_dir_all(&cache).unwrap();
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(&item, "remote placeholder").unwrap();

    let cases = [
        (
            "fileprovider invalidation scan",
            vec![
                "fileprovider-invalidation-scan".to_string(),
                state.display().to_string(),
                item.display().to_string(),
            ],
        ),
        (
            "fileprovider invalidation event",
            vec![
                "fileprovider-invalidation-event".to_string(),
                state.display().to_string(),
                "metadata".to_string(),
                item.display().to_string(),
            ],
        ),
        (
            "fileprovider observed metadata invalidation",
            vec![
                "fileprovider-observed-metadata-invalidation".to_string(),
                state.display().to_string(),
                "metadata".to_string(),
                item.display().to_string(),
            ],
        ),
        (
            "preview cache fileprovider observed invalidation",
            vec![
                "preview-cache-fileprovider-observed-invalidation".to_string(),
                cache.display().to_string(),
                state.display().to_string(),
                "thumbnail".to_string(),
                "metadata".to_string(),
                item.display().to_string(),
            ],
        ),
    ];

    for (worker, args) in cases {
        let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
            .args(&args)
            .output()
            .unwrap();

        assert!(!output.status.success(), "{args:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stdout.is_empty(), "{args:?}: {stdout}");
        assert!(
            stderr.contains("volume access blocked: unreachable volume network"),
            "{args:?}: {stderr}"
        );
        assert!(
            !stderr.contains(&format!("security-worker-admission\tworker={worker}\t")),
            "{args:?}: {stderr}"
        );
    }

    assert!(!state.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancellable_adaptive_extraction_worker_stops_before_launch_from_binary() {
    let root = unique_temp_dir("gfm-cli-extract-worker-cancel-root");
    let path = root.join("document.txt");
    fs::write(&path, "cancel worker marker").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "extract-worker-cancel-adaptive",
            path.to_str().unwrap(),
            "nominal",
            "nominal",
            "ac",
            "idle",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout.trim(),
        "extract-worker\tstatus=cancelled\treason=cancelled-before-launch"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancellable_adaptive_extraction_worker_stops_before_unreachable_volume_read_from_binary() {
    let root = unique_temp_dir("gfm-cli-extract-worker-cancel-unreachable");
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let path = root.join("document.txt");
    fs::write(&path, "cancel worker marker").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "extract-worker-cancel-adaptive",
            path.to_str().unwrap(),
            "nominal",
            "nominal",
            "ac",
            "idle",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout.trim(),
        "extract-worker\tstatus=cancelled\treason=cancelled-before-launch"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn quarantined_adaptive_extraction_worker_records_timeout_from_binary() {
    let root = unique_temp_dir("gfm-cli-extract-worker-quarantine-root");
    let path = root.join("document.txt");
    let store = root.join("quarantine.gfmquarantine");
    fs::write(&path, "timeout worker marker").unwrap();

    for expected in ["quarantine\tallow", "quarantine\tblocked"] {
        let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
            .args([
                "extract-worker-quarantine-adaptive",
                path.to_str().unwrap(),
                store.to_str().unwrap(),
                "nominal",
                "nominal",
                "ac",
                "idle",
                "0",
                "2",
            ])
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            stderr
                .matches(&format!(
                    "security-worker-admission\tworker=quarantined adaptive extraction\tpath={}\tintent=read",
                    path.display()
                ))
                .count(),
            1,
            "{stderr}"
        );
        assert_eq!(
            stderr
                .matches(&format!(
                    "security-worker-admission\tworker=quarantined adaptive extraction\tpath={}\tintent=write",
                    root.display()
                ))
                .count(),
            1,
            "{stderr}"
        );
        assert!(stdout.starts_with(expected), "{stdout}");
        assert!(stdout.contains("\treason=worker-timeout\t") || expected == "quarantine\tallow");
    }
    assert!(store.is_file());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn quarantined_adaptive_extraction_worker_refuses_unreachable_scratch_before_recording_from_binary()
{
    let root = unique_temp_dir("gfm-cli-extract-worker-quarantine-scratch-root");
    let scratch = unique_temp_dir("gfm-cli-extract-worker-quarantine-scratch-unreachable");
    let path = root.join("document.txt");
    let store = root.join("quarantine.gfmquarantine");
    fs::write(&path, "quarantine scratch preflight marker").unwrap();
    fs::write(scratch.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("TMPDIR", &scratch)
        .args([
            "extract-worker-quarantine-adaptive",
            path.to_str().unwrap(),
            store.to_str().unwrap(),
            "nominal",
            "nominal",
            "ac",
            "idle",
            "0",
            "2",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("quarantine\t"), "{stdout}");
    assert!(
        stderr.contains("adaptive extraction volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !store.exists(),
        "scratch failure must not write quarantine state"
    );
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "quarantine scratch preflight marker"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn deferred_quarantined_adaptive_extraction_worker_does_not_touch_unreachable_store_from_binary() {
    let source_root = unique_temp_dir("gfm-cli-extract-worker-quarantine-deferred-source");
    let store_root = unique_temp_dir("gfm-cli-extract-worker-quarantine-deferred-store");
    let catalog = unique_temp_path(
        "gfm-cli-extract-worker-quarantine-deferred-runtime",
        "gfmjobs",
    );
    let progress = unique_temp_path(
        "gfm-cli-extract-worker-quarantine-deferred-runtime",
        "gfmprogress",
    );
    fs::write(store_root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let path = source_root.join("document.txt");
    let store = store_root.join("quarantine.gfmquarantine");
    fs::write(&path, "deferred quarantine marker").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "extract-worker-quarantine-adaptive",
            path.to_str().unwrap(),
            store.to_str().unwrap(),
            "saturated",
            "nominal",
            "ac",
            "idle",
            "0",
            "2",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("quarantine\t"), "{stdout}");
    assert!(
        stderr.contains("quarantined-adaptive-extraction-deferred\taction=Defer"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "\tworker=quarantined adaptive extraction\tpath={}",
            path.display()
        )),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "\tworker=quarantined adaptive extraction\tpath={}",
            store_root.display()
        )),
        "{stderr}"
    );
    assert!(!store.exists());
    let catalog_text = fs::read_to_string(&catalog).unwrap();
    assert!(catalog_text.contains("\textraction\t"), "{catalog_text}");
    assert!(
        catalog_text.contains("quarantined adaptive extraction"),
        "{catalog_text}"
    );
    assert!(
        catalog_text.contains(&path.display().to_string()),
        "{catalog_text}"
    );
    let progress_text = fs::read_to_string(&progress).unwrap();
    assert!(
        progress_text
            .contains("progress\t1\tbackground\tbackground\tquarantined adaptive extraction"),
        "{progress_text}"
    );
    assert!(
        progress_text.contains("\tpaused\t0\t1\tdeferred:Defer\t"),
        "{progress_text}"
    );

    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
    fs::remove_dir_all(source_root).unwrap();
    fs::remove_dir_all(store_root).unwrap();
}

#[test]
fn quarantined_adaptive_extraction_worker_refuses_unreachable_store_before_recording_from_binary() {
    let source_root = unique_temp_dir("gfm-cli-extract-worker-quarantine-source");
    let store_root = unique_temp_dir("gfm-cli-extract-worker-quarantine-store-unreachable");
    fs::write(store_root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let path = source_root.join("document.txt");
    let store = store_root.join(format!(
        "{}.gfmquarantine",
        "extract-quarantine-store-unavailable".repeat(8)
    ));
    fs::write(&path, "timeout worker marker").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "extract-worker-quarantine-adaptive",
            path.to_str().unwrap(),
            store.to_str().unwrap(),
            "nominal",
            "nominal",
            "ac",
            "idle",
            "0",
            "2",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("quarantine\t"), "{stdout}");
    assert!(
        stderr.contains("extraction quarantine volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "\tworker=quarantined adaptive extraction\tpath={}",
            path.display()
        )),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "\tworker=quarantined adaptive extraction\tpath={}",
            store_root.display()
        )),
        "{stderr}"
    );
    assert!(!store.exists());

    fs::remove_dir_all(source_root).unwrap();
    fs::remove_dir_all(store_root).unwrap();
}

#[test]
fn reports_extraction_cache_hits_from_binary() {
    let root = unique_temp_dir("gfm-cli-extract-cache-root");
    let path = root.join("Cache.md");
    fs::write(&path, "cached cli needle").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["extract-cache", path.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(&stderr, "content extraction cache", &path);
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 2, "{stdout}");
    assert!(
        lines[0].starts_with("extract-cache\tstatus=miss\t"),
        "{stdout}"
    );
    assert!(
        lines[1].starts_with("extract-cache\tstatus=hit\t"),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "\tversion={}\t",
            extractor_version_for_path(&path)
        )),
        "{stdout}"
    );
    assert!(stdout.contains("\tmetadata-epoch="), "{stdout}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extract_cache_refuses_unreachable_volume_before_metadata_from_binary() {
    let root = unique_temp_dir("gfm-cli-extract-cache-unreachable");
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let path = root.join("Cache.md");
    fs::write(&path, "offline cache needle").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["extract-cache", path.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("extract-cache\t"), "{stdout}");
    assert!(
        stderr
            .contains("content extraction cache volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=content extraction cache\tpath={}",
            path.display()
        )),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reports_extraction_quarantine_from_binary() {
    let root = unique_temp_dir("gfm-cli-extract-quarantine-root");
    let path = root.join("Slow.pdf");
    let store = root.join("quarantine.gfmquarantine");
    fs::write(&path, minimal_pdf("slow worker")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "extract-quarantine",
            path.to_str().unwrap(),
            store.to_str().unwrap(),
            "timeout",
            "2",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(&stderr, "extraction quarantine", &path);
    assert_worker_admitted(&stderr, "extraction quarantine", &root);
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 2, "{stdout}");
    assert!(lines[0].starts_with("quarantine\tblocked\t"), "{stdout}");
    assert!(lines[1].starts_with("quarantine\tblocked\t"), "{stdout}");
    assert!(stdout.contains("\treason=worker-timeout\t"), "{stdout}");
    assert!(store.is_file());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extract_quarantine_refuses_unreachable_store_before_recording_from_binary() {
    let source_root = unique_temp_dir("gfm-cli-extract-quarantine-source");
    let store_root = unique_temp_dir("gfm-cli-extract-quarantine-store-unreachable");
    fs::write(store_root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let path = source_root.join("Slow.pdf");
    let store = store_root.join("quarantine.gfmquarantine");
    fs::write(&path, minimal_pdf("slow worker")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "extract-quarantine",
            path.to_str().unwrap(),
            store.to_str().unwrap(),
            "timeout",
            "2",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("quarantine\t"), "{stdout}");
    assert!(
        stderr.contains("extraction quarantine volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("content write path metadata unavailable"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("security-worker-admission\tworker=extraction quarantine\t"),
        "{stderr}"
    );
    assert!(!store.exists());

    fs::remove_dir_all(source_root).unwrap();
    fs::remove_dir_all(store_root).unwrap();
}

#[test]
fn extract_quarantine_surfaces_store_probe_failure_before_recording_from_binary() {
    let root = unique_temp_dir("gfm-cli-extract-quarantine-store-probe-source");
    let path = root.join("Slow.pdf");
    let store = root.join("quarantine-store-unavailable".repeat(16));
    fs::write(&path, minimal_pdf("probe failure worker")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "extract-quarantine",
            path.to_str().unwrap(),
            store.to_str().unwrap(),
            "timeout",
            "2",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("quarantine\t"), "{stdout}");
    assert!(
        stderr.contains("content write path metadata unavailable"),
        "{stderr}"
    );
    assert!(stderr.contains("quarantine-store-unavailable"), "{stderr}");
    assert!(
        !stderr.contains("security-worker-admission\tworker=extraction quarantine\t"),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn searches_persisted_text_content_from_binary() {
    let root = unique_temp_dir("gfm-cli-durable-content-root");
    let records = unique_temp_path("gfm-cli-durable-records", "gfmidx");
    let content = unique_temp_path("gfm-cli-durable-content", "gfmcontent");
    fs::write(root.join("archive.md"), "the body contains durablemarker").unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-content",
            root.to_str().unwrap(),
            records.to_str().unwrap(),
            content.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );
    let index_stderr = String::from_utf8_lossy(&index_output.stderr);
    assert_eq!(
        index_stderr
            .matches(&format!(
                "security-worker-admission\tworker=content index\tpath={}\tintent=index",
                root.display()
            ))
            .count(),
        1,
        "{index_stderr}"
    );

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-index",
            records.to_str().unwrap(),
            content.to_str().unwrap(),
            "durablemarker",
        ])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );

    let stdout = String::from_utf8(search_output.stdout).unwrap();
    let stderr = String::from_utf8(search_output.stderr).unwrap();
    assert!(stdout.contains("archive.md"), "{stdout}");
    assert!(
        stderr.contains(&format!(
            "security-worker-admission\tworker=content index search records\tpath={}",
            records.display()
        )) && stderr.contains(&format!(
            "security-worker-admission\tworker=content index search content\tpath={}",
            content.display()
        )) && stderr.contains("content-keys 1")
            && stderr.contains("records-loaded 1")
            && stderr.contains("candidate-ids 1")
            && stderr.contains("full-hydration false"),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(content).unwrap();
}

#[test]
fn index_content_refuses_unreachable_records_output_before_scanning_from_binary() {
    let root = unique_temp_dir("gfm-cli-index-content-source");
    let output_root = unique_temp_dir("gfm-cli-index-content-records-unreachable");
    fs::write(root.join("archive.md"), "the body contains nooutputmarker").unwrap();
    fs::write(
        output_root.join(".gfm-volume-kind"),
        "network-unreachable\n",
    )
    .unwrap();
    let records = output_root.join(format!(
        "{}.gfmidx",
        "index-content-records-unavailable".repeat(8)
    ));
    let content = unique_temp_path("gfm-cli-index-content-local", "gfmcontent");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-content",
            root.to_str().unwrap(),
            records.to_str().unwrap(),
            content.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("hit\t"), "{stdout}");
    assert!(
        stderr.contains("content index volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("content write path metadata unavailable"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=content index\tpath={}",
            root.display()
        )),
        "{stderr}"
    );
    assert!(!stderr.contains("indexed "), "{stderr}");
    assert!(!records.exists());
    assert!(!content.exists());

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(output_root).unwrap();
}

#[test]
fn index_content_refuses_unreachable_content_output_before_scanning_from_binary() {
    let root = unique_temp_dir("gfm-cli-index-content-content-source");
    let output_root = unique_temp_dir("gfm-cli-index-content-content-unreachable");
    fs::write(root.join("archive.md"), "the body contains noarchivewrite").unwrap();
    fs::write(
        output_root.join(".gfm-volume-kind"),
        "network-unreachable\n",
    )
    .unwrap();
    let records = unique_temp_path("gfm-cli-index-content-records-local", "gfmidx");
    let content = output_root.join(format!(
        "{}.gfmcontent",
        "index-content-archive-unavailable".repeat(8)
    ));

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-content",
            root.to_str().unwrap(),
            records.to_str().unwrap(),
            content.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("hit\t"), "{stdout}");
    assert!(
        stderr.contains("content index volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("content write path metadata unavailable"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=content index\tpath={}",
            root.display()
        )),
        "{stderr}"
    );
    assert!(!stderr.contains("indexed "), "{stderr}");
    assert!(!records.exists());
    assert!(!content.exists());

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(output_root).unwrap();
}

#[test]
fn index_content_retries_transient_failure_from_binary() {
    let root = unique_temp_dir("gfm-cli-index-content-retry-root");
    let records = unique_temp_path("gfm-cli-index-content-retry-records", "gfmidx");
    let content = unique_temp_path("gfm-cli-index-content-retry-content", "gfmcontent");
    let journal = unique_temp_path("gfm-cli-index-content-retry", "journal");
    let catalog = unique_temp_path("gfm-cli-index-content-retry", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-index-content-retry", "gfmprogress");
    let retry_probe = unique_temp_path("gfm-cli-index-content-retry", "state");
    fs::write(
        root.join("retry.md"),
        "the body contains foregroundretrymarker",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "index-content-retry-probe",
            root.to_str().unwrap(),
            records.to_str().unwrap(),
            content.to_str().unwrap(),
            retry_probe.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_to_string(&retry_probe).unwrap(), "2");
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(
        journal_text.contains("1\t1\tstarted\tcontent index"),
        "{journal_text}"
    );
    assert!(
        journal_text
            .contains("1\t1\tfailed:temporary content index retry probe busy\tcontent index"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tstarted\tcontent index"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tcompleted\tcontent index"),
        "{journal_text}"
    );

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-index",
            records.to_str().unwrap(),
            content.to_str().unwrap(),
            "foregroundretrymarker",
        ])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );
    let stdout = String::from_utf8(search_output.stdout).unwrap();
    assert!(stdout.contains("retry.md"), "{stdout}");

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(content).unwrap();
    fs::remove_file(journal).unwrap();
    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
    fs::remove_file(retry_probe).unwrap();
}

#[test]
fn search_content_index_retries_transient_archive_read_failure_from_binary() {
    let root = unique_temp_dir("gfm-cli-search-content-index-retry-root");
    let records = unique_temp_path("gfm-cli-search-content-index-retry-records", "gfmidx");
    let content = unique_temp_path("gfm-cli-search-content-index-retry-content", "gfmcontent");
    let journal = unique_temp_path("gfm-cli-search-content-index-retry", "journal");
    let catalog = unique_temp_path("gfm-cli-search-content-index-retry", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-search-content-index-retry", "gfmprogress");
    let retry_probe = unique_temp_path("gfm-cli-search-content-index-retry", "state");
    fs::write(
        root.join("content-retry.md"),
        "the body contains contentreadretrymarker",
    )
    .unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-content",
            root.to_str().unwrap(),
            records.to_str().unwrap(),
            content.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "search-content-index-retry-probe",
            records.to_str().unwrap(),
            content.to_str().unwrap(),
            "contentreadretrymarker",
            retry_probe.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );

    let stdout = String::from_utf8(search_output.stdout).unwrap();
    assert!(stdout.contains("content-retry.md"), "{stdout}");
    assert_eq!(fs::read_to_string(&retry_probe).unwrap(), "2");
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(
        journal_text.contains("1\t1\tstarted\tcontent index search"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains(
            "1\t1\tfailed:temporary content index search retry probe busy\tcontent index search"
        ),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tstarted\tcontent index search"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tcompleted\tcontent index search"),
        "{journal_text}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(content).unwrap();
    fs::remove_file(journal).unwrap();
    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
    fs::remove_file(retry_probe).unwrap();
}

#[test]
fn search_content_index_set_retries_transient_archive_read_failure_from_binary() {
    let root = unique_temp_dir("gfm-cli-search-content-index-set-retry-root");
    let records = unique_temp_path("gfm-cli-search-content-index-set-retry-records", "gfmidx");
    let first_content =
        unique_temp_path("gfm-cli-search-content-index-set-retry-first", "gfmcontent");
    let second_content = unique_temp_path(
        "gfm-cli-search-content-index-set-retry-second",
        "gfmcontent",
    );
    let journal = unique_temp_path("gfm-cli-search-content-index-set-retry", "journal");
    let catalog = unique_temp_path("gfm-cli-search-content-index-set-retry", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-search-content-index-set-retry", "gfmprogress");
    let retry_probe = unique_temp_path("gfm-cli-search-content-index-set-retry", "state");
    fs::write(root.join("left-set-retry.md"), "metadata only").unwrap();
    fs::write(root.join("right-set-retry.md"), "metadata only").unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), records.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );

    let indexed_records = read_records(&records).unwrap();
    let left = indexed_records
        .iter()
        .find(|record| record.path.ends_with("left-set-retry.md"))
        .unwrap()
        .id;
    let right = indexed_records
        .iter()
        .find(|record| record.path.ends_with("right-set-retry.md"))
        .unwrap()
        .id;
    write_content_postings(
        &first_content,
        &[ContentPosting {
            term: "setretrymarker".to_string(),
            ids: vec![left],
            positions: vec![],
        }],
    )
    .unwrap();
    write_content_postings(
        &second_content,
        &[ContentPosting {
            term: "setretrymarker".to_string(),
            ids: vec![right],
            positions: vec![],
        }],
    )
    .unwrap();

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "search-content-index-set-retry-probe",
            records.to_str().unwrap(),
            "setretrymarker",
            retry_probe.to_str().unwrap(),
            first_content.to_str().unwrap(),
            second_content.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );

    let stdout = String::from_utf8(search_output.stdout).unwrap();
    assert!(stdout.contains("left-set-retry.md"), "{stdout}");
    assert!(stdout.contains("right-set-retry.md"), "{stdout}");
    assert_eq!(fs::read_to_string(&retry_probe).unwrap(), "2");
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(
        journal_text.contains("1\t1\tstarted\tcontent index set search"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains(
            "1\t1\tfailed:temporary content index set search retry probe busy\tcontent index set search"
        ),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tstarted\tcontent index set search"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tcompleted\tcontent index set search"),
        "{journal_text}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(first_content).unwrap();
    fs::remove_file(second_content).unwrap();
    fs::remove_file(journal).unwrap();
    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
    fs::remove_file(retry_probe).unwrap();
}

#[test]
fn search_content_index_manifest_retries_transient_archive_read_failure_from_binary() {
    let root = unique_temp_dir("gfm-cli-search-content-index-manifest-retry-root");
    let records = unique_temp_path(
        "gfm-cli-search-content-index-manifest-retry-records",
        "gfmidx",
    );
    let manifest = unique_temp_path("gfm-cli-search-content-index-manifest-retry", "gfmmanifest");
    let first_content = unique_temp_path(
        "gfm-cli-search-content-index-manifest-retry-first",
        "gfmcontent",
    );
    let second_content = unique_temp_path(
        "gfm-cli-search-content-index-manifest-retry-second",
        "gfmcontent",
    );
    let journal = unique_temp_path("gfm-cli-search-content-index-manifest-retry", "journal");
    let catalog = unique_temp_path("gfm-cli-search-content-index-manifest-retry", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-search-content-index-manifest-retry", "gfmprogress");
    let retry_probe = unique_temp_path("gfm-cli-search-content-index-manifest-retry", "state");
    fs::write(root.join("left-manifest-retry.md"), "metadata only").unwrap();
    fs::write(root.join("right-manifest-retry.md"), "metadata only").unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), records.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );

    let indexed_records = read_records(&records).unwrap();
    let left = indexed_records
        .iter()
        .find(|record| record.path.ends_with("left-manifest-retry.md"))
        .unwrap()
        .id;
    let right = indexed_records
        .iter()
        .find(|record| record.path.ends_with("right-manifest-retry.md"))
        .unwrap()
        .id;
    write_content_postings(
        &first_content,
        &[ContentPosting {
            term: "manifestretrymarker".to_string(),
            ids: vec![left],
            positions: vec![],
        }],
    )
    .unwrap();
    write_content_postings(
        &second_content,
        &[ContentPosting {
            term: "manifestretrymarker".to_string(),
            ids: vec![right],
            positions: vec![],
        }],
    )
    .unwrap();
    ContentArchiveManifest::new(vec![
        ContentArchiveManifestEntry {
            tier: ContentMergeTier::Hot,
            path: first_content.clone(),
        },
        ContentArchiveManifestEntry {
            tier: ContentMergeTier::Warm,
            path: second_content.clone(),
        },
    ])
    .unwrap()
    .write(&manifest)
    .unwrap();

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "search-content-index-manifest-retry-probe",
            records.to_str().unwrap(),
            manifest.to_str().unwrap(),
            "manifestretrymarker",
            retry_probe.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );

    let stdout = String::from_utf8(search_output.stdout).unwrap();
    assert!(stdout.contains("left-manifest-retry.md"), "{stdout}");
    assert!(stdout.contains("right-manifest-retry.md"), "{stdout}");
    assert_eq!(fs::read_to_string(&retry_probe).unwrap(), "2");
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(
        journal_text.contains("1\t1\tstarted\tcontent index manifest search"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains(
            "1\t1\tfailed:temporary content index manifest search retry probe busy\tcontent index manifest search"
        ),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tstarted\tcontent index manifest search"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tcompleted\tcontent index manifest search"),
        "{journal_text}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(manifest).unwrap();
    fs::remove_file(first_content).unwrap();
    fs::remove_file(second_content).unwrap();
    fs::remove_file(journal).unwrap();
    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
    fs::remove_file(retry_probe).unwrap();
}

#[test]
fn search_content_index_manifest_session_retries_transient_archive_read_failure_from_binary() {
    let root = unique_temp_dir("gfm-cli-search-content-index-manifest-session-retry-root");
    let records = unique_temp_path(
        "gfm-cli-search-content-index-manifest-session-retry-records",
        "gfmidx",
    );
    let manifest = unique_temp_path(
        "gfm-cli-search-content-index-manifest-session-retry",
        "gfmmanifest",
    );
    let first_content = unique_temp_path(
        "gfm-cli-search-content-index-manifest-session-retry-first",
        "gfmcontent",
    );
    let second_content = unique_temp_path(
        "gfm-cli-search-content-index-manifest-session-retry-second",
        "gfmcontent",
    );
    let journal = unique_temp_path(
        "gfm-cli-search-content-index-manifest-session-retry",
        "journal",
    );
    let catalog = unique_temp_path(
        "gfm-cli-search-content-index-manifest-session-retry",
        "gfmjobs",
    );
    let progress = unique_temp_path(
        "gfm-cli-search-content-index-manifest-session-retry",
        "gfmprogress",
    );
    let retry_probe = unique_temp_path(
        "gfm-cli-search-content-index-manifest-session-retry",
        "state",
    );
    fs::write(root.join("left-manifest-session-retry.md"), "metadata only").unwrap();
    fs::write(
        root.join("right-manifest-session-retry.md"),
        "metadata only",
    )
    .unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), records.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );

    let indexed_records = read_records(&records).unwrap();
    let left = indexed_records
        .iter()
        .find(|record| record.path.ends_with("left-manifest-session-retry.md"))
        .unwrap()
        .id;
    let right = indexed_records
        .iter()
        .find(|record| record.path.ends_with("right-manifest-session-retry.md"))
        .unwrap()
        .id;
    write_content_postings(
        &first_content,
        &[ContentPosting {
            term: "manifestsessionretrymarker".to_string(),
            ids: vec![left],
            positions: vec![],
        }],
    )
    .unwrap();
    write_content_postings(
        &second_content,
        &[ContentPosting {
            term: "manifestsessionretrymarker".to_string(),
            ids: vec![right],
            positions: vec![],
        }],
    )
    .unwrap();
    ContentArchiveManifest::new(vec![
        ContentArchiveManifestEntry {
            tier: ContentMergeTier::Hot,
            path: first_content.clone(),
        },
        ContentArchiveManifestEntry {
            tier: ContentMergeTier::Warm,
            path: second_content.clone(),
        },
    ])
    .unwrap()
    .write(&manifest)
    .unwrap();

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "search-content-index-manifest-session-retry-probe",
            records.to_str().unwrap(),
            manifest.to_str().unwrap(),
            "manifestsessionretrymarker",
            retry_probe.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );

    let stdout = String::from_utf8(search_output.stdout).unwrap();
    assert!(
        stdout.contains("left-manifest-session-retry.md"),
        "{stdout}"
    );
    assert!(
        stdout.contains("right-manifest-session-retry.md"),
        "{stdout}"
    );
    assert_eq!(fs::read_to_string(&retry_probe).unwrap(), "2");
    let stderr = String::from_utf8(search_output.stderr).unwrap();
    assert!(
        stderr.contains(
            "content-manifest-session-second\tcontent-archives=2\tcontent-keys=1\trecords-loaded=2"
        ),
        "{stderr}"
    );
    assert!(
        stderr.contains("\tposting-cache-hits=0\tposting-cache-misses=0")
            && stderr.contains("\trecord-cache-hits=0\trecord-cache-misses=0")
            && stderr.contains("\tresult-cache-hits=1\tresult-cache-misses=0"),
        "{stderr}"
    );
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(
        journal_text.contains("1\t1\tstarted\tcontent index manifest session"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains(
            "1\t1\tfailed:temporary content index manifest session retry probe busy\tcontent index manifest session"
        ),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tstarted\tcontent index manifest session"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tcompleted\tcontent index manifest session"),
        "{journal_text}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(manifest).unwrap();
    fs::remove_file(first_content).unwrap();
    fs::remove_file(second_content).unwrap();
    fs::remove_file(journal).unwrap();
    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
    fs::remove_file(retry_probe).unwrap();
}

#[test]
fn search_content_index_set_session_retries_transient_archive_read_failure_from_binary() {
    let root = unique_temp_dir("gfm-cli-search-content-index-set-session-retry-root");
    let records = unique_temp_path(
        "gfm-cli-search-content-index-set-session-retry-records",
        "gfmidx",
    );
    let first_content = unique_temp_path(
        "gfm-cli-search-content-index-set-session-retry-first",
        "gfmcontent",
    );
    let second_content = unique_temp_path(
        "gfm-cli-search-content-index-set-session-retry-second",
        "gfmcontent",
    );
    let journal = unique_temp_path("gfm-cli-search-content-index-set-session-retry", "journal");
    let catalog = unique_temp_path("gfm-cli-search-content-index-set-session-retry", "gfmjobs");
    let progress = unique_temp_path(
        "gfm-cli-search-content-index-set-session-retry",
        "gfmprogress",
    );
    let retry_probe = unique_temp_path("gfm-cli-search-content-index-set-session-retry", "state");
    fs::write(root.join("left-set-session-retry.md"), "metadata only").unwrap();
    fs::write(root.join("right-set-session-retry.md"), "metadata only").unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), records.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );

    let indexed_records = read_records(&records).unwrap();
    let left = indexed_records
        .iter()
        .find(|record| record.path.ends_with("left-set-session-retry.md"))
        .unwrap()
        .id;
    let right = indexed_records
        .iter()
        .find(|record| record.path.ends_with("right-set-session-retry.md"))
        .unwrap()
        .id;
    write_content_postings(
        &first_content,
        &[ContentPosting {
            term: "setsessionretrymarker".to_string(),
            ids: vec![left],
            positions: vec![],
        }],
    )
    .unwrap();
    write_content_postings(
        &second_content,
        &[ContentPosting {
            term: "setsessionretrymarker".to_string(),
            ids: vec![right],
            positions: vec![],
        }],
    )
    .unwrap();

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "search-content-index-set-session-retry-probe",
            records.to_str().unwrap(),
            "setsessionretrymarker",
            retry_probe.to_str().unwrap(),
            first_content.to_str().unwrap(),
            second_content.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );

    let stdout = String::from_utf8(search_output.stdout).unwrap();
    assert!(stdout.contains("left-set-session-retry.md"), "{stdout}");
    assert!(stdout.contains("right-set-session-retry.md"), "{stdout}");
    assert_eq!(fs::read_to_string(&retry_probe).unwrap(), "2");
    let stderr = String::from_utf8(search_output.stderr).unwrap();
    assert!(
        stderr.contains(
            "content-session-second\tcontent-archives=2\tcontent-keys=1\trecords-loaded=2"
        ),
        "{stderr}"
    );
    assert!(
        stderr.contains("\tposting-cache-hits=0\tposting-cache-misses=0")
            && stderr.contains("\trecord-cache-hits=0\trecord-cache-misses=0")
            && stderr.contains("\tresult-cache-hits=1\tresult-cache-misses=0"),
        "{stderr}"
    );
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(
        journal_text.contains("1\t1\tstarted\tcontent index set session"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains(
            "1\t1\tfailed:temporary content index set session retry probe busy\tcontent index set session"
        ),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tstarted\tcontent index set session"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tcompleted\tcontent index set session"),
        "{journal_text}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(first_content).unwrap();
    fs::remove_file(second_content).unwrap();
    fs::remove_file(journal).unwrap();
    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
    fs::remove_file(retry_probe).unwrap();
}

#[test]
fn adaptive_persisted_content_search_applies_snippet_pressure_budget_from_binary() {
    let root = unique_temp_dir("gfm-cli-durable-adaptive-snippet-root");
    let records = unique_temp_path("gfm-cli-durable-adaptive-snippet-records", "gfmidx");
    let content = unique_temp_path("gfm-cli-durable-adaptive-snippet-content", "gfmcontent");
    let mut body = "x".repeat(1024 * 1024 + 1);
    body.push_str(" persistedpressuremarker");
    fs::write(root.join("large.md"), body).unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-content",
            root.to_str().unwrap(),
            records.to_str().unwrap(),
            content.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-index-adaptive",
            records.to_str().unwrap(),
            content.to_str().unwrap(),
            "persistedpressuremarker",
            "elevated",
            "serious",
            "low",
            "active",
        ])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );

    let stdout = String::from_utf8(search_output.stdout).unwrap();
    let stderr = String::from_utf8(search_output.stderr).unwrap();
    assert!(stdout.contains("large.md"), "{stdout}");
    assert!(
        stderr.contains(&format!(
            "security-worker-admission\tworker=adaptive content index search records\tpath={}",
            records.display()
        )) && stderr.contains(&format!(
            "security-worker-admission\tworker=adaptive content index search content\tpath={}",
            content.display()
        )),
        "{stderr}"
    );
    assert!(!stdout.contains("[[persistedpressuremarker]]"), "{stdout}");

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(content).unwrap();
}

#[test]
fn searches_persisted_content_phrases_from_binary() {
    let root = unique_temp_dir("gfm-cli-durable-phrase-root");
    let records = unique_temp_path("gfm-cli-durable-phrase-records", "gfmidx");
    let content = unique_temp_path("gfm-cli-durable-phrase-content", "gfmcontent");
    fs::write(
        root.join("keep.md"),
        "this body has a durable phrase marker",
    )
    .unwrap();
    fs::write(
        root.join("skip.md"),
        "this durable body phrase marker is not adjacent",
    )
    .unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-content",
            root.to_str().unwrap(),
            records.to_str().unwrap(),
            content.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-index",
            records.to_str().unwrap(),
            content.to_str().unwrap(),
            r#""durable phrase marker""#,
        ])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );

    let stdout = String::from_utf8(search_output.stdout).unwrap();
    assert!(stdout.contains("keep.md"), "{stdout}");
    assert!(!stdout.contains("skip.md"), "{stdout}");
    assert!(stdout.contains("[[durable phrase marker]]"), "{stdout}");

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(content).unwrap();
}

#[test]
fn searches_persisted_content_proximity_from_binary() {
    let root = unique_temp_dir("gfm-cli-durable-proximity-root");
    let records = unique_temp_path("gfm-cli-durable-proximity-records", "gfmidx");
    let content = unique_temp_path("gfm-cli-durable-proximity-content", "gfmcontent");
    fs::write(root.join("keep.md"), "alpha one two beta").unwrap();
    fs::write(root.join("skip.md"), "alpha one two three four beta").unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-content",
            root.to_str().unwrap(),
            records.to_str().unwrap(),
            content.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-index",
            records.to_str().unwrap(),
            content.to_str().unwrap(),
            "near:3:alpha,beta",
        ])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );

    let stdout = String::from_utf8(search_output.stdout).unwrap();
    assert!(stdout.contains("keep.md"), "{stdout}");
    assert!(!stdout.contains("skip.md"), "{stdout}");

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(content).unwrap();
}

#[test]
fn search_content_index_refuses_unreachable_records_before_loading_from_binary() {
    let root = unique_temp_dir("gfm-cli-search-content-index-unreachable-root");
    let local = unique_temp_dir("gfm-cli-search-content-index-local");
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let records = root.join("records.gfmidx");
    let content = local.join("content.gfmcontent");
    fs::write(&records, "not readable records").unwrap();
    fs::write(&content, "not read yet").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-index",
            records.to_str().unwrap(),
            content.to_str().unwrap(),
            "needle",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("hit\t"), "{stdout}");
    assert!(
        stderr.contains(
            "content index search records volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=content index search records\tpath={}",
            records.display()
        )),
        "{stderr}"
    );
    assert!(!stderr.contains("invalid magic"), "{stderr}");

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(local).unwrap();
}

#[test]
fn search_content_index_refuses_unreachable_content_before_loading_from_binary() {
    let root = unique_temp_dir("gfm-cli-search-content-index-content-root");
    let offline = unique_temp_dir("gfm-cli-search-content-index-content-offline");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let records = root.join("records.gfmidx");
    let content = offline.join("content.gfmcontent");
    fs::write(&records, "not parsed records").unwrap();
    fs::write(&content, "not readable content").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-index",
            records.to_str().unwrap(),
            content.to_str().unwrap(),
            "needle",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("hit\t"), "{stdout}");
    assert!(
        stderr.contains(
            "content index search content volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=content index search content\tpath={}",
            content.display()
        )),
        "{stderr}"
    );
    assert!(!stderr.contains("invalid magic"), "{stderr}");

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn search_content_index_set_refuses_unreachable_content_before_loading_from_binary() {
    let root = unique_temp_dir("gfm-cli-search-content-index-set-root");
    let offline = unique_temp_dir("gfm-cli-search-content-index-set-offline");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let records = root.join("records.gfmidx");
    let content = offline.join("content.gfmcontent");
    fs::write(&records, "not parsed records").unwrap();
    fs::write(&content, "not readable content").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-index-set",
            records.to_str().unwrap(),
            "needle",
            content.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("hit\t"), "{stdout}");
    assert!(
        stderr.contains(
            "content index set search content volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=content index set search content\tpath={}",
            content.display()
        )),
        "{stderr}"
    );
    assert!(!stderr.contains("invalid magic"), "{stderr}");

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn resolves_content_ids_from_archive_directory() {
    let root = unique_temp_dir("gfm-cli-content-ids-root");
    let records = unique_temp_path("gfm-cli-content-ids-records", "gfmidx");
    let content = unique_temp_path("gfm-cli-content-ids", "gfmcontent");
    let direct_retry_journal = unique_temp_path("gfm-cli-content-ids-retry", "journal");
    let direct_retry_catalog = unique_temp_path("gfm-cli-content-ids-retry", "gfmjobs");
    let direct_retry_progress = unique_temp_path("gfm-cli-content-ids-retry", "gfmprogress");
    let direct_retry_probe = unique_temp_path("gfm-cli-content-ids-retry", "state");
    let mmap_retry_journal = unique_temp_path("gfm-cli-content-ids-mmap-retry", "journal");
    let mmap_retry_catalog = unique_temp_path("gfm-cli-content-ids-mmap-retry", "gfmjobs");
    let mmap_retry_progress = unique_temp_path("gfm-cli-content-ids-mmap-retry", "gfmprogress");
    let mmap_retry_probe = unique_temp_path("gfm-cli-content-ids-mmap-retry", "state");
    let block_retry_journal = unique_temp_path("gfm-cli-content-id-block-retry", "journal");
    let block_retry_catalog = unique_temp_path("gfm-cli-content-id-block-retry", "gfmjobs");
    let block_retry_progress = unique_temp_path("gfm-cli-content-id-block-retry", "gfmprogress");
    let block_retry_probe = unique_temp_path("gfm-cli-content-id-block-retry", "state");
    fs::write(root.join("archive.md"), "the body contains directmarker").unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-content",
            root.to_str().unwrap(),
            records.to_str().unwrap(),
            content.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );

    let ids_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["content-ids", content.to_str().unwrap(), "directmarker"])
        .output()
        .unwrap();
    assert!(
        ids_output.status.success(),
        "{}",
        String::from_utf8_lossy(&ids_output.stderr)
    );
    let ids_stderr = String::from_utf8_lossy(&ids_output.stderr);
    assert!(
        ids_stderr.contains(&format!(
            "security-worker-admission\tworker=content ids\tpath={}",
            content.display()
        )),
        "{ids_stderr}"
    );

    let stdout = String::from_utf8(ids_output.stdout).unwrap();
    assert_eq!(stdout.lines().count(), 1, "{stdout}");
    assert!(stdout.lines().all(|line| line.split('\t').count() == 2));

    let ids_retry_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &direct_retry_journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &direct_retry_catalog)
        .env("GFM_JOB_PROGRESS_STORE", &direct_retry_progress)
        .args([
            "content-ids-retry-probe",
            content.to_str().unwrap(),
            "directmarker",
            direct_retry_probe.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        ids_retry_output.status.success(),
        "{}",
        String::from_utf8_lossy(&ids_retry_output.stderr)
    );
    assert_eq!(String::from_utf8(ids_retry_output.stdout).unwrap(), stdout);
    assert_eq!(fs::read_to_string(&direct_retry_probe).unwrap(), "2");
    let direct_retry_journal_text = fs::read_to_string(&direct_retry_journal).unwrap();
    assert!(
        direct_retry_journal_text.contains("1\t1\tstarted\tcontent ids"),
        "{direct_retry_journal_text}"
    );
    assert!(
        direct_retry_journal_text
            .contains("1\t1\tfailed:temporary content ids retry probe busy\tcontent ids"),
        "{direct_retry_journal_text}"
    );
    assert!(
        direct_retry_journal_text.contains("1\t2\tcompleted\tcontent ids"),
        "{direct_retry_journal_text}"
    );

    let mmap_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-ids-mmap",
            content.to_str().unwrap(),
            "directmarker",
        ])
        .output()
        .unwrap();
    assert!(
        mmap_output.status.success(),
        "{}",
        String::from_utf8_lossy(&mmap_output.stderr)
    );
    let mmap_stderr = String::from_utf8_lossy(&mmap_output.stderr);
    assert!(
        mmap_stderr.contains(&format!(
            "security-worker-admission\tworker=content ids mmap\tpath={}",
            content.display()
        )),
        "{mmap_stderr}"
    );

    let mmap_stdout = String::from_utf8(mmap_output.stdout).unwrap();
    assert_eq!(mmap_stdout, stdout);

    let mmap_retry_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &mmap_retry_journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &mmap_retry_catalog)
        .env("GFM_JOB_PROGRESS_STORE", &mmap_retry_progress)
        .args([
            "content-ids-mmap-retry-probe",
            content.to_str().unwrap(),
            "directmarker",
            mmap_retry_probe.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        mmap_retry_output.status.success(),
        "{}",
        String::from_utf8_lossy(&mmap_retry_output.stderr)
    );
    assert_eq!(String::from_utf8(mmap_retry_output.stdout).unwrap(), stdout);
    assert_eq!(fs::read_to_string(&mmap_retry_probe).unwrap(), "2");
    let mmap_retry_journal_text = fs::read_to_string(&mmap_retry_journal).unwrap();
    assert!(
        mmap_retry_journal_text.contains("1\t1\tstarted\tcontent ids mmap"),
        "{mmap_retry_journal_text}"
    );
    assert!(
        mmap_retry_journal_text
            .contains("1\t1\tfailed:temporary content ids mmap retry probe busy\tcontent ids mmap"),
        "{mmap_retry_journal_text}"
    );
    assert!(
        mmap_retry_journal_text.contains("1\t2\tcompleted\tcontent ids mmap"),
        "{mmap_retry_journal_text}"
    );

    let block_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-id-block-mmap",
            content.to_str().unwrap(),
            "directmarker",
            "0",
        ])
        .output()
        .unwrap();
    assert!(
        block_output.status.success(),
        "{}",
        String::from_utf8_lossy(&block_output.stderr)
    );
    let block_stderr = String::from_utf8_lossy(&block_output.stderr);
    assert!(
        block_stderr.contains(&format!(
            "security-worker-admission\tworker=content id block mmap\tpath={}",
            content.display()
        )),
        "{block_stderr}"
    );

    let block_stdout = String::from_utf8(block_output.stdout).unwrap();
    assert_eq!(block_stdout, stdout);

    let block_retry_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &block_retry_journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &block_retry_catalog)
        .env("GFM_JOB_PROGRESS_STORE", &block_retry_progress)
        .args([
            "content-id-block-mmap-retry-probe",
            content.to_str().unwrap(),
            "directmarker",
            "0",
            block_retry_probe.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        block_retry_output.status.success(),
        "{}",
        String::from_utf8_lossy(&block_retry_output.stderr)
    );
    assert_eq!(
        String::from_utf8(block_retry_output.stdout).unwrap(),
        stdout
    );
    assert_eq!(fs::read_to_string(&block_retry_probe).unwrap(), "2");
    let block_retry_journal_text = fs::read_to_string(&block_retry_journal).unwrap();
    assert!(
        block_retry_journal_text.contains("1\t1\tstarted\tcontent id block mmap"),
        "{block_retry_journal_text}"
    );
    assert!(
        block_retry_journal_text.contains(
            "1\t1\tfailed:temporary content id block mmap retry probe busy\tcontent id block mmap"
        ),
        "{block_retry_journal_text}"
    );
    assert!(
        block_retry_journal_text.contains("1\t2\tcompleted\tcontent id block mmap"),
        "{block_retry_journal_text}"
    );

    let verify_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["content-verify", content.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        verify_output.status.success(),
        "{}",
        String::from_utf8_lossy(&verify_output.stderr)
    );
    let verify_stderr = String::from_utf8_lossy(&verify_output.stderr);
    assert!(
        verify_stderr.contains(&format!(
            "security-worker-admission\tworker=content verify\tpath={}",
            content.display()
        )),
        "{verify_stderr}"
    );
    let verify_stdout = String::from_utf8(verify_output.stdout).unwrap();
    assert!(
        verify_stdout.contains("\tchecksum=verified"),
        "{verify_stdout}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(content).unwrap();
    fs::remove_file(direct_retry_journal).unwrap();
    fs::remove_file(direct_retry_catalog).unwrap();
    fs::remove_file(direct_retry_progress).unwrap();
    fs::remove_file(direct_retry_probe).unwrap();
    fs::remove_file(mmap_retry_journal).unwrap();
    fs::remove_file(mmap_retry_catalog).unwrap();
    fs::remove_file(mmap_retry_progress).unwrap();
    fs::remove_file(mmap_retry_probe).unwrap();
    fs::remove_file(block_retry_journal).unwrap();
    fs::remove_file(block_retry_catalog).unwrap();
    fs::remove_file(block_retry_progress).unwrap();
    fs::remove_file(block_retry_probe).unwrap();
}

#[test]
fn content_ids_mmap_refuses_unreachable_archive_before_mapping_from_binary() {
    let root = unique_temp_dir("gfm-cli-content-ids-unreachable-root");
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let content = root.join("content.gfmcontent");
    fs::write(&content, "not a readable archive").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["content-ids-mmap", content.to_str().unwrap(), "term"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("file\t"), "{stdout}");
    assert!(
        stderr.contains("content ids mmap volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=content ids mmap\tpath={}",
            content.display()
        )),
        "{stderr}"
    );
    assert!(!stderr.contains("invalid magic"), "{stderr}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn content_ids_mmap_set_refuses_unreachable_archive_before_mapping_from_binary() {
    let root = unique_temp_dir("gfm-cli-content-ids-set-unreachable-root");
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let content = root.join("content.gfmcontent");
    fs::write(&content, "not a readable archive").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["content-ids-mmap-set", "term", content.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains("content ids mmap set volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=content ids mmap set\tpath={}",
            content.display()
        )),
        "{stderr}"
    );
    assert!(!stderr.contains("invalid magic"), "{stderr}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn content_ids_mmap_manifest_refuses_unreachable_archive_before_mapping_from_binary() {
    let manifest_root = unique_temp_dir("gfm-cli-content-manifest-access-local");
    let offline = unique_temp_dir("gfm-cli-content-manifest-access-offline");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let manifest = manifest_root.join("content.gfmmanifest");
    let content = offline.join("content.gfmcontent");
    fs::write(&content, "not mapped after admission denial").unwrap();
    ContentArchiveManifest::new(vec![ContentArchiveManifestEntry {
        tier: ContentMergeTier::Hot,
        path: content.clone(),
    }])
    .unwrap()
    .write(&manifest)
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-ids-mmap-manifest",
            manifest.to_str().unwrap(),
            "term",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains(
            "content ids mmap manifest volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=content ids mmap manifest\tpath={}",
            content.display()
        )),
        "{stderr}"
    );
    assert!(!stderr.contains("invalid magic"), "{stderr}");

    fs::remove_dir_all(manifest_root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn search_content_index_manifest_refuses_unreachable_archive_before_loading_from_binary() {
    let root = unique_temp_dir("gfm-cli-content-manifest-search-local");
    let offline = unique_temp_dir("gfm-cli-content-manifest-search-offline");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let records = root.join("records.gfmidx");
    let manifest = root.join("content.gfmmanifest");
    let content = offline.join("content.gfmcontent");
    fs::write(&records, "not parsed after content admission denial").unwrap();
    fs::write(&content, "not mapped after admission denial").unwrap();
    ContentArchiveManifest::new(vec![ContentArchiveManifestEntry {
        tier: ContentMergeTier::Hot,
        path: content.clone(),
    }])
    .unwrap()
    .write(&manifest)
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-index-manifest",
            records.to_str().unwrap(),
            manifest.to_str().unwrap(),
            "needle",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("hit\t"), "{stdout}");
    assert!(
        stderr.contains(
            "content index manifest search content volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=content index manifest search content\tpath={}",
            content.display()
        )),
        "{stderr}"
    );
    assert!(!stderr.contains("invalid magic"), "{stderr}");

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn searches_persisted_content_across_mmap_archive_set_from_binary() {
    let root = unique_temp_dir("gfm-cli-content-set-root");
    let records = unique_temp_path("gfm-cli-content-set-records", "gfmidx");
    let first_content = unique_temp_path("gfm-cli-content-set-first", "gfmcontent");
    let second_content = unique_temp_path("gfm-cli-content-set-second", "gfmcontent");
    let third_content = unique_temp_path("gfm-cli-content-set-third", "gfmcontent");
    let manifest = unique_temp_path("gfm-cli-content-set", "gfmmanifest");
    let set_ids_retry_journal = unique_temp_path("gfm-cli-content-ids-set-retry", "journal");
    let set_ids_retry_catalog = unique_temp_path("gfm-cli-content-ids-set-retry", "gfmjobs");
    let set_ids_retry_progress = unique_temp_path("gfm-cli-content-ids-set-retry", "gfmprogress");
    let set_ids_retry_probe = unique_temp_path("gfm-cli-content-ids-set-retry", "state");
    let manifest_ids_retry_journal =
        unique_temp_path("gfm-cli-content-ids-manifest-retry", "journal");
    let manifest_ids_retry_catalog =
        unique_temp_path("gfm-cli-content-ids-manifest-retry", "gfmjobs");
    let manifest_ids_retry_progress =
        unique_temp_path("gfm-cli-content-ids-manifest-retry", "gfmprogress");
    let manifest_ids_retry_probe = unique_temp_path("gfm-cli-content-ids-manifest-retry", "state");
    fs::write(root.join("left.md"), "metadata only").unwrap();
    fs::write(root.join("right.md"), "metadata only").unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), records.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );

    let indexed_records = read_records(&records).unwrap();
    let left = indexed_records
        .iter()
        .find(|record| record.path.ends_with("left.md"))
        .unwrap()
        .id;
    let right = indexed_records
        .iter()
        .find(|record| record.path.ends_with("right.md"))
        .unwrap()
        .id;
    write_content_postings(
        &first_content,
        &[ContentPosting {
            term: "setneedle".to_string(),
            ids: vec![left],
            positions: vec![],
        }],
    )
    .unwrap();
    write_content_postings(
        &second_content,
        &[ContentPosting {
            term: "setneedle".to_string(),
            ids: vec![left, right],
            positions: vec![],
        }],
    )
    .unwrap();
    write_content_postings(
        &third_content,
        &[ContentPosting {
            term: "promotedneedle".to_string(),
            ids: vec![right],
            positions: vec![],
        }],
    )
    .unwrap();

    let ids_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-ids-mmap-set",
            "setneedle",
            first_content.to_str().unwrap(),
            second_content.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        ids_output.status.success(),
        "{}",
        String::from_utf8_lossy(&ids_output.stderr)
    );
    let ids_stderr = String::from_utf8_lossy(&ids_output.stderr);
    assert!(
        ids_stderr.contains(&format!(
            "security-worker-admission\tworker=content ids mmap set\tpath={}",
            first_content.display()
        )) && ids_stderr.contains(&format!(
            "security-worker-admission\tworker=content ids mmap set\tpath={}",
            second_content.display()
        )),
        "{ids_stderr}"
    );
    let ids_stdout = String::from_utf8(ids_output.stdout).unwrap();
    assert_eq!(ids_stdout.lines().count(), 2, "{ids_stdout}");

    let ids_retry_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &set_ids_retry_journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &set_ids_retry_catalog)
        .env("GFM_JOB_PROGRESS_STORE", &set_ids_retry_progress)
        .args([
            "content-ids-mmap-set-retry-probe",
            "setneedle",
            set_ids_retry_probe.to_str().unwrap(),
            first_content.to_str().unwrap(),
            second_content.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        ids_retry_output.status.success(),
        "{}",
        String::from_utf8_lossy(&ids_retry_output.stderr)
    );
    assert_eq!(
        String::from_utf8(ids_retry_output.stdout).unwrap(),
        ids_stdout
    );
    assert_eq!(fs::read_to_string(&set_ids_retry_probe).unwrap(), "2");
    let set_ids_retry_journal_text = fs::read_to_string(&set_ids_retry_journal).unwrap();
    assert!(
        set_ids_retry_journal_text.contains("1\t1\tstarted\tcontent ids mmap set"),
        "{set_ids_retry_journal_text}"
    );
    assert!(
        set_ids_retry_journal_text.contains(
            "1\t1\tfailed:temporary content ids mmap set retry probe busy\tcontent ids mmap set"
        ),
        "{set_ids_retry_journal_text}"
    );
    assert!(
        set_ids_retry_journal_text.contains("1\t2\tcompleted\tcontent ids mmap set"),
        "{set_ids_retry_journal_text}"
    );

    let manifest_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-manifest-write",
            manifest.to_str().unwrap(),
            &format!("hot:{}", first_content.display()),
            &format!("warm:{}", second_content.display()),
        ])
        .output()
        .unwrap();
    assert!(
        manifest_output.status.success(),
        "{}",
        String::from_utf8_lossy(&manifest_output.stderr)
    );
    let manifest_stderr = String::from_utf8_lossy(&manifest_output.stderr);
    assert_worker_admitted(
        &manifest_stderr,
        "content manifest write",
        manifest.parent().unwrap(),
    );
    assert_worker_admitted(
        &manifest_stderr,
        "content manifest write archive",
        &first_content,
    );
    assert_worker_admitted(
        &manifest_stderr,
        "content manifest write archive",
        &second_content,
    );

    let manifest_inspect = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["content-manifest-inspect", manifest.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        manifest_inspect.status.success(),
        "{}",
        String::from_utf8_lossy(&manifest_inspect.stderr)
    );
    let inspect_stderr = String::from_utf8_lossy(&manifest_inspect.stderr);
    assert!(
        inspect_stderr.contains(&format!(
            "security-worker-admission\tworker=content manifest inspect\tpath={}",
            manifest.display()
        )) && inspect_stderr.contains(&format!(
            "security-worker-admission\tworker=content manifest inspect archive\tpath={}",
            first_content.display()
        )) && inspect_stderr.contains(&format!(
            "security-worker-admission\tworker=content manifest inspect archive\tpath={}",
            second_content.display()
        )),
        "{inspect_stderr}"
    );
    let inspect_stdout = String::from_utf8(manifest_inspect.stdout).unwrap();
    assert!(
        inspect_stdout.contains("content-manifest\tarchives=2")
            && inspect_stdout.contains("\tterms=2")
            && inspect_stdout.contains("archive\thot\t")
            && inspect_stdout.contains("archive\twarm\t"),
        "{inspect_stdout}"
    );

    let duplicate_manifest = root.join("duplicate-content.gfmmanifest");
    fs::write(
        &duplicate_manifest,
        format!(
            "gfm-content-manifest-v1\narchive\thot\t{}\narchive\twarm\t{}\n",
            first_content.display(),
            first_content.display()
        ),
    )
    .unwrap();
    let duplicate_inspect = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-manifest-inspect",
            duplicate_manifest.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!duplicate_inspect.status.success());
    let duplicate_stdout = String::from_utf8_lossy(&duplicate_inspect.stdout);
    let duplicate_stderr = String::from_utf8_lossy(&duplicate_inspect.stderr);
    assert!(
        !duplicate_stdout.contains("content-manifest\t"),
        "{duplicate_stdout}"
    );
    assert!(
        duplicate_stderr.contains("duplicate content archive path"),
        "{duplicate_stderr}"
    );
    assert!(duplicate_stderr.contains("line 3"), "{duplicate_stderr}");
    assert_worker_admitted(
        &duplicate_stderr,
        "content manifest inspect",
        &duplicate_manifest,
    );

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-index-set",
            records.to_str().unwrap(),
            "setneedle",
            first_content.to_str().unwrap(),
            second_content.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );
    let stdout = String::from_utf8(search_output.stdout).unwrap();
    assert!(stdout.contains("left.md"), "{stdout}");
    assert!(stdout.contains("right.md"), "{stdout}");
    let stderr = String::from_utf8(search_output.stderr).unwrap();
    assert!(
        stderr.contains(&format!(
            "security-worker-admission\tworker=content index set search records\tpath={}",
            records.display()
        )) && stderr.contains(&format!(
            "security-worker-admission\tworker=content index set search content\tpath={}",
            first_content.display()
        )) && stderr.contains(&format!(
            "security-worker-admission\tworker=content index set search content\tpath={}",
            second_content.display()
        )) && stderr.contains("content-archives 2")
            && stderr.contains("content-keys 1")
            && stderr.contains("records-loaded 2")
            && stderr.contains("candidate-ids 2")
            && stderr.contains("full-hydration false"),
        "{stderr}"
    );

    let manifest_ids_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-ids-mmap-manifest",
            manifest.to_str().unwrap(),
            "setneedle",
        ])
        .output()
        .unwrap();
    assert!(
        manifest_ids_output.status.success(),
        "{}",
        String::from_utf8_lossy(&manifest_ids_output.stderr)
    );
    let manifest_ids_stderr = String::from_utf8_lossy(&manifest_ids_output.stderr);
    assert!(
        manifest_ids_stderr.contains(&format!(
            "security-worker-admission\tworker=content ids mmap manifest\tpath={}",
            manifest.display()
        )) && manifest_ids_stderr.contains(&format!(
            "security-worker-admission\tworker=content ids mmap manifest\tpath={}",
            first_content.display()
        )) && manifest_ids_stderr.contains(&format!(
            "security-worker-admission\tworker=content ids mmap manifest\tpath={}",
            second_content.display()
        )),
        "{manifest_ids_stderr}"
    );
    assert_eq!(
        String::from_utf8(manifest_ids_output.stdout).unwrap(),
        ids_stdout
    );

    let manifest_ids_retry_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &manifest_ids_retry_journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &manifest_ids_retry_catalog)
        .env("GFM_JOB_PROGRESS_STORE", &manifest_ids_retry_progress)
        .args([
            "content-ids-mmap-manifest-retry-probe",
            manifest.to_str().unwrap(),
            "setneedle",
            manifest_ids_retry_probe.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        manifest_ids_retry_output.status.success(),
        "{}",
        String::from_utf8_lossy(&manifest_ids_retry_output.stderr)
    );
    assert_eq!(
        String::from_utf8(manifest_ids_retry_output.stdout).unwrap(),
        ids_stdout
    );
    assert_eq!(fs::read_to_string(&manifest_ids_retry_probe).unwrap(), "2");
    let manifest_ids_retry_journal_text = fs::read_to_string(&manifest_ids_retry_journal).unwrap();
    assert!(
        manifest_ids_retry_journal_text.contains("1\t1\tstarted\tcontent ids mmap manifest"),
        "{manifest_ids_retry_journal_text}"
    );
    assert!(
        manifest_ids_retry_journal_text.contains(
            "1\t1\tfailed:temporary content ids mmap manifest retry probe busy\tcontent ids mmap manifest"
        ),
        "{manifest_ids_retry_journal_text}"
    );
    assert!(
        manifest_ids_retry_journal_text.contains("1\t2\tcompleted\tcontent ids mmap manifest"),
        "{manifest_ids_retry_journal_text}"
    );

    let manifest_search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-index-manifest",
            records.to_str().unwrap(),
            manifest.to_str().unwrap(),
            "setneedle",
        ])
        .output()
        .unwrap();
    assert!(
        manifest_search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&manifest_search_output.stderr)
    );
    assert_eq!(
        String::from_utf8(manifest_search_output.stdout).unwrap(),
        stdout
    );
    let manifest_stderr = String::from_utf8(manifest_search_output.stderr).unwrap();
    assert!(
        manifest_stderr.contains(&format!(
            "security-worker-admission\tworker=content index manifest search records\tpath={}",
            records.display()
        )) && manifest_stderr.contains(&format!(
            "security-worker-admission\tworker=content index manifest search manifest\tpath={}",
            manifest.display()
        )) && manifest_stderr.contains(&format!(
            "security-worker-admission\tworker=content index manifest search content\tpath={}",
            first_content.display()
        )) && manifest_stderr.contains(&format!(
            "security-worker-admission\tworker=content index manifest search content\tpath={}",
            second_content.display()
        )) && manifest_stderr.contains("content-manifest-keys 1")
            && manifest_stderr.contains("records-loaded 2")
            && manifest_stderr.contains("candidate-ids 2")
            && manifest_stderr.contains("full-hydration false"),
        "{manifest_stderr}"
    );

    let session_search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-index-set-session",
            records.to_str().unwrap(),
            "setneedle",
            first_content.to_str().unwrap(),
            second_content.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        session_search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&session_search_output.stderr)
    );
    let session_stdout = String::from_utf8(session_search_output.stdout).unwrap();
    assert!(session_stdout.contains("left.md"), "{session_stdout}");
    assert!(session_stdout.contains("right.md"), "{session_stdout}");
    let session_stderr = String::from_utf8(session_search_output.stderr).unwrap();
    assert!(
        session_stderr.contains(&format!(
            "security-worker-admission\tworker=content index set session records\tpath={}",
            records.display()
        )) && session_stderr.contains(&format!(
            "security-worker-admission\tworker=content index set session content\tpath={}",
            first_content.display()
        )) && session_stderr.contains(&format!(
            "security-worker-admission\tworker=content index set session content\tpath={}",
            second_content.display()
        )) && session_stderr.contains(
            "content-session-first\tcontent-archives=2\tcontent-keys=1\trecords-loaded=2"
        ) && session_stderr.contains("\tposting-cache-hits=0\tposting-cache-misses=1")
            && session_stderr.contains("\trecord-cache-hits=0\trecord-cache-misses=2")
            && session_stderr.contains("\tresult-cache-hits=0\tresult-cache-misses=1")
            && session_stderr.contains(
                "content-session-second\tcontent-archives=2\tcontent-keys=1\trecords-loaded=2"
            )
            && session_stderr.contains("\tposting-cache-hits=0\tposting-cache-misses=0")
            && session_stderr.contains("\trecord-cache-hits=0\trecord-cache-misses=0")
            && session_stderr.contains("\tresult-cache-hits=1\tresult-cache-misses=0"),
        "{session_stderr}"
    );

    let manifest_session_search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-index-manifest-session",
            records.to_str().unwrap(),
            manifest.to_str().unwrap(),
            "setneedle",
        ])
        .output()
        .unwrap();
    assert!(
        manifest_session_search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&manifest_session_search_output.stderr)
    );
    assert_eq!(
        String::from_utf8(manifest_session_search_output.stdout).unwrap(),
        session_stdout
    );
    let manifest_session_stderr = String::from_utf8(manifest_session_search_output.stderr).unwrap();
    assert!(
        manifest_session_stderr.contains(&format!(
            "security-worker-admission\tworker=content index manifest session records\tpath={}",
            records.display()
        )) && manifest_session_stderr.contains(&format!(
            "security-worker-admission\tworker=content index manifest session manifest\tpath={}",
            manifest.display()
        )) && manifest_session_stderr.contains(&format!(
            "security-worker-admission\tworker=content index manifest session content\tpath={}",
            first_content.display()
        )) && manifest_session_stderr.contains(&format!(
            "security-worker-admission\tworker=content index manifest session content\tpath={}",
            second_content.display()
        )) && manifest_session_stderr.contains(
            "content-manifest-session-second\tcontent-archives=2\tcontent-keys=1\trecords-loaded=2"
        ) && manifest_session_stderr.contains("\tposting-cache-hits=0\tposting-cache-misses=0")
            && manifest_session_stderr.contains("\trecord-cache-hits=0\trecord-cache-misses=0")
            && manifest_session_stderr.contains("\tresult-cache-hits=1\tresult-cache-misses=0"),
        "{manifest_session_stderr}"
    );

    let promote_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-manifest-promote",
            manifest.to_str().unwrap(),
            &format!("warm:{}", third_content.display()),
            first_content.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        promote_output.status.success(),
        "{}",
        String::from_utf8_lossy(&promote_output.stderr)
    );
    let promote_stderr = String::from_utf8(promote_output.stderr).unwrap();
    assert_worker_admitted(
        &promote_stderr,
        "content manifest promotion manifest",
        &manifest,
    );
    assert_worker_admitted(
        &promote_stderr,
        "content manifest promotion archive",
        &third_content,
    );
    assert_worker_admitted(
        &promote_stderr,
        "content manifest promotion retirement",
        &first_content,
    );
    assert!(
        promote_stderr.contains("content-manifest-promoted")
            && promote_stderr.contains("archives=2")
            && promote_stderr.contains("retired=1"),
        "{promote_stderr}"
    );
    let promote_stdout = String::from_utf8(promote_output.stdout).unwrap();
    assert!(
        promote_stdout.contains(&format!("retire\t{}", first_content.display())),
        "{promote_stdout}"
    );

    let ready_recovery_plan_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-manifest-promotion-recovery-plan",
            manifest.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        ready_recovery_plan_output.status.success(),
        "{}",
        String::from_utf8_lossy(&ready_recovery_plan_output.stderr)
    );
    let ready_recovery_plan_stderr = String::from_utf8_lossy(&ready_recovery_plan_output.stderr);
    assert_worker_admitted(
        &ready_recovery_plan_stderr,
        "content manifest promotion recovery plan",
        &manifest,
    );
    let ready_recovery_plan_stdout = String::from_utf8(ready_recovery_plan_output.stdout).unwrap();
    assert!(
        ready_recovery_plan_stdout.contains("action=ready"),
        "{ready_recovery_plan_stdout}"
    );

    let promoted_ids_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-ids-mmap-manifest",
            manifest.to_str().unwrap(),
            "promotedneedle",
        ])
        .output()
        .unwrap();
    assert!(
        promoted_ids_output.status.success(),
        "{}",
        String::from_utf8_lossy(&promoted_ids_output.stderr)
    );
    let promoted_ids_stdout = String::from_utf8(promoted_ids_output.stdout).unwrap();
    assert_eq!(
        promoted_ids_stdout.lines().count(),
        1,
        "{promoted_ids_stdout}"
    );

    let crash_manifest = unique_temp_path("gfm-cli-content-promotion-crash", "gfmmanifest");
    let crash_old = unique_temp_path("gfm-cli-content-promotion-crash-old", "gfmcontent");
    let crash_new = unique_temp_path("gfm-cli-content-promotion-crash-new", "gfmcontent");
    write_content_postings(
        &crash_old,
        &[ContentPosting {
            term: "oldneedle".to_string(),
            ids: vec![left],
            positions: vec![],
        }],
    )
    .unwrap();
    write_content_postings(
        &crash_new,
        &[ContentPosting {
            term: "crashneedle".to_string(),
            ids: vec![right],
            positions: vec![],
        }],
    )
    .unwrap();
    let previous_manifest = ContentArchiveManifest::new(vec![ContentArchiveManifestEntry {
        tier: ContentMergeTier::Hot,
        path: crash_old.clone(),
    }])
    .unwrap();
    previous_manifest.write(&crash_manifest).unwrap();
    let promotion_journal = ContentManifestPromotionJournal::new(
        previous_manifest,
        ContentArchiveManifestEntry {
            tier: ContentMergeTier::Warm,
            path: crash_new.clone(),
        },
        vec![crash_old.clone()],
    )
    .unwrap();
    let promotion_journal_path = content_manifest_promotion_journal_path(&crash_manifest);
    promotion_journal.write(&promotion_journal_path).unwrap();

    let pending_recovery_plan_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-manifest-promotion-recovery-plan",
            crash_manifest.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        pending_recovery_plan_output.status.success(),
        "{}",
        String::from_utf8_lossy(&pending_recovery_plan_output.stderr)
    );
    let pending_recovery_plan_stderr =
        String::from_utf8_lossy(&pending_recovery_plan_output.stderr);
    assert_worker_admitted(
        &pending_recovery_plan_stderr,
        "content manifest promotion recovery plan",
        &crash_manifest,
    );
    assert_worker_admitted(
        &pending_recovery_plan_stderr,
        "content manifest promotion recovery journal",
        &promotion_journal_path,
    );
    assert_worker_admitted(
        &pending_recovery_plan_stderr,
        "content manifest promotion recovery archive",
        &crash_new,
    );
    let pending_recovery_plan_stdout =
        String::from_utf8(pending_recovery_plan_output.stdout).unwrap();
    assert!(
        pending_recovery_plan_stdout.contains("action=complete-promotion"),
        "{pending_recovery_plan_stdout}"
    );

    let recovery_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-manifest-promotion-recover",
            crash_manifest.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        recovery_output.status.success(),
        "{}",
        String::from_utf8_lossy(&recovery_output.stderr)
    );
    let recovery_stderr = String::from_utf8_lossy(&recovery_output.stderr);
    assert_worker_admitted(
        &recovery_stderr,
        "content manifest promotion recovery",
        &crash_manifest,
    );
    assert_worker_admitted(
        &recovery_stderr,
        "content manifest promotion recovery journal",
        &promotion_journal_path,
    );
    assert_worker_admitted(
        &recovery_stderr,
        "content manifest promotion recovery archive",
        &crash_new,
    );
    let recovery_stdout = String::from_utf8(recovery_output.stdout).unwrap();
    assert!(
        recovery_stdout.contains("completed-promotion=true\tremoved-journal=true")
            && recovery_stdout.contains("action=ready"),
        "{recovery_stdout}"
    );
    assert!(!promotion_journal_path.exists());

    let crash_promoted_ids_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-ids-mmap-manifest",
            crash_manifest.to_str().unwrap(),
            "crashneedle",
        ])
        .output()
        .unwrap();
    assert!(
        crash_promoted_ids_output.status.success(),
        "{}",
        String::from_utf8_lossy(&crash_promoted_ids_output.stderr)
    );
    let crash_promoted_ids_stdout = String::from_utf8(crash_promoted_ids_output.stdout).unwrap();
    assert_eq!(
        crash_promoted_ids_stdout.lines().count(),
        1,
        "{crash_promoted_ids_stdout}"
    );

    let cleanup_plan_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-cleanup-plan",
            manifest.to_str().unwrap(),
            "1",
            "0",
            "1",
            first_content.to_str().unwrap(),
            second_content.to_str().unwrap(),
            &format!("{}.missing", first_content.display()),
        ])
        .output()
        .unwrap();
    assert!(
        cleanup_plan_output.status.success(),
        "{}",
        String::from_utf8_lossy(&cleanup_plan_output.stderr)
    );
    let cleanup_plan_stderr = String::from_utf8(cleanup_plan_output.stderr).unwrap();
    assert_worker_admitted(&cleanup_plan_stderr, "content cleanup plan", &manifest);
    assert_worker_admitted(
        &cleanup_plan_stderr,
        "content manifest cleanup candidate",
        &first_content,
    );
    assert_worker_admitted(
        &cleanup_plan_stderr,
        "content cleanup plan active archive",
        &second_content,
    );
    assert!(
        cleanup_plan_stderr.contains("content-cleanup-plan")
            && cleanup_plan_stderr.contains("action=Cleanup")
            && cleanup_plan_stderr.contains("cleanup=1")
            && cleanup_plan_stderr.contains("active=1")
            && cleanup_plan_stderr.contains("missing=1"),
        "{cleanup_plan_stderr}"
    );
    let cleanup_plan_stdout = String::from_utf8(cleanup_plan_output.stdout).unwrap();
    assert!(
        cleanup_plan_stdout.contains(&format!("cleanup\t{}", first_content.display()))
            && cleanup_plan_stdout.contains(&format!("active\t{}", second_content.display()))
            && cleanup_plan_stdout.contains("missing\t"),
        "{cleanup_plan_stdout}"
    );
    assert!(first_content.exists());
    assert!(second_content.exists());

    let cleanup_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-manifest-cleanup",
            manifest.to_str().unwrap(),
            first_content.to_str().unwrap(),
            second_content.to_str().unwrap(),
            &format!("{}.missing", first_content.display()),
        ])
        .output()
        .unwrap();
    assert!(
        cleanup_output.status.success(),
        "{}",
        String::from_utf8_lossy(&cleanup_output.stderr)
    );
    let cleanup_stderr = String::from_utf8(cleanup_output.stderr).unwrap();
    assert_worker_admitted(&cleanup_stderr, "content manifest cleanup", &manifest);
    assert!(
        cleanup_stderr.contains("content-manifest-cleanup")
            && cleanup_stderr.contains("removed=1")
            && cleanup_stderr.contains("active=1")
            && cleanup_stderr.contains("missing=1"),
        "{cleanup_stderr}"
    );
    let cleanup_stdout = String::from_utf8(cleanup_output.stdout).unwrap();
    assert!(
        cleanup_stdout.contains(&format!("removed\t{}", first_content.display()))
            && cleanup_stdout.contains(&format!("active\t{}", second_content.display()))
            && cleanup_stdout.contains("missing\t"),
        "{cleanup_stdout}"
    );
    assert!(!first_content.exists());
    assert!(second_content.exists());

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(second_content).unwrap();
    fs::remove_file(third_content).unwrap();
    fs::remove_file(manifest).unwrap();
    fs::remove_file(crash_manifest).unwrap();
    fs::remove_file(crash_old).unwrap();
    fs::remove_file(crash_new).unwrap();
    fs::remove_file(set_ids_retry_journal).unwrap();
    fs::remove_file(set_ids_retry_catalog).unwrap();
    fs::remove_file(set_ids_retry_progress).unwrap();
    fs::remove_file(set_ids_retry_probe).unwrap();
    fs::remove_file(manifest_ids_retry_journal).unwrap();
    fs::remove_file(manifest_ids_retry_catalog).unwrap();
    fs::remove_file(manifest_ids_retry_progress).unwrap();
    fs::remove_file(manifest_ids_retry_probe).unwrap();
}

#[test]
fn content_manifest_write_uses_cwd_write_probe_for_relative_manifest_from_binary() {
    let root = unique_temp_dir("gfm-cli-content-manifest-relative-root");
    fs::write(root.join("content.gfmcontent"), b"archive bytes").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .current_dir(&root)
        .args([
            "content-manifest-write",
            "content.gfmmanifest",
            "hot:content.gfmcontent",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(root.join("content.gfmmanifest").exists());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(&stderr, "content manifest write", &root);
    assert_worker_admitted(
        &stderr,
        "content manifest write archive",
        &root.join("content.gfmcontent"),
    );
    assert!(
        !stderr.contains("security-worker-admission\tworker=content manifest write\tpath=\t"),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn content_manifest_write_reports_manifest_probe_failure_before_publishing_from_binary() {
    let root = unique_temp_dir("gfm-cli-content-manifest-write-probe");
    let manifest = root.join(format!("{}.gfmmanifest", "manifest-unavailable".repeat(16)));
    let content = root.join("content.gfmcontent");
    fs::write(&content, b"archive bytes").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-manifest-write",
            manifest.to_str().unwrap(),
            &format!("hot:{}", content.display()),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains("manifest write path metadata unavailable"),
        "{stderr}"
    );
    assert!(stderr.contains("manifest-unavailable"), "{stderr}");
    assert!(
        !stderr.contains("security-worker-admission\tworker=content manifest write\t"),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn content_manifest_write_reports_manifest_probe_failure_before_archive_admission_from_binary() {
    let root = unique_temp_dir("gfm-cli-content-manifest-write-admission-probe");
    let content = root.join("content.gfmcontent");
    let manifest = root.join(format!(
        "{}.gfmmanifest",
        "content-manifest-unavailable".repeat(16)
    ));
    fs::write(&content, b"archive bytes").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-manifest-write",
            manifest.to_str().unwrap(),
            &format!("hot:{}", content.display()),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains("manifest write path metadata unavailable"),
        "{stderr}"
    );
    assert!(stderr.contains("content-manifest-unavailable"), "{stderr}");
    assert!(
        !stderr.contains("security-worker-admission\tworker=content manifest write\t"),
        "{stderr}"
    );
    assert!(!manifest.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn content_manifest_write_refuses_unreachable_archive_before_publishing_from_binary() {
    let root = unique_temp_dir("gfm-cli-content-manifest-write-unreachable");
    let local = root.join("local");
    let offline = root.join("offline");
    fs::create_dir_all(&local).unwrap();
    fs::create_dir_all(&offline).unwrap();
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let manifest = local.join("content.gfmmanifest");
    let content = offline.join("content.gfmcontent");
    fs::write(&content, "not admitted into manifest").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-manifest-write",
            manifest.to_str().unwrap(),
            &format!("hot:{}", content.display()),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains(
            "content manifest write archive volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=content manifest write archive\tpath={}",
            content.display()
        )),
        "{stderr}"
    );
    assert!(!manifest.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn content_manifest_inspect_refuses_unreachable_archive_before_mapping_from_binary() {
    let root = unique_temp_dir("gfm-cli-content-manifest-inspect-unreachable");
    let local = root.join("local");
    let offline = root.join("offline");
    fs::create_dir_all(&local).unwrap();
    fs::create_dir_all(&offline).unwrap();
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let manifest = local.join("content.gfmmanifest");
    let content = offline.join("content.gfmcontent");
    fs::write(&content, "not mmap content").unwrap();
    ContentArchiveManifest::new(vec![ContentArchiveManifestEntry {
        tier: ContentMergeTier::Hot,
        path: content.clone(),
    }])
    .unwrap()
    .write(&manifest)
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["content-manifest-inspect", manifest.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains(
            "content manifest inspect archive volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "security-worker-admission\tworker=content manifest inspect\tpath={}",
            manifest.display()
        )),
        "{stderr}"
    );
    assert!(
        !stderr.contains("invalid magic") && !stderr.contains("content archive"),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn content_manifest_promotion_recovery_plan_refuses_unreachable_archive_before_mapping_from_binary()
{
    let root = unique_temp_dir("gfm-cli-content-promotion-recovery-plan-unreachable");
    let local = root.join("local");
    let offline = root.join("offline");
    fs::create_dir_all(&local).unwrap();
    fs::create_dir_all(&offline).unwrap();
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let manifest = local.join("content.gfmmanifest");
    let old = local.join("old.gfmcontent");
    let new_archive = offline.join("new.gfmcontent");
    write_content_postings(
        &old,
        &[ContentPosting {
            term: "oldneedle".to_string(),
            ids: vec![FileId::new(VolumeId(1), 1)],
            positions: vec![],
        }],
    )
    .unwrap();
    fs::write(&new_archive, "not mmap content").unwrap();
    let previous = ContentArchiveManifest::new(vec![ContentArchiveManifestEntry {
        tier: ContentMergeTier::Hot,
        path: old.clone(),
    }])
    .unwrap();
    previous.write(&manifest).unwrap();
    let journal = ContentManifestPromotionJournal::new(
        previous,
        ContentArchiveManifestEntry {
            tier: ContentMergeTier::Warm,
            path: new_archive.clone(),
        },
        vec![old.clone()],
    )
    .unwrap();
    let journal_path = content_manifest_promotion_journal_path(&manifest);
    journal.write(&journal_path).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-manifest-promotion-recovery-plan",
            manifest.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("content-manifest-promotion-recovery-plan\t"),
        "{stdout}"
    );
    assert!(
        stderr.contains(
            "content manifest promotion recovery archive volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert_worker_admitted(
        &stderr,
        "content manifest promotion recovery plan",
        &manifest,
    );
    assert_worker_admitted(
        &stderr,
        "content manifest promotion recovery journal",
        &journal_path,
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=content manifest promotion recovery archive\tpath={}",
            new_archive.display()
        )),
        "{stderr}"
    );
    assert!(
        !stderr.contains("invalid magic") && !stderr.contains("not readable"),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn content_manifest_promotion_recover_refuses_unreachable_archive_before_completing_from_binary() {
    let root = unique_temp_dir("gfm-cli-content-promotion-recover-unreachable");
    let local = root.join("local");
    let offline = root.join("offline");
    fs::create_dir_all(&local).unwrap();
    fs::create_dir_all(&offline).unwrap();
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let manifest = local.join("content.gfmmanifest");
    let old = local.join("old.gfmcontent");
    let new_archive = offline.join("new.gfmcontent");
    write_content_postings(
        &old,
        &[ContentPosting {
            term: "oldneedle".to_string(),
            ids: vec![FileId::new(VolumeId(1), 1)],
            positions: vec![],
        }],
    )
    .unwrap();
    fs::write(&new_archive, "not mmap content").unwrap();
    let previous = ContentArchiveManifest::new(vec![ContentArchiveManifestEntry {
        tier: ContentMergeTier::Hot,
        path: old.clone(),
    }])
    .unwrap();
    previous.write(&manifest).unwrap();
    let original_manifest = fs::read_to_string(&manifest).unwrap();
    let journal = ContentManifestPromotionJournal::new(
        previous,
        ContentArchiveManifestEntry {
            tier: ContentMergeTier::Warm,
            path: new_archive.clone(),
        },
        vec![old.clone()],
    )
    .unwrap();
    let journal_path = content_manifest_promotion_journal_path(&manifest);
    journal.write(&journal_path).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-manifest-promotion-recover",
            manifest.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("content-manifest-promotion-recovery\t"),
        "{stdout}"
    );
    assert!(
        stderr.contains(
            "content manifest promotion recovery archive volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert_worker_admitted(&stderr, "content manifest promotion recovery", &manifest);
    assert_worker_admitted(
        &stderr,
        "content manifest promotion recovery journal",
        &journal_path,
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=content manifest promotion recovery archive\tpath={}",
            new_archive.display()
        )),
        "{stderr}"
    );
    assert!(
        !stderr.contains("invalid magic") && !stderr.contains("not readable"),
        "{stderr}"
    );
    assert_eq!(fs::read_to_string(&manifest).unwrap(), original_manifest);
    assert!(journal_path.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn content_manifest_promotion_recovery_plan_surfaces_journal_probe_failure_from_binary() {
    let root = unique_temp_dir("gfm-cli-content-promotion-recovery-plan-journal-probe");
    let manifest = root.join(format!("{}.gfmmanifest", "m".repeat(230)));
    fs::write(
        &manifest,
        "manifest is not parsed after journal probe failure\n",
    )
    .unwrap();
    let journal = content_manifest_promotion_journal_path(&manifest);

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-manifest-promotion-recovery-plan",
            manifest.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("content-manifest-promotion-recovery-plan\t"),
        "{stdout}"
    );
    assert!(
        stderr.contains("manifest read path existence unavailable"),
        "{stderr}"
    );
    assert!(stderr.contains(&journal.display().to_string()), "{stderr}");
    assert!(
        !stderr.contains("action=ready") && !stderr.contains("no pending promotion journal"),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn content_manifest_promotion_recovery_plan_rejects_duplicate_retire_journal_from_binary() {
    let root = unique_temp_dir("gfm-cli-content-promotion-recovery-duplicate-retire");
    let manifest = root.join("content.gfmmanifest");
    fs::write(
        &manifest,
        "gfm-content-manifest-v1\narchive\thot\thot-a.gfmcontent\n",
    )
    .unwrap();
    let journal = content_manifest_promotion_journal_path(&manifest);
    fs::write(
        &journal,
        "gfm-content-promotion-journal-v1\nprevious\thot\thot-a.gfmcontent\nnew\twarm\twarm-b.gfmcontent\nretire\thot-a.gfmcontent\nretire\thot-a.gfmcontent\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-manifest-promotion-recovery-plan",
            manifest.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("content-manifest-promotion-recovery-plan\t"),
        "{stdout}"
    );
    assert!(
        stderr.contains("duplicate retired archive path `hot-a.gfmcontent`"),
        "{stderr}"
    );
    assert!(stderr.contains("line 5"), "{stderr}");
    assert_worker_admitted(
        &stderr,
        "content manifest promotion recovery plan",
        &manifest,
    );
    assert_worker_admitted(
        &stderr,
        "content manifest promotion recovery journal",
        &journal,
    );
    assert!(
        !stderr.contains(
            "security-worker-admission\tworker=content manifest promotion recovery archive\t"
        ),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn content_manifest_promotion_recover_surfaces_journal_probe_failure_before_mutation_from_binary() {
    let root = unique_temp_dir("gfm-cli-content-promotion-recover-journal-probe");
    let manifest = root.join(format!("{}.gfmmanifest", "m".repeat(230)));
    fs::write(
        &manifest,
        "manifest is not mutated after journal probe failure\n",
    )
    .unwrap();
    let original_manifest = fs::read_to_string(&manifest).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-manifest-promotion-recover",
            manifest.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("content-manifest-promotion-recovery\t"),
        "{stdout}"
    );
    assert!(
        stderr.contains("manifest read path existence unavailable"),
        "{stderr}"
    );
    assert_eq!(fs::read_to_string(&manifest).unwrap(), original_manifest);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn content_manifest_recovery_plan_surfaces_manifest_path_probe_failures_from_binary() {
    let root = unique_temp_dir("gfm-cli-content-recovery-plan-manifest-probe");
    let manifest = root.join("content-manifest-unavailable".repeat(64));

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["content-manifest-recovery-plan", manifest.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains("manifest read path existence unavailable"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("content-manifest-recovery-plan\t"),
        "{stderr}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn content_cleanup_plan_refuses_unreachable_active_archive_before_metadata_from_binary() {
    let root = unique_temp_dir("gfm-cli-content-cleanup-plan-active-unreachable");
    let local = root.join("local");
    let offline = root.join("offline");
    fs::create_dir_all(&local).unwrap();
    fs::create_dir_all(&offline).unwrap();
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let manifest = local.join("content.gfmmanifest");
    let active = offline.join("active.gfmcontent");
    let retired = local.join("retired.gfmcontent");
    fs::write(
        &active,
        "active archive is not statted after access denial\n",
    )
    .unwrap();
    write_content_postings(
        &retired,
        &[ContentPosting {
            term: "retiredneedle".to_string(),
            ids: vec![FileId::new(VolumeId(1), 1)],
            positions: vec![],
        }],
    )
    .unwrap();
    ContentArchiveManifest::new(vec![ContentArchiveManifestEntry {
        tier: ContentMergeTier::Hot,
        path: active.clone(),
    }])
    .unwrap()
    .write(&manifest)
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-cleanup-plan",
            manifest.to_str().unwrap(),
            "1",
            "0",
            "1",
            retired.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("content-cleanup-plan\t"), "{stdout}");
    assert!(
        stderr.contains(
            "content cleanup plan active archive volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert_worker_admitted(&stderr, "content cleanup plan", &manifest);
    assert_worker_admitted(&stderr, "content manifest cleanup candidate", &retired);
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=content cleanup plan active archive\tpath={}",
            active.display()
        )),
        "{stderr}"
    );
    assert!(
        !stderr.contains("after access denial") && !stderr.contains("metadata"),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn content_manifest_recovery_plan_refuses_unreachable_archive_before_classifying_from_binary() {
    let root = unique_temp_dir("gfm-cli-content-manifest-recovery-plan-unreachable");
    let local = root.join("local");
    let offline = root.join("offline");
    fs::create_dir_all(&local).unwrap();
    fs::create_dir_all(&offline).unwrap();
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let manifest = local.join("content.gfmmanifest");
    let content = offline.join("content.gfmcontent");
    fs::write(&manifest, "not-a-content-manifest").unwrap();
    fs::write(&content, "not mmap content").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-manifest-recovery-plan",
            manifest.to_str().unwrap(),
            &format!("hot:{}", content.display()),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("content-manifest-recovery-plan\t"),
        "{stdout}"
    );
    assert!(
        stderr.contains(
            "content manifest recovery discovered archive volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=content manifest recovery discovered archive\tpath={}",
            content.display()
        )),
        "{stderr}"
    );
    assert!(!stderr.contains("invalid magic"), "{stderr}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn content_manifest_cleanup_reports_candidate_probe_failure_before_removing_from_binary() {
    let root = unique_temp_dir("gfm-cli-content-manifest-cleanup-probe");
    let manifest = root.join("content.gfmmanifest");
    let active = root.join("active.gfmcontent");
    let retired = root.join("retired.gfmcontent");
    let unavailable = root.join(format!("{}.gfmcontent", "cleanup-unavailable".repeat(16)));
    write_content_postings(
        &active,
        &[ContentPosting {
            term: "activeneedle".to_string(),
            ids: vec![FileId::new(VolumeId(1), 1)],
            positions: vec![],
        }],
    )
    .unwrap();
    write_content_postings(
        &retired,
        &[ContentPosting {
            term: "retiredneedle".to_string(),
            ids: vec![FileId::new(VolumeId(1), 2)],
            positions: vec![],
        }],
    )
    .unwrap();
    ContentArchiveManifest::new(vec![
        ContentArchiveManifestEntry {
            tier: ContentMergeTier::Hot,
            path: active.clone(),
        },
        ContentArchiveManifestEntry {
            tier: ContentMergeTier::Cold,
            path: retired.clone(),
        },
    ])
    .unwrap()
    .write(&manifest)
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-manifest-cleanup",
            manifest.to_str().unwrap(),
            unavailable.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("removed\t"), "{stdout}");
    assert!(
        stderr.contains("manifest write path metadata unavailable"),
        "{stderr}"
    );
    assert!(stderr.contains("cleanup-unavailable"), "{stderr}");
    assert!(
        !stderr.contains("security-worker-admission\tworker=content manifest cleanup candidate\t"),
        "{stderr}"
    );
    assert!(active.exists());
    assert!(retired.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn recovers_corrupt_content_manifest_from_binary() {
    let manifest = unique_temp_path("gfm-cli-content-manifest-recovery", "gfmmanifest");
    let content = unique_temp_path("gfm-cli-content-manifest-recovery", "gfmcontent");
    let quarantine = unique_temp_dir("gfm-cli-content-manifest-recovery-quarantine");
    write_content_postings(
        &content,
        &[ContentPosting {
            term: "recoverneedle".to_string(),
            ids: vec![FileId::new(VolumeId(1), 7)],
            positions: Vec::new(),
        }],
    )
    .unwrap();
    fs::write(&manifest, "not-a-content-manifest").unwrap();

    let plan_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-manifest-recovery-plan",
            manifest.to_str().unwrap(),
            &format!("hot:{}", content.display()),
        ])
        .output()
        .unwrap();
    assert!(
        plan_output.status.success(),
        "{}",
        String::from_utf8_lossy(&plan_output.stderr)
    );
    let plan_stderr = String::from_utf8_lossy(&plan_output.stderr);
    assert_worker_admitted(&plan_stderr, "content manifest recovery plan", &manifest);
    assert_worker_admitted(
        &plan_stderr,
        "content manifest recovery discovered archive",
        &content,
    );
    let plan_stdout = String::from_utf8(plan_output.stdout).unwrap();
    assert!(
        plan_stdout.contains("action=quarantine-manifest-and-write-discovered")
            && plan_stdout.contains("reason=unreadable-manifest")
            && plan_stdout.contains("valid=1"),
        "{plan_stdout}"
    );

    let recover_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-manifest-recover",
            manifest.to_str().unwrap(),
            quarantine.to_str().unwrap(),
            &format!("hot:{}", content.display()),
        ])
        .output()
        .unwrap();
    assert!(
        recover_output.status.success(),
        "{}",
        String::from_utf8_lossy(&recover_output.stderr)
    );
    let recover_stderr = String::from_utf8_lossy(&recover_output.stderr);
    assert_worker_admitted(&recover_stderr, "content manifest recovery plan", &manifest);
    assert_worker_admitted(
        &recover_stderr,
        "content manifest recovery discovered archive",
        &content,
    );
    assert_worker_admitted(
        &recover_stderr,
        "content manifest recovery manifest",
        manifest.parent().unwrap(),
    );
    assert_worker_admitted(
        &recover_stderr,
        "content manifest recovery quarantine",
        &quarantine,
    );
    let recover_stdout = String::from_utf8(recover_output.stdout).unwrap();
    assert!(
        recover_stdout.contains("wrote-manifest=true") && recover_stdout.contains("action=ready"),
        "{recover_stdout}"
    );
    assert!(quarantine.read_dir().unwrap().any(|entry| entry
        .unwrap()
        .path()
        .extension()
        .is_some()));

    let inspect_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["content-manifest-inspect", manifest.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        inspect_output.status.success(),
        "{}",
        String::from_utf8_lossy(&inspect_output.stderr)
    );
    let inspect_stdout = String::from_utf8(inspect_output.stdout).unwrap();
    assert!(
        inspect_stdout.contains("content-manifest\tarchives=1"),
        "{inspect_stdout}"
    );

    fs::remove_file(manifest).unwrap();
    fs::remove_file(content).unwrap();
    fs::remove_dir_all(quarantine).unwrap();
}

#[test]
fn content_manifest_recover_persists_parent_volume_for_missing_manifest_from_binary() {
    let root = unique_temp_dir("gfm-cli-content-manifest-recovery-volume");
    let manifest = root.join("missing.gfmmanifest");
    let content = root.join("content.gfmcontent");
    let quarantine = root.join("quarantine");
    let catalog = unique_temp_path("gfm-cli-content-manifest-recovery-volume", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-content-manifest-recovery-volume", "gfmprogress");
    fs::create_dir_all(&quarantine).unwrap();
    write_content_postings(
        &content,
        &[ContentPosting {
            term: "recoverneedle".to_string(),
            ids: vec![FileId::new(VolumeId(1), 7)],
            positions: Vec::new(),
        }],
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "content-manifest-recover",
            manifest.to_str().unwrap(),
            quarantine.to_str().unwrap(),
            &format!("hot:{}", content.display()),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let volume = test_volume_id(&root);
    let catalog_text = fs::read_to_string(&catalog).unwrap();
    assert!(
        catalog_text
            .lines()
            .any(|line| line.starts_with("payload\t1\t")
                && line.contains("\tcontent manifest recovery\t")
                && line.contains(&format!("\t{volume}\t"))),
        "{catalog_text}"
    );
    let progress_text = fs::read_to_string(&progress).unwrap();
    assert!(
        progress_text
            .lines()
            .any(|line| line.starts_with("progress\t1\t")
                && line.contains("\tcontent manifest recovery\t")
                && line.contains(&format!("\t{volume}\t"))
                && line.contains("\tcompleted\t")),
        "{progress_text}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
}

#[test]
fn content_manifest_promote_refuses_unreachable_volume_before_journaling_from_binary() {
    let root = unique_temp_dir("gfm-cli-content-manifest-promote-unreachable");
    let manifest = root.join("content.gfmmanifest");
    let old_content = root.join("old.gfmcontent");
    let new_content = root.join("new.gfmcontent");
    write_content_postings(
        &old_content,
        &[ContentPosting {
            term: "oldneedle".to_string(),
            ids: vec![FileId::new(VolumeId(1), 1)],
            positions: Vec::new(),
        }],
    )
    .unwrap();
    write_content_postings(
        &new_content,
        &[ContentPosting {
            term: "newneedle".to_string(),
            ids: vec![FileId::new(VolumeId(1), 2)],
            positions: Vec::new(),
        }],
    )
    .unwrap();
    ContentArchiveManifest::new(vec![ContentArchiveManifestEntry {
        tier: ContentMergeTier::Hot,
        path: old_content.clone(),
    }])
    .unwrap()
    .write(&manifest)
    .unwrap();
    let original_manifest = fs::read_to_string(&manifest).unwrap();
    let journal = content_manifest_promotion_journal_path(&manifest);
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-manifest-promote",
            manifest.to_str().unwrap(),
            &format!("warm:{}", new_content.display()),
            old_content.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("retire\t"), "{stdout}");
    assert!(
        stderr.contains(
            "content manifest promotion manifest volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=content manifest promotion\tpath={}",
            manifest.display()
        )),
        "{stderr}"
    );
    assert_eq!(fs::read_to_string(&manifest).unwrap(), original_manifest);
    assert!(!journal.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn content_manifest_recover_refuses_unreachable_volume_before_quarantine_from_binary() {
    let root = unique_temp_dir("gfm-cli-content-manifest-recover-unreachable");
    let manifest = root.join("content.gfmmanifest");
    let content = root.join("content.gfmcontent");
    let quarantine = root.join("quarantine");
    fs::create_dir_all(&quarantine).unwrap();
    write_content_postings(
        &content,
        &[ContentPosting {
            term: "recoverneedle".to_string(),
            ids: vec![FileId::new(VolumeId(1), 7)],
            positions: Vec::new(),
        }],
    )
    .unwrap();
    fs::write(&manifest, "not-a-content-manifest").unwrap();
    let original_manifest = fs::read_to_string(&manifest).unwrap();
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-manifest-recover",
            manifest.to_str().unwrap(),
            quarantine.to_str().unwrap(),
            &format!("hot:{}", content.display()),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("content-manifest-recovery\t"), "{stdout}");
    assert!(
        stderr.contains(
            "content manifest recovery plan volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=content manifest recovery plan\tpath={}",
            manifest.display()
        )),
        "{stderr}"
    );
    assert_eq!(fs::read_to_string(&manifest).unwrap(), original_manifest);
    assert!(fs::read_dir(&quarantine).unwrap().next().is_none());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn content_manifest_recover_refuses_unreachable_quarantine_before_writing_from_binary() {
    let root = unique_temp_dir("gfm-cli-content-manifest-recover-quarantine-unreachable");
    let local = root.join("local");
    let offline = root.join("offline");
    fs::create_dir_all(&local).unwrap();
    fs::create_dir_all(&offline).unwrap();
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let manifest = local.join("content.gfmmanifest");
    let content = local.join("content.gfmcontent");
    let quarantine = offline.join("quarantine");
    fs::create_dir_all(&quarantine).unwrap();
    write_content_postings(
        &content,
        &[ContentPosting {
            term: "recoverneedle".to_string(),
            ids: vec![FileId::new(VolumeId(1), 7)],
            positions: Vec::new(),
        }],
    )
    .unwrap();
    fs::write(&manifest, "not-a-content-manifest").unwrap();
    let original_manifest = fs::read_to_string(&manifest).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-manifest-recover",
            manifest.to_str().unwrap(),
            quarantine.to_str().unwrap(),
            &format!("hot:{}", content.display()),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("content-manifest-recovery\t"), "{stdout}");
    assert!(
        stderr.contains(
            "content manifest recovery quarantine volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=content manifest recovery manifest\tpath={}",
            manifest.parent().unwrap().display()
        )),
        "{stderr}"
    );
    assert_eq!(fs::read_to_string(&manifest).unwrap(), original_manifest);
    assert!(fs::read_dir(&quarantine).unwrap().next().is_none());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn content_manifest_recover_reports_quarantine_probe_failure_before_mutation_from_binary() {
    let root = unique_temp_dir("gfm-cli-content-manifest-recover-quarantine-probe");
    let manifest = root.join("content.gfmmanifest");
    let content = root.join("content.gfmcontent");
    let quarantine = root.join("quarantine-unavailable".repeat(16));
    write_content_postings(
        &content,
        &[ContentPosting {
            term: "recoverneedle".to_string(),
            ids: vec![FileId::new(VolumeId(1), 7)],
            positions: Vec::new(),
        }],
    )
    .unwrap();
    fs::write(&manifest, "not-a-content-manifest").unwrap();
    let original_manifest = fs::read_to_string(&manifest).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-manifest-recover",
            manifest.to_str().unwrap(),
            quarantine.to_str().unwrap(),
            &format!("hot:{}", content.display()),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("content-manifest-recovery\t"), "{stdout}");
    assert!(
        stderr.contains("manifest write path metadata unavailable"),
        "{stderr}"
    );
    assert!(stderr.contains("quarantine-unavailable"), "{stderr}");
    assert!(
        !stderr.contains("security-worker-admission\tworker=content manifest recovery manifest\t"),
        "{stderr}"
    );
    assert_eq!(fs::read_to_string(&manifest).unwrap(), original_manifest);
    assert!(!quarantine.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn compacts_content_segments_from_binary() {
    let root = unique_temp_dir("gfm-cli-segment-content-root");
    let records = unique_temp_path("gfm-cli-segment-records", "gfmidx");
    let segment = unique_temp_path("gfm-cli-segment-content", "gfmseg");
    let content = unique_temp_path("gfm-cli-segment-compact", "gfmcontent");
    fs::write(root.join("segment.md"), "the body contains segmentmarker").unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), records.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );

    let segment_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-content-segment",
            root.to_str().unwrap(),
            segment.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        segment_output.status.success(),
        "{}",
        String::from_utf8_lossy(&segment_output.stderr)
    );

    let compact_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "compact-content",
            content.to_str().unwrap(),
            segment.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        compact_output.status.success(),
        "{}",
        String::from_utf8_lossy(&compact_output.stderr)
    );
    let compact_stderr = String::from_utf8(compact_output.stderr).unwrap();
    assert_worker_admitted(
        &compact_stderr,
        "content compaction",
        content.parent().unwrap(),
    );
    assert_worker_admitted(&compact_stderr, "content compaction", &segment);
    let tiered_content = unique_temp_path("gfm-cli-segment-tiered-compact", "gfmcontent");
    let tiered_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "compact-content-tiered",
            tiered_content.to_str().unwrap(),
            segment.to_str().unwrap(),
            segment.to_str().unwrap(),
            segment.to_str().unwrap(),
            segment.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        tiered_output.status.success(),
        "{}",
        String::from_utf8_lossy(&tiered_output.stderr)
    );
    let tiered_stderr = String::from_utf8(tiered_output.stderr).unwrap();
    assert_worker_admitted(
        &tiered_stderr,
        "tiered content compaction",
        tiered_content.parent().unwrap(),
    );
    assert_worker_admitted(&tiered_stderr, "tiered content compaction", &segment);
    assert!(
        tiered_stderr.contains("tiered-compacted")
            && tiered_stderr.contains("merged 4")
            && tiered_stderr.contains("retained 0"),
        "{tiered_stderr}"
    );
    let manifest = unique_temp_path("gfm-cli-segment-maintenance", "gfmmanifest");
    let maintained_content = unique_temp_path("gfm-cli-segment-maintained", "gfmcontent");
    let manifest_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-manifest-write",
            manifest.to_str().unwrap(),
            &format!("hot:{}", content.display()),
        ])
        .output()
        .unwrap();
    assert!(
        manifest_output.status.success(),
        "{}",
        String::from_utf8_lossy(&manifest_output.stderr)
    );
    let footprint_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-footprint",
            records.to_str().unwrap(),
            "-",
            "-",
            "-",
            "-",
            "-",
            manifest.to_str().unwrap(),
            segment.to_str().unwrap(),
            segment.to_str().unwrap(),
            segment.to_str().unwrap(),
            segment.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        footprint_output.status.success(),
        "{}",
        String::from_utf8_lossy(&footprint_output.stderr)
    );
    let footprint_stderr = String::from_utf8(footprint_output.stderr).unwrap();
    assert_worker_admitted(&footprint_stderr, "index footprint records", &records);
    assert_worker_admitted(
        &footprint_stderr,
        "index footprint content manifest",
        &manifest,
    );
    assert_worker_admitted(
        &footprint_stderr,
        "index footprint content archive",
        &content,
    );
    assert_worker_admitted(
        &footprint_stderr,
        "index footprint content segment",
        &segment,
    );
    assert_eq!(
        worker_admission_count(
            &footprint_stderr,
            "index footprint content segment",
            &segment
        ),
        1,
        "{footprint_stderr}"
    );
    assert!(
        footprint_stderr.contains("index-footprint")
            && footprint_stderr.contains("compaction-scheduled=true")
            && footprint_stderr.contains("reason=TierPressure"),
        "{footprint_stderr}"
    );
    let footprint_stdout = String::from_utf8(footprint_output.stdout).unwrap();
    assert!(
        footprint_stdout.contains("records\tcount=")
            && footprint_stdout.contains("content\tarchives=1")
            && footprint_stdout.contains("compaction\tscheduled=true"),
        "{footprint_stdout}"
    );
    let adaptive_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-compaction-plan",
            records.to_str().unwrap(),
            manifest.to_str().unwrap(),
            "saturated",
            "nominal",
            "ac",
            "idle",
            segment.to_str().unwrap(),
            segment.to_str().unwrap(),
            segment.to_str().unwrap(),
            segment.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        adaptive_output.status.success(),
        "{}",
        String::from_utf8_lossy(&adaptive_output.stderr)
    );
    let adaptive_stderr = String::from_utf8(adaptive_output.stderr).unwrap();
    assert_worker_admitted(&adaptive_stderr, "index compaction plan records", &records);
    assert_worker_admitted(
        &adaptive_stderr,
        "index compaction plan content manifest",
        &manifest,
    );
    assert_worker_admitted(
        &adaptive_stderr,
        "index compaction plan content archive",
        &content,
    );
    assert_worker_admitted(
        &adaptive_stderr,
        "index compaction plan content segment",
        &segment,
    );
    assert!(
        adaptive_stderr.contains("index-compaction-plan")
            && adaptive_stderr.contains("action=Defer")
            && adaptive_stderr.contains("scheduled=false"),
        "{adaptive_stderr}"
    );
    let adaptive_stdout = String::from_utf8(adaptive_output.stdout).unwrap();
    assert!(
        adaptive_stdout.contains("compaction\taction=Defer")
            && adaptive_stdout.contains("effective-max-bytes=0"),
        "{adaptive_stdout}"
    );
    let deferred_content = unique_temp_path("gfm-cli-segment-maintained-deferred", "gfmcontent");
    let adaptive_maintenance_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-maintain-segments-adaptive",
            manifest.to_str().unwrap(),
            deferred_content.to_str().unwrap(),
            "saturated",
            "nominal",
            "ac",
            "idle",
            segment.to_str().unwrap(),
            segment.to_str().unwrap(),
            segment.to_str().unwrap(),
            segment.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        adaptive_maintenance_output.status.success(),
        "{}",
        String::from_utf8_lossy(&adaptive_maintenance_output.stderr)
    );
    let adaptive_maintenance_stderr =
        String::from_utf8(adaptive_maintenance_output.stderr).unwrap();
    assert!(
        adaptive_maintenance_stderr.contains("content-maintenance-deferred")
            && adaptive_maintenance_stderr.contains("action=Defer"),
        "{adaptive_maintenance_stderr}"
    );
    assert!(!deferred_content.exists());

    let maintenance_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-maintain-segments",
            manifest.to_str().unwrap(),
            maintained_content.to_str().unwrap(),
            segment.to_str().unwrap(),
            segment.to_str().unwrap(),
            segment.to_str().unwrap(),
            segment.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        maintenance_output.status.success(),
        "{}",
        String::from_utf8_lossy(&maintenance_output.stderr)
    );
    let maintenance_stderr = String::from_utf8(maintenance_output.stderr).unwrap();
    assert_worker_admitted(&maintenance_stderr, "content maintenance", &manifest);
    assert_worker_admitted(
        &maintenance_stderr,
        "content maintenance",
        maintained_content.parent().unwrap(),
    );
    assert_worker_admitted(&maintenance_stderr, "content maintenance", &segment);
    assert!(
        maintenance_stderr.contains("content-maintenance")
            && maintenance_stderr.contains("scheduled=true")
            && maintenance_stderr.contains("merged=4")
            && maintenance_stderr.contains("manifest-archives=2"),
        "{maintenance_stderr}"
    );
    let maintenance_stdout = String::from_utf8(maintenance_output.stdout).unwrap();
    assert!(
        maintenance_stdout.contains(&format!("published\t{}", maintained_content.display())),
        "{maintenance_stdout}"
    );

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-index",
            records.to_str().unwrap(),
            content.to_str().unwrap(),
            "segmentmarker",
        ])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );

    let stdout = String::from_utf8(search_output.stdout).unwrap();
    assert!(stdout.contains("segment.md"), "{stdout}");
    let manifest_search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-index-manifest",
            records.to_str().unwrap(),
            manifest.to_str().unwrap(),
            "segmentmarker",
        ])
        .output()
        .unwrap();
    assert!(
        manifest_search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&manifest_search_output.stderr)
    );
    let manifest_stdout = String::from_utf8(manifest_search_output.stdout).unwrap();
    assert!(manifest_stdout.contains("segment.md"), "{manifest_stdout}");

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(segment).unwrap();
    fs::remove_file(content).unwrap();
    fs::remove_file(tiered_content).unwrap();
    fs::remove_file(maintained_content).unwrap();
    fs::remove_file(manifest).unwrap();
}

#[test]
fn index_content_segment_retries_transient_failure_from_binary() {
    let root = unique_temp_dir("gfm-cli-segment-retry-root");
    let segment = unique_temp_path("gfm-cli-segment-retry", "gfmseg");
    let content = unique_temp_path("gfm-cli-segment-retry-compact", "gfmcontent");
    let journal = unique_temp_path("gfm-cli-segment-retry", "journal");
    let catalog = unique_temp_path("gfm-cli-segment-retry", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-segment-retry", "gfmprogress");
    let retry_probe = unique_temp_path("gfm-cli-segment-retry", "state");
    fs::write(
        root.join("segment-retry.md"),
        "the body contains segmentretrymarker",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "index-content-segment-retry-probe",
            root.to_str().unwrap(),
            segment.to_str().unwrap(),
            retry_probe.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_to_string(&retry_probe).unwrap(), "2");
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(
        journal_text.contains("1\t1\tstarted\tcontent segment index"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains(
            "1\t1\tfailed:temporary content segment index retry probe busy\tcontent segment index"
        ),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tstarted\tcontent segment index"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tcompleted\tcontent segment index"),
        "{journal_text}"
    );

    let compact_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "compact-content",
            content.to_str().unwrap(),
            segment.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        compact_output.status.success(),
        "{}",
        String::from_utf8_lossy(&compact_output.stderr)
    );
    assert!(fs::metadata(&content).unwrap().len() > 0);

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(segment).unwrap();
    fs::remove_file(content).unwrap();
    fs::remove_file(journal).unwrap();
    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
    fs::remove_file(retry_probe).unwrap();
}

#[test]
fn index_footprint_refuses_unreachable_manifest_archive_before_mapping_from_binary() {
    let root = unique_temp_dir("gfm-cli-footprint-source");
    let offline = unique_temp_dir("gfm-cli-footprint-archive-unreachable");
    fs::write(root.join("note.md"), "footprint source").unwrap();
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let records = unique_temp_path("gfm-cli-footprint-records", "gfmidx");
    let manifest = root.join("content.gfmmanifest");
    let content = offline.join("offline.gfmcontent");
    fs::write(&content, "not a mmap content archive").unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), records.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );
    ContentArchiveManifest::new(vec![ContentArchiveManifestEntry {
        tier: ContentMergeTier::Hot,
        path: content.clone(),
    }])
    .unwrap()
    .write(&manifest)
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-footprint",
            records.to_str().unwrap(),
            "-",
            "-",
            "-",
            "-",
            "-",
            manifest.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("records\tcount="), "{stdout}");
    assert!(
        stderr.contains(
            "index footprint content archive volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert_worker_admitted(&stderr, "index footprint records", &records);
    assert_worker_admitted(&stderr, "index footprint content manifest", &manifest);
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=index footprint content archive\tpath={}",
            content.display()
        )),
        "{stderr}"
    );
    assert!(
        !stderr.contains("invalid magic") && !stderr.contains("not mmap content"),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
    fs::remove_file(records).unwrap();
}

#[test]
fn index_content_segment_refuses_unreachable_output_before_scanning_from_binary() {
    let root = unique_temp_dir("gfm-cli-segment-source");
    let output_root = unique_temp_dir("gfm-cli-segment-output-unreachable");
    fs::write(root.join("segment.md"), "the body contains nosegmentwrite").unwrap();
    fs::write(
        output_root.join(".gfm-volume-kind"),
        "network-unreachable\n",
    )
    .unwrap();
    let segment = output_root.join(format!(
        "{}.gfmseg",
        "content-segment-output-unavailable".repeat(8)
    ));

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-content-segment",
            root.to_str().unwrap(),
            segment.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("hit\t"), "{stdout}");
    assert!(
        stderr.contains("content segment index volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("content write path metadata unavailable"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=content segment index\tpath={}",
            root.display()
        )),
        "{stderr}"
    );
    assert!(!stderr.contains("content-segmented"), "{stderr}");
    assert!(!segment.exists());

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(output_root).unwrap();
}

#[test]
fn compact_content_refuses_unreachable_output_before_merge_from_binary() {
    let source_root = unique_temp_dir("gfm-cli-segment-compact-source");
    let output_root = unique_temp_dir("gfm-cli-segment-compact-output-unreachable");
    let segment = unique_temp_path("gfm-cli-segment-compact-source", "gfmseg");
    let content = output_root.join(format!(
        "{}.gfmcontent",
        "content-compaction-output-unavailable".repeat(8)
    ));
    fs::write(source_root.join("segment.md"), "offline segment marker").unwrap();

    let segment_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-content-segment",
            source_root.to_str().unwrap(),
            segment.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        segment_output.status.success(),
        "{}",
        String::from_utf8_lossy(&segment_output.stderr)
    );

    fs::write(
        output_root.join(".gfm-volume-kind"),
        "network-unreachable\n",
    )
    .unwrap();
    let compact_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "compact-content",
            content.to_str().unwrap(),
            segment.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!compact_output.status.success());
    let stderr = String::from_utf8_lossy(&compact_output.stderr);
    assert!(
        stderr.contains("content compaction volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("content write path metadata unavailable"),
        "{stderr}"
    );
    assert!(!content.exists());

    fs::remove_dir_all(source_root).unwrap();
    fs::remove_dir_all(output_root).unwrap();
    fs::remove_file(segment).unwrap();
}

#[test]
fn content_maintenance_refuses_unreachable_output_before_merge_from_binary() {
    let source_root = unique_temp_dir("gfm-cli-segment-maintenance-source");
    let output_root = unique_temp_dir("gfm-cli-segment-maintenance-output-unreachable");
    let segment = unique_temp_path("gfm-cli-segment-maintenance-source", "gfmseg");
    let content = unique_temp_path("gfm-cli-segment-maintenance-source", "gfmcontent");
    let manifest = unique_temp_path("gfm-cli-segment-maintenance-source", "gfmmanifest");
    let maintained_content = output_root.join("maintained.gfmcontent");
    let deferred_content = output_root.join("deferred.gfmcontent");
    fs::write(source_root.join("segment.md"), "offline maintenance marker").unwrap();

    let segment_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-content-segment",
            source_root.to_str().unwrap(),
            segment.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        segment_output.status.success(),
        "{}",
        String::from_utf8_lossy(&segment_output.stderr)
    );
    let compact_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "compact-content",
            content.to_str().unwrap(),
            segment.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        compact_output.status.success(),
        "{}",
        String::from_utf8_lossy(&compact_output.stderr)
    );
    let manifest_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-manifest-write",
            manifest.to_str().unwrap(),
            &format!("hot:{}", content.display()),
        ])
        .output()
        .unwrap();
    assert!(
        manifest_output.status.success(),
        "{}",
        String::from_utf8_lossy(&manifest_output.stderr)
    );

    fs::write(
        output_root.join(".gfm-volume-kind"),
        "network-unreachable\n",
    )
    .unwrap();
    let maintenance_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-maintain-segments",
            manifest.to_str().unwrap(),
            maintained_content.to_str().unwrap(),
            segment.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!maintenance_output.status.success());
    let maintenance_stderr = String::from_utf8_lossy(&maintenance_output.stderr);
    assert!(
        maintenance_stderr
            .contains("content maintenance volume access blocked: unreachable volume network"),
        "{maintenance_stderr}"
    );
    assert!(!maintained_content.exists());

    let adaptive_blocked = output_root.join("adaptive-blocked.gfmcontent");
    let adaptive_blocked_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-maintain-segments-adaptive",
            manifest.to_str().unwrap(),
            adaptive_blocked.to_str().unwrap(),
            "nominal",
            "nominal",
            "ac",
            "idle",
            segment.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!adaptive_blocked_output.status.success());
    let adaptive_blocked_stdout = String::from_utf8_lossy(&adaptive_blocked_output.stdout);
    let adaptive_blocked_stderr = String::from_utf8_lossy(&adaptive_blocked_output.stderr);
    assert!(
        !adaptive_blocked_stdout.contains("published\t"),
        "{adaptive_blocked_stdout}"
    );
    assert!(
        adaptive_blocked_stderr
            .contains("content maintenance volume access blocked: unreachable volume network"),
        "{adaptive_blocked_stderr}"
    );
    assert!(
        !adaptive_blocked_stderr.contains(&format!(
            "security-worker-admission\tworker=content maintenance\tpath={}",
            manifest.display()
        )),
        "{adaptive_blocked_stderr}"
    );
    assert!(!adaptive_blocked.exists());

    let adaptive_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-maintain-segments-adaptive",
            manifest.to_str().unwrap(),
            deferred_content.to_str().unwrap(),
            "saturated",
            "nominal",
            "ac",
            "idle",
            segment.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        adaptive_output.status.success(),
        "{}",
        String::from_utf8_lossy(&adaptive_output.stderr)
    );
    let adaptive_stderr = String::from_utf8_lossy(&adaptive_output.stderr);
    assert!(
        adaptive_stderr.contains("content-maintenance-deferred")
            && adaptive_stderr.contains("action=Defer"),
        "{adaptive_stderr}"
    );
    assert!(
        !adaptive_stderr.contains("volume access blocked: unreachable volume network"),
        "{adaptive_stderr}"
    );
    assert!(!deferred_content.exists());

    fs::remove_dir_all(source_root).unwrap();
    fs::remove_dir_all(output_root).unwrap();
    fs::remove_file(segment).unwrap();
    fs::remove_file(content).unwrap();
    fs::remove_file(manifest).unwrap();
}

#[test]
fn deferred_content_maintenance_does_not_touch_unreachable_inputs_from_binary() {
    let root = unique_temp_dir("gfm-cli-segment-maintenance-deferred-inputs");
    let output = unique_temp_path("gfm-cli-segment-maintenance-deferred-output", "gfmcontent");
    let manifest = root.join("manifest.gfmmanifest");
    let segment = root.join("content.gfmseg");
    fs::write(&manifest, "not-a-content-manifest").unwrap();
    fs::write(&segment, "not-a-content-segment").unwrap();
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();

    let output_result = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-maintain-segments-adaptive",
            manifest.to_str().unwrap(),
            output.to_str().unwrap(),
            "saturated",
            "critical",
            "low",
            "active",
            segment.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output_result.status.success(),
        "{}",
        String::from_utf8_lossy(&output_result.stderr)
    );
    let stdout = String::from_utf8_lossy(&output_result.stdout);
    let stderr = String::from_utf8_lossy(&output_result.stderr);
    assert!(!stdout.contains("published\t"), "{stdout}");
    assert!(
        stderr.contains("content-maintenance-deferred") && stderr.contains("action=Defer"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert_eq!(
        fs::read_to_string(&manifest).unwrap(),
        "not-a-content-manifest"
    );
    assert_eq!(
        fs::read_to_string(&segment).unwrap(),
        "not-a-content-segment"
    );
    assert!(!output.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn runs_background_content_indexer_from_binary() {
    let root = unique_temp_dir("gfm-cli-background-content-root");
    let segments = unique_temp_dir("gfm-cli-background-content-segments");
    let records = unique_temp_path("gfm-cli-background-records", "gfmidx");
    let content = unique_temp_path("gfm-cli-background-content", "gfmcontent");
    let journal = unique_temp_path("gfm-cli-background-jobs", "journal");
    let spec = unique_temp_path("gfm-cli-background-content", "job");
    let catalog = unique_temp_path("gfm-cli-background-content", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-background-content", "gfmprogress");
    fs::write(root.join("worker.md"), "the body contains workermarker").unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_CONTENT_JOB", &spec)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "index-content-background",
            root.to_str().unwrap(),
            segments.to_str().unwrap(),
            records.to_str().unwrap(),
            content.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-index",
            records.to_str().unwrap(),
            content.to_str().unwrap(),
            "workermarker",
        ])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );

    let stdout = String::from_utf8(search_output.stdout).unwrap();
    assert!(stdout.contains("worker.md"), "{stdout}");
    assert!(segments.join("content-00000000.gfmseg").exists());
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(journal_text.contains("started"));
    assert!(journal_text.contains("completed"));
    assert!(fs::read_to_string(&spec)
        .unwrap()
        .contains("gfm-content-job-v1"));
    assert!(fs::read_to_string(&spec).unwrap().contains("volume_id\t"));
    let catalog_text = fs::read_to_string(&catalog).unwrap();
    assert!(catalog_text.contains("\tindexing\t"), "{catalog_text}");
    assert!(
        catalog_text.contains("background content index"),
        "{catalog_text}"
    );
    assert!(
        catalog_text.contains(spec.to_str().unwrap()),
        "{catalog_text}"
    );
    assert!(
        !catalog_text.contains("runtime/indexing/background-content-index.gfmjob"),
        "{catalog_text}"
    );
    let progress_text = fs::read_to_string(&progress).unwrap();
    assert!(
        progress_text.contains("progress\t1\tbackground\tbackground\tbackground content index"),
        "{progress_text}"
    );
    assert!(
        progress_text.contains("\tcompleted\t2\t2\tcompleted\t"),
        "{progress_text}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(segments).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(content).unwrap();
    fs::remove_file(journal).unwrap();
    fs::remove_file(spec).unwrap();
    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
}

#[test]
fn background_content_indexer_refuses_unreachable_outputs_before_job_state_from_binary() {
    let root = unique_temp_dir("gfm-cli-background-content-access-root");
    let output_root = unique_temp_dir("gfm-cli-background-content-access-output");
    let segments = output_root.join("segments");
    let records = output_root.join("records.gfmidx");
    let content = output_root.join("content.gfmcontent");
    let journal = unique_temp_path("gfm-cli-background-content-access", "journal");
    let spec = unique_temp_path("gfm-cli-background-content-access", "job");
    let catalog = unique_temp_path("gfm-cli-background-content-access", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-background-content-access", "gfmprogress");
    fs::write(root.join("worker.md"), "blocked worker marker").unwrap();
    fs::write(
        output_root.join(".gfm-volume-kind"),
        "network-unreachable\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_CONTENT_JOB", &spec)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "index-content-background",
            root.to_str().unwrap(),
            segments.to_str().unwrap(),
            records.to_str().unwrap(),
            content.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr
            .contains("background content index volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=background content index\tpath={}",
            root.display()
        )),
        "{stderr}"
    );
    assert!(!segments.exists());
    assert!(!records.exists());
    assert!(!content.exists());
    assert!(!journal.exists());
    assert!(!spec.exists());
    assert!(!catalog.exists());
    assert!(!progress.exists());

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(output_root).unwrap();
}

#[test]
fn background_content_indexer_reports_output_probe_failure_before_job_state_from_binary() {
    let root = unique_temp_dir("gfm-cli-background-content-output-probe-root");
    let segments = unique_temp_path(
        "gfm-cli-background-content-output-probe-segments",
        "segments",
    );
    let records = unique_temp_path("gfm-cli-background-content-output-probe-records", "gfmidx");
    let content = root.join(format!("{}.gfmcontent", "content-unavailable".repeat(16)));
    let journal = unique_temp_path("gfm-cli-background-content-output-probe", "journal");
    let spec = unique_temp_path("gfm-cli-background-content-output-probe", "job");
    let catalog = unique_temp_path("gfm-cli-background-content-output-probe", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-background-content-output-probe", "gfmprogress");
    fs::write(root.join("worker.md"), "blocked worker marker").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_CONTENT_JOB", &spec)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "index-content-background",
            root.to_str().unwrap(),
            segments.to_str().unwrap(),
            records.to_str().unwrap(),
            content.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("content write"), "{stderr}");
    assert!(stderr.contains("content-unavailable"), "{stderr}");
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=background content index\tpath={}",
            root.display()
        )),
        "{stderr}"
    );
    assert!(!segments.exists());
    assert!(!records.exists());
    assert!(!journal.exists());
    assert!(!spec.exists());
    assert!(!catalog.exists());
    assert!(!progress.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn deferred_background_content_indexer_does_not_touch_unreachable_outputs_from_binary() {
    let root = unique_temp_dir("gfm-cli-background-content-deferred-output-root");
    let output_root = unique_temp_dir("gfm-cli-background-content-deferred-output-unreachable");
    let segments = output_root.join("segments");
    let records = output_root.join("records.gfmidx");
    let content = output_root.join("content.gfmcontent");
    let journal = unique_temp_path("gfm-cli-background-content-deferred-output", "journal");
    let spec = unique_temp_path("gfm-cli-background-content-deferred-output", "job");
    let catalog = unique_temp_path("gfm-cli-background-content-deferred-output", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-background-content-deferred-output", "gfmprogress");
    fs::write(root.join("worker.md"), "deferred blocked worker marker").unwrap();
    fs::write(
        output_root.join(".gfm-volume-kind"),
        "network-unreachable\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_CONTENT_JOB", &spec)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "index-content-background",
            root.to_str().unwrap(),
            segments.to_str().unwrap(),
            records.to_str().unwrap(),
            content.to_str().unwrap(),
            "saturated",
            "nominal",
            "ac",
            "idle",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("background-content-deferred action=Defer"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(!segments.exists());
    assert!(!records.exists());
    assert!(!content.exists());
    assert!(!journal.exists());
    assert!(fs::read_to_string(&spec)
        .unwrap()
        .contains("gfm-content-job-v1"));
    let progress_text = fs::read_to_string(&progress).unwrap();
    assert!(
        progress_text.contains("\tpaused\t0\t1\tdeferred:Defer\t"),
        "{progress_text}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(output_root).unwrap();
    fs::remove_file(spec).unwrap();
    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
}

#[test]
fn deferred_background_content_indexer_does_not_touch_unreachable_root_from_binary() {
    let root = unique_temp_dir("gfm-cli-background-content-deferred-root-unreachable");
    let segments = unique_temp_dir("gfm-cli-background-content-deferred-root-segments");
    let records = unique_temp_path("gfm-cli-background-content-deferred-root-records", "gfmidx");
    let content = unique_temp_path(
        "gfm-cli-background-content-deferred-root-content",
        "gfmcontent",
    );
    let journal = unique_temp_path("gfm-cli-background-content-deferred-root", "journal");
    let spec = unique_temp_path("gfm-cli-background-content-deferred-root", "job");
    let catalog = unique_temp_path("gfm-cli-background-content-deferred-root", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-background-content-deferred-root", "gfmprogress");
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(root.join("worker.md"), "deferred blocked worker marker").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_CONTENT_JOB", &spec)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "index-content-background",
            root.to_str().unwrap(),
            segments.to_str().unwrap(),
            records.to_str().unwrap(),
            content.to_str().unwrap(),
            "saturated",
            "nominal",
            "ac",
            "idle",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("background-content-deferred action=Defer"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(fs::read_dir(&segments).unwrap().next().is_none());
    assert!(!records.exists());
    assert!(!content.exists());
    assert!(!journal.exists());
    assert!(fs::read_to_string(&spec)
        .unwrap()
        .contains("gfm-content-job-v1"));
    let progress_text = fs::read_to_string(&progress).unwrap();
    assert!(
        progress_text.contains("\tpaused\t0\t1\tdeferred:Defer\t"),
        "{progress_text}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(segments).unwrap();
    fs::remove_file(spec).unwrap();
    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
}

#[test]
fn background_content_indexer_refuses_unreachable_journal_before_job_state_from_binary() {
    let root = unique_temp_dir("gfm-cli-background-content-journal-root");
    let segments = unique_temp_dir("gfm-cli-background-content-journal-segments");
    let records = unique_temp_path("gfm-cli-background-content-journal-records", "gfmidx");
    let content = unique_temp_path("gfm-cli-background-content-journal-content", "gfmcontent");
    let offline = unique_temp_dir("gfm-cli-background-content-journal-unreachable");
    let journal = offline.join("jobs.journal");
    let spec = unique_temp_path("gfm-cli-background-content-journal", "job");
    let catalog = unique_temp_path("gfm-cli-background-content-journal", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-background-content-journal", "gfmprogress");
    fs::write(root.join("worker.md"), "blocked journal worker marker").unwrap();
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_CONTENT_JOB", &spec)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "index-content-background",
            root.to_str().unwrap(),
            segments.to_str().unwrap(),
            records.to_str().unwrap(),
            content.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr
            .contains("background content index volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(fs::read_dir(&segments).unwrap().next().is_none());
    assert!(!records.exists());
    assert!(!content.exists());
    assert!(!journal.exists());
    assert!(!spec.exists());
    assert!(!catalog.exists());
    assert!(!progress.exists());

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(segments).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn background_content_indexer_incrementally_updates_archive_from_binary() {
    let root = unique_temp_dir("gfm-cli-background-content-incremental-root");
    let segments = unique_temp_dir("gfm-cli-background-content-incremental-segments");
    let records = unique_temp_path("gfm-cli-background-incremental-records", "gfmidx");
    let content = unique_temp_path("gfm-cli-background-incremental-content", "gfmcontent");
    let first_journal = unique_temp_path("gfm-cli-background-incremental-first", "journal");
    let second_journal = unique_temp_path("gfm-cli-background-incremental-second", "journal");
    fs::write(root.join("keep.md"), "stable binarykeeptoken").unwrap();
    fs::write(root.join("change.md"), "binaryoldtoken before change").unwrap();
    fs::write(root.join("delete.md"), "binarydeletetoken before removal").unwrap();

    let first = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &first_journal)
        .args([
            "index-content-background",
            root.to_str().unwrap(),
            segments.to_str().unwrap(),
            records.to_str().unwrap(),
            content.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );

    fs::write(
        root.join("change.md"),
        "binarychangedtoken after mutation with a longer body",
    )
    .unwrap();
    fs::remove_file(root.join("delete.md")).unwrap();
    fs::write(root.join("add.md"), "binaryaddedtoken newly indexed").unwrap();

    let second = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &second_journal)
        .args([
            "index-content-background",
            root.to_str().unwrap(),
            segments.to_str().unwrap(),
            records.to_str().unwrap(),
            content.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let second_stderr = String::from_utf8(second.stderr).unwrap();
    assert!(
        second_stderr.contains("background-content-indexed 2 files"),
        "{second_stderr}"
    );
    assert!(second_stderr.contains("unchanged 1"), "{second_stderr}");
    assert!(second_stderr.contains("tombstoned 2"), "{second_stderr}");

    for (query, expected) in [
        ("binarykeeptoken", true),
        ("binarychangedtoken", true),
        ("binaryaddedtoken", true),
        ("binaryoldtoken", false),
        ("binarydeletetoken", false),
    ] {
        let search = Command::new(env!("CARGO_BIN_EXE_gfm"))
            .args([
                "search-content-index",
                records.to_str().unwrap(),
                content.to_str().unwrap(),
                query,
            ])
            .output()
            .unwrap();
        assert!(
            search.status.success(),
            "{}",
            String::from_utf8_lossy(&search.stderr)
        );
        let stdout = String::from_utf8(search.stdout).unwrap();
        assert_eq!(
            stdout.contains(".md"),
            expected,
            "query {query} expected {expected}, got {stdout}"
        );
    }

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(segments).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(content).unwrap();
    fs::remove_file(first_journal).unwrap();
    fs::remove_file(second_journal).unwrap();
}

#[test]
fn background_content_indexer_persists_extraction_quarantine_from_binary() {
    let root = unique_temp_dir("gfm-cli-background-content-quarantine-root");
    let segments = unique_temp_dir("gfm-cli-background-content-quarantine-segments");
    let records = unique_temp_path("gfm-cli-background-quarantine-records", "gfmidx");
    let content = unique_temp_path("gfm-cli-background-quarantine-content", "gfmcontent");
    let quarantine = unique_temp_path("gfm-cli-background-quarantine", "gfmquarantine");
    let first_journal = unique_temp_path("gfm-cli-background-quarantine-first", "journal");
    let second_journal = unique_temp_path("gfm-cli-background-quarantine-second", "journal");
    fs::write(root.join("corrupt.pdf"), corrupt_pdf()).unwrap();

    for (journal, expected_unchanged) in [
        (&first_journal, "unchanged 0"),
        (&second_journal, "unchanged 1"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
            .env("GFM_JOB_JOURNAL", journal)
            .env("GFM_EXTRACTION_QUARANTINE", &quarantine)
            .args([
                "index-content-background",
                root.to_str().unwrap(),
                segments.to_str().unwrap(),
                records.to_str().unwrap(),
                content.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.contains("background-content-indexed 0 files"),
            "{stderr}"
        );
        assert!(stderr.contains("quarantined 1"), "{stderr}");
        assert!(stderr.contains(expected_unchanged), "{stderr}");
    }

    let quarantine_text = fs::read_to_string(&quarantine).unwrap();
    assert!(quarantine_text.contains("corrupt-pdf"), "{quarantine_text}");
    assert!(quarantine_text.contains("\t2\t"), "{quarantine_text}");

    let search = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-index",
            records.to_str().unwrap(),
            content.to_str().unwrap(),
            "not-valid-zlib",
        ])
        .output()
        .unwrap();
    assert!(
        search.status.success(),
        "{}",
        String::from_utf8_lossy(&search.stderr)
    );
    assert!(String::from_utf8(search.stdout).unwrap().is_empty());

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(segments).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(content).unwrap();
    fs::remove_file(quarantine).unwrap();
    fs::remove_file(first_journal).unwrap();
    fs::remove_file(second_journal).unwrap();
}

#[test]
fn defers_background_content_indexer_under_saturated_io_from_binary() {
    let root = unique_temp_dir("gfm-cli-background-content-defer-root");
    let segments = unique_temp_dir("gfm-cli-background-content-defer-segments");
    let records = unique_temp_path("gfm-cli-background-defer-records", "gfmidx");
    let content = unique_temp_path("gfm-cli-background-defer-content", "gfmcontent");
    let journal = unique_temp_path("gfm-cli-background-defer-jobs", "journal");
    let spec = unique_temp_path("gfm-cli-background-defer-content", "job");
    let catalog = unique_temp_path("gfm-cli-background-defer-content", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-background-defer-content", "gfmprogress");
    fs::write(root.join("worker.md"), "deferred workermarker").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_CONTENT_JOB", &spec)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "index-content-background",
            root.to_str().unwrap(),
            segments.to_str().unwrap(),
            records.to_str().unwrap(),
            content.to_str().unwrap(),
            "saturated",
            "nominal",
            "ac",
            "idle",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("background-content-deferred action=Defer"),
        "{stderr}"
    );
    assert!(!records.exists());
    assert!(!content.exists());
    assert!(fs::read_to_string(&spec)
        .unwrap()
        .contains("gfm-content-job-v1"));
    let catalog_text = fs::read_to_string(&catalog).unwrap();
    assert!(catalog_text.contains("\tindexing\t"), "{catalog_text}");
    assert!(
        catalog_text.contains(spec.to_str().unwrap()),
        "{catalog_text}"
    );
    let progress_text = fs::read_to_string(&progress).unwrap();
    assert!(
        progress_text.contains("progress\t1\tbackground\tbackground\tbackground content index"),
        "{progress_text}"
    );
    assert!(
        progress_text.contains("\tpaused\t0\t1\tdeferred:Defer\t"),
        "{progress_text}"
    );
    assert!(!journal.exists());
    assert!(fs::read_dir(&segments).unwrap().next().is_none());

    let resume_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "resume-content-background",
            spec.to_str().unwrap(),
            journal.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        resume_output.status.success(),
        "{}",
        String::from_utf8_lossy(&resume_output.stderr)
    );
    let resume_stderr = String::from_utf8(resume_output.stderr).unwrap();
    assert!(
        resume_stderr.contains("resumed-background-content-indexed"),
        "{resume_stderr}"
    );

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-index",
            records.to_str().unwrap(),
            content.to_str().unwrap(),
            "workermarker",
        ])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );
    let search_stdout = String::from_utf8(search_output.stdout).unwrap();
    assert!(search_stdout.contains("worker.md"), "{search_stdout}");
    let resumed_progress_text = fs::read_to_string(&progress).unwrap();
    assert!(
        resumed_progress_text.contains("\tcompleted\t2\t2\tcompleted\t"),
        "{resumed_progress_text}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(segments).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(content).unwrap();
    fs::remove_file(journal).unwrap();
    fs::remove_file(spec).unwrap();
    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
}

#[test]
fn throttled_background_content_indexer_applies_pressure_budgets_from_binary() {
    let root = unique_temp_dir("gfm-cli-background-content-budget-root");
    let segments = unique_temp_dir("gfm-cli-background-content-budget-segments");
    let records = unique_temp_path("gfm-cli-background-budget-records", "gfmidx");
    let content = unique_temp_path("gfm-cli-background-budget-content", "gfmcontent");
    let journal = unique_temp_path("gfm-cli-background-budget-jobs", "journal");
    let spec = unique_temp_path("gfm-cli-background-budget-content", "job");
    let mut body = "x".repeat(1024 * 1024 + 1);
    body.push_str(" pressurebudgetmarker");
    fs::write(root.join("large.md"), body).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_CONTENT_JOB", &spec)
        .args([
            "index-content-background",
            root.to_str().unwrap(),
            segments.to_str().unwrap(),
            records.to_str().unwrap(),
            content.to_str().unwrap(),
            "elevated",
            "serious",
            "low",
            "active",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("action=Throttle"), "{stderr}");
    assert!(
        stderr.contains("background-content-indexed 0 files"),
        "{stderr}"
    );
    assert!(stderr.contains("terms 0"), "{stderr}");

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-index",
            records.to_str().unwrap(),
            content.to_str().unwrap(),
            "pressurebudgetmarker",
        ])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );
    let stdout = String::from_utf8(search_output.stdout).unwrap();
    assert!(!stdout.contains("large.md"), "{stdout}");

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(segments).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(content).unwrap();
    fs::remove_file(journal).unwrap();
    fs::remove_file(spec).unwrap();
}

#[test]
fn resumes_content_index_job_from_binary() {
    let root = unique_temp_dir("gfm-cli-resume-content-root");
    let segments = unique_temp_dir("gfm-cli-resume-content-segments");
    let records = unique_temp_path("gfm-cli-resume-records", "gfmidx");
    let content = unique_temp_path("gfm-cli-resume-content", "gfmcontent");
    let journal = unique_temp_path("gfm-cli-resume-jobs", "journal");
    let spec = unique_temp_path("gfm-cli-resume-content", "job");
    let catalog = unique_temp_path("gfm-cli-resume-content", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-resume-content", "gfmprogress");
    fs::write(root.join("resume.md"), "the body contains resumemarker").unwrap();
    fs::write(
        &spec,
        format!(
            "gfm-content-job-v1\nroot\t{}\nsegment_dir\t{}\nrecords_path\t{}\ncontent_path\t{}\nbatch_size\t1024\n",
            root.display(),
            segments.display(),
            records.display(),
            content.display()
        ),
    )
    .unwrap();
    fs::write(&journal, "99\t1\tstarted\tbackground content index\n").unwrap();

    let resume_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "resume-content-background",
            spec.to_str().unwrap(),
            journal.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        resume_output.status.success(),
        "{}",
        String::from_utf8_lossy(&resume_output.stderr)
    );
    let resume_stderr = String::from_utf8(resume_output.stderr).unwrap();
    assert_worker_admitted(
        &resume_stderr,
        "background content recovery journal",
        journal.parent().unwrap(),
    );
    assert_worker_admitted(
        &resume_stderr,
        "background content recovery progress",
        progress.parent().unwrap(),
    );
    assert_worker_admitted(&resume_stderr, "resume background content index", &spec);
    assert_worker_admitted(&resume_stderr, "background content index", &root);

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-index",
            records.to_str().unwrap(),
            content.to_str().unwrap(),
            "resumemarker",
        ])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );

    let stdout = String::from_utf8(search_output.stdout).unwrap();
    assert!(stdout.contains("resume.md"), "{stdout}");
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(journal_text.contains("99\t1\tstarted"));
    assert!(journal_text.contains("completed"));
    let progress_text = fs::read_to_string(&progress).unwrap();
    assert!(
        progress_text.contains("background content index"),
        "{progress_text}"
    );
    assert!(
        !progress_text.contains("resume background content recovery"),
        "{progress_text}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(segments).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(content).unwrap();
    fs::remove_file(journal).unwrap();
    fs::remove_file(spec).unwrap();
    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
}

#[test]
fn resume_content_index_job_reports_retry_recovery_metadata_from_binary() {
    let root = unique_temp_dir("gfm-cli-resume-content-retry-root");
    let segments = unique_temp_dir("gfm-cli-resume-content-retry-segments");
    let records = unique_temp_path("gfm-cli-resume-retry-records", "gfmidx");
    let content = unique_temp_path("gfm-cli-resume-retry-content", "gfmcontent");
    let journal = unique_temp_path("gfm-cli-resume-retry-jobs", "journal");
    let spec = unique_temp_path("gfm-cli-resume-retry-content", "job");
    let catalog = unique_temp_path("gfm-cli-resume-retry-content", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-resume-retry-content", "gfmprogress");
    fs::write(
        root.join("resume-retry.md"),
        "the body contains resumetrymarker",
    )
    .unwrap();
    fs::write(
        &spec,
        format!(
            "gfm-content-job-v1\nroot\t{}\nsegment_dir\t{}\nrecords_path\t{}\ncontent_path\t{}\nbatch_size\t1024\n",
            root.display(),
            segments.display(),
            records.display(),
            content.display()
        ),
    )
    .unwrap();
    fs::write(
        &journal,
        "99\t1\tstarted\tbackground content index\n99\t1\tfailed:temporary runtime busy\tbackground content index\n100\t1\tstarted\tbackground content index\n100\t1\tfailed:volume is offline and not mounted\tbackground content index\n",
    )
    .unwrap();

    let resume_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "resume-content-background",
            spec.to_str().unwrap(),
            journal.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        resume_output.status.success(),
        "{}",
        String::from_utf8_lossy(&resume_output.stderr)
    );
    let resume_stderr = String::from_utf8(resume_output.stderr).unwrap();
    assert!(
        resume_stderr.contains("resumed-background-content-indexed"),
        "{resume_stderr}"
    );
    assert!(
        resume_stderr.contains("recoverable 2; recovery-interrupted 0; recovery-retryable 2"),
        "{resume_stderr}"
    );
    assert!(
        resume_stderr.contains("recovery-classes offline-volume:1,transient:1; next-delay-ms 250"),
        "{resume_stderr}"
    );

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-index",
            records.to_str().unwrap(),
            content.to_str().unwrap(),
            "resumetrymarker",
        ])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );
    let stdout = String::from_utf8(search_output.stdout).unwrap();
    assert!(stdout.contains("resume-retry.md"), "{stdout}");

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(segments).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(content).unwrap();
    fs::remove_file(journal).unwrap();
    fs::remove_file(spec).unwrap();
    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
}

#[test]
fn resume_content_index_job_refuses_unreachable_outputs_before_worker_from_binary() {
    let root = unique_temp_dir("gfm-cli-resume-content-access-root");
    let output_root = unique_temp_dir("gfm-cli-resume-content-access-output");
    let segments = output_root.join("segments");
    let records = output_root.join("records.gfmidx");
    let content = output_root.join("content.gfmcontent");
    let journal = unique_temp_path("gfm-cli-resume-content-access", "journal");
    let spec = unique_temp_path("gfm-cli-resume-content-access", "job");
    fs::write(root.join("resume.md"), "blocked resume marker").unwrap();
    fs::write(
        &spec,
        format!(
            "gfm-content-job-v1\nroot\t{}\nsegment_dir\t{}\nrecords_path\t{}\ncontent_path\t{}\nbatch_size\t1024\n",
            root.display(),
            segments.display(),
            records.display(),
            content.display()
        ),
    )
    .unwrap();
    fs::write(&journal, "99\t1\tstarted\tbackground content index\n").unwrap();
    fs::write(
        output_root.join(".gfm-volume-kind"),
        "network-unreachable\n",
    )
    .unwrap();

    let resume_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "resume-content-background",
            spec.to_str().unwrap(),
            journal.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!resume_output.status.success());
    let stderr = String::from_utf8_lossy(&resume_output.stderr);
    assert!(
        stderr
            .contains("background content index volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=background content index\tpath={}",
            root.display()
        )),
        "{stderr}"
    );
    assert!(!segments.exists());
    assert!(!records.exists());
    assert!(!content.exists());
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert_eq!(journal_text, "99\t1\tstarted\tbackground content index\n");

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(output_root).unwrap();
    fs::remove_file(journal).unwrap();
    fs::remove_file(spec).unwrap();
}

#[test]
fn resume_content_index_job_refuses_unreachable_journal_before_recovery_read_from_binary() {
    let offline = unique_temp_dir("gfm-cli-resume-content-journal-unreachable");
    let journal = offline.join("jobs.journal");
    let spec = unique_temp_path("gfm-cli-resume-content-journal-unread", "job");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(&journal, "99\t1\tstarted\tbackground content index\n").unwrap();
    fs::write(&spec, "not-a-content-job-spec\n").unwrap();

    let resume_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "resume-content-background",
            spec.to_str().unwrap(),
            journal.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!resume_output.status.success());
    let stderr = String::from_utf8_lossy(&resume_output.stderr);
    assert!(
        stderr.contains(
            "background content recovery journal volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert!(!stderr.contains("content job spec"), "{stderr}");

    fs::remove_dir_all(offline).unwrap();
    fs::remove_file(spec).unwrap();
}

#[test]
fn resume_content_index_job_reports_journal_path_probe_failure_before_recovery_read_from_binary() {
    let root = unique_temp_dir("gfm-cli-resume-content-journal-probe-root");
    let journal = root.join("gfm-recovery-journal".repeat(64));
    let spec = unique_temp_path("gfm-cli-resume-content-journal-probe-unread", "job");
    fs::write(&spec, "not-a-content-job-spec\n").unwrap();

    let resume_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "resume-content-background",
            spec.to_str().unwrap(),
            journal.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!resume_output.status.success());
    let stderr = String::from_utf8_lossy(&resume_output.stderr);
    assert!(
        stderr.contains("background content recovery journal existence unavailable"),
        "{stderr}"
    );
    assert!(!stderr.contains("content job spec"), "{stderr}");

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(spec).unwrap();
}

#[test]
fn resume_content_index_job_refuses_unreachable_progress_store_before_recovery_read_from_binary() {
    let offline = unique_temp_dir("gfm-cli-resume-content-progress-unreachable");
    let progress = offline.join("jobs.gfmprogress");
    let journal = unique_temp_path("gfm-cli-resume-content-progress", "journal");
    let spec = unique_temp_path("gfm-cli-resume-content-progress-unread", "job");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(&journal, "99\t1\tstarted\tbackground content index\n").unwrap();
    fs::write(
        &progress,
        "gfm-job-progress-v1\nprogress\t100\tbackground\tbackground\tbackground content index\t-\tpaused\t0\t1\tdeferred:Defer\t123\n",
    )
    .unwrap();
    fs::write(&spec, "not-a-content-job-spec\n").unwrap();

    let resume_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "resume-content-background",
            spec.to_str().unwrap(),
            journal.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!resume_output.status.success());
    let stderr = String::from_utf8_lossy(&resume_output.stderr);
    assert!(
        stderr.contains(
            "background content recovery progress volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert!(!stderr.contains("content job spec"), "{stderr}");
    assert_eq!(
        fs::read_to_string(&journal).unwrap(),
        "99\t1\tstarted\tbackground content index\n"
    );

    fs::remove_dir_all(offline).unwrap();
    fs::remove_file(journal).unwrap();
    fs::remove_file(spec).unwrap();
}

#[test]
fn resume_content_index_job_reports_progress_path_probe_failure_before_recovery_read_from_binary() {
    let root = unique_temp_dir("gfm-cli-resume-content-progress-probe-root");
    let progress = root.join("gfm-recovery-progress".repeat(64));
    let journal = unique_temp_path("gfm-cli-resume-content-progress-probe", "journal");
    let spec = unique_temp_path("gfm-cli-resume-content-progress-probe-unread", "job");
    fs::write(&journal, "99\t1\tstarted\tbackground content index\n").unwrap();
    fs::write(&spec, "not-a-content-job-spec\n").unwrap();

    let resume_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "resume-content-background",
            spec.to_str().unwrap(),
            journal.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!resume_output.status.success());
    let stderr = String::from_utf8_lossy(&resume_output.stderr);
    assert!(
        stderr.contains("background content recovery progress existence unavailable"),
        "{stderr}"
    );
    assert!(!stderr.contains("content job spec"), "{stderr}");
    assert_eq!(
        fs::read_to_string(&journal).unwrap(),
        "99\t1\tstarted\tbackground content index\n"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(journal).unwrap();
    fs::remove_file(spec).unwrap();
}

#[test]
fn adaptive_resume_content_index_job_applies_pressure_budgets_from_binary() {
    let root = unique_temp_dir("gfm-cli-resume-content-budget-root");
    let segments = unique_temp_dir("gfm-cli-resume-content-budget-segments");
    let records = unique_temp_path("gfm-cli-resume-budget-records", "gfmidx");
    let content = unique_temp_path("gfm-cli-resume-budget-content", "gfmcontent");
    let journal = unique_temp_path("gfm-cli-resume-budget-jobs", "journal");
    let spec = unique_temp_path("gfm-cli-resume-budget-content", "job");
    let mut body = "x".repeat(1024 * 1024 + 1);
    body.push_str(" resumepressurebudgetmarker");
    fs::write(root.join("large-resume.md"), body).unwrap();
    fs::write(
        &spec,
        format!(
            "gfm-content-job-v1\nroot\t{}\nsegment_dir\t{}\nrecords_path\t{}\ncontent_path\t{}\nbatch_size\t1024\n",
            root.display(),
            segments.display(),
            records.display(),
            content.display()
        ),
    )
    .unwrap();
    fs::write(&journal, "99\t1\tstarted\tbackground content index\n").unwrap();

    let resume_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "resume-content-background-adaptive",
            spec.to_str().unwrap(),
            journal.to_str().unwrap(),
            "elevated",
            "serious",
            "low",
            "active",
        ])
        .output()
        .unwrap();
    assert!(
        resume_output.status.success(),
        "{}",
        String::from_utf8_lossy(&resume_output.stderr)
    );
    let stderr = String::from_utf8(resume_output.stderr).unwrap();
    assert!(stderr.contains("action=Throttle"), "{stderr}");
    assert!(
        stderr.contains("resumed-background-content-indexed 0 files"),
        "{stderr}"
    );
    assert!(stderr.contains("terms 0"), "{stderr}");

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-index",
            records.to_str().unwrap(),
            content.to_str().unwrap(),
            "resumepressurebudgetmarker",
        ])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );
    let stdout = String::from_utf8(search_output.stdout).unwrap();
    assert!(!stdout.contains("large-resume.md"), "{stdout}");

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(segments).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(content).unwrap();
    fs::remove_file(journal).unwrap();
    fs::remove_file(spec).unwrap();
}

#[test]
fn deferred_adaptive_resume_content_index_job_skips_unreachable_recovery_paths_from_binary() {
    let offline = unique_temp_dir("gfm-cli-resume-content-adaptive-deferred-unreachable");
    let journal = offline.join("jobs.journal");
    let spec = offline.join("content.job");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(&journal, "99\t1\tstarted\tbackground content index\n").unwrap();
    fs::write(&spec, "not-a-content-job-spec\n").unwrap();

    let resume_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "resume-content-background-adaptive",
            spec.to_str().unwrap(),
            journal.to_str().unwrap(),
            "saturated",
            "nominal",
            "ac",
            "idle",
        ])
        .output()
        .unwrap();

    assert!(
        resume_output.status.success(),
        "{}",
        String::from_utf8_lossy(&resume_output.stderr)
    );
    let stderr = String::from_utf8_lossy(&resume_output.stderr);
    assert!(
        stderr.contains("resumed-background-content-deferred action=Defer; recoverable unknown"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(!stderr.contains("content job spec"), "{stderr}");

    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn reports_recoverable_jobs_from_binary() {
    let journal = unique_temp_path("gfm-cli-recovery-jobs", "journal");
    fs::write(
        &journal,
        "10\t1\tstarted\tinterrupted job\n11\t1\tstarted\tretry job\n11\t1\tfailed:temporary\tretry job\n12\t1\tstarted\tdone job\n12\t1\tcompleted\tdone job\n13\t1\tstarted\toffline job\n13\t1\tfailed:volume is offline and not mounted\toffline job\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["jobs-recover", journal.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(&stderr, "jobs recover", &journal);

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("10\t1\tinterrupted\tclass=-\tnext-delay-ms=0\tinterrupted job"),
        "{stdout}"
    );
    assert!(
        stdout.contains("11\t1\tretryable-failure\tclass=transient\tnext-delay-ms=25\tretry job"),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "13\t1\tretryable-failure\tclass=offline-volume\tnext-delay-ms=250\toffline job"
        ),
        "{stdout}"
    );
    assert!(!stdout.contains("done job"), "{stdout}");

    fs::remove_file(journal).unwrap();
}

#[test]
fn ui_progress_surfaces_refuse_unreachable_store_before_reading_from_binary() {
    let root = unique_temp_dir("gfm-cli-ui-progress-store-unreachable");
    let progress = root.join("jobs.gfmprogress");
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(
        &progress,
        "gfm-job-progress-v1\nprogress\t2\tvisible\tvisible\tcopy file\t-\trunning\t42\t100\tcopying\t123\n",
    )
    .unwrap();

    for mut command in [
        vec![
            "ui-progress-job-contract".to_string(),
            progress.display().to_string(),
            "2".to_string(),
        ],
        vec!["ui-contract".to_string()],
    ] {
        let mut output = Command::new(env!("CARGO_BIN_EXE_gfm"));
        output.env("GFM_JOB_PROGRESS_STORE", &progress);
        output.args(command.drain(..));
        let output = output.output().unwrap();

        assert!(!output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stdout.contains("operation-progress\t"), "{stdout}");
        assert!(
            stderr.contains("ui progress store volume access blocked: unreachable volume network"),
            "{stderr}"
        );
        assert!(
            !stderr.contains(&format!(
                "security-worker-admission\tworker=ui progress store\tpath={}",
                progress.display()
            )),
            "{stderr}"
        );
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reports_retry_backoff_plan_from_binary() {
    let transient = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["jobs-retry-plan", "3", "1", "temporary", "busy"])
        .output()
        .unwrap();
    assert!(
        transient.status.success(),
        "{}",
        String::from_utf8_lossy(&transient.stderr)
    );
    let transient_stdout = String::from_utf8(transient.stdout).unwrap();
    assert!(
        transient_stdout.contains("retry-plan\tclass=transient\tretryable=true\tnext-delay-ms=25"),
        "{transient_stdout}"
    );

    let permission = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["jobs-retry-plan", "3", "1", "permission", "denied"])
        .output()
        .unwrap();
    assert!(
        permission.status.success(),
        "{}",
        String::from_utf8_lossy(&permission.stderr)
    );
    let permission_stdout = String::from_utf8(permission.stdout).unwrap();
    assert!(
        permission_stdout
            .contains("retry-plan\tclass=permission\tretryable=false\tnext-delay-ms=0"),
        "{permission_stdout}"
    );
}

#[test]
fn scheduled_runtime_retry_probe_retries_transient_failure_from_binary() {
    let state = unique_temp_path("gfm-cli-runtime-retry-probe", "state");
    let journal = unique_temp_path("gfm-cli-runtime-retry-probe", "journal");
    let catalog = unique_temp_path("gfm-cli-runtime-retry-probe", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-runtime-retry-probe", "gfmprogress");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args(["jobs-runtime-retry-probe", state.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("runtime-retry-probe\tcompleted\t2\tRun"),
        "{stdout}"
    );
    assert_eq!(fs::read_to_string(&state).unwrap(), "2");

    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(
        journal_text.contains("1\t1\tstarted\truntime retry probe"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t1\tfailed:temporary runtime probe busy\truntime retry probe"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tstarted\truntime retry probe"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tcompleted\truntime retry probe"),
        "{journal_text}"
    );

    let catalog_text = fs::read_to_string(&catalog).unwrap();
    assert!(
        catalog_text.contains("runtime retry probe"),
        "{catalog_text}"
    );
    let progress_text = fs::read_to_string(&progress).unwrap();
    assert!(
        progress_text.contains("progress\t1\tbackground\tbackground\truntime retry probe"),
        "{progress_text}"
    );
    assert!(
        progress_text.contains("\tcompleted\t1\t1\tcompleted\t"),
        "{progress_text}"
    );

    fs::remove_file(state).unwrap();
    fs::remove_file(journal).unwrap();
    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
}

#[test]
fn runtime_retry_probe_persists_cwd_volume_for_relative_state_from_binary() {
    let root = unique_temp_dir("gfm-cli-runtime-retry-relative-volume");
    let journal = root.join("jobs.journal");
    let catalog = root.join("runtime.gfmjobs");
    let progress = root.join("runtime.gfmprogress");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .current_dir(&root)
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args(["jobs-runtime-retry-probe", "retry.state"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_to_string(root.join("retry.state")).unwrap(), "2");
    let volume = test_volume_id(&root);
    let catalog_text = fs::read_to_string(&catalog).unwrap();
    assert!(
        catalog_text
            .lines()
            .any(|line| line.starts_with("payload\t1\t")
                && line.contains("\truntime retry probe\t")
                && line.contains(&format!("\t{volume}\t"))),
        "{catalog_text}"
    );
    let progress_text = fs::read_to_string(&progress).unwrap();
    assert!(
        progress_text
            .lines()
            .any(|line| line.starts_with("progress\t1\t")
                && line.contains("\truntime retry probe\t")
                && line.contains(&format!("\t{volume}\t"))
                && line.contains("\tcompleted\t")),
        "{progress_text}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn scheduled_runtime_refuses_unreachable_job_journal_before_runtime_state_from_binary() {
    let root = unique_temp_dir("gfm-cli-runtime-journal-preflight-root");
    let offline = unique_temp_dir("gfm-cli-runtime-journal-preflight-offline");
    let state = root.join("retry.state");
    let journal = offline.join("jobs.journal");
    let catalog = root.join("runtime.gfmjobs");
    let progress = root.join("runtime.gfmprogress");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args(["jobs-runtime-retry-probe", state.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("runtime-retry-probe\t"), "{stdout}");
    assert!(
        stderr.contains("runtime retry probe volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(!state.exists());
    assert!(!journal.exists());
    assert!(!catalog.exists());
    assert!(!progress.exists());

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn deferred_runtime_retry_probe_does_not_touch_attempt_state_from_binary() {
    let root = unique_temp_dir("gfm-cli-runtime-retry-probe-deferred");
    let attempts = root.join("attempts");
    let state = attempts.join("retry.state");
    let journal = root.join("jobs.journal");
    let catalog = root.join("runtime.gfmjobs");
    let progress = root.join("runtime.gfmprogress");
    fs::create_dir_all(&attempts).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "jobs-runtime-retry-probe",
            state.to_str().unwrap(),
            "saturated",
            "critical",
            "low",
            "active",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("runtime-retry-probe\tdeferred\tDefer"),
        "{stdout}"
    );
    assert!(
        !stderr.contains(&format!("path={}", attempts.display())),
        "{stderr}"
    );
    assert!(!state.exists());
    assert!(attempts.exists());
    assert!(catalog.exists());
    assert!(progress.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reports_job_payload_catalog_from_binary() {
    let catalog = unique_temp_path("gfm-cli-job-payload-catalog", "gfmjobs");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["jobs-payload-catalog", catalog.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(&stderr, "jobs payload catalog", catalog.parent().unwrap());

    let stdout = String::from_utf8(output.stdout).unwrap();
    for kind in [
        "operation",
        "indexing",
        "extraction",
        "thumbnail",
        "preview",
        "repair",
    ] {
        assert!(stdout.contains(&format!("\t{kind}\t")), "{stdout}");
    }
    assert!(catalog.is_file());

    fs::remove_file(catalog).unwrap();
}

#[test]
fn reports_job_fairness_plan_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("jobs-fairness-plan")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("ready\tfirst\t1\tforeground\tinteractive\topen folder"),
        "{stdout}"
    );
    assert!(
        stdout.contains("ready\tfirst\t2\tvisible\tvisible\trender visible rows"),
        "{stdout}"
    );
    assert!(
        stdout.contains("ready\tfirst\t3\tbackground\tbackground\tindex content"),
        "{stdout}"
    );
    assert!(
        stdout.contains("ready\tfirst\t4\tmaintenance\tbackground\tcompact sidecars"),
        "{stdout}"
    );
    assert!(
        stdout.contains("blocked\tfirst\t5\trepair\t4\trepair derived sidecar"),
        "{stdout}"
    );
    assert!(
        stdout.contains("scheduler-ingest\tcompleted=1\tcancelled=0\tfailed=0\tcompleted-ids=4\tcancelled-ids=-\tfailed-ids=-"),
        "{stdout}"
    );
    assert!(
        stdout.contains("ready\tafter-completion\t5\trepair\tvisible\trepair derived sidecar"),
        "{stdout}"
    );
    assert!(
        stdout.contains("blocked\tafter-completion\t6\trepair\t999\trepair missing thumbnail"),
        "{stdout}"
    );
}

#[test]
fn reports_restorable_job_progress_from_binary() {
    let progress = unique_temp_path("gfm-cli-job-progress", "gfmprogress");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["jobs-progress-snapshot", progress.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(
        &stderr,
        "jobs progress snapshot",
        progress.parent().unwrap(),
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("progress\t1\tforeground\tinteractive\tcopy selected files\t1\trunning\t42\t100\tcopy:/source->/target\t1000"), "{stdout}");
    assert!(stdout.contains("progress\t2\tbackground\tbackground\tindex content\t1\tpaused\t128\t250\tpressure:throttled\t1001"), "{stdout}");
    assert!(!stdout.contains("compact content segments"), "{stdout}");
    assert!(progress.is_file());

    fs::remove_file(progress).unwrap();
}

#[test]
fn normalizes_interrupted_job_progress_for_restore_from_binary() {
    let progress = unique_temp_path("gfm-cli-job-progress-restore", "gfmprogress");

    let snapshot = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["jobs-progress-snapshot", progress.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        snapshot.status.success(),
        "{}",
        String::from_utf8_lossy(&snapshot.stderr)
    );

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["jobs-progress-restore", progress.to_str().unwrap(), "2000"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(&stderr, "jobs progress restore", progress.parent().unwrap());

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("progress\t1\tforeground\tinteractive\tcopy selected files\t1\tpaused\t42\t100\tinterrupted:running:copy:/source->/target\t2000"), "{stdout}");
    assert!(stdout.contains("progress\t2\tbackground\tbackground\tindex content\t1\tpaused\t128\t250\tpressure:throttled\t1001"), "{stdout}");
    assert!(!stdout.contains("compact content segments"), "{stdout}");

    let progress_text = fs::read_to_string(&progress).unwrap();
    assert!(
        progress_text
            .contains("\tpaused\t42\t100\tinterrupted:running:copy:/source->/target\t2000"),
        "{progress_text}"
    );
    assert!(
        progress_text.contains("compact content segments"),
        "{progress_text}"
    );

    fs::remove_file(progress).unwrap();
}

#[test]
fn progress_control_pause_resume_and_stop_persist_from_binary() {
    let progress = unique_temp_path("gfm-cli-job-progress-control", "gfmprogress");
    let seed = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["jobs-progress-snapshot", progress.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        seed.status.success(),
        "{}",
        String::from_utf8_lossy(&seed.stderr)
    );

    let pause = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "jobs-progress-control",
            progress.to_str().unwrap(),
            "1",
            "pause",
            "3000",
        ])
        .output()
        .unwrap();
    assert!(
        pause.status.success(),
        "{}",
        String::from_utf8_lossy(&pause.stderr)
    );
    let pause_stderr = String::from_utf8_lossy(&pause.stderr);
    assert_worker_admitted(
        &pause_stderr,
        "jobs progress control",
        progress.parent().unwrap(),
    );
    let pause_stdout = String::from_utf8(pause.stdout).unwrap();
    assert!(
        pause_stdout.contains(
            "progress-control\tpause\tjob=1\tstate=paused\tdetail=paused-by-user:copy:/source->/target"
        ),
        "{pause_stdout}"
    );

    let resume = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "jobs-progress-control",
            progress.to_str().unwrap(),
            "1",
            "resume",
            "3001",
        ])
        .output()
        .unwrap();
    assert!(
        resume.status.success(),
        "{}",
        String::from_utf8_lossy(&resume.stderr)
    );
    let resume_stdout = String::from_utf8(resume.stdout).unwrap();
    assert!(
        resume_stdout.contains("progress-control\tresume\tjob=1\tstate=running\t"),
        "{resume_stdout}"
    );

    let stop = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "jobs-progress-control",
            progress.to_str().unwrap(),
            "1",
            "stop",
            "3002",
        ])
        .output()
        .unwrap();
    assert!(
        stop.status.success(),
        "{}",
        String::from_utf8_lossy(&stop.stderr)
    );
    let stop_stdout = String::from_utf8(stop.stdout).unwrap();
    assert!(
        stop_stdout.contains("progress-control\tstop\tjob=1\tstate=cancelled\t"),
        "{stop_stdout}"
    );

    let progress_text = fs::read_to_string(&progress).unwrap();
    assert!(
        progress_text.contains("\tcancelled\t42\t100\tcancelled-by-user:"),
        "{progress_text}"
    );

    fs::remove_file(progress).unwrap();
}

#[test]
fn reports_payload_restore_plan_from_existing_stores() {
    let catalog = unique_temp_path("gfm-cli-job-payload-restore-plan", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-job-payload-restore-plan", "gfmprogress");

    let payload_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["jobs-payload-catalog", catalog.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        payload_output.status.success(),
        "{}",
        String::from_utf8_lossy(&payload_output.stderr)
    );
    let progress_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["jobs-progress-snapshot", progress.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        progress_output.status.success(),
        "{}",
        String::from_utf8_lossy(&progress_output.stderr)
    );

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "jobs-payload-restore-plan",
            catalog.to_str().unwrap(),
            progress.to_str().unwrap(),
            "2000",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(
        &stderr,
        "jobs payload restore plan",
        progress.parent().unwrap(),
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains(
            "restore\tpaused\tpayload\t1\toperation\tcopy operation\toperations/copy.gfmjob\t1\tcopy:/source->/target"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "restore\tpaused\tpayload\t2\tindexing\tcontent indexing\tindex/content.gfmjob\t1\tindex:/workspace"
        ),
        "{stdout}"
    );
    assert!(!stdout.contains("payload\t3\textraction"), "{stdout}");
    assert!(!stdout.contains("missing-payload"), "{stdout}");

    let progress_text = fs::read_to_string(&progress).unwrap();
    assert!(
        progress_text
            .contains("\tpaused\t42\t100\tinterrupted:running:copy:/source->/target\t2000"),
        "{progress_text}"
    );

    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
}

#[test]
fn jobs_file_store_routes_refuse_unreachable_volume_before_persisting_from_binary() {
    let root = unique_temp_dir("gfm-cli-jobs-store-unreachable");
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let catalog = root.join(format!("{}.gfmjobs", "jobs-catalog-unavailable".repeat(8)));
    let progress = root.join(format!(
        "{}.gfmprogress",
        "jobs-progress-unavailable".repeat(8)
    ));
    let retry_state = root.join(format!("{}.state", "jobs-retry-unavailable".repeat(8)));

    for args in [
        vec![
            "jobs-payload-catalog".to_string(),
            catalog.display().to_string(),
        ],
        vec![
            "jobs-progress-snapshot".to_string(),
            progress.display().to_string(),
        ],
        vec![
            "jobs-runtime-retry-probe".to_string(),
            retry_state.display().to_string(),
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
            .args(&args)
            .output()
            .unwrap();

        assert!(!output.status.success(), "{args:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stdout.is_empty(), "{args:?}: {stdout}");
        assert!(
            stderr.contains("volume access blocked: unreachable volume network"),
            "{args:?}: {stderr}"
        );
        assert!(
            !stderr.contains("jobs write path metadata unavailable"),
            "{args:?}: {stderr}"
        );
    }

    assert!(!catalog.exists());
    assert!(!progress.exists());
    assert!(!retry_state.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn jobs_file_store_routes_report_path_probe_failure_before_worker_admission_from_binary() {
    let root = unique_temp_dir("gfm-cli-jobs-store-probe-root");
    let bad_catalog = root.join(format!("{}.gfmjobs", "jobs-unavailable".repeat(16)));
    let bad_progress = root.join(format!("{}.gfmprogress", "progress-unavailable".repeat(16)));
    let retry_state = root.join(format!("{}.state", "retry-unavailable".repeat(16)));

    for (args, token) in [
        (
            vec![
                "jobs-payload-catalog".to_string(),
                bad_catalog.display().to_string(),
            ],
            "jobs-unavailable",
        ),
        (
            vec![
                "jobs-progress-snapshot".to_string(),
                bad_progress.display().to_string(),
            ],
            "progress-unavailable",
        ),
        (
            vec![
                "jobs-runtime-retry-probe".to_string(),
                retry_state.display().to_string(),
            ],
            "retry-unavailable",
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
            .args(&args)
            .output()
            .unwrap();

        assert!(!output.status.success(), "{args:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stdout.is_empty(), "{args:?}: {stdout}");
        assert!(
            stderr.contains("jobs write path metadata unavailable"),
            "{args:?}: {stderr}"
        );
        assert!(stderr.contains(token), "{args:?}: {stderr}");
        assert!(
            !stderr.contains("security-worker-admission\tworker="),
            "{args:?}: {stderr}"
        );
    }

    assert!(!bad_catalog.exists());
    assert!(!bad_progress.exists());
    assert!(!retry_state.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn scheduled_runtime_reports_journal_probe_failure_before_attempt_state_from_binary() {
    let root = unique_temp_dir("gfm-cli-runtime-journal-probe-root");
    let state = root.join("retry.state");
    let journal = root.join(format!("{}.journal", "journal-unavailable".repeat(16)));
    let catalog = root.join("runtime.gfmjobs");
    let progress = root.join("runtime.gfmprogress");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args(["jobs-runtime-retry-probe", state.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains("runtime write path metadata unavailable"),
        "{stderr}"
    );
    assert!(stderr.contains("journal-unavailable"), "{stderr}");
    assert!(!state.exists());
    assert!(!journal.exists());
    assert!(!catalog.exists());
    assert!(!progress.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn jobs_restore_routes_refuse_unreachable_stores_before_mutating_from_binary() {
    let root = unique_temp_dir("gfm-cli-jobs-restore-unreachable");
    let source = unique_temp_dir("gfm-cli-jobs-restore-source");
    let catalog = source.join("jobs.gfmjobs");
    let progress = root.join("jobs.gfmprogress");
    let offline_catalog = root.join("jobs.gfmjobs");
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();

    let catalog_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["jobs-payload-catalog", catalog.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        catalog_output.status.success(),
        "{}",
        String::from_utf8_lossy(&catalog_output.stderr)
    );

    let progress_restore = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["jobs-progress-restore", progress.to_str().unwrap(), "2000"])
        .output()
        .unwrap();
    assert!(!progress_restore.status.success());
    let progress_restore_stderr = String::from_utf8_lossy(&progress_restore.stderr);
    assert!(
        progress_restore_stderr
            .contains("jobs progress restore volume access blocked: unreachable volume network"),
        "{progress_restore_stderr}"
    );
    assert!(
        !progress_restore_stderr.contains("jobs write path metadata unavailable"),
        "{progress_restore_stderr}"
    );
    assert!(!progress.exists());

    let progress_control = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "jobs-progress-control",
            progress.to_str().unwrap(),
            "1",
            "pause",
            "2000",
        ])
        .output()
        .unwrap();
    assert!(!progress_control.status.success());
    let progress_control_stderr = String::from_utf8_lossy(&progress_control.stderr);
    assert!(
        progress_control_stderr
            .contains("jobs progress control volume access blocked: unreachable volume network"),
        "{progress_control_stderr}"
    );
    assert!(
        !progress_control_stderr.contains("jobs write path metadata unavailable"),
        "{progress_control_stderr}"
    );
    assert!(!progress.exists());

    let payload_restore = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "jobs-payload-restore-plan",
            catalog.to_str().unwrap(),
            progress.to_str().unwrap(),
            "2000",
        ])
        .output()
        .unwrap();
    assert!(!payload_restore.status.success());
    let payload_restore_stderr = String::from_utf8_lossy(&payload_restore.stderr);
    assert!(
        payload_restore_stderr.contains(
            "jobs payload restore plan volume access blocked: unreachable volume network"
        ),
        "{payload_restore_stderr}"
    );
    assert!(
        !payload_restore_stderr.contains("jobs write path metadata unavailable"),
        "{payload_restore_stderr}"
    );
    assert!(!progress.exists());

    let recover = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["jobs-recover", offline_catalog.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!recover.status.success());
    let recover_stderr = String::from_utf8_lossy(&recover.stderr);
    assert!(
        recover_stderr.contains("jobs recover volume access blocked: unreachable volume network"),
        "{recover_stderr}"
    );

    fs::remove_file(catalog).unwrap();
    fs::remove_dir_all(source).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn volume_producers_persist_runtime_payload_and_progress_from_binary() {
    let root = unique_temp_dir("gfm-cli-runtime-producer-root");
    let image = root.join("Image.png");
    fs::write(&image, b"\x89PNG\r\n\x1a\nruntime metadata").unwrap();
    let catalog = unique_temp_path("gfm-cli-runtime-producer", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-runtime-producer", "gfmprogress");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args(["thumbnail-generation", image.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("\tintent=preview\t") && stderr.contains(&image.display().to_string()),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "security-worker-admission\tworker=thumbnail generation\tpath={}",
            image.display()
        )),
        "{stderr}"
    );

    let catalog_text = fs::read_to_string(&catalog).unwrap();
    assert!(catalog_text.contains("\tthumbnail\t"), "{catalog_text}");
    assert!(
        catalog_text.contains("thumbnail generation"),
        "{catalog_text}"
    );
    assert!(
        catalog_text.contains(&image.display().to_string()),
        "{catalog_text}"
    );

    let progress_text = fs::read_to_string(&progress).unwrap();
    assert!(
        progress_text.contains("progress\t1\tvisible\tvisible\tthumbnail generation"),
        "{progress_text}"
    );
    assert!(
        progress_text.contains("\tcompleted\t1\t1\tcompleted\t"),
        "{progress_text}"
    );

    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn repeated_runtime_producers_replace_stale_payload_for_same_job_id_from_binary() {
    let root = unique_temp_dir("gfm-cli-runtime-producer-replace-root");
    let first = root.join("First.png");
    let second = root.join("Second.png");
    fs::write(&first, b"\x89PNG\r\n\x1a\nruntime first metadata").unwrap();
    fs::write(&second, b"\x89PNG\r\n\x1a\nruntime second metadata").unwrap();
    let catalog = unique_temp_path("gfm-cli-runtime-producer-replace", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-runtime-producer-replace", "gfmprogress");

    for image in [&first, &second] {
        let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
            .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
            .env("GFM_JOB_PROGRESS_STORE", &progress)
            .args(["thumbnail-generation", image.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let catalog_text = fs::read_to_string(&catalog).unwrap();
    assert_eq!(
        catalog_text
            .matches("\npayload\t1\tthumbnail\tthumbnail generation\t")
            .count(),
        1,
        "{catalog_text}"
    );
    assert!(
        catalog_text.contains(&second.display().to_string()),
        "{catalog_text}"
    );
    assert!(
        !catalog_text.contains(&first.display().to_string()),
        "{catalog_text}"
    );

    let progress_text = fs::read_to_string(&progress).unwrap();
    assert_eq!(
        progress_text
            .matches("\nprogress\t1\tvisible\tvisible\tthumbnail generation\t")
            .count(),
        1,
        "{progress_text}"
    );
    assert!(
        progress_text.contains("\tcompleted\t1\t1\tcompleted\t"),
        "{progress_text}"
    );

    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn volume_producers_refuse_unreachable_runtime_stores_before_progress_from_binary() {
    let root = unique_temp_dir("gfm-cli-runtime-producer-store-root");
    let store_root = unique_temp_dir("gfm-cli-runtime-producer-store-unreachable");
    let image = root.join("Image.png");
    let document = root.join("Visible.pdf");
    let catalog = store_root.join("runtime.gfmjobs");
    let progress = store_root.join("runtime.gfmprogress");
    fs::write(&image, b"\x89PNG\r\n\x1a\nruntime blocked").unwrap();
    fs::write(&document, b"%PDF-1.7\nruntime blocked").unwrap();
    fs::write(store_root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();

    for args in [
        vec![
            "thumbnail-generation".to_string(),
            image.display().to_string(),
        ],
        vec![
            "quicklook-session-adaptive".to_string(),
            document.display().to_string(),
            "saturated".to_string(),
            "critical".to_string(),
            "low".to_string(),
            "active".to_string(),
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
            .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
            .env("GFM_JOB_PROGRESS_STORE", &progress)
            .args(&args)
            .output()
            .unwrap();

        assert!(!output.status.success(), "{args:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stdout.contains("thumbnail-generation\t"),
            "{args:?}: {stdout}"
        );
        assert!(
            !stdout.contains("quicklook-session\t"),
            "{args:?}: {stdout}"
        );
        assert!(
            stderr.contains("volume access blocked: unreachable volume network"),
            "{args:?}: {stderr}"
        );
    }

    assert!(!catalog.exists());
    assert!(!progress.exists());

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(store_root).unwrap();
}

#[test]
fn visible_preview_producers_refuse_unreachable_targets_before_runtime_state_from_binary() {
    let root = unique_temp_dir("gfm-cli-runtime-preview-target-root");
    let quicklook = root.join("Preview.pdf");
    let thumbnail = root.join("Preview.png");
    let catalog = unique_temp_path("gfm-cli-runtime-preview-target", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-runtime-preview-target", "gfmprogress");
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(&quicklook, b"%PDF-1.7\nruntime blocked").unwrap();
    fs::write(&thumbnail, b"\x89PNG\r\n\x1a\nruntime blocked").unwrap();

    for args in [
        vec![
            "quicklook-session".to_string(),
            quicklook.display().to_string(),
        ],
        vec![
            "quicklook-session-adaptive".to_string(),
            quicklook.display().to_string(),
            "nominal".to_string(),
            "nominal".to_string(),
            "ac".to_string(),
            "idle".to_string(),
        ],
        vec![
            "thumbnail-generation".to_string(),
            thumbnail.display().to_string(),
        ],
        vec![
            "thumbnail-generation-adaptive".to_string(),
            thumbnail.display().to_string(),
            "nominal".to_string(),
            "nominal".to_string(),
            "ac".to_string(),
            "idle".to_string(),
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
            .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
            .env("GFM_JOB_PROGRESS_STORE", &progress)
            .args(&args)
            .output()
            .unwrap();

        assert!(!output.status.success(), "{args:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stdout.contains("quicklook-session\t"),
            "{args:?}: {stdout}"
        );
        assert!(
            !stdout.contains("thumbnail-generation\t"),
            "{args:?}: {stdout}"
        );
        assert!(
            stderr.contains("volume access blocked: unreachable volume network"),
            "{args:?}: {stderr}"
        );
        assert!(!catalog.exists(), "{args:?}");
        assert!(!progress.exists(), "{args:?}");
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn deferred_adaptive_thumbnail_persists_runtime_payload_and_progress_from_binary() {
    let root = unique_temp_dir("gfm-cli-runtime-deferred-thumbnail-root");
    let image = root.join("Deferred.png");
    fs::write(&image, b"\x89PNG\r\n\x1a\ndeferred thumbnail").unwrap();
    let catalog = unique_temp_path("gfm-cli-runtime-deferred-thumbnail", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-runtime-deferred-thumbnail", "gfmprogress");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "thumbnail-generation-adaptive",
            image.to_str().unwrap(),
            "saturated",
            "nominal",
            "ac",
            "idle",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("\tintent=preview\t"), "{stderr}");
    assert!(!stderr.contains(&image.display().to_string()), "{stderr}");

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("thumbnail-generation\tstatus=deferred\taction=Defer\tdeferred=true"),
        "{stdout}"
    );
    let catalog_text = fs::read_to_string(&catalog).unwrap();
    assert!(catalog_text.contains("\tthumbnail\t"), "{catalog_text}");
    assert!(
        catalog_text.contains("thumbnail generation"),
        "{catalog_text}"
    );
    assert!(
        catalog_text.contains(&image.display().to_string()),
        "{catalog_text}"
    );
    let progress_text = fs::read_to_string(&progress).unwrap();
    assert!(
        progress_text.contains("progress\t1\tbackground\tbackground\tthumbnail generation"),
        "{progress_text}"
    );
    assert!(
        progress_text.contains("\tpaused\t0\t1\tdeferred:Defer\t"),
        "{progress_text}"
    );

    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn deferred_adaptive_thumbnail_does_not_touch_unreachable_target_from_binary() {
    let root = unique_temp_dir("gfm-cli-runtime-deferred-thumbnail-unreachable-root");
    let image = root.join("Deferred.png");
    let catalog = unique_temp_path("gfm-cli-runtime-deferred-thumbnail-unreachable", "gfmjobs");
    let progress = unique_temp_path(
        "gfm-cli-runtime-deferred-thumbnail-unreachable",
        "gfmprogress",
    );
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(&image, b"\x89PNG\r\n\x1a\ndeferred thumbnail").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "thumbnail-generation-adaptive",
            image.to_str().unwrap(),
            "saturated",
            "critical",
            "low",
            "active",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("\tintent=preview\t"), "{stderr}");
    assert!(!stderr.contains(&image.display().to_string()), "{stderr}");
    assert!(
        !stderr.contains("volume access blocked: unreachable volume network"),
        "{stderr}"
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout,
        "thumbnail-generation\tstatus=deferred\taction=Defer\tdeferred=true\n"
    );
    let progress_text = fs::read_to_string(&progress).unwrap();
    assert!(
        progress_text.contains("\tpaused\t0\t1\tdeferred:Defer\t"),
        "{progress_text}"
    );
    let catalog_text = fs::read_to_string(&catalog).unwrap();
    assert!(
        catalog_text.contains(&image.display().to_string()),
        "{catalog_text}"
    );

    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn visible_adaptive_quicklook_persists_runtime_payload_and_progress_under_pressure_from_binary() {
    let root = unique_temp_dir("gfm-cli-runtime-visible-quicklook-root");
    let document = root.join("Visible.pdf");
    fs::write(&document, b"%PDF-1.7\nquicklook visible runtime metadata").unwrap();
    let catalog = unique_temp_path("gfm-cli-runtime-visible-quicklook", "gfmjobs");
    let progress = unique_temp_path("gfm-cli-runtime-visible-quicklook", "gfmprogress");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "quicklook-session-adaptive",
            document.to_str().unwrap(),
            "saturated",
            "critical",
            "low",
            "active",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("security-scope\t"), "{stderr}");
    assert!(stderr.contains("\tintent=preview\t"), "{stderr}");

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("quicklook-session\t"), "{stdout}");
    assert!(stdout.contains("\taction=Run\tdeferred=false"), "{stdout}");
    let catalog_text = fs::read_to_string(&catalog).unwrap();
    assert!(catalog_text.contains("\tpreview\t"), "{catalog_text}");
    assert!(catalog_text.contains("quicklook preview"), "{catalog_text}");
    assert!(
        catalog_text.contains(&document.display().to_string()),
        "{catalog_text}"
    );
    let progress_text = fs::read_to_string(&progress).unwrap();
    assert!(
        progress_text.contains("progress\t1\tvisible\tvisible\tquicklook preview"),
        "{progress_text}"
    );
    assert!(
        progress_text.contains("\tcompleted\t1\t1\tcompleted\t"),
        "{progress_text}"
    );

    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reports_structured_cancellation_tree_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("jobs-cancel-tree")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("after-child-cancel\troot\tcancelled=false"),
        "{stdout}"
    );
    assert!(
        stdout.contains("after-child-cancel\tchild\tcancelled=true"),
        "{stdout}"
    );
    assert!(
        stdout.contains("after-child-cancel\tsibling\tcancelled=false"),
        "{stdout}"
    );
    assert!(
        stdout.contains("after-child-cancel\tgrandchild\tcancelled=true"),
        "{stdout}"
    );
    assert!(
        stdout.contains("after-root-cancel\troot\tcancelled=true"),
        "{stdout}"
    );
    assert!(
        stdout.contains("after-root-cancel\tsibling\tcancelled=true"),
        "{stdout}"
    );
}

#[test]
fn reports_volume_scoped_job_cancellation_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("jobs-cancel-volume")
        .arg("7")
        .arg("background")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.starts_with("volume-job-cancellation\tvolume=7\tclass=background\tcancelled=1\n")
    );
    assert!(stdout.contains("cancelled-job\t1\tbackground\tbackground\tindex detached volume"));
    assert!(!stdout.contains("render visible thumbnails"));
    assert!(!stdout.contains("index mounted volume"));
}

fn run_gfm<const N: usize>(journal: &std::path::Path, args: [&str; N]) {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPS_JOURNAL", journal)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_index_security_preflight(stderr: &[u8]) {
    let stderr = String::from_utf8_lossy(stderr);
    assert!(stderr.contains("security-scope\t"), "{stderr}");
    assert!(stderr.contains("\tintent=index\t"), "{stderr}");
    assert!(stderr.contains("\taction=allow\t"), "{stderr}");
}

fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
    let path = unique_temp_path(prefix, "");
    fs::create_dir_all(&path).unwrap();
    path
}

fn unique_temp_path(prefix: &str, extension: &str) -> std::path::PathBuf {
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
    fs::canonicalize(std::env::temp_dir())
        .unwrap_or_else(|_| std::env::temp_dir())
        .join(name)
}

fn seed_stale_permission_state(state: &std::path::Path) {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("permission-invalidation")
        .arg(state)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = fs::read_to_string(state).unwrap();
    let mut lines = text.lines().map(str::to_string).collect::<Vec<_>>();
    let first_scope = lines
        .iter_mut()
        .skip(1)
        .find(|line| !line.trim().is_empty())
        .expect("permission snapshot should include at least one scope");
    let mut fields = first_scope
        .split('\t')
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert_eq!(fields.len(), 4, "{first_scope}");
    fields[1] = if fields[1] == "unknown" {
        "granted".to_string()
    } else {
        "unknown".to_string()
    };
    *first_scope = fields.join("\t");
    fs::write(state, format!("{}\n", lines.join("\n"))).unwrap();
}

#[cfg(unix)]
fn make_symlink(target: &std::path::Path, link: &std::path::Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[cfg(not(unix))]
fn make_symlink(_target: &std::path::Path, link: &std::path::Path) {
    fs::write(link, "link").unwrap();
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

fn compressed_pdf(text: &str) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    let stream = format!("BT /F1 12 Tf 72 720 Td ({text}) Tj ET");
    encoder.write_all(stream.as_bytes()).unwrap();
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

fn write_legacy_content_archive(path: &std::path::Path, postings: &[ContentPosting]) {
    let mut bytes = Vec::new();
    bytes.extend(b"gfm-content-v1\n");
    push_varint(&mut bytes, postings.len() as u64);
    for posting in postings {
        push_varint(&mut bytes, posting.term.len() as u64);
        bytes.extend(posting.term.as_bytes());
        write_legacy_file_ids(&mut bytes, &posting.ids);
    }
    fs::write(path, bytes).unwrap();
}

fn write_legacy_metadata_archive(path: &std::path::Path, postings: &[MetadataPosting]) {
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
    fs::write(path, bytes).unwrap();
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

fn ooxml_package(parts: &[(&str, &str)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, text) in parts {
        writer.start_file(*name, options).unwrap();
        writer.write_all(text.as_bytes()).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn tar_gz_package(parts: &[(&str, &str)]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&tar_package(parts)).unwrap();
    encoder.finish().unwrap()
}

fn tar_pax_package(path: &str, text: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    append_tar_entry(
        &mut bytes,
        "./PaxHeaders/gfm",
        b'x',
        pax_path_record(path).as_bytes(),
    );
    append_tar_entry(&mut bytes, "truncated-name.txt", b'0', text.as_bytes());
    bytes.extend([0u8; 1024]);
    bytes
}

fn tar_package(parts: &[(&str, &str)]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (name, text) in parts {
        append_tar_entry(&mut bytes, name, b'0', text.as_bytes());
    }
    bytes.extend([0u8; 1024]);
    bytes
}

fn pax_path_record(path: &str) -> String {
    let mut length = 0usize;
    loop {
        let record = format!("{length} path={path}\n");
        let next = record.len();
        if next == length {
            return record;
        }
        length = next;
    }
}

fn append_tar_entry(bytes: &mut Vec<u8>, name: &str, typeflag: u8, payload: &[u8]) {
    let mut header = [0u8; 512];
    write_tar_string(&mut header[0..100], name);
    write_tar_octal(&mut header[100..108], 0o644);
    write_tar_octal(&mut header[108..116], 0);
    write_tar_octal(&mut header[116..124], 0);
    write_tar_octal(&mut header[124..136], payload.len() as u64);
    write_tar_octal(&mut header[136..148], 0);
    for byte in &mut header[148..156] {
        *byte = b' ';
    }
    header[156] = typeflag;
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let checksum: u64 = header.iter().map(|byte| u64::from(*byte)).sum();
    write_tar_octal(&mut header[148..156], checksum);
    bytes.extend_from_slice(&header);
    bytes.extend_from_slice(payload);
    let padding = (512 - payload.len() % 512) % 512;
    bytes.extend(std::iter::repeat_n(0, padding));
}

fn write_tar_string(field: &mut [u8], value: &str) {
    let bytes = value.as_bytes();
    let len = bytes.len().min(field.len());
    field[..len].copy_from_slice(&bytes[..len]);
}

fn write_tar_octal(field: &mut [u8], value: u64) {
    field.fill(0);
    let encoded = format!("{value:0width$o}", width = field.len().saturating_sub(1));
    let bytes = encoded.as_bytes();
    let start = field.len().saturating_sub(1 + bytes.len());
    field[start..start + bytes.len()].copy_from_slice(bytes);
    field[field.len() - 1] = 0;
}

fn assert_worker_admitted(stderr: &str, worker: &str, path: &std::path::Path) {
    assert!(worker_admission_count(stderr, worker, path) > 0, "{stderr}");
}

fn worker_admission_count(stderr: &str, worker: &str, path: &std::path::Path) -> usize {
    let expected_worker = format!("worker={worker}");
    let expected = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let expected_path = format!("path={}", expected.display());
    stderr
        .lines()
        .filter(|line| {
            line.starts_with("security-worker-admission\t")
                && line.split('\t').any(|field| field == expected_worker)
                && line.split('\t').any(|field| field == expected_path)
        })
        .count()
}

#[cfg(unix)]
fn test_volume_id(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;

    fs::metadata(path).unwrap().dev()
}

#[cfg(not(unix))]
fn test_volume_id(_path: &Path) -> u64 {
    0
}
