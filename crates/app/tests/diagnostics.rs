use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn diagnostics_rebuilds_and_inspects_indexes_from_binary() {
    let root = unique_temp_dir("gfm-cli-diagnostics-rebuild");
    let records = root.join("records.gfmidx");
    let content = root.join("content.gfmcontent");
    fs::write(root.join("needle.md"), "diagnostic needle").unwrap();

    let rebuild = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "diagnostics-index-rebuild",
            root.to_str().unwrap(),
            records.to_str().unwrap(),
            content.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        rebuild.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuild.stderr)
    );
    let rebuild_stdout = String::from_utf8(rebuild.stdout).unwrap();
    assert!(
        rebuild_stdout.contains("records.gfmidx"),
        "{rebuild_stdout}"
    );
    assert!(
        rebuild_stdout.contains("content.gfmcontent"),
        "{rebuild_stdout}"
    );

    let records_inspect = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["diagnostics-storage-inspect", records.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(records_inspect.status.success());
    let records_stdout = String::from_utf8(records_inspect.stdout).unwrap();
    assert!(records_stdout.starts_with("records\t"), "{records_stdout}");

    let content_inspect = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["diagnostics-storage-inspect", content.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(content_inspect.status.success());
    let content_stdout = String::from_utf8(content_inspect.stdout).unwrap();
    assert!(content_stdout.starts_with("content\t"), "{content_stdout}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn diagnostics_exports_trace_and_selects_parity_baseline_from_binary() {
    let root = unique_temp_dir("gfm-cli-diagnostics-config");
    let trace = root.join("trace.json");
    let config = root.join("config.toml");
    let baseline = root.join("baselines");

    let trace_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["diagnostics-trace-export", trace.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        trace_output.status.success(),
        "{}",
        String::from_utf8_lossy(&trace_output.stderr)
    );
    assert!(trace.exists());
    let encoded = fs::read_to_string(&trace).unwrap();
    assert!(encoded.contains("\"schema_version\""));

    let parity_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "diagnostics-parity-baseline",
            config.to_str().unwrap(),
            baseline.to_str().unwrap(),
            "25A354",
        ])
        .output()
        .unwrap();
    assert!(
        parity_output.status.success(),
        "{}",
        String::from_utf8_lossy(&parity_output.stderr)
    );
    let saved = fs::read_to_string(config).unwrap();
    assert!(saved.contains("25A354"), "{saved}");
    assert!(saved.contains("baselines"), "{saved}");

    fs::remove_dir_all(root).unwrap();
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&path).unwrap();
    path
}
