use std::process::Command;

#[test]
fn reports_host_support_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("support-check")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let fields: Vec<_> = stdout.trim().split('\t').collect();

    assert_eq!(fields.len(), 6, "{stdout}");
    assert!(matches!(
        fields[0],
        "primary" | "compatible" | "unsupported"
    ));
    assert!(fields[1].split('.').count() == 3, "{stdout}");
    assert!(matches!(
        fields[3],
        "apple-silicon" | "intel64" | "unsupported"
    ));
    assert!(fields[4].parse::<u64>().unwrap() > 0);
    assert!(fields[5].parse::<u16>().unwrap() > 0);
}

#[test]
fn reports_permission_onboarding_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("permission-onboarding")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let mut lines = stdout.lines();
    let header = lines.next().expect("header line");
    let fields: Vec<_> = header.split('\t').collect();

    assert_eq!(fields.len(), 3, "{stdout}");
    assert!(matches!(
        fields[0],
        "continue-normally"
            | "continue-degraded"
            | "explain-full-disk-access"
            | "block-until-granted"
    ));
    assert_eq!(fields[1], "defer-until-needed", "{stdout}");
    assert_eq!(fields[2], "true", "{stdout}");
    assert!(lines.any(|line| line.starts_with("desktop\t")), "{stdout}");
}

#[test]
fn reports_ui_lifecycle_contract_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["ui-contract", "/tmp/gfm"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert_eq!(
        stdout.trim(),
        "window\tGFM\t/tmp/gfm\t1040x720\tmin=640x420\ttransparent-titlebar=true\tactivate=true\ttabs=gfm-main-window"
    );
}

#[test]
fn reports_preview_security_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["preview-check", "/tmp/example.app", "quick-look"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let fields: Vec<_> = stdout.trim().split('\t').collect();

    assert_eq!(fields.len(), 7, "{stdout}");
    assert_eq!(fields[0], "quick-look", "{stdout}");
    assert_eq!(fields[1], "untrusted", "{stdout}");
    assert_eq!(fields[2], "true", "{stdout}");
    assert_eq!(fields[4], "metadata-only", "{stdout}");
    assert_eq!(fields[5], "true", "{stdout}");
}

#[test]
fn reports_preview_scheduling_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("preview-schedule")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<_> = stdout.lines().collect();

    assert_eq!(lines.len(), 2, "{stdout}");
    assert_eq!(lines[0], "scheduled\tvisible", "{stdout}");
    assert_eq!(lines[1], "scheduled\tprefetch", "{stdout}");
}
