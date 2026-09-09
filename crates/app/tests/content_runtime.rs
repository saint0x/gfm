use gfm_store::{
    content_manifest_promotion_journal_path, write_content_postings, write_content_segment,
    ContentArchiveManifest, ContentArchiveManifestEntry, ContentMergeTier,
};
use gfm_types::{ContentPosting, ContentSegment, FileId, VolumeId};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn content_compaction_jobs_persist_runtime_progress_from_binary() {
    let root = unique_temp_dir("gfm-content-runtime-root");
    let segment = root.join("segment.gfmseg");
    let compacted = root.join("compacted.gfmcontent");
    let tiered = root.join("tiered.gfmcontent");
    let manifest = root.join("content.gfmmanifest");
    let maintained = root.join("maintained.gfmcontent");
    let catalog = unique_temp_path("gfm-content-runtime", "gfmjobs");
    let progress = unique_temp_path("gfm-content-runtime", "gfmprogress");
    write_content_segment(
        &segment,
        &ContentSegment {
            postings: vec![ContentPosting {
                term: "runtimecontent".to_string(),
                ids: vec![FileId::new(VolumeId(1), 11)],
                positions: Vec::new(),
            }],
            tombstones: Vec::new(),
        },
    )
    .unwrap();

    let compact_output = content_command(&catalog, &progress)
        .args([
            "compact-content",
            compacted.to_str().unwrap(),
            segment.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        compact_output.status.success(),
        "{}",
        String::from_utf8_lossy(&compact_output.stderr)
    );

    let tiered_output = content_command(&catalog, &progress)
        .args([
            "compact-content-tiered",
            tiered.to_str().unwrap(),
            segment.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        tiered_output.status.success(),
        "{}",
        String::from_utf8_lossy(&tiered_output.stderr)
    );

    ContentArchiveManifest::new(vec![ContentArchiveManifestEntry {
        tier: ContentMergeTier::Hot,
        path: compacted.clone(),
    }])
    .unwrap()
    .write(&manifest)
    .unwrap();
    fs::remove_file(content_manifest_promotion_journal_path(&manifest)).unwrap_or(());
    let maintenance_output = content_command(&catalog, &progress)
        .args([
            "content-maintain-segments",
            manifest.to_str().unwrap(),
            maintained.to_str().unwrap(),
            segment.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        maintenance_output.status.success(),
        "{}",
        String::from_utf8_lossy(&maintenance_output.stderr)
    );

    let catalog_text = fs::read_to_string(&catalog).unwrap();
    assert_runtime_payload(&catalog_text, 1, "content compaction", &compacted);
    assert_runtime_payload(&catalog_text, 2, "tiered content compaction", &tiered);
    assert_runtime_payload(&catalog_text, 3, "content maintenance", &maintained);

    let progress_text = fs::read_to_string(&progress).unwrap();
    assert_runtime_progress(&progress_text, 1, "content compaction", "completed:terms:1");
    assert_runtime_progress(
        &progress_text,
        2,
        "tiered content compaction",
        "completed:terms:0 merged:0 retained:1",
    );
    assert_runtime_progress(
        &progress_text,
        3,
        "content maintenance",
        "completed:terms:0 merged:0 retained:1 manifest:1",
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
}

#[test]
fn content_extraction_and_footprint_jobs_persist_runtime_progress_from_binary() {
    let root = unique_temp_dir("gfm-content-runtime-inspect-root");
    let source = root.join("source.txt");
    let records = root.join("records.gfmidx");
    let quarantine = root.join("quarantine.gfmquarantine");
    let archive = root.join("archive.gfmcontent");
    let manifest = root.join("content.gfmmanifest");
    let catalog = unique_temp_path("gfm-content-runtime-inspect", "gfmjobs");
    let progress = unique_temp_path("gfm-content-runtime-inspect", "gfmprogress");
    fs::write(&source, "runtime extraction cache footprint text").unwrap();
    write_content_postings(
        &archive,
        &[ContentPosting {
            term: "footprintterm".to_string(),
            ids: vec![FileId::new(VolumeId(1), 1)],
            positions: Vec::new(),
        }],
    )
    .unwrap();
    ContentArchiveManifest::new(vec![ContentArchiveManifestEntry {
        tier: ContentMergeTier::Hot,
        path: archive.clone(),
    }])
    .unwrap()
    .write(&manifest)
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

    let cache_output = content_command(&catalog, &progress)
        .args(["extract-cache", source.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        cache_output.status.success(),
        "{}",
        String::from_utf8_lossy(&cache_output.stderr)
    );
    let cache_stdout = String::from_utf8(cache_output.stdout).unwrap();
    assert_eq!(cache_stdout.lines().count(), 2, "{cache_stdout}");

    let footprint_output = content_command(&catalog, &progress)
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
    assert!(
        footprint_output.status.success(),
        "{}",
        String::from_utf8_lossy(&footprint_output.stderr)
    );

    let quarantine_output = content_command(&catalog, &progress)
        .args([
            "extract-quarantine",
            source.to_str().unwrap(),
            quarantine.to_str().unwrap(),
            "timeout",
            "2",
        ])
        .output()
        .unwrap();
    assert!(
        quarantine_output.status.success(),
        "{}",
        String::from_utf8_lossy(&quarantine_output.stderr)
    );

    let catalog_text = fs::read_to_string(&catalog).unwrap();
    assert_runtime_payload_kind(
        &catalog_text,
        1,
        "extraction",
        "content extraction cache",
        &source,
    );
    assert_runtime_payload_kind(&catalog_text, 2, "indexing", "index footprint", &records);
    assert_runtime_payload_kind(
        &catalog_text,
        3,
        "extraction",
        "extraction quarantine",
        &quarantine,
    );

    let progress_text = fs::read_to_string(&progress).unwrap();
    assert_runtime_progress_units(
        &progress_text,
        1,
        "content extraction cache",
        4,
        "completed:cache-probe",
    );
    assert_runtime_progress_units(
        &progress_text,
        2,
        "index footprint",
        4,
        "completed:records:",
    );
    assert_runtime_progress_units(
        &progress_text,
        3,
        "extraction quarantine",
        4,
        "completed:attempts:2",
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
}

fn content_command(catalog: &Path, progress: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_gfm"));
    command
        .env("GFM_JOB_PAYLOAD_CATALOG", catalog)
        .env("GFM_JOB_PROGRESS_STORE", progress);
    command
}

fn assert_runtime_payload(catalog_text: &str, id: u64, label: &str, payload_path: &Path) {
    assert_runtime_payload_kind(catalog_text, id, "indexing", label, payload_path);
}

fn assert_runtime_payload_kind(
    catalog_text: &str,
    id: u64,
    kind: &str,
    label: &str,
    payload_path: &Path,
) {
    assert!(
        catalog_text.lines().any(|line| {
            let prefix = format!(
                "payload\t{id}\t{kind}\t{label}\t{}\t",
                payload_path.display()
            );
            line.starts_with(&prefix)
                && line
                    .strip_prefix(&prefix)
                    .and_then(|fields| fields.split('\t').next())
                    .is_some_and(|volume| !volume.is_empty() && volume != "-")
                && line.contains(&format!("\tvisible:{label}:adaptive"))
        }),
        "{catalog_text}"
    );
}

fn assert_runtime_progress(progress_text: &str, id: u64, label: &str, detail: &str) {
    assert_runtime_progress_units(progress_text, id, label, 3, detail);
}

fn assert_runtime_progress_units(
    progress_text: &str,
    id: u64,
    label: &str,
    total_units: u64,
    detail: &str,
) {
    assert!(
        progress_text.lines().any(|line| line
            .starts_with(&format!("progress\t{id}\tvisible\tvisible\t{label}\t"))
            && line.contains(&format!("\tcompleted\t{total_units}\t{total_units}\t"))
            && line.contains(detail)),
        "{progress_text}"
    );
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let path = unique_temp_path(prefix, "dir");
    fs::create_dir_all(&path).unwrap();
    path
}

fn unique_temp_path(prefix: &str, suffix: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    path.push(format!("{prefix}-{}-{nanos}.{suffix}", std::process::id()));
    path
}
