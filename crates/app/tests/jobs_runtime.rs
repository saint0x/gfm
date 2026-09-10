use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn jobs_store_routes_persist_runtime_progress_from_binary() {
    let root = unique_temp_dir("gfm-jobs-runtime-root");
    let target_catalog = root.join("target.gfmjobs");
    let target_progress = root.join("target.gfmprogress");
    let runtime_catalog = root.join("runtime.gfmjobs");
    let runtime_progress = root.join("runtime.gfmprogress");

    let payload_output = jobs_command(&runtime_catalog, &runtime_progress)
        .args(["jobs-payload-catalog", target_catalog.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        payload_output.status.success(),
        "{}",
        String::from_utf8_lossy(&payload_output.stderr)
    );

    let snapshot_output = jobs_command(&runtime_catalog, &runtime_progress)
        .args(["jobs-progress-snapshot", target_progress.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        snapshot_output.status.success(),
        "{}",
        String::from_utf8_lossy(&snapshot_output.stderr)
    );

    let restore_plan_output = jobs_command(&runtime_catalog, &runtime_progress)
        .args([
            "jobs-payload-restore-plan",
            target_catalog.to_str().unwrap(),
            target_progress.to_str().unwrap(),
            "2000",
        ])
        .output()
        .unwrap();
    assert!(
        restore_plan_output.status.success(),
        "{}",
        String::from_utf8_lossy(&restore_plan_output.stderr)
    );

    let control_output = jobs_command(&runtime_catalog, &runtime_progress)
        .args([
            "jobs-progress-control",
            target_progress.to_str().unwrap(),
            "1",
            "pause",
            "3000",
        ])
        .output()
        .unwrap();
    assert!(
        control_output.status.success(),
        "{}",
        String::from_utf8_lossy(&control_output.stderr)
    );

    let runtime_catalog_text = fs::read_to_string(&runtime_catalog).unwrap();
    assert_runtime_payload(
        &runtime_catalog_text,
        1,
        "jobs payload catalog",
        &target_catalog,
    );
    assert_runtime_payload(
        &runtime_catalog_text,
        2,
        "jobs progress snapshot",
        &target_progress,
    );
    assert_runtime_payload(
        &runtime_catalog_text,
        3,
        "jobs payload restore plan",
        &target_progress,
    );
    assert_runtime_payload(
        &runtime_catalog_text,
        4,
        "jobs progress control",
        &target_progress,
    );

    let runtime_progress_text = fs::read_to_string(&runtime_progress).unwrap();
    assert_runtime_progress(
        &runtime_progress_text,
        1,
        "jobs payload catalog",
        "completed:payloads:6",
    );
    assert_runtime_progress(
        &runtime_progress_text,
        2,
        "jobs progress snapshot",
        "completed:snapshots:2",
    );
    assert_runtime_progress(
        &runtime_progress_text,
        3,
        "jobs payload restore plan",
        "completed:restore-plan:2",
    );
    assert_runtime_progress(
        &runtime_progress_text,
        4,
        "jobs progress control",
        "completed:pause:job:1",
    );

    let target_progress_text = fs::read_to_string(&target_progress).unwrap();
    assert!(
        target_progress_text.contains(
            "\tpaused\t42\t100\tpaused-by-user:interrupted:running:copy:/source->/target"
        ),
        "{target_progress_text}"
    );

    fs::remove_dir_all(root).unwrap();
}

fn jobs_command(catalog: &Path, progress: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_gfm"));
    command
        .env("GFM_JOB_PAYLOAD_CATALOG", catalog)
        .env("GFM_JOB_PROGRESS_STORE", progress);
    command
}

fn assert_runtime_payload(catalog_text: &str, id: u64, label: &str, payload_path: &Path) {
    assert!(
        catalog_text.lines().any(|line| {
            let prefix = format!(
                "payload\t{id}\toperation\t{label}\t{}\t",
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
    assert!(
        progress_text.lines().any(|line| line
            .starts_with(&format!("progress\t{id}\tvisible\tvisible\t{label}\t"))
            && line.contains("\tcompleted\t3\t3\t")
            && line.contains(detail)),
        "{progress_text}"
    );
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    path.push(format!("{prefix}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&path).unwrap();
    path
}
