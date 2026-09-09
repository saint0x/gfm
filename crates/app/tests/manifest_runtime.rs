use gfm_store::write_content_postings;
use gfm_types::{ContentPosting, FileId, VolumeId};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn content_manifest_jobs_persist_runtime_progress_from_binary() {
    let root = unique_temp_dir("gfm-manifest-runtime-root");
    let manifest = root.join("content.gfmmanifest");
    let quarantine = root.join("quarantine");
    let archive = root.join("archive.gfmcontent");
    let catalog = unique_temp_path("gfm-manifest-runtime", "gfmjobs");
    let progress = unique_temp_path("gfm-manifest-runtime", "gfmprogress");
    fs::create_dir_all(&quarantine).unwrap();
    write_content_postings(
        &archive,
        &[ContentPosting {
            term: "manifestneedle".to_string(),
            ids: vec![FileId::new(VolumeId(1), 7)],
            positions: Vec::new(),
        }],
    )
    .unwrap();

    let write_output = manifest_command(&catalog, &progress)
        .args([
            "content-manifest-write",
            manifest.to_str().unwrap(),
            &format!("hot:{}", archive.display()),
        ])
        .output()
        .unwrap();
    assert!(
        write_output.status.success(),
        "{}",
        String::from_utf8_lossy(&write_output.stderr)
    );

    let inspect_output = manifest_command(&catalog, &progress)
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
        inspect_stdout.contains("content-manifest\tarchives=1\tterms=1\t")
            && inspect_stdout.contains("archive\thot\t"),
        "{inspect_stdout}"
    );

    let plan_output = manifest_command(&catalog, &progress)
        .args([
            "content-manifest-recovery-plan",
            manifest.to_str().unwrap(),
            &format!("hot:{}", archive.display()),
        ])
        .output()
        .unwrap();
    assert!(
        plan_output.status.success(),
        "{}",
        String::from_utf8_lossy(&plan_output.stderr)
    );

    let missing_manifest = root.join("missing.gfmmanifest");
    let recover_output = manifest_command(&catalog, &progress)
        .args([
            "content-manifest-recover",
            missing_manifest.to_str().unwrap(),
            quarantine.to_str().unwrap(),
            &format!("hot:{}", archive.display()),
        ])
        .output()
        .unwrap();
    assert!(
        recover_output.status.success(),
        "{}",
        String::from_utf8_lossy(&recover_output.stderr)
    );

    let catalog_text = fs::read_to_string(&catalog).unwrap();
    assert_runtime_payload(&catalog_text, 1, "content manifest write", &manifest);
    assert_runtime_payload(&catalog_text, 2, "content manifest inspect", &manifest);
    assert_runtime_payload(
        &catalog_text,
        3,
        "content manifest recovery plan",
        &manifest,
    );
    assert_runtime_payload(
        &catalog_text,
        4,
        "content manifest recovery",
        &missing_manifest,
    );

    let progress_text = fs::read_to_string(&progress).unwrap();
    assert_runtime_progress(
        &progress_text,
        1,
        "content manifest write",
        "completed:archives:1",
    );
    assert_runtime_progress(
        &progress_text,
        2,
        "content manifest inspect",
        "completed:archives:1 terms:1",
    );
    assert_runtime_progress(
        &progress_text,
        3,
        "content manifest recovery plan",
        "completed:invalid:0",
    );
    assert_runtime_progress(
        &progress_text,
        4,
        "content manifest recovery",
        "completed:wrote:true invalid-before:0",
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(catalog).unwrap();
    fs::remove_file(progress).unwrap();
}

fn manifest_command(catalog: &Path, progress: &Path) -> Command {
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
                "payload\t{id}\tindexing\t{label}\t{}\t",
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
            && line.contains("\tcompleted\t")
            && line.contains(&format!("\t{detail}\t"))),
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
