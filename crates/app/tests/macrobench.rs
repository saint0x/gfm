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

#[test]
fn materializes_macrobench_fixture_from_binary() {
    let root = unique_temp_dir("gfm-cli-macrobench-fixture");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["macrobench-fixture", root.to_str().unwrap(), "smoke"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let fixture = root.join("gfm-macrobench-fixture");

    assert!(stdout.contains("fixture\t"), "{stdout}");
    assert!(stdout.contains("\tfiles\t201\t"), "{stdout}");
    assert!(stdout.contains("\tscenarios\t9"), "{stdout}");
    assert!(stdout.contains("documents\t"), "{stdout}");
    assert!(fixture.join("manifest.tsv").exists());
    assert!(fixture
        .join("documents")
        .join("year-2020")
        .join("Briefing Project 00000000.md")
        .exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn materializes_parity_fixture_from_binary() {
    let root = unique_temp_dir("gfm-cli-parity-fixture");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["parity-fixture", root.to_str().unwrap(), "smoke"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let fixture = root.join("gfm-parity-fixture");

    assert!(stdout.contains("fixture\t"), "{stdout}");
    assert!(stdout.contains("\tscenarios\t19"), "{stdout}");
    assert!(stdout.contains("icon\ticon\t"), "{stdout}");
    assert!(stdout.contains("network-volume\ticon\t"), "{stdout}");
    assert!(stdout.contains("conflict-sheet\ticon\t"), "{stdout}");
    assert!(stdout.contains("sheet\ticon\t"), "{stdout}");
    assert!(stdout.contains("menu\ticon\t"), "{stdout}");
    assert!(fixture.join("manifest.tsv").exists());
    assert!(fixture.join("search").join("Needle Name.txt").exists());
    assert!(fixture
        .join("conflict-sheet")
        .join(".gfm-operation-conflicts.tsv")
        .exists());
    assert!(fixture.join("sheet").join(".gfm-sheet-states.tsv").exists());
    assert!(fixture.join("menu").join(".gfm-menu-states.tsv").exists());
    assert_eq!(fs::read_dir(fixture.join("empty")).unwrap().count(), 0);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn benchmark_workspace_routes_refuse_unreachable_volume_before_materializing_from_binary() {
    let offline = unique_temp_dir("gfm-cli-gate-workspace-preflight-offline");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();

    let cases = [
        (
            vec!["macrobench", offline.to_str().unwrap(), "smoke"],
            "macrobench workspace",
            "fixture\t",
        ),
        (
            vec!["macrobench-fixture", offline.to_str().unwrap(), "smoke"],
            "macrobench fixture workspace",
            "fixture\t",
        ),
        (
            vec!["parity-fixture", offline.to_str().unwrap(), "smoke"],
            "parity fixture workspace",
            "fixture\t",
        ),
        (
            vec!["regression-gate", offline.to_str().unwrap(), "smoke"],
            "regression gate workspace",
            "fixture\t",
        ),
        (
            vec!["large-sidecar-gate", offline.to_str().unwrap(), "128"],
            "large sidecar gate workspace",
            "large-sidecar-gate\t",
        ),
        (
            vec![
                "search-typing-benchmark",
                offline.to_str().unwrap(),
                "128",
                "1",
                "needle",
            ],
            "search typing benchmark workspace",
            "search-typing-benchmark\t",
        ),
        (
            vec![
                "search-typing-session-benchmark",
                offline.to_str().unwrap(),
                "128",
                "1",
                "needle",
            ],
            "search typing session benchmark workspace",
            "search-typing-session-benchmark\t",
        ),
    ];

    for (args, worker, forbidden_stdout) in cases {
        let route = args[0];
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
    }

    assert!(!offline.join("gfm-macrobench-fixture").exists());
    assert!(!offline.join("gfm-parity-fixture").exists());
    assert!(!offline.join("gfm-large-sidecar-gate").exists());
    assert!(!offline.join("gfm-search-typing-benchmark").exists());

    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn compares_pixel_diff_from_binary() {
    let root = unique_temp_dir("gfm-cli-pixel-diff");
    let expected = root.join("expected.rgba");
    let actual = root.join("actual.rgba");
    let mask = root.join("mask.tsv");
    fs::write(&expected, [0, 0, 0, 255, 10, 10, 10, 255, 20, 20, 20, 255]).unwrap();
    fs::write(&actual, [0, 0, 0, 255, 9, 10, 10, 255, 20, 20, 20, 255]).unwrap();
    fs::write(&mask, "1\t0\t1\t1\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "pixel-diff",
            expected.to_str().unwrap(),
            actual.to_str().unwrap(),
            "3",
            "1",
            mask.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();

    assert!(stdout.contains("pixel-diff\t3x1\t"), "{stdout}");
    assert!(
        stdout.contains("mismatched=1\tunmasked=0\tmasked=1\tmax-channel-delta=1\tpassed=true"),
        "{stdout}"
    );
    assert!(
        stderr.contains("security-worker-admission\tworker=pixel expected\t")
            && stderr.contains("security-worker-admission\tworker=pixel actual\t")
            && stderr.contains("security-worker-admission\tworker=pixel mask\t"),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn checks_pixel_threshold_from_binary() {
    let root = unique_temp_dir("gfm-cli-pixel-threshold");
    let expected = root.join("expected.rgba");
    let actual = root.join("actual.rgba");
    let mask = root.join("mask.tsv");
    fs::write(&expected, [0, 0, 0, 255, 10, 10, 10, 255, 20, 20, 20, 255]).unwrap();
    fs::write(&actual, [0, 0, 0, 255, 9, 10, 10, 255, 20, 20, 20, 255]).unwrap();
    fs::write(&mask, "1\t0\t1\t1\tOS-owned toolbar repaint\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "pixel-threshold-check",
            "toolbar",
            expected.to_str().unwrap(),
            actual.to_str().unwrap(),
            "3",
            "1",
            mask.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();

    assert!(
        stdout.contains("threshold\ttoolbar\tunmasked<=0"),
        "{stdout}"
    );
    assert!(
        stdout.contains("passed=true\tmismatched=1\tunmasked=0\tmasked=1"),
        "{stdout}"
    );
    assert!(
        stderr.contains("security-worker-admission\tworker=pixel expected\t")
            && stderr.contains("security-worker-admission\tworker=pixel actual\t")
            && stderr.contains("security-worker-admission\tworker=pixel mask\t"),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pixel_threshold_accepts_sidebar_sheet_and_menu_surfaces_from_binary() {
    let root = unique_temp_dir("gfm-cli-pixel-threshold-finder-surfaces");
    let expected = root.join("expected.rgba");
    let actual = root.join("actual.rgba");
    fs::write(&expected, [0, 0, 0, 255]).unwrap();
    fs::write(&actual, [0, 0, 0, 255]).unwrap();

    for surface in ["sidebar", "sheet", "menu"] {
        let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
            .args([
                "pixel-threshold-check",
                surface,
                expected.to_str().unwrap(),
                actual.to_str().unwrap(),
                "1",
                "1",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}: {}",
            surface,
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(
            stdout.contains(&format!("threshold\t{surface}\t")),
            "{stdout}"
        );
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pixel_threshold_rejects_ungoverned_mask_from_binary() {
    let root = unique_temp_dir("gfm-cli-pixel-threshold-ungoverned");
    let expected = root.join("expected.rgba");
    let actual = root.join("actual.rgba");
    let mask = root.join("mask.tsv");
    fs::write(&expected, [0, 0, 0, 255, 10, 10, 10, 255]).unwrap();
    fs::write(&actual, [0, 0, 0, 255, 9, 10, 10, 255]).unwrap();
    fs::write(&mask, "1\t0\t1\t1\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "pixel-threshold-check",
            "toolbar",
            expected.to_str().unwrap(),
            actual.to_str().unwrap(),
            "2",
            "1",
            mask.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("must contain x, y, width, height, reason"),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pixel_threshold_rejects_loose_governed_mask_from_binary() {
    let root = unique_temp_dir("gfm-cli-pixel-threshold-loose-mask");
    let expected = root.join("expected.rgba");
    let actual = root.join("actual.rgba");
    let mask = root.join("mask.tsv");
    fs::write(&expected, [0, 0, 0, 255, 10, 10, 10, 255]).unwrap();
    fs::write(&actual, [0, 0, 0, 255, 9, 10, 10, 255]).unwrap();
    fs::write(&mask, "0\t0\t1\t1\tOS-owned toolbar repaint\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "pixel-threshold-check",
            "toolbar",
            expected.to_str().unwrap(),
            actual.to_str().unwrap(),
            "2",
            "1",
            mask.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("threshold\ttoolbar\t"), "{stdout}");
    assert!(stderr.contains("governed mask 0,0,1,1"), "{stderr}");
    assert!(stderr.contains("loose or stale"), "{stderr}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pixel_threshold_rejects_duplicate_governed_mask_rectangles_from_binary() {
    let root = unique_temp_dir("gfm-cli-pixel-threshold-duplicate-governed-mask");
    let expected = root.join("expected.rgba");
    let actual = root.join("actual.rgba");
    let mask = root.join("mask.tsv");
    fs::write(&expected, [0, 0, 0, 255, 10, 10, 10, 255]).unwrap();
    fs::write(&actual, [0, 0, 0, 255, 9, 10, 10, 255]).unwrap();
    fs::write(
        &mask,
        "1\t0\t1\t1\tOS-owned toolbar repaint\n1\t0\t1\t1\tOS-owned clock tick\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "pixel-threshold-check",
            "toolbar",
            expected.to_str().unwrap(),
            actual.to_str().unwrap(),
            "2",
            "1",
            mask.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("threshold\ttoolbar\t"), "{stdout}");
    assert!(stderr.contains("line 2"), "{stderr}");
    assert!(stderr.contains("duplicates rectangle 1,0,1,1"), "{stderr}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pixel_routes_refuse_unreachable_inputs_before_reading_from_binary() {
    let root = unique_temp_dir("gfm-cli-pixel-preflight-root");
    let offline = unique_temp_dir("gfm-cli-pixel-preflight-offline");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let expected = root.join("expected.rgba");
    let actual = offline.join("actual.rgba");
    let mask = root.join("mask.tsv");
    fs::write(&expected, [0, 0, 0, 255]).unwrap();
    fs::write(&actual, "not read").unwrap();
    fs::write(&mask, "0\t0\t1\t1\tOS-owned repaint\n").unwrap();

    let diff = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "pixel-diff",
            expected.to_str().unwrap(),
            actual.to_str().unwrap(),
            "1",
            "1",
            mask.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!diff.status.success());
    let diff_stdout = String::from_utf8_lossy(&diff.stdout);
    let diff_stderr = String::from_utf8_lossy(&diff.stderr);
    assert!(!diff_stdout.contains("pixel-diff\t"), "{diff_stdout}");
    assert!(
        diff_stderr.contains("pixel actual volume access blocked: unreachable volume network"),
        "{diff_stderr}"
    );
    assert!(
        !diff_stderr.contains("security-worker-admission\t"),
        "{diff_stderr}"
    );
    assert!(!diff_stderr.contains("RGBA"), "{diff_stderr}");

    let threshold = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "pixel-threshold-check",
            "toolbar",
            expected.to_str().unwrap(),
            actual.to_str().unwrap(),
            "1",
            "1",
            mask.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!threshold.status.success());
    let threshold_stdout = String::from_utf8_lossy(&threshold.stdout);
    let threshold_stderr = String::from_utf8_lossy(&threshold.stderr);
    assert!(
        !threshold_stdout.contains("threshold\ttoolbar\t"),
        "{threshold_stdout}"
    );
    assert!(
        threshold_stderr.contains("pixel actual volume access blocked: unreachable volume network"),
        "{threshold_stderr}"
    );
    assert!(
        !threshold_stderr.contains("security-worker-admission\t"),
        "{threshold_stderr}"
    );
    assert!(
        !threshold_stderr.contains("must contain x, y, width, height, reason"),
        "{threshold_stderr}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn runs_parity_gate_from_binary_manifest() {
    let root = unique_temp_dir("gfm-cli-parity-gate");
    fs::write(root.join("expected.rgba"), [0, 0, 0, 255, 10, 10, 10, 255]).unwrap();
    fs::write(root.join("actual.rgba"), [0, 0, 0, 255, 9, 10, 10, 255]).unwrap();
    fs::write(
        root.join("mask.tsv"),
        "1\t0\t1\t1\tOS-owned toolbar repaint\n",
    )
    .unwrap();
    fs::write(
        root.join("gate.tsv"),
        "manifest-version\t1\nprofile\tmacos-build=25A354\thardware-profile=macbookpro18,3\tdisplay-profile=studio-display-p3\tapp-version=0.1.0\tfixture-manifest=fixtures/manifest.tsv\tcaptured-at=2026-08-27T00:00:00Z\tcapture-command=screencapture:-x\treviewer=codex\tsigner=codex\tapproved-mask-set=macos-25A354-default\tappearance=dark\tscale=2x\tcolor-profile=display-p3\nentry\ttoolbar\texpected.rgba\tactual.rgba\t2\t1\tmask.tsv\t1040\t720\tactive\ticon\tfixtures/toolbar\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["parity-gate", root.join("gate.tsv").to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();

    assert!(stdout.contains("parity-gate\tmanifest="), "{stdout}");
    assert!(
        stdout.contains("entries=1\tviolations=0\tpassed=true"),
        "{stdout}"
    );
    assert!(
        stdout.contains("threshold\ttoolbar\tunmasked<=0"),
        "{stdout}"
    );
    assert!(
        stderr.contains("security-worker-admission\tworker=parity gate\t"),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn parity_gate_rejects_identical_capture_artifacts_from_binary() {
    let root = unique_temp_dir("gfm-cli-parity-gate-identical-artifacts");
    let manifest = root.join("gate.tsv");
    fs::write(root.join("capture.rgba"), [0, 0, 0, 255]).unwrap();
    fs::write(
        &manifest,
        "manifest-version\t1\nprofile\tmacos-build=25A354\thardware-profile=macbookpro18,3\tdisplay-profile=studio-display-p3\tapp-version=0.1.0\tfixture-manifest=fixtures/manifest.tsv\tcaptured-at=2026-08-27T00:00:00Z\tcapture-command=screencapture:-x\treviewer=codex\tsigner=codex\tapproved-mask-set=macos-25A354-default\tappearance=dark\tscale=2x\tcolor-profile=display-p3\nentry\ttoolbar\tcapture.rgba\tcapture.rgba\t1\t1\t\t1040\t720\tactive\ticon\tfixtures/toolbar\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["parity-gate", manifest.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("parity-gate\t"), "{stdout}");
    assert!(
        stderr.contains("must compare distinct Finder and GFM capture artifacts"),
        "{stderr}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn parity_gate_rejects_duplicate_capture_profile_keys_from_binary() {
    let root = unique_temp_dir("gfm-cli-parity-gate-duplicate-profile-key");
    let manifest = root.join("gate.tsv");
    fs::write(root.join("expected.rgba"), [0, 0, 0, 255]).unwrap();
    fs::write(root.join("actual.rgba"), [0, 0, 0, 255]).unwrap();
    fs::write(
        &manifest,
        "manifest-version\t1\nprofile\tmacos-build=25A354\tmacos-build=25A999\thardware-profile=macbookpro18,3\tdisplay-profile=studio-display-p3\tapp-version=0.1.0\tfixture-manifest=fixtures/manifest.tsv\tcaptured-at=2026-08-27T00:00:00Z\tcapture-command=screencapture:-x\treviewer=codex\tsigner=codex\tapproved-mask-set=macos-25A354-default\tappearance=dark\tscale=2x\tcolor-profile=display-p3\nentry\ttoolbar\texpected.rgba\tactual.rgba\t1\t1\t\t1040\t720\tactive\ticon\tfixtures/toolbar\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["parity-gate", manifest.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("parity-gate\t"), "{stdout}");
    assert!(
        stderr.contains("duplicate profile key `macos-build`"),
        "{stderr}"
    );
    assert!(stderr.contains("line 2"), "{stderr}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn parity_gate_rejects_duplicate_capture_profile_rows_from_binary() {
    let root = unique_temp_dir("gfm-cli-parity-gate-duplicate-profile-row");
    let manifest = root.join("gate.tsv");
    fs::write(root.join("expected.rgba"), [0, 0, 0, 255]).unwrap();
    fs::write(root.join("actual.rgba"), [0, 0, 0, 255]).unwrap();
    let profile = "profile\tmacos-build=25A354\thardware-profile=macbookpro18,3\tdisplay-profile=studio-display-p3\tapp-version=0.1.0\tfixture-manifest=fixtures/manifest.tsv\tcaptured-at=2026-08-27T00:00:00Z\tcapture-command=screencapture:-x\treviewer=codex\tsigner=codex\tapproved-mask-set=macos-25A354-default\tappearance=dark\tscale=2x\tcolor-profile=display-p3";
    fs::write(
        &manifest,
        format!(
            "manifest-version\t1\n{profile}\nentry\ttoolbar\texpected.rgba\tactual.rgba\t1\t1\t\t1040\t720\tactive\ticon\tfixtures/toolbar\n{profile}\nentry\ttext\texpected.rgba\tactual.rgba\t1\t1\t\t1040\t720\tactive\tlist\tfixtures/text\n"
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["parity-gate", manifest.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("parity-gate\t"), "{stdout}");
    assert!(stderr.contains("duplicate capture profile"), "{stderr}");
    assert!(stderr.contains("line 4"), "{stderr}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn parity_routes_refuse_unreachable_paths_before_manifest_or_bundle_io_from_binary() {
    let root = unique_temp_dir("gfm-cli-parity-preflight-root");
    let offline = unique_temp_dir("gfm-cli-parity-preflight-offline");
    fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let manifest = offline.join("gate.tsv");
    fs::write(&manifest, "not parsed").unwrap();

    let gate = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["parity-gate", manifest.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!gate.status.success());
    let gate_stdout = String::from_utf8_lossy(&gate.stdout);
    let gate_stderr = String::from_utf8_lossy(&gate.stderr);
    assert!(!gate_stdout.contains("parity-gate\t"), "{gate_stdout}");
    assert!(
        gate_stderr.contains("parity gate volume access blocked: unreachable volume network"),
        "{gate_stderr}"
    );
    assert!(
        !gate_stderr.contains("security-worker-admission\t"),
        "{gate_stderr}"
    );
    assert!(
        !gate_stderr.contains("missing capture provenance"),
        "{gate_stderr}"
    );

    let local_manifest = root.join("gate.tsv");
    fs::write(&local_manifest, "not parsed").unwrap();
    let review = offline.join("review");
    let review_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "parity-review",
            local_manifest.to_str().unwrap(),
            review.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!review_output.status.success());
    let review_stdout = String::from_utf8_lossy(&review_output.stdout);
    let review_stderr = String::from_utf8_lossy(&review_output.stderr);
    assert!(
        !review_stdout.contains("parity-review\t"),
        "{review_stdout}"
    );
    assert!(
        review_stderr
            .contains("parity review output volume access blocked: unreachable volume network"),
        "{review_stderr}"
    );
    assert!(
        !review_stderr.contains("security-worker-admission\t"),
        "{review_stderr}"
    );
    assert!(
        !review_stderr.contains("missing capture provenance"),
        "{review_stderr}"
    );
    assert!(!review.exists());

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(offline).unwrap();
}

#[test]
fn gate_routes_report_output_probe_failures_before_manifest_or_fixture_io_from_binary() {
    let root = unique_temp_dir("gfm-cli-gate-output-probe");
    let manifest = root.join("gate.tsv");
    let review = root.join("parity-review-unavailable".repeat(16));
    let fixture = root.join("macrobench-fixture-unavailable".repeat(16));
    fs::write(&manifest, "not parsed").unwrap();

    let review_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "parity-review",
            manifest.to_str().unwrap(),
            review.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!review_output.status.success());
    let review_stdout = String::from_utf8_lossy(&review_output.stdout);
    let review_stderr = String::from_utf8_lossy(&review_output.stderr);
    assert!(
        !review_stdout.contains("parity-review\t"),
        "{review_stdout}"
    );
    assert!(
        review_stderr.contains("gate write path metadata unavailable"),
        "{review_stderr}"
    );
    assert!(
        review_stderr.contains("parity-review-unavailable"),
        "{review_stderr}"
    );
    assert!(
        !review_stderr.contains("security-worker-admission\tworker=parity review manifest\t"),
        "{review_stderr}"
    );
    assert!(
        !review_stderr.contains("missing capture provenance"),
        "{review_stderr}"
    );
    assert!(!review.exists());

    let fixture_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["macrobench-fixture", fixture.to_str().unwrap(), "smoke"])
        .output()
        .unwrap();
    assert!(!fixture_output.status.success());
    let fixture_stdout = String::from_utf8_lossy(&fixture_output.stdout);
    let fixture_stderr = String::from_utf8_lossy(&fixture_output.stderr);
    assert!(!fixture_stdout.contains("fixture\t"), "{fixture_stdout}");
    assert!(
        fixture_stderr.contains("gate write path metadata unavailable"),
        "{fixture_stderr}"
    );
    assert!(
        fixture_stderr.contains("macrobench-fixture-unavailable"),
        "{fixture_stderr}"
    );
    assert!(
        !fixture_stderr
            .contains("security-worker-admission\tworker=macrobench fixture workspace\t"),
        "{fixture_stderr}"
    );
    assert!(!fixture.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn writes_parity_review_bundle_from_binary_manifest() {
    let root = unique_temp_dir("gfm-cli-parity-review");
    let review = root.join("review");
    fs::write(root.join("expected.rgba"), [0, 0, 0, 255, 10, 10, 10, 255]).unwrap();
    fs::write(root.join("actual.rgba"), [0, 0, 0, 255, 9, 10, 10, 255]).unwrap();
    fs::write(
        root.join("gate.tsv"),
        "manifest-version\t1\nprofile\tmacos-build=25A354\thardware-profile=macbookpro18,3\tdisplay-profile=studio-display-p3\tapp-version=0.1.0\tfixture-manifest=fixtures/manifest.tsv\tcaptured-at=2026-08-27T00:00:00Z\tcapture-command=screencapture:-x\treviewer=codex\tsigner=codex\tapproved-mask-set=macos-25A354-default\tappearance=dark\tscale=2x\tcolor-profile=display-p3\nentry\ttext\texpected.rgba\tactual.rgba\t2\t1\t\t1040\t720\tactive\tlist\tfixtures/text\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "parity-review",
            root.join("gate.tsv").to_str().unwrap(),
            review.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();

    assert!(
        stdout.contains("entries=1\tviolations=1\tpassed=false"),
        "{stdout}"
    );
    assert!(
        stderr.contains("security-worker-admission\tworker=parity review manifest\t")
            && stderr.contains("security-worker-admission\tworker=parity review output\t"),
        "{stderr}"
    );
    assert!(review.join("review.md").exists());
    assert!(review.join("entries.tsv").exists());
    assert!(review.join("violations.tsv").exists());
    assert!(review.join("first-unmasked.tsv").exists());
    let review_markdown = fs::read_to_string(review.join("review.md")).unwrap();
    assert!(
        review_markdown.contains("## Capture Provenance"),
        "{review_markdown}"
    );
    assert!(
        review_markdown.contains("| text | 25A354 | dark | 2x | display-p3 |"),
        "{review_markdown}"
    );
    assert!(
        review_markdown.contains("| codex | codex | macos-25A354-default |"),
        "{review_markdown}"
    );
    assert!(fs::read_to_string(review.join("violations.tsv"))
        .unwrap()
        .contains("unmasked-mismatch-budget"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reports_parity_profile_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["parity-profile", "25A354", "dark", "2x", "display-p3"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(
        stdout.contains("profile\tbuild=25A354\tappearance=dark\tscale=2x"),
        "{stdout}"
    );
    assert!(
        stdout.contains("dimension\ttoolbar.height\t54px\tui/toolbar"),
        "{stdout}"
    );
    assert!(
        stdout.contains("symbol\tview.column\trectangle.split.3x1"),
        "{stdout}"
    );
}

#[test]
fn runs_regression_gate_from_binary() {
    let root = unique_temp_dir("gfm-cli-regression-gate");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["regression-gate", root.to_str().unwrap(), "smoke"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.contains("fixture\t"), "{stdout}");
    assert!(stdout.contains("index-bytes\t"), "{stdout}");
    assert!(stdout.contains("sidecar-prefix-candidates\t"), "{stdout}");
    assert!(stdout.contains("sidecar-fuzzy-verified\t"), "{stdout}");
    assert!(stdout.contains("passed\ttrue"), "{stdout}");
    assert!(root
        .join("gfm-macrobench-fixture")
        .join("gate-indexes")
        .join("small.gfmidx")
        .exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn runs_large_sidecar_gate_from_binary() {
    let root = unique_temp_dir("gfm-cli-large-sidecar-gate");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["large-sidecar-gate", root.to_str().unwrap(), "4096"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.contains("large-sidecar-gate\t"), "{stdout}");
    assert!(stdout.contains("\trecords=4096\t"), "{stdout}");
    assert!(
        stdout.contains("\tprofile=production-macos-million-v1\t"),
        "{stdout}"
    );
    assert!(stdout.contains("\tmin-ci-records=1000000\t"), "{stdout}");
    assert!(stdout.contains("\tprefix-keys="), "{stdout}");
    assert!(stdout.contains("\tprobe-records=4096\t"), "{stdout}");
    assert!(stdout.contains("\tfuzzy-keys="), "{stdout}");
    assert!(stdout.contains("\tprefix-cache-hits="), "{stdout}");
    assert!(stdout.contains("\tviolations=0\t"), "{stdout}");
    assert!(stdout.contains("\tpassed=true"), "{stdout}");
    let fixture = root.join("gfm-large-sidecar-gate");
    assert!(fixture.join("records.gfmprefix").exists());
    assert!(fixture.join("thresholds.tsv").exists());
    assert!(root.join("gfm-large-sidecar-history.tsv").exists());
    assert!(fs::read_to_string(fixture.join("thresholds.tsv"))
        .unwrap()
        .contains("large-sidecar-thresholds\tprofile=production-macos-million-v1"));
    assert!(
        fs::read_to_string(root.join("gfm-large-sidecar-history.tsv"))
            .unwrap()
            .contains("large-sidecar-history\trun=1")
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
