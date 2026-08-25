use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn runs_macrobench_from_binary() {
    let root = unique_temp_dir("gfm-cli-macrobench");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["macrobench", root.to_str().unwrap(), "smoke"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.contains("fixture\t"), "{stdout}");
    assert!(stdout.contains("small\tindex-build\t"), "{stdout}");
    assert!(stdout.contains("network\tcontent-search\t"), "{stdout}");
    assert!(root.join("gfm-macrobench-fixture").exists());

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
