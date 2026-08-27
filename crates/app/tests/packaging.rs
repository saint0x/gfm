use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn bundles_unsigned_app_from_binary() {
    let root = unique_temp_dir("gfm-cli-bundle");
    let executable = root.join("gfm");
    let icon = root.join("GFM.icns");
    let dist = root.join("dist");
    fs::write(&executable, b"#!/bin/sh\n").unwrap();
    fs::write(&icon, b"icns-test").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "bundle-app",
            executable.to_str().unwrap(),
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
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(&stderr, "bundle app executable", &executable);
    assert_worker_admitted(&stderr, "bundle app icon", &icon);
    assert_worker_admitted(&stderr, "bundle app output", &root);

    assert!(stdout.contains("GFM.app"), "{stdout}");
    assert!(dist.join("GFM.app/Contents/MacOS/gfm").is_file());
    assert!(dist.join("GFM.app/Contents/Info.plist").is_file());
    assert!(dist.join("GFM.app/Contents/Resources/GFM.icns").is_file());
    assert!(dist.join("GFM.entitlements").is_file());

    let validate = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "release-validate",
            dist.join("GFM.app").to_str().unwrap(),
            "--allow-unsigned",
            "--skip-notarization",
            "--skip-gatekeeper",
        ])
        .output()
        .unwrap();
    assert!(
        validate.status.success(),
        "{}",
        String::from_utf8_lossy(&validate.stderr)
    );
    let validate_stdout = String::from_utf8(validate.stdout).unwrap();
    let validate_stderr = String::from_utf8_lossy(&validate.stderr);
    assert_worker_admitted(
        &validate_stderr,
        "release validate app",
        &dist.join("GFM.app"),
    );
    assert!(
        validate_stdout.contains("com.saint0x.gfm"),
        "{validate_stdout}"
    );
    assert!(
        validate_stdout.contains("not-required"),
        "{validate_stdout}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn packaging_routes_refuse_unreachable_paths_before_toolchain_or_bundle_io_from_binary() {
    let root = unique_temp_dir("gfm-cli-packaging-preflight-root");
    let offline = unique_temp_dir("gfm-cli-packaging-preflight-offline");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let executable = root.join("gfm");
    let icon = root.join("GFM.icns");
    let app = root.join("GFM.app");
    let offline_app = offline.join("GFM.app");
    let offline_dist = offline.join("dist");
    let offline_notary = offline.join("notary");
    let offline_key = offline.join("AuthKey.p8");
    let local_notary = root.join("notary");
    fs::write(&executable, b"bin").unwrap();
    fs::write(&icon, b"icns").unwrap();
    fs::create_dir_all(&app).unwrap();

    let cases = [
        (
            vec![
                "release-validate".to_string(),
                offline_app.to_string_lossy().into_owned(),
                "--allow-unsigned".to_string(),
                "--skip-notarization".to_string(),
                "--skip-gatekeeper".to_string(),
            ],
            "release validate app",
            "com.saint0x.gfm",
        ),
        (
            vec![
                "bundle-app".to_string(),
                executable.to_string_lossy().into_owned(),
                icon.to_string_lossy().into_owned(),
                offline_dist.to_string_lossy().into_owned(),
                "--unsigned".to_string(),
            ],
            "bundle app output",
            "GFM.app",
        ),
        (
            vec![
                "register-app".to_string(),
                offline_app.to_string_lossy().into_owned(),
            ],
            "register app",
            "GFM.app",
        ),
        (
            vec![
                "notarize-app".to_string(),
                app.to_string_lossy().into_owned(),
                offline_notary.to_string_lossy().into_owned(),
                "--keychain-profile".to_string(),
                "release".to_string(),
            ],
            "notarize output",
            "accepted",
        ),
        (
            vec![
                "notarize-app".to_string(),
                app.to_string_lossy().into_owned(),
                local_notary.to_string_lossy().into_owned(),
                "--api-key".to_string(),
                offline_key.to_string_lossy().into_owned(),
                "--key-id".to_string(),
                "KEYID".to_string(),
                "--issuer".to_string(),
                "ISSUER".to_string(),
            ],
            "notarize api key",
            "accepted",
        ),
    ];

    for (args, worker, forbidden_stdout) in cases {
        let route = args[0].clone();
        let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
            .args(args)
            .output()
            .unwrap();

        assert!(!output.status.success(), "{route}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stdout.contains(forbidden_stdout), "{route}: {stdout}");
        assert!(
            stderr.contains(&format!(
                "{worker} volume access blocked: unreachable volume network"
            )),
            "{route}: {stderr}"
        );
        assert!(
            !stderr.contains("requires xcrun") && !stderr.contains("requires codesign"),
            "{route}: {stderr}"
        );
    }

    assert!(!offline.join("dist").exists());
    assert!(!offline.join("notary").exists());

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
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

fn assert_worker_admitted(stderr: &str, worker: &str, path: &Path) {
    let expected_worker = format!("worker={worker}");
    let expected_path = format!("path={}", path.display());
    assert!(
        stderr.lines().any(|line| {
            line.starts_with("security-worker-admission\t")
                && line.split('\t').any(|field| field == expected_worker)
                && line.split('\t').any(|field| field == expected_path)
        }),
        "{stderr}"
    );
}
