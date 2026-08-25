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

    let dump_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["config-dump", config.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        dump_output.status.success(),
        "{}",
        String::from_utf8_lossy(&dump_output.stderr)
    );
    let dump_stdout = String::from_utf8(dump_output.stdout).unwrap();
    assert!(dump_stdout.contains("schema_version = 3"), "{dump_stdout}");
    assert!(
        dump_stdout.contains("strict_finder_parity = true"),
        "{dump_stdout}"
    );

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
