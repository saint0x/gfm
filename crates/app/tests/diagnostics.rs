use std::fs;
use std::path::{Path, PathBuf};
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
    assert!(
        !deferred_stderr.contains(&format!(
            "\tworker=index rebuild root\tpath={}",
            root.display()
        )),
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
    let rebuild_stderr = String::from_utf8_lossy(&rebuild.stderr);
    assert_worker_admitted(&rebuild_stderr, "index rebuild root", &root);
    assert_worker_admitted(&rebuild_stderr, "index rebuild records", &root);
    assert_worker_admitted(&rebuild_stderr, "index rebuild content", &root);
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
    let records_stderr = String::from_utf8_lossy(&records_inspect.stderr);
    assert_worker_admitted(&records_stderr, "diagnostics storage", &records);
    let records_stdout = String::from_utf8(records_inspect.stdout).unwrap();
    assert!(records_stdout.starts_with("records\t"), "{records_stdout}");

    let content_inspect = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["diagnostics-storage-inspect", content.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(content_inspect.status.success());
    let content_stderr = String::from_utf8_lossy(&content_inspect.stderr);
    assert_worker_admitted(&content_stderr, "diagnostics storage", &content);
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
fn diagnostics_rebuild_adaptive_defers_before_unreachable_volume_from_binary() {
    let root = unique_temp_dir("gfm-cli-diagnostics-rebuild-adaptive-unreachable");
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    fs::write(root.join("needle.md"), "diagnostic needle").unwrap();
    let records = root.join("records.gfmidx");
    let content = root.join("content.gfmcontent");
    let catalog = unique_temp_path("gfm-cli-diagnostics-rebuild-adaptive-runtime", "gfmjobs");
    let progress = unique_temp_path(
        "gfm-cli-diagnostics-rebuild-adaptive-runtime",
        "gfmprogress",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "diagnostics-index-rebuild-adaptive",
            root.to_str().unwrap(),
            records.to_str().unwrap(),
            "saturated",
            "critical",
            "low",
            "active",
            content.to_str().unwrap(),
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
    assert!(!stdout.contains("records.gfmidx"), "{stdout}");
    assert!(
        stderr.contains("index-rebuild-deferred") && stderr.contains("action=Defer"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(!records.exists());
    assert!(!content.exists());
    let catalog_text = fs::read_to_string(&catalog).unwrap();
    assert!(catalog_text.contains("\tindexing\t"), "{catalog_text}");
    assert!(catalog_text.contains("index rebuild"), "{catalog_text}");
    assert!(
        catalog_text.contains(&records.display().to_string()),
        "{catalog_text}"
    );
    let progress_text = fs::read_to_string(&progress).unwrap();
    assert!(
        progress_text.contains("progress\t1\tbackground\tbackground\tindex rebuild"),
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
fn adaptive_diagnostics_rebuild_refuses_unreachable_outputs_before_worker_from_binary() {
    let root = unique_temp_dir("gfm-cli-diagnostics-rebuild-adaptive-root");
    let offline = unique_temp_dir("gfm-cli-diagnostics-rebuild-adaptive-offline");
    fs::write(root.join("needle.md"), "diagnostic needle").unwrap();
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let records = offline.join("records.gfmidx");
    let content = offline.join("content.gfmcontent");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "diagnostics-index-rebuild-adaptive",
            root.to_str().unwrap(),
            records.to_str().unwrap(),
            "nominal",
            "nominal",
            "ac",
            "idle",
            content.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("records.gfmidx"), "{stdout}");
    assert!(
        stderr.contains("index rebuild records volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=index rebuild\tpath={}",
            root.display()
        )),
        "{stderr}"
    );
    assert!(!records.exists());
    assert!(!content.exists());

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn diagnostics_rebuild_reports_output_probe_failure_before_worker_from_binary() {
    let root = unique_temp_dir("gfm-cli-diagnostics-rebuild-output-probe");
    fs::write(root.join("needle.md"), "diagnostic needle").unwrap();
    let records = root.join(format!("{}.gfmidx", "records-unavailable".repeat(16)));
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
        stderr.contains("diagnostics write path metadata unavailable"),
        "{stderr}"
    );
    assert!(stderr.contains("records-unavailable"), "{stderr}");
    assert!(
        !stderr.contains("security-worker-admission\tworker=index rebuild root\t"),
        "{stderr}"
    );
    assert!(!content.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn diagnostics_trace_and_storage_refuse_unreachable_paths_before_io_from_binary() {
    let offline = unique_temp_dir("gfm-cli-diagnostics-io-preflight-offline");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let trace = offline.join("trace.json");
    let storage = offline.join("records.gfmidx");
    fs::write(&storage, "gfm-records-v1\n").unwrap();

    let trace_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["diagnostics-trace-export", trace.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!trace_output.status.success());
    let trace_stdout = String::from_utf8_lossy(&trace_output.stdout);
    let trace_stderr = String::from_utf8_lossy(&trace_output.stderr);
    assert!(!trace_stdout.contains("trace.json"), "{trace_stdout}");
    assert!(
        trace_stderr
            .contains("diagnostics trace export volume access blocked: unreachable volume network"),
        "{trace_stderr}"
    );
    assert!(
        !trace_stderr.contains("security-worker-admission\tworker=diagnostics trace export\t"),
        "{trace_stderr}"
    );
    assert!(!trace.exists());

    let storage_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["diagnostics-storage-inspect", storage.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!storage_output.status.success());
    let storage_stdout = String::from_utf8_lossy(&storage_output.stdout);
    let storage_stderr = String::from_utf8_lossy(&storage_output.stderr);
    assert!(!storage_stdout.starts_with("records\t"), "{storage_stdout}");
    assert!(
        storage_stderr
            .contains("diagnostics storage volume access blocked: unreachable volume network"),
        "{storage_stderr}"
    );
    assert!(
        !storage_stderr.contains("security-worker-admission\tworker=diagnostics storage\t"),
        "{storage_stderr}"
    );

    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn diagnostics_trace_export_reports_output_probe_failure_before_io_from_binary() {
    let root = unique_temp_dir("gfm-cli-diagnostics-trace-probe");
    let trace = root.join(format!("{}.json", "trace-unavailable".repeat(16)));

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["diagnostics-trace-export", trace.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("trace-unavailable"), "{stdout}");
    assert!(
        stderr.contains("diagnostics write path metadata unavailable"),
        "{stderr}"
    );
    assert!(stderr.contains("trace-unavailable"), "{stderr}");
    assert!(
        !stderr.contains("security-worker-admission\tworker=diagnostics trace export\t"),
        "{stderr}"
    );

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
    let plan_stderr = String::from_utf8_lossy(&plan.stderr);
    assert_worker_admitted(&plan_stderr, "persistent index repair root", &root);
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
    assert!(
        !deferred_stderr.contains(&format!(
            "\tworker=persistent index repair root\tpath={}",
            root.display()
        )),
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
    let recover_stderr = String::from_utf8_lossy(&recover.stderr);
    assert_worker_admitted(&recover_stderr, "persistent index repair root", &root);
    assert_worker_admitted(&recover_stderr, "persistent index repair records", &root);
    assert_worker_admitted(&recover_stderr, "persistent index repair state", &root);
    assert_worker_admitted(&recover_stderr, "persistent index repair quarantine", &root);
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
fn diagnostics_recover_adaptive_defers_before_unreachable_volume_from_binary() {
    let root = unique_temp_dir("gfm-cli-diagnostics-recovery-adaptive-unreachable");
    let records = root.join("records.gfmidx");
    let state = root.join("state.gfmstate");
    let quarantine = root.join("quarantine");
    let catalog = unique_temp_path("gfm-cli-diagnostics-recovery-adaptive-runtime", "gfmjobs");
    let progress = unique_temp_path(
        "gfm-cli-diagnostics-recovery-adaptive-runtime",
        "gfmprogress",
    );
    fs::write(&records, "not-records").unwrap();
    fs::write(&state, "not-state").unwrap();
    fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args([
            "diagnostics-index-recover-adaptive",
            root.to_str().unwrap(),
            records.to_str().unwrap(),
            state.to_str().unwrap(),
            "saturated",
            "critical",
            "low",
            "active",
            quarantine.to_str().unwrap(),
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
    assert!(!stdout.contains("persistent-index-recovery"), "{stdout}");
    assert!(
        stderr.contains("persistent-index-recovery-deferred") && stderr.contains("action=Defer"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert_eq!(fs::read_to_string(&records).unwrap(), "not-records");
    assert_eq!(fs::read_to_string(&state).unwrap(), "not-state");
    assert!(!quarantine.exists());
    let catalog_text = fs::read_to_string(&catalog).unwrap();
    assert!(catalog_text.contains("\trepair\t"), "{catalog_text}");
    assert!(
        catalog_text.contains("persistent index repair"),
        "{catalog_text}"
    );
    assert!(
        catalog_text.contains(&state.display().to_string()),
        "{catalog_text}"
    );
    let progress_text = fs::read_to_string(&progress).unwrap();
    assert!(
        progress_text.contains("progress\t1\tbackground\tbackground\tpersistent index repair"),
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
fn adaptive_diagnostics_recover_refuses_unreachable_outputs_before_worker_from_binary() {
    let root = unique_temp_dir("gfm-cli-diagnostics-recovery-adaptive-root");
    let offline = unique_temp_dir("gfm-cli-diagnostics-recovery-adaptive-offline");
    fs::write(root.join("needle.md"), "diagnostic needle").unwrap();
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let records = offline.join("records.gfmidx");
    let state = offline.join("state.gfmstate");
    let quarantine = offline.join("quarantine");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "diagnostics-index-recover-adaptive",
            root.to_str().unwrap(),
            records.to_str().unwrap(),
            state.to_str().unwrap(),
            "nominal",
            "nominal",
            "ac",
            "idle",
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
            "persistent index repair records volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=persistent index repair\tpath={}",
            root.display()
        )),
        "{stderr}"
    );
    assert!(!records.exists());
    assert!(!state.exists());
    assert!(!quarantine.exists());

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn diagnostics_recover_reports_output_probe_failures_before_worker_from_binary() {
    for target in ["records", "state", "quarantine"] {
        let root = unique_temp_dir(&format!("gfm-cli-diagnostics-recovery-{target}-probe"));
        let records = if target == "records" {
            root.join(format!("{}.gfmidx", "records-unavailable".repeat(16)))
        } else {
            let path = root.join("records.gfmidx");
            fs::write(&path, "not-records").unwrap();
            path
        };
        let state = if target == "state" {
            root.join(format!("{}.gfmstate", "state-unavailable".repeat(16)))
        } else {
            let path = root.join("state.gfmstate");
            fs::write(&path, "not-state").unwrap();
            path
        };
        let quarantine = if target == "quarantine" {
            root.join(format!("{}", "quarantine-unavailable".repeat(16)))
        } else {
            root.join("quarantine")
        };
        fs::write(root.join("needle.md"), "diagnostic needle").unwrap();

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

        assert!(!output.status.success(), "{target}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stdout.contains("persistent-index-recovery"), "{target}: {stdout}");
        assert!(
            stderr.contains("diagnostics write path metadata unavailable"),
            "{target}: {stderr}"
        );
        assert!(stderr.contains(&format!("{target}-unavailable")), "{target}: {stderr}");
        assert!(
            !stderr.contains("security-worker-admission\tworker=persistent index repair\t"),
            "{target}: {stderr}"
        );
        if target != "records" {
            assert_eq!(fs::read_to_string(&records).unwrap(), "not-records");
        }
        if target != "state" {
            assert_eq!(fs::read_to_string(&state).unwrap(), "not-state");
        }
        assert!(!quarantine.exists());

        fs::remove_dir_all(root).unwrap();
    }
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
    let trace_stderr = String::from_utf8_lossy(&trace_output.stderr);
    assert_worker_admitted(&trace_stderr, "diagnostics trace export", &root);
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
    let parity_stderr = String::from_utf8_lossy(&parity_output.stderr);
    assert_worker_admitted(&parity_stderr, "diagnostics parity config", &root);
    assert_worker_admitted(&parity_stderr, "diagnostics parity baseline", &root);
    let saved = fs::read_to_string(config).unwrap();
    assert!(saved.contains("25A354"), "{saved}");
    assert!(saved.contains("baselines"), "{saved}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn diagnostics_parity_baseline_refuses_unreachable_paths_before_config_write_from_binary() {
    let root = unique_temp_dir("gfm-cli-diagnostics-parity-preflight-root");
    let offline = unique_temp_dir("gfm-cli-diagnostics-parity-preflight-offline");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let config = root.join("config.toml");
    let offline_config = offline.join("config.toml");
    let baseline = root.join("baselines");
    let offline_baseline = offline.join("baselines");
    fs::create_dir_all(&baseline).unwrap();

    let baseline_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "diagnostics-parity-baseline",
            config.to_str().unwrap(),
            offline_baseline.to_str().unwrap(),
            "25A354",
        ])
        .output()
        .unwrap();
    assert!(!baseline_output.status.success());
    let baseline_stdout = String::from_utf8_lossy(&baseline_output.stdout);
    let baseline_stderr = String::from_utf8_lossy(&baseline_output.stderr);
    assert!(!baseline_stdout.contains("25A354"), "{baseline_stdout}");
    assert!(
        baseline_stderr.contains(
            "diagnostics parity baseline volume access blocked: unreachable volume network"
        ),
        "{baseline_stderr}"
    );
    assert!(!config.exists());

    let config_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "diagnostics-parity-baseline",
            offline_config.to_str().unwrap(),
            baseline.to_str().unwrap(),
            "25A354",
        ])
        .output()
        .unwrap();
    assert!(!config_output.status.success());
    let config_stdout = String::from_utf8_lossy(&config_output.stdout);
    let config_stderr = String::from_utf8_lossy(&config_output.stderr);
    assert!(!config_stdout.contains("25A354"), "{config_stdout}");
    assert!(
        config_stderr.contains(
            "diagnostics parity config volume access blocked: unreachable volume network"
        ),
        "{config_stderr}"
    );
    assert!(!offline_config.exists());

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn diagnostics_parity_baseline_surfaces_config_probe_failure_before_writing_from_binary() {
    let root = unique_temp_dir("gfm-cli-diagnostics-parity-config-probe");
    let config = root.join(format!("{}.toml", "config-unavailable".repeat(32)));
    let baseline = root.join("baselines");
    fs::create_dir_all(&baseline).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "diagnostics-parity-baseline",
            config.to_str().unwrap(),
            baseline.to_str().unwrap(),
            "25A354",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("25A354"), "{stdout}");
    assert!(
        stderr.contains("config write path metadata unavailable"),
        "{stderr}"
    );
    assert!(stderr.contains(&config.display().to_string()), "{stderr}");
    assert!(
        !stderr.contains("security-worker-admission\tworker=diagnostics parity config\t"),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
}

fn assert_worker_admitted(stderr: &str, worker: &str, path: &Path) {
    let expected_worker = format!("worker={worker}");
    let expected = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let expected_path = format!("path={}", expected.display());
    assert!(
        stderr.lines().any(|line| {
            line.starts_with("security-worker-admission\t")
                && line.split('\t').any(|field| field == expected_worker)
                && line.split('\t').any(|field| field == expected_path)
        }),
        "{stderr}"
    );
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

fn unique_temp_path(prefix: &str, extension: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "{prefix}-{}-{nanos}.{extension}",
        std::process::id()
    ))
}
