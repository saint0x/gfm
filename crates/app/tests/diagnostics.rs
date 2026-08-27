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

    let deferred = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "diagnostics-index-rebuild-adaptive",
            root.to_str().unwrap(),
            records.to_str().unwrap(),
            "saturated",
            "nominal",
            "ac",
            "idle",
            content.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        deferred.status.success(),
        "{}",
        String::from_utf8_lossy(&deferred.stderr)
    );
    let deferred_stderr = String::from_utf8(deferred.stderr).unwrap();
    assert!(
        deferred_stderr.contains("index-rebuild-deferred")
            && deferred_stderr.contains("action=Defer"),
        "{deferred_stderr}"
    );
    assert!(!records.exists());
    assert!(!content.exists());

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
fn diagnostics_rebuild_refuses_unreachable_volume_before_writing_indexes_from_binary() {
    let root = unique_temp_dir("gfm-cli-diagnostics-rebuild-unreachable");
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(root.join("needle.md"), "diagnostic needle").unwrap();
    let records = root.join("records.gfmidx");
    let content = root.join("content.gfmcontent");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "diagnostics-index-rebuild",
            root.to_str().unwrap(),
            records.to_str().unwrap(),
            content.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("records.gfmidx"), "{stdout}");
    assert!(
        stderr.contains("index rebuild root volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(!records.exists());
    assert!(!content.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn diagnostics_plans_and_recovers_persistent_index_from_binary() {
    let root = unique_temp_dir("gfm-cli-diagnostics-recovery");
    let records = root.join("records.gfmidx");
    let state = root.join("state.gfmstate");
    let quarantine = root.join("quarantine");
    fs::write(root.join("needle.md"), "diagnostic needle").unwrap();

    let rebuild = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-state",
            root.to_str().unwrap(),
            records.to_str().unwrap(),
            state.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        rebuild.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuild.stderr)
    );
    fs::remove_file(&state).unwrap();

    let plan = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "diagnostics-index-recovery-plan",
            root.to_str().unwrap(),
            records.to_str().unwrap(),
            state.to_str().unwrap(),
            quarantine.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(plan.status.success());
    let plan_stdout = String::from_utf8(plan.stdout).unwrap();
    assert!(
        plan_stdout.contains("action=rebuild-state"),
        "{plan_stdout}"
    );
    assert!(
        plan_stdout.contains("reason=missing-state"),
        "{plan_stdout}"
    );

    let deferred = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "diagnostics-index-recover-adaptive",
            root.to_str().unwrap(),
            records.to_str().unwrap(),
            state.to_str().unwrap(),
            "saturated",
            "nominal",
            "ac",
            "idle",
            quarantine.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        deferred.status.success(),
        "{}",
        String::from_utf8_lossy(&deferred.stderr)
    );
    let deferred_stderr = String::from_utf8(deferred.stderr).unwrap();
    assert!(
        deferred_stderr.contains("persistent-index-recovery-deferred")
            && deferred_stderr.contains("action=Defer"),
        "{deferred_stderr}"
    );
    assert!(!state.exists());
    assert!(!quarantine.exists());

    let recover = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "diagnostics-index-recover",
            root.to_str().unwrap(),
            records.to_str().unwrap(),
            state.to_str().unwrap(),
            quarantine.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        recover.status.success(),
        "{}",
        String::from_utf8_lossy(&recover.stderr)
    );
    let recover_stdout = String::from_utf8(recover.stdout).unwrap();
    assert!(
        recover_stdout.contains("rebuilt-records=false"),
        "{recover_stdout}"
    );
    assert!(
        recover_stdout.contains("rebuilt-state=true"),
        "{recover_stdout}"
    );
    assert!(recover_stdout.contains("action=ready"), "{recover_stdout}");
    assert!(state.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn diagnostics_recover_refuses_unreachable_volume_before_repair_from_binary() {
    let root = unique_temp_dir("gfm-cli-diagnostics-recovery-unreachable");
    let records = root.join("records.gfmidx");
    let state = root.join("state.gfmstate");
    let quarantine = root.join("quarantine");
    fs::write(root.join("needle.md"), "diagnostic needle").unwrap();

    let rebuild = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-state",
            root.to_str().unwrap(),
            records.to_str().unwrap(),
            state.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        rebuild.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuild.stderr)
    );
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::remove_file(&state).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "diagnostics-index-recover",
            root.to_str().unwrap(),
            records.to_str().unwrap(),
            state.to_str().unwrap(),
            quarantine.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("persistent-index-recovery"), "{stdout}");
    assert!(
        stderr.contains(
            "persistent index repair root volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert!(!state.exists());
    assert!(!quarantine.exists());

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
