use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn archive_rebuild_jobs_persist_runtime_progress_from_binary() {
    let root = unique_temp_dir("gfm-archive-runtime-root");
    let records = unique_temp_path("gfm-archive-runtime-records", "gfmidx");
    let columns = unique_temp_path("gfm-archive-runtime-columns", "gfmcols");
    let prefixes = unique_temp_path("gfm-archive-runtime-prefixes", "gfmprefix");
    let backup = unique_temp_dir("gfm-archive-runtime-backup");
    let catalog = unique_temp_path("gfm-archive-runtime", "gfmjobs");
    let progress = unique_temp_path("gfm-archive-runtime", "gfmprogress");
    fs::write(root.join("RuntimeArchive.md"), "archive runtime rebuild").unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), records.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );

    let schema_output = archive_command(&catalog, &progress)
        .args(["archive-schema", "records", records.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        schema_output.status.success(),
        "{}",
        String::from_utf8_lossy(&schema_output.stderr)
    );

    let plan_output = archive_command(&catalog, &progress)
        .args([
            "columns-rebuild-plan",
            records.to_str().unwrap(),
            columns.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        plan_output.status.success(),
        "{}",
        String::from_utf8_lossy(&plan_output.stderr)
    );

    let rebuild_output = archive_command(&catalog, &progress)
        .args([
            "columns-rebuild",
            records.to_str().unwrap(),
            columns.to_str().unwrap(),
            backup.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        rebuild_output.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuild_output.stderr)
    );

    let prefixes_output = archive_command(&catalog, &progress)
        .args([
            "index-prefixes",
            records.to_str().unwrap(),
            prefixes.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        prefixes_output.status.success(),
        "{}",
        String::from_utf8_lossy(&prefixes_output.stderr)
    );

    let recovery_plan_output = archive_command(&catalog, &progress)
        .args([
            "sidecar-recovery-plan",
            records.to_str().unwrap(),
            columns.to_str().unwrap(),
            "-",
            prefixes.to_str().unwrap(),
            "-",
            "-",
            "-",
        ])
        .output()
        .unwrap();
    assert!(
        recovery_plan_output.status.success(),
        "{}",
        String::from_utf8_lossy(&recovery_plan_output.stderr)
    );

    let catalog_text = fs::read_to_string(&catalog).unwrap();
    assert_runtime_payload(&catalog_text, 1, "indexing", "archive schema", &records);
    assert_runtime_payload(
        &catalog_text,
        2,
        "indexing",
        "columns rebuild plan",
        &columns,
    );
    assert_runtime_payload(&catalog_text, 3, "indexing", "columns rebuild", &columns);
    assert_runtime_payload(&catalog_text, 4, "indexing", "index prefixes", &prefixes);
    assert_runtime_payload(&catalog_text, 5, "repair", "sidecar repair plan", &records);

    let progress_text = fs::read_to_string(&progress).unwrap();
    assert_runtime_progress(
        &progress_text,
        1,
        "archive schema",
        3,
        "completed:archive-read",
    );
    assert_runtime_progress(
        &progress_text,
        2,
        "columns rebuild plan",
        3,
        "completed:columns-rebuild-plan",
    );
    assert_runtime_progress(
        &progress_text,
        3,
        "columns rebuild",
        3,
        "completed:records:2",
    );
    assert_runtime_progress(
        &progress_text,
        4,
        "index prefixes",
        4,
        "completed:records:2",
    );
    assert_runtime_progress(
        &progress_text,
        5,
        "sidecar repair plan",
        3,
        "completed:valid:2 invalid:0",
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(columns).unwrap();
    fs::remove_file(prefixes).unwrap();
    fs::remove_dir_all(backup).unwrap();
    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
}

#[test]
fn archive_migration_jobs_persist_runtime_progress_from_binary() {
    let records = unique_temp_path("gfm-archive-runtime-migrate-records", "gfmidx");
    let backup = unique_temp_dir("gfm-archive-runtime-migrate-backup");
    let catalog = unique_temp_path("gfm-archive-runtime-migrate", "gfmjobs");
    let progress = unique_temp_path("gfm-archive-runtime-migrate", "gfmprogress");
    fs::write(
        &records,
        "gfm-store-v1\n1\t2\t0\tf\t1\t0\t0\t0\t0\t/tmp/legacy.txt\n",
    )
    .unwrap();

    let plan_output = archive_command(&catalog, &progress)
        .args(["records-migration-plan", records.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        plan_output.status.success(),
        "{}",
        String::from_utf8_lossy(&plan_output.stderr)
    );

    let migrate_output = archive_command(&catalog, &progress)
        .args([
            "records-migrate",
            records.to_str().unwrap(),
            backup.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        migrate_output.status.success(),
        "{}",
        String::from_utf8_lossy(&migrate_output.stderr)
    );

    let catalog_text = fs::read_to_string(&catalog).unwrap();
    assert_runtime_payload(
        &catalog_text,
        1,
        "indexing",
        "records migration plan",
        &records,
    );
    assert_runtime_payload(&catalog_text, 2, "repair", "records migrate", &records);

    let progress_text = fs::read_to_string(&progress).unwrap();
    assert_runtime_progress(
        &progress_text,
        1,
        "records migration plan",
        3,
        "completed:archive-read",
    );
    assert_runtime_progress(
        &progress_text,
        2,
        "records migrate",
        3,
        "completed:archive-migration",
    );

    fs::remove_file(records).unwrap();
    fs::remove_dir_all(backup).unwrap();
    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
}

fn archive_command(catalog: &Path, progress: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_gfm"));
    command
        .env("GFM_JOB_PAYLOAD_CATALOG", catalog)
        .env("GFM_JOB_PROGRESS_STORE", progress);
    command
}

fn assert_runtime_payload(
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

fn assert_runtime_progress(
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
