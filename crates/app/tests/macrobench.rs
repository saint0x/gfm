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
    assert!(stdout.contains("\tscenarios\t16"), "{stdout}");
    assert!(stdout.contains("icon\ticon\t"), "{stdout}");
    assert!(stdout.contains("network-volume\ticon\t"), "{stdout}");
    assert!(fixture.join("manifest.tsv").exists());
    assert!(fixture.join("search").join("Needle Name.txt").exists());
    assert_eq!(fs::read_dir(fixture.join("empty")).unwrap().count(), 0);

    fs::remove_dir_all(root).unwrap();
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

    assert!(stdout.contains("pixel-diff\t3x1\t"), "{stdout}");
    assert!(
        stdout.contains("mismatched=1\tunmasked=0\tmasked=1\tpassed=true"),
        "{stdout}"
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
    fs::write(&mask, "1\t0\t1\t1\n").unwrap();

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

    assert!(
        stdout.contains("threshold\ttoolbar\tunmasked<=0"),
        "{stdout}"
    );
    assert!(
        stdout.contains("passed=true\tmismatched=1\tunmasked=0\tmasked=1"),
        "{stdout}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn runs_parity_gate_from_binary_manifest() {
    let root = unique_temp_dir("gfm-cli-parity-gate");
    fs::write(root.join("expected.rgba"), [0, 0, 0, 255, 10, 10, 10, 255]).unwrap();
    fs::write(root.join("actual.rgba"), [0, 0, 0, 255, 9, 10, 10, 255]).unwrap();
    fs::write(root.join("mask.tsv"), "1\t0\t1\t1\n").unwrap();
    fs::write(
        root.join("gate.tsv"),
        "toolbar\texpected.rgba\tactual.rgba\t2\t1\tmask.tsv\n",
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

    assert!(stdout.contains("parity-gate\tmanifest="), "{stdout}");
    assert!(
        stdout.contains("entries=1\tviolations=0\tpassed=true"),
        "{stdout}"
    );
    assert!(
        stdout.contains("threshold\ttoolbar\tunmasked<=0"),
        "{stdout}"
    );

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
        "text\texpected.rgba\tactual.rgba\t2\t1\n",
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

    assert!(
        stdout.contains("entries=1\tviolations=1\tpassed=false"),
        "{stdout}"
    );
    assert!(review.join("review.md").exists());
    assert!(review.join("entries.tsv").exists());
    assert!(review.join("violations.tsv").exists());
    assert!(review.join("first-unmasked.tsv").exists());
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

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&path).unwrap();
    path
}
