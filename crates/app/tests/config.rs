use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn creates_checks_and_dumps_config_from_binary() {
    let root = unique_temp_dir("gfm-cli-config-root");
    let config = root.join("config.toml");

    let init_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["config-init", config.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        init_output.status.success(),
        "{}",
        String::from_utf8_lossy(&init_output.stderr)
    );
    let init_stderr = String::from_utf8_lossy(&init_output.stderr);
    assert_worker_admitted(&init_stderr, "config init", &root);
    let init_stdout = String::from_utf8(init_output.stdout).unwrap();
    assert!(init_stdout.contains("3\t"), "{init_stdout}");
    assert!(config.exists());

    let check_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["config-check", config.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        check_output.status.success(),
        "{}",
        String::from_utf8_lossy(&check_output.stderr)
    );
    let check_stderr = String::from_utf8_lossy(&check_output.stderr);
    assert_worker_admitted(&check_stderr, "config check", &config);

    let dump_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["config-dump", config.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        dump_output.status.success(),
        "{}",
        String::from_utf8_lossy(&dump_output.stderr)
    );
    let dump_stderr = String::from_utf8_lossy(&dump_output.stderr);
    assert_worker_admitted(&dump_stderr, "config dump", &config);
    let dump_stdout = String::from_utf8(dump_output.stdout).unwrap();
    assert!(dump_stdout.contains("schema_version = 3"), "{dump_stdout}");
    assert!(
        dump_stdout.contains("strict_finder_parity = true"),
        "{dump_stdout}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn config_init_uses_cwd_write_probe_for_relative_store_from_binary() {
    let root = unique_temp_dir("gfm-cli-config-relative-root");

    let init_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .current_dir(&root)
        .args(["config-init", "config.toml"])
        .output()
        .unwrap();
    assert!(
        init_output.status.success(),
        "{}",
        String::from_utf8_lossy(&init_output.stderr)
    );
    assert!(root.join("config.toml").exists());
    let init_stderr = String::from_utf8_lossy(&init_output.stderr);
    assert_worker_admitted(&init_stderr, "config init", &root);
    assert!(
        !init_stderr.contains("security-worker-admission\tworker=config init\tpath=\t"),
        "{init_stderr}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn config_routes_refuse_unreachable_volume_before_loading_or_persisting_from_binary() {
    let root = unique_temp_dir("gfm-cli-config-preflight-root");
    let offline = unique_temp_dir("gfm-cli-config-preflight-offline");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let offline_config = offline.join("config.toml");
    let local_config = root.join("config.toml");
    fs::write(&offline_config, "not valid = [\n").unwrap();
    fs::write(&local_config, "not valid = [\n").unwrap();

    let init_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["config-init", offline.join("new.toml").to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!init_output.status.success());
    let init_stdout = String::from_utf8_lossy(&init_output.stdout);
    let init_stderr = String::from_utf8_lossy(&init_output.stderr);
    assert!(!init_stdout.contains("3\t"), "{init_stdout}");
    assert!(
        init_stderr.contains("config init volume access blocked: unreachable volume network"),
        "{init_stderr}"
    );
    assert!(
        !init_stderr.contains("security-worker-admission\tworker=config init\t"),
        "{init_stderr}"
    );
    assert!(!offline.join("new.toml").exists());

    for (route, worker) in [
        ("config-check", "config check"),
        ("config-dump", "config dump"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
            .args([route, offline_config.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{route}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stdout.contains("schema_version"), "{route}: {stdout}");
        assert!(
            stderr.contains(&format!(
                "{worker} volume access blocked: unreachable volume network"
            )),
            "{route}: {stderr}"
        );
        assert!(
            !stderr.contains("invalid GFM config TOML"),
            "{route}: {stderr}"
        );
        assert!(
            !stderr.contains(&format!("security-worker-admission\tworker={worker}\t")),
            "{route}: {stderr}"
        );
    }

    let reachable_parse = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["config-check", local_config.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!reachable_parse.status.success());
    let reachable_stderr = String::from_utf8_lossy(&reachable_parse.stderr);
    assert!(
        reachable_stderr.contains("invalid GFM config TOML"),
        "{reachable_stderr}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
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

fn assert_worker_admitted(stderr: &str, worker: &str, path: &std::path::Path) {
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
