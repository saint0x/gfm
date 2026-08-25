use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn bundles_unsigned_app_from_binary() {
    let root = unique_temp_dir("gfm-cli-bundle");
    let icon = root.join("GFM.icns");
    let dist = root.join("dist");
    fs::write(&icon, b"icns-test").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "bundle-app",
            env!("CARGO_BIN_EXE_gfm"),
            icon.to_str().unwrap(),
            dist.to_str().unwrap(),
            "--unsigned",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.contains("GFM.app"), "{stdout}");
    assert!(dist.join("GFM.app/Contents/MacOS/gfm").is_file());
    assert!(dist.join("GFM.app/Contents/Info.plist").is_file());
    assert!(dist.join("GFM.app/Contents/Resources/GFM.icns").is_file());
    assert!(dist.join("GFM.entitlements").is_file());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn notarize_app_requires_explicit_credentials_from_binary() {
    let root = unique_temp_dir("gfm-cli-notarize");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "notarize-app",
            root.join("GFM.app").to_str().unwrap(),
            root.join("dist").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();

    assert!(stderr.contains("requires --keychain-profile"), "{stderr}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn release_policy_reports_private_defaults_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("release-policy")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.contains("channel\tstable"), "{stdout}");
    assert!(stdout.contains("updates\tdisabled"), "{stdout}");
    assert!(stdout.contains("crash-reports\tlocal-only"), "{stdout}");
    assert!(stdout.contains("diagnostics\tlocal-only"), "{stdout}");
    assert!(stdout.contains("remote-allowed=false"), "{stdout}");
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
