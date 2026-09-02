use gfm_preview::{PreviewCache, PreviewCacheConfig, PreviewEntry, PreviewKind, PreviewRequestKey};
use gfm_types::{FileId, VolumeId};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

fn assert_fileprovider_state_entry(state_text: &str, state: &str, path: &Path) {
    assert!(
        state_text.lines().any(|line| {
            line.starts_with(&format!("{state}\t"))
                && line.ends_with(&format!("\t{}", path.display()))
        }),
        "{state_text}"
    );
}

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
fn ui_permission_access_contract_reports_bookmark_prompt_orchestration_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-ui-permission-access-bookmark-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let documents = root.join("Documents");
    std::fs::create_dir_all(&documents).unwrap();
    let path = documents.join("Plan.md");
    std::fs::write(&path, "plan").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("HOME", &root)
        .arg("ui-permission-access-contract")
        .arg(&path)
        .arg("preview")
        .arg("preview worker")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(
        stdout.starts_with("dialog\tsurface=permission\tpresentation=window-sheet\t"),
        "{stdout}"
    );
    assert!(stdout.contains("permission-access\t"), "{stdout}");
    assert!(stdout.contains("\tprompt-kind=bookmark-acquisition\t"));
    assert!(stdout.contains("\tprompt-action=choose-location\t"));
    assert!(stdout.contains("\tpromptable=true\t"));
    assert!(stdout.contains("\tprompt-source=security-scoped-bookmark\t"));
    assert!(stdout.contains("\tbookmark-required=true\t"));
    assert!(stdout.contains("security-worker-admission\tworker=preview worker\t"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ui_permission_access_contract_reports_denied_protected_bookmark_orchestration_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-ui-permission-access-denied-bookmark-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let documents = root.join("Documents");
    std::fs::create_dir_all(&documents).unwrap();
    let path = documents.join("Private.md");
    std::fs::write(&path, "private").unwrap();
    let original_permissions = std::fs::metadata(&path).unwrap().permissions();
    let mut denied_permissions = original_permissions.clone();
    denied_permissions.set_mode(0o0);
    std::fs::set_permissions(&path, denied_permissions).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("HOME", &root)
        .arg("ui-permission-access-contract")
        .arg(&path)
        .arg("preview")
        .arg("preview worker")
        .output()
        .unwrap();
    let _ = std::fs::set_permissions(&path, original_permissions);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(
        stdout.starts_with("dialog\tsurface=permission\tpresentation=window-sheet\t"),
        "{stdout}"
    );
    assert!(
        stdout.contains("\tprompt-kind=bookmark-acquisition\t"),
        "{stdout}"
    );
    assert!(
        stdout.contains("\tprompt-action=choose-location\t"),
        "{stdout}"
    );
    assert!(stdout.contains("\tpromptable=true\t"), "{stdout}");
    assert!(
        stdout.contains("\tprompt-source=security-scoped-bookmark\t"),
        "{stdout}"
    );
    assert!(stdout.contains("\tprobe=denied\t"), "{stdout}");
    assert!(stdout.contains("\taccess-action=prompt\t"), "{stdout}");
    assert!(stdout.contains("\tworker-action=prompt\t"), "{stdout}");
    assert!(stdout.contains("\tbookmark-required=true\t"), "{stdout}");
    assert!(
        !stdout.contains("\tprompt-source=metadata-only\t"),
        "{stdout}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ui_permission_access_contract_reports_blocked_volume_orchestration_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-ui-permission-access-volume-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let path = root.join("Preview.pdf");
    std::fs::write(&path, "%PDF-1.7\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-permission-access-contract")
        .arg(&path)
        .arg("preview")
        .arg("preview worker")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(
        stdout.starts_with("dialog\tsurface=permission\tpresentation=window-sheet\t"),
        "{stdout}"
    );
    assert!(stdout.contains("permission-access\t"), "{stdout}");
    assert!(stdout.contains("\tprompt-kind=blocked\t"));
    assert!(stdout.contains("\tprompt-action=blocked-volume\t"));
    assert!(stdout.contains("\tpromptable=false\t"));
    assert!(stdout.contains("\tprompt-source=volume\t"));
    assert!(stdout.contains("\trefresh-on-permission-change=true\t"));
    assert!(stdout.contains("unreachable volume network"));
    assert!(stdout.contains("security-worker-admission\tworker=preview worker\t"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ui_permission_access_contract_reports_missing_path_orchestration_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-ui-permission-access-missing-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("Missing.pdf");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-permission-access-contract")
        .arg(&path)
        .arg("preview")
        .arg("preview worker")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(
        stdout.starts_with("dialog\tsurface=permission\tpresentation=window-sheet\t"),
        "{stdout}"
    );
    assert!(stdout.contains("\tprompt-kind=blocked\t"), "{stdout}");
    assert!(
        stdout.contains("\tprompt-action=blocked-missing-path\t"),
        "{stdout}"
    );
    assert!(stdout.contains("\tpromptable=false\t"), "{stdout}");
    assert!(
        stdout.contains("\tprompt-source=missing-path\t"),
        "{stdout}"
    );
    assert!(stdout.contains("\tprobe=missing\t"), "{stdout}");
    assert!(stdout.contains("\tworker-action=deny\t"), "{stdout}");
    assert!(
        stdout.contains("preview worker access is denied"),
        "{stdout}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ui_permission_access_contract_reports_unavailable_path_orchestration_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-ui-permission-access-unavailable-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("permission-path-unavailable".repeat(16));

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-permission-access-contract")
        .arg(&path)
        .arg("preview")
        .arg("preview worker")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(
        stdout.starts_with("dialog\tsurface=permission\tpresentation=window-sheet\t"),
        "{stdout}"
    );
    assert!(stdout.contains("\tprompt-kind=blocked\t"), "{stdout}");
    assert!(
        stdout.contains("\tprompt-action=blocked-unavailable\t"),
        "{stdout}"
    );
    assert!(stdout.contains("\tpromptable=false\t"), "{stdout}");
    assert!(stdout.contains("\tprompt-source=unavailable\t"), "{stdout}");
    assert!(stdout.contains("\tprobe=unavailable\t"), "{stdout}");
    assert!(stdout.contains("\tworker-action=deny\t"), "{stdout}");
    assert!(
        stdout.contains("preview worker access is denied"),
        "{stdout}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn persists_permission_invalidation_state_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-permission-invalidation-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("permission-state.tsv");

    let first = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("permission-invalidation")
        .arg(&state)
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first_stdout = String::from_utf8(first.stdout).unwrap();
    assert!(first_stdout.starts_with("permission-invalidation\tinitialized=true\t"));
    assert!(
        first_stdout.contains("\trefresh-ui=true\t"),
        "{first_stdout}"
    );
    assert!(
        first_stdout.contains("\npermission-change\tdesktop\tkind=initialized\t"),
        "{first_stdout}"
    );
    assert!(state.is_file());

    let second = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("permission-invalidation")
        .arg(&state)
        .output()
        .unwrap();
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let second_stdout = String::from_utf8(second.stdout).unwrap();
    assert!(second_stdout.starts_with("permission-invalidation\tinitialized=false\t"));
    assert!(second_stdout.contains("\tchanged=0\t"), "{second_stdout}");
    assert!(
        second_stdout.contains("\trefresh-ui=false\t"),
        "{second_stdout}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn permission_invalidation_refuses_duplicate_previous_scope_before_persisting_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-permission-invalidation-refresh-duplicate-scope-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("permission-state.tsv");
    let documents = root.join("Documents");
    std::fs::write(
        &state,
        format!(
            "gfm-permission-state-v1\ndocuments\tgranted\t{}\treadable\ndocuments\tdenied\t{}\tdenied\n",
            documents.display(),
            documents.display()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("permission-invalidation")
        .arg(&state)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("permission-invalidation\t"), "{stdout}");
    assert!(
        stderr.contains("duplicate permission state scope `documents`"),
        "{stderr}"
    );
    assert!(stderr.contains(":3"), "{stderr}");
    let state_text = std::fs::read_to_string(&state).unwrap();
    assert!(state_text.contains("documents\tdenied\t"), "{state_text}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn permission_invalidation_creates_nested_state_parent_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-permission-invalidation-parent-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("runtime").join("permission-state.tsv");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("permission-invalidation")
        .arg(&state)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("permission-invalidation\tinitialized=true\t"));
    assert!(state.is_file());
    assert!(std::fs::read_to_string(&state)
        .unwrap()
        .starts_with("gfm-permission-state-v1\n"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn permission_invalidation_surfaces_state_probe_failure_before_persisting_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-permission-invalidation-probe-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join(format!("{}.tsv", "permission-state-unavailable".repeat(16)));

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("permission-invalidation")
        .arg(&state)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("permission-invalidation\t"), "{stdout}");
    assert!(
        stderr.contains("permission state write probe unavailable"),
        "{stderr}"
    );
    assert!(stderr.contains(&state.display().to_string()), "{stderr}");
    assert!(!state.exists());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn permission_invalidation_refuses_unreachable_state_before_persisting_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-permission-invalidation-offline-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let state = root.join("permission-state-unavailable".repeat(16));

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("permission-invalidation")
        .arg(&state)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("permission-invalidation\t"), "{stdout}");
    assert!(
        stderr.contains("permission state volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("permission state write probe unavailable"),
        "{stderr}"
    );
    assert!(!state.exists());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn permission_invalidation_prefers_native_write_truth_over_read_only_marker_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-permission-invalidation-read-only-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join(".gfm-volume-kind"),
        "external-removable-read-only\n",
    )
    .unwrap();
    let state = root.join("permission-state.tsv");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("permission-invalidation")
        .arg(&state)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.starts_with("permission-invalidation\tinitialized=true\t"),
        "{stdout}"
    );
    assert!(
        !stderr.contains("permission state volume access blocked")
            && !stderr.contains("read-only volume external"),
        "{stderr}"
    );
    assert!(state.is_file());
    assert!(std::fs::read_to_string(&state)
        .unwrap()
        .starts_with("gfm-permission-state-v1\n"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn permission_invalidation_refuses_unavailable_volume_api_state_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-permission-invalidation-api-unavailable-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("runtime").join("permission-state.tsv");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("permission-invalidation-unavailable-volume-api")
        .arg(&state)
        .arg(&root)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("permission-invalidation\t"), "{stdout}");
    assert!(
        stderr.contains("permission state volume access blocked: unavailable volume network"),
        "{stderr}"
    );
    assert!(stderr.contains("native-status=unavailable"), "{stderr}");
    assert!(stderr.contains("native-reason=DiskArbitration"), "{stderr}");
    assert!(stderr.contains("resource-status=unavailable"), "{stderr}");
    assert!(stderr.contains("resource-reason="), "{stderr}");
    assert!(stderr.contains("mount-status=unavailable"), "{stderr}");
    assert!(stderr.contains("mount-reason="), "{stderr}");
    assert!(!state.exists());
    assert!(!state.parent().unwrap().exists());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn security_worker_admission_refuses_unavailable_volume_api_state_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-security-admission-api-unavailable-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("Preview.pdf");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("security-worker-admission-unavailable-volume-api")
        .arg("preview worker")
        .arg(&path)
        .arg(&root)
        .arg("preview")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("security-worker-admission\tworker=preview worker\t"));
    assert!(stdout.contains(&format!("\tpath={}\t", path.display())));
    assert!(stdout.contains("\tintent=preview\t"));
    assert!(stdout.contains("\tprobe=unavailable\t"));
    assert!(stdout.contains("\taccess-action=deny\tworker-action=deny\t"));
    assert!(stdout.contains("\tcan-touch-filesystem=false\t"));
    assert!(stdout.contains("\trefresh-on-permission-change=true\t"));
    assert!(stdout.contains("unavailable volume network"));
    assert!(stdout.contains("native-status=unavailable"));
    assert!(stdout.contains("resource-status=unavailable"));
    assert!(stdout.contains("mount-status=unavailable"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn permission_invalidation_compare_reports_removed_scope_as_unavailable_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-permission-invalidation-removed-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let previous = root.join("previous-permission-state.tsv");
    let current = root.join("current-permission-state.tsv");
    let documents = root.join("Documents");
    std::fs::write(
        &previous,
        format!(
            "gfm-permission-state-v1\ndocuments\tgranted\t{}\treadable\n",
            documents.display()
        ),
    )
    .unwrap();
    std::fs::write(&current, "gfm-permission-state-v1\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("permission-invalidation-compare")
        .arg(&previous)
        .arg(&current)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with(
        "permission-invalidation\tinitialized=false\tchanged=1\trefresh-ui=true\trefresh-workers=true\trefresh-operations=true\n"
    ));
    assert!(stdout.contains(
        "\npermission-change\tdocuments\tkind=removed\tprevious=granted\tcurrent=unavailable\t"
    ));
    assert!(
        stdout.contains("\treason=permission scope no longer reported by permission onboarding\n")
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn permission_invalidation_compare_reports_denied_and_unavailable_kinds_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-permission-invalidation-kinds-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let previous = root.join("previous-permission-state.tsv");
    let current = root.join("current-permission-state.tsv");
    let documents = root.join("Documents");
    let mail = root.join("Library/Mail");
    std::fs::create_dir_all(mail.parent().unwrap()).unwrap();
    std::fs::write(
        &previous,
        format!(
            "gfm-permission-state-v1\ndocuments\tmissing\t{}\tpath is not present on this host\nfull-disk-access\tdenied\t{}\tmacOS denied read access\n",
            documents.display(),
            mail.display()
        ),
    )
    .unwrap();
    std::fs::write(
        &current,
        format!(
            "gfm-permission-state-v1\ndocuments\tdenied\t{}\tmacOS denied read access\nfull-disk-access\tunavailable\t{}\tTCC service unavailable\n",
            documents.display(),
            mail.display()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("permission-invalidation-compare")
        .arg(&previous)
        .arg(&current)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with(
        "permission-invalidation\tinitialized=false\tchanged=2\trefresh-ui=true\trefresh-workers=true\trefresh-operations=true\n"
    ));
    assert!(stdout.contains(
        "\npermission-change\tdocuments\tkind=denied\tprevious=missing\tcurrent=denied\t"
    ));
    assert!(stdout.contains(
        "\npermission-change\tfull-disk-access\tkind=unavailable\tprevious=denied\tcurrent=unavailable\t"
    ));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn permission_invalidation_compare_refuses_duplicate_scope_state_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-permission-invalidation-duplicate-scope-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let previous = root.join("previous-permission-state.tsv");
    let current = root.join("current-permission-state.tsv");
    let documents = root.join("Documents");
    std::fs::write(
        &previous,
        format!(
            "gfm-permission-state-v1\ndocuments\tgranted\t{}\treadable\ndocuments\tdenied\t{}\tmacOS denied read access\n",
            documents.display(),
            documents.display()
        ),
    )
    .unwrap();
    std::fs::write(&current, "gfm-permission-state-v1\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("permission-invalidation-compare")
        .arg(&previous)
        .arg(&current)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("permission-invalidation\t"), "{stdout}");
    assert!(
        stderr.contains("duplicate permission state scope `documents`"),
        "{stderr}"
    );
    assert!(stderr.contains(":3"), "{stderr}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn permission_invalidation_compare_refuses_unreachable_volume_before_snapshot_read_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-permission-invalidation-compare-offline-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let previous = root.join("previous-permission-state.tsv");
    let current = root.join("current-permission-state.tsv");
    std::fs::write(&previous, "not a permission snapshot\n").unwrap();
    std::fs::write(&current, "not a permission snapshot\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("permission-invalidation-compare")
        .arg(&previous)
        .arg(&current)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("permission-invalidation\t"), "{stdout}");
    assert!(
        stderr.contains(
            "permission invalidation previous state volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert!(
        !stderr.contains("unsupported permission state header"),
        "{stderr}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_security_scoped_access_from_binary() {
    let root = std::env::temp_dir().join(format!("gfm-security-{}", std::process::id()));
    let unprotected = root.join("plain.md");
    let protected_named_temp = root.join("Documents").join("Plan.md");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(protected_named_temp.parent().unwrap()).unwrap();
    std::fs::write(&unprotected, "plain").unwrap();
    std::fs::write(&protected_named_temp, "plan").unwrap();

    let plain = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("security-scope")
        .arg(&unprotected)
        .arg("read")
        .output()
        .unwrap();
    assert!(
        plain.status.success(),
        "{}",
        String::from_utf8_lossy(&plain.stderr)
    );
    let plain_stdout = String::from_utf8(plain.stdout).unwrap();
    assert!(plain_stdout.starts_with("security-scope\t"));
    assert!(plain_stdout.contains("\tintent=read\tscope=none\tprobe=granted\t"));
    assert!(plain_stdout.contains("\tmode=plain-filesystem\taction=allow\t"));
    assert!(plain_stdout.contains("\tbookmark-required=false\t"));

    let named_temp = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("security-scope")
        .arg(&protected_named_temp)
        .arg("read")
        .output()
        .unwrap();
    assert!(
        named_temp.status.success(),
        "{}",
        String::from_utf8_lossy(&named_temp.stderr)
    );
    let named_temp_stdout = String::from_utf8(named_temp.stdout).unwrap();
    assert!(named_temp_stdout.contains("\tintent=read\tscope=none\tprobe=granted\t"));
    assert!(named_temp_stdout.contains("\tmode=plain-filesystem\taction=allow\t"));
    assert!(named_temp_stdout.contains("\tbookmark-required=false\t"));
    assert!(named_temp_stdout.contains("\tleast-privilege=true\t"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn security_scope_allows_missing_write_target_when_parent_is_writable_from_binary() {
    let root = std::env::temp_dir().join(format!("gfm-security-write-{}", std::process::id()));
    let path = root.join("New.md");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("security-scope")
        .arg(&path)
        .arg("write")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("security-scope\t"));
    assert!(stdout.contains("\tintent=write\tscope=none\tprobe=granted\t"));
    assert!(stdout.contains("\tmode=plain-filesystem\taction=allow\t"));
    assert!(stdout.contains("\tbookmark-required=false\t"));
    assert!(stdout.contains("\tcan-write=true\t"));
    assert!(!path.exists());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_security_worker_admission_from_binary() {
    let root = std::env::temp_dir().join(format!("gfm-security-worker-{}", std::process::id()));
    let path = root.join("plain.md");
    let missing = root.join("missing.md");
    let state = root.join("permission-state.tsv");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(&path, "plain").unwrap();
    seed_stale_permission_state(&state);

    let allowed = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_PERMISSION_STATE", &state)
        .arg("security-worker-admission")
        .arg("index worker")
        .arg(&path)
        .arg("index")
        .output()
        .unwrap();
    assert!(
        allowed.status.success(),
        "{}",
        String::from_utf8_lossy(&allowed.stderr)
    );
    let allowed_stdout = String::from_utf8(allowed.stdout).unwrap();
    let allowed_stderr = String::from_utf8(allowed.stderr).unwrap();
    assert!(allowed_stdout.starts_with("security-worker-admission\t"));
    assert!(allowed_stdout.contains("\tintent=index\tscope=none\tprobe=granted\t"));
    assert!(allowed_stdout.contains("\tworker-action=start\t"));
    assert!(allowed_stdout.contains("\tcan-touch-filesystem=true\t"));
    assert!(
        allowed_stderr.contains(
            "permission-refresh\taudience=workers\tsubject=index worker\tinitialized=false\tchanged=1\t",
        ),
        "{allowed_stderr}"
    );
    assert!(
        allowed_stderr.contains("\tfirst-change-scope=")
            && !allowed_stderr.contains("\tfirst-change-scope=-")
            && allowed_stderr.contains("\tfirst-change-kind=")
            && allowed_stderr.contains("\tfirst-change-current=")
            && allowed_stderr.contains("\tfirst-change-path=")
            && allowed_stderr.contains("\tfirst-change-reason="),
        "{allowed_stderr}"
    );

    let denied = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_PERMISSION_STATE", &state)
        .arg("security-worker-admission")
        .arg("preview worker")
        .arg(&missing)
        .arg("preview")
        .output()
        .unwrap();
    assert!(
        denied.status.success(),
        "{}",
        String::from_utf8_lossy(&denied.stderr)
    );
    let denied_stdout = String::from_utf8(denied.stdout).unwrap();
    let denied_stderr = String::from_utf8(denied.stderr).unwrap();
    assert!(denied_stdout.contains("\tintent=preview\tscope=none\tprobe=missing\t"));
    assert!(denied_stdout.contains("\tworker-action=deny\t"));
    assert!(denied_stdout.contains("\tcan-touch-filesystem=false\t"));
    assert!(
        !denied_stderr.contains("permission-refresh\taudience=workers\t"),
        "{denied_stderr}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_security_worker_admission_fanout_from_binary() {
    let root =
        std::env::temp_dir().join(format!("gfm-security-worker-fanout-{}", std::process::id()));
    let state_root = std::env::temp_dir().join(format!(
        "gfm-security-worker-fanout-state-{}",
        std::process::id()
    ));
    let state = state_root.join("permission-state.tsv");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&state_root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&state_root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    seed_stale_permission_state(&state);
    let root = std::fs::canonicalize(root).unwrap();
    let path = root.join("Preview.pdf");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_PERMISSION_STATE", &state)
        .arg("security-worker-admission-fanout")
        .arg(&path)
        .arg("index worker")
        .arg("index")
        .arg("preview worker")
        .arg("preview")
        .arg("thumbnail worker")
        .arg("preview")
        .arg("extraction worker")
        .arg("read")
        .arg("operation worker")
        .arg("operate")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();

    assert!(stdout.starts_with(
        "security-worker-admission-fanout\tworkers=5\tworker-families=index:1,preview:1,thumbnail:1,extraction:1,operation:1,other:0\tblocked-worker-families=index:1,preview:1,thumbnail:1,extraction:1,operation:1,other:0\tstart=0\tprompt=0\tmetadata-only=0\tdeny=5\t"
    ));
    assert!(stdout.contains("\tcan-touch-filesystem=0\t"));
    assert!(stdout.contains("\tbookmark-access=0\t"));
    assert!(stdout.contains("\trefresh-on-permission-change=5\t"));
    assert!(stdout.contains("\tany-blocked=true\t"));
    assert!(stdout.contains("\tall-blocked=true\t"));
    assert!(stdout.contains("\trefresh-required=true\t"));
    assert!(stdout.contains("\tfirst-blocked-worker=index worker\t"));
    assert!(stdout.contains("\tfirst-blocked-action=deny\t"));
    assert!(stdout.contains("\tfirst-blocked-scope=none\t"));
    assert!(stdout.contains("\tfirst-blocked-probe=unknown\t"));
    assert!(stdout.contains("\tfirst-blocked-reason="));
    assert!(stdout.contains("\tfirst-blocked-volume-id="), "{stdout}");
    assert!(
        stdout.contains("\tfirst-blocked-volume-class=network\t"),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!("\tfirst-blocked-volume-root={}\t", root.display())),
        "{stdout}"
    );
    assert!(stdout.contains("\tfirst-blocked-volume-label="), "{stdout}");
    assert!(
        !stdout.contains("\tfirst-blocked-volume-stable-id=-\t"),
        "{stdout}"
    );
    assert!(stdout.contains("\tfirst-refresh-worker=index worker\t"));
    assert!(stdout.contains("\tfirst-refresh-scope=none\n"));
    for (worker, intent) in [
        ("index worker", "index"),
        ("preview worker", "preview"),
        ("thumbnail worker", "preview"),
        ("extraction worker", "read"),
        ("operation worker", "operate"),
    ] {
        assert!(
            stdout.contains(&format!(
                "security-worker-admission\tworker={worker}\tpath={}",
                path.display()
            )),
            "{stdout}"
        );
        assert!(stdout.contains(&format!("\tintent={intent}\t")), "{stdout}");
    }
    assert_eq!(stdout.matches("\tprobe=unknown\t").count(), 5, "{stdout}");
    assert_eq!(
        stdout.matches("\tworker-action=deny\t").count(),
        5,
        "{stdout}"
    );
    assert_eq!(
        stdout.matches("\tcan-touch-filesystem=false\t").count(),
        5,
        "{stdout}"
    );
    assert_eq!(
        stdout.matches("unreachable volume network").count(),
        6,
        "{stdout}"
    );
    assert!(
        stderr.contains("permission-refresh\taudience=workers\tsubject=worker-admission-fanout;index worker:index;preview worker:preview;thumbnail worker:preview;extraction worker:read;operation worker:operate\tinitialized=false\tchanged=1\t"),
        "{stderr}"
    );
    assert!(
        stderr.contains("\tfirst-change-scope=")
            && !stderr.contains("\tfirst-change-scope=-")
            && stderr.contains("\tfirst-change-kind=")
            && stderr.contains("\tfirst-change-current=")
            && stderr.contains("\tfirst-change-path=")
            && stderr.contains("\tfirst-change-reason="),
        "{stderr}"
    );
    assert!(!stdout.contains("\tprobe=missing\t"), "{stdout}");

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(state_root);
}

#[test]
fn security_worker_admission_fanout_refuses_unavailable_volume_api_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-security-worker-fanout-unavailable-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let path = root.join("Missing.pdf");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("security-worker-admission-fanout-unavailable-volume-api")
        .arg(&path)
        .arg(&root)
        .arg("index worker")
        .arg("index")
        .arg("preview worker")
        .arg("preview")
        .arg("thumbnail worker")
        .arg("preview")
        .arg("extraction worker")
        .arg("read")
        .arg("operation worker")
        .arg("operate")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with(
        "security-worker-admission-fanout\tworkers=5\tworker-families=index:1,preview:1,thumbnail:1,extraction:1,operation:1,other:0\tblocked-worker-families=index:1,preview:1,thumbnail:1,extraction:1,operation:1,other:0\tstart=0\tprompt=0\tmetadata-only=0\tdeny=5\t"
    ));
    assert!(stdout.contains("\tcan-touch-filesystem=0\t"));
    assert!(stdout.contains("\trefresh-on-permission-change=5\t"));
    assert!(stdout.contains("\tany-blocked=true\t"));
    assert!(stdout.contains("\tall-blocked=true\t"));
    assert!(stdout.contains("\trefresh-required=true\t"));
    assert!(stdout.contains("\tfirst-blocked-worker=index worker\t"));
    assert!(stdout.contains("\tfirst-blocked-action=deny\t"));
    assert!(stdout.contains("\tfirst-blocked-scope=none\t"));
    assert!(stdout.contains("\tfirst-blocked-probe=unavailable\t"));
    assert!(stdout.contains("\tfirst-blocked-reason="));
    assert!(stdout.contains("\tfirst-blocked-volume-id="), "{stdout}");
    assert!(
        stdout.contains("\tfirst-blocked-volume-class=network\t"),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!("\tfirst-blocked-volume-root={}\t", root.display())),
        "{stdout}"
    );
    assert!(stdout.contains("\tfirst-blocked-volume-label="), "{stdout}");
    assert!(
        !stdout.contains("\tfirst-blocked-volume-stable-id=-\t"),
        "{stdout}"
    );
    assert!(stdout.contains("\tfirst-refresh-worker=index worker\t"));
    assert!(stdout.contains("\tfirst-refresh-scope=none\n"));
    assert_eq!(
        stdout.matches("\tprobe=unavailable\t").count(),
        5,
        "{stdout}"
    );
    assert_eq!(
        stdout.matches("\tworker-action=deny\t").count(),
        5,
        "{stdout}"
    );
    assert_eq!(
        stdout.matches("unavailable volume network").count(),
        6,
        "{stdout}"
    );
    assert_eq!(
        stdout.matches("native-status=unavailable").count(),
        6,
        "{stdout}"
    );
    assert_eq!(
        stdout.matches("resource-status=unavailable").count(),
        6,
        "{stdout}"
    );
    assert_eq!(
        stdout.matches("mount-status=unavailable").count(),
        6,
        "{stdout}"
    );
    assert!(!stdout.contains("\tprobe=unknown\t"), "{stdout}");
    assert!(!stdout.contains("\tprobe=missing\t"), "{stdout}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn security_scope_classifies_relative_home_paths_from_binary() {
    let root =
        std::env::temp_dir().join(format!("gfm-security-relative-home-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let documents = root.join("Documents");
    let protected = documents.join("Plan.md");
    std::fs::create_dir_all(&documents).unwrap();
    std::fs::write(&protected, "plan").unwrap();
    let protected = std::fs::canonicalize(&protected).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("HOME", &root)
        .current_dir(&root)
        .arg("security-scope")
        .arg("./Documents/../Documents/Plan.md")
        .arg("read")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.starts_with(&format!("security-scope\t{}", protected.display())),
        "{stdout}"
    );
    assert!(stdout.contains("\tintent=read\tscope=documents\tprobe=granted\t"));
    assert!(stdout.contains("\tmode=security-scoped-bookmark\taction=allow\t"));
    assert!(stdout.contains("\tbookmark-required=true\t"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn security_worker_admission_refuses_unreachable_volume_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-security-worker-unreachable-volume-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let path = root.join("Preview.pdf");
    std::fs::write(&path, "%PDF-1.7\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("security-worker-admission")
        .arg("preview worker")
        .arg(&path)
        .arg("preview")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("security-worker-admission\t"));
    assert!(stdout.contains("\tintent=preview\tscope=none\tprobe=unknown\t"));
    assert!(stdout.contains("\taccess-action=deny\tworker-action=deny\t"));
    assert!(stdout.contains("\tcan-touch-filesystem=false\t"));
    assert!(stdout.contains("\tbookmark-access=false\t"));
    assert!(stdout.contains("\trefresh-on-permission-change=true\t"));
    assert!(stdout.contains("preview worker volume access blocked"));
    assert!(stdout.contains("unreachable volume network"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn security_worker_admission_prefers_native_write_truth_over_read_only_marker_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-security-worker-read-only-volume-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join(".gfm-volume-kind"),
        "external-removable-read-only\n",
    )
    .unwrap();
    let path = root.join("Export.pdf");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("security-worker-admission")
        .arg("export worker")
        .arg(&path)
        .arg("write")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("security-worker-admission\t"));
    assert!(stdout.contains("\tintent=write\tscope=none\tprobe=granted\t"));
    assert!(stdout.contains("\tmode=plain-filesystem\t"));
    assert!(stdout.contains("\taccess-action=allow\tworker-action=start\t"));
    assert!(stdout.contains("\tcan-touch-filesystem=true\t"));
    assert!(stdout.contains("\trefresh-on-permission-change=false\t"));
    assert!(stdout.contains("export worker may start with filesystem access"));
    assert!(!stdout.contains("read-only volume external"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn protected_worker_route_fails_closed_without_retained_bookmark_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-protected-worker-missing-bookmark-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let protected = root.join("Documents").join("Preview.md");
    std::fs::create_dir_all(protected.parent().unwrap()).unwrap();
    std::fs::write(&protected, "protected preview").unwrap();
    let bookmarks = root.join("bookmarks.tsv");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("HOME", &root)
        .env("GFM_SECURITY_BOOKMARKS", &bookmarks)
        .arg("native-icon")
        .arg(&protected)
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("security-worker-admission\tworker=native icon\t"));
    assert!(stderr.contains("\tscope=documents\t"));
    assert!(stderr.contains("\tbookmark-access=true\t"));
    assert!(stderr.contains("\trefresh-on-permission-change=true\t"));
    assert!(stderr.contains("security-scope-access\t"));
    assert!(stderr.contains("\tstatus=missing\t"));
    assert!(
        stderr.contains("retained security-scoped bookmark required before touching filesystem")
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn quicklook_refuses_missing_path_before_preview_from_binary() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "gfm-quicklook-missing-{}-{}.pdf",
        std::process::id(),
        nanos
    ));

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("quicklook-session")
        .arg(&path)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("security-scope\t"), "{stderr}");
    assert!(stderr.contains("\tintent=preview\t"), "{stderr}");
    assert!(stderr.contains("\taction=deny\t"), "{stderr}");
    assert!(
        stderr.contains("quicklook preview access blocked: path is not present on this host"),
        "{stderr}"
    );
}

#[test]
fn thumbnail_generation_refuses_missing_path_before_generation_from_binary() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "gfm-thumbnail-missing-{}-{}.png",
        std::process::id(),
        nanos
    ));

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("thumbnail-generation")
        .arg(&path)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("security-scope\t"), "{stderr}");
    assert!(stderr.contains("security-worker-admission\tworker=thumbnail generation\t"));
    assert!(stderr.contains("\tintent=preview\t"), "{stderr}");
    assert!(stderr.contains("\tprobe=missing\t"), "{stderr}");
    assert!(stderr.contains("\taction=deny\t"), "{stderr}");
    assert!(stderr.contains("\tworker-action=deny\t"), "{stderr}");
    assert!(
        stderr.contains("\tcan-touch-filesystem=false\t"),
        "{stderr}"
    );
    assert!(
        stderr.contains("thumbnail generation access blocked: path is not present on this host"),
        "{stderr}"
    );
}

#[test]
fn quicklook_session_retries_transient_preview_failure_from_binary() {
    let root =
        std::env::temp_dir().join(format!("gfm-quicklook-retry-probe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let document = root.join("Retry.pdf");
    let retry_probe = root.join("quicklook-retry.state");
    let journal = root.join("jobs.gfmjournal");
    std::fs::write(&document, b"%PDF-1.7\nretry quicklook").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .arg("quicklook-session-retry-probe")
        .arg(&document)
        .arg(&retry_probe)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.starts_with("quicklook-session\tquick-look\t"),
        "{stdout}"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains(&format!(
            "preview-volume-access\tworker=quicklook preview\tpath={}\tintent=preview",
            document.display()
        )) && stderr.contains("\tstable-id=")
            && !stderr.contains("\tstable-id=-\t")
            && stderr.contains("\twritable=")
            && stderr.contains("\treason=cached-volume-report"),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "preview-retry-volume-access\tworker=quicklook preview\tpath={}\tintent=write",
            retry_probe.parent().unwrap().display()
        )) && stderr.contains("\tstable-id=")
            && !stderr.contains("\tstable-id=-\t")
            && stderr.contains("\twritable=")
            && stderr.contains("\treason=cached-volume-report"),
        "{stderr}"
    );
    assert_eq!(std::fs::read_to_string(&retry_probe).unwrap(), "2");
    let journal_text = std::fs::read_to_string(&journal).unwrap();
    assert!(
        journal_text.contains("1\t1\tstarted\tquicklook preview"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains(
            "1\t1\tfailed:temporary quicklook preview retry probe busy\tquicklook preview"
        ),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tstarted\tquicklook preview"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tcompleted\tquicklook preview"),
        "{journal_text}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn thumbnail_generation_retries_transient_preview_failure_from_binary() {
    let root =
        std::env::temp_dir().join(format!("gfm-thumbnail-retry-probe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let image = root.join("Retry.png");
    let retry_probe = root.join("thumbnail-retry.state");
    let journal = root.join("jobs.gfmjournal");
    std::fs::write(&image, b"\x89PNG\r\n\x1a\nretry thumbnail").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .arg("thumbnail-generation-retry-probe")
        .arg(&image)
        .arg(&retry_probe)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("thumbnail-generation\t"), "{stdout}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains(&format!(
            "preview-volume-access\tworker=thumbnail generation\tpath={}\tintent=preview",
            image.display()
        )) && stderr.contains("\tstable-id=")
            && !stderr.contains("\tstable-id=-\t")
            && stderr.contains("\twritable=")
            && stderr.contains("\treason=cached-volume-report"),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "preview-retry-volume-access\tworker=thumbnail generation\tpath={}\tintent=write",
            retry_probe.parent().unwrap().display()
        )) && stderr.contains("\tstable-id=")
            && !stderr.contains("\tstable-id=-\t")
            && stderr.contains("\treason=cached-volume-report"),
        "{stderr}"
    );
    assert_eq!(std::fs::read_to_string(&retry_probe).unwrap(), "2");
    let journal_text = std::fs::read_to_string(&journal).unwrap();
    assert!(
        journal_text.contains("1\t1\tstarted\tthumbnail generation"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains(
            "1\t1\tfailed:temporary thumbnail generation retry probe busy\tthumbnail generation"
        ),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tstarted\tthumbnail generation"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tcompleted\tthumbnail generation"),
        "{journal_text}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_mac_bridge_contract_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("mac-bridges")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("mac-bridges\timplemented=8\trequired=4\ttotal=12"));
    assert!(stdout.contains(
        "bridge\tfoundation-host-profile\tfoundation\tcrates/mac\tsw-vers-uname-sysctl-host-profile\tbackground-safe\timplemented"
    ));
    assert!(stdout.contains(
        "bridge\tfsevents-file-event-stream\tfile-events\tcrates/mac\ttyped-create-modify-remove-rename-rescan-events\tdedicated-worker\timplemented"
    ));
    assert!(stdout.contains(
        "bridge\tlaunchservices-icons-and-packages\tlaunchservices\tcrates/mac\tnative-icons-bundle-identities-package-classification\tbackground-safe\timplemented"
    ));
}

#[test]
fn reports_native_icon_descriptor_from_binary() {
    let root = std::env::temp_dir().join(format!("gfm-native-icon-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("GFM.app")).unwrap();
    std::fs::write(root.join("Report.PDF"), "pdf").unwrap();

    let app = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("native-icon")
        .arg(root.join("GFM.app"))
        .output()
        .unwrap();
    assert!(
        app.status.success(),
        "{}",
        String::from_utf8_lossy(&app.stderr)
    );
    let app_stderr = String::from_utf8_lossy(&app.stderr);
    assert_worker_admitted(&app_stderr, "native icon", &root.join("GFM.app"));
    assert!(
        app_stderr.contains(&format!(
            "preview-volume-access\tworker=native icon\tpath={}\tintent=preview",
            root.join("GFM.app").display()
        )) && app_stderr.contains("\tstable-id=")
            && !app_stderr.contains("\tstable-id=-\t")
            && app_stderr.contains("\twritable=")
            && app_stderr.contains("\treason=cached-volume-report"),
        "{app_stderr}"
    );
    let app_stdout = String::from_utf8(app.stdout).unwrap();
    assert_eq!(
        app_stdout.trim(),
        "native-icon\tapplication\tlaunchservices-application-icon\tcom.apple.application-bundle\tapplication:com.apple.application-bundle:package\tbadges=package"
    );

    let document = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("native-icon")
        .arg(root.join("Report.PDF"))
        .output()
        .unwrap();
    assert!(
        document.status.success(),
        "{}",
        String::from_utf8_lossy(&document.stderr)
    );
    let document_stderr = String::from_utf8_lossy(&document.stderr);
    assert_worker_admitted(&document_stderr, "native icon", &root.join("Report.PDF"));
    assert!(
        document_stderr.contains(&format!(
            "preview-volume-access\tworker=native icon\tpath={}\tintent=preview",
            root.join("Report.PDF").display()
        )) && document_stderr.contains("\tstable-id=")
            && !document_stderr.contains("\tstable-id=-\t")
            && document_stderr.contains("\twritable=")
            && document_stderr.contains("\treason=cached-volume-report"),
        "{document_stderr}"
    );
    let document_stdout = String::from_utf8(document.stdout).unwrap();
    assert_eq!(
        document_stdout.trim(),
        "native-icon\tdocument\tlaunchservices-document-icon\textension:pdf\tdocument:extension:pdf\tbadges="
    );

    let bridge = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("native-icon-bridge")
        .arg(root.join("Report.PDF"))
        .output()
        .unwrap();
    assert!(
        bridge.status.success(),
        "{}",
        String::from_utf8_lossy(&bridge.stderr)
    );
    let bridge_stderr = String::from_utf8_lossy(&bridge.stderr);
    assert_worker_admitted(
        &bridge_stderr,
        "native icon bridge",
        &root.join("Report.PDF"),
    );
    assert!(
        bridge_stderr.contains(&format!(
            "preview-volume-access\tworker=native icon bridge\tpath={}\tintent=preview",
            root.join("Report.PDF").display()
        )) && bridge_stderr.contains("\tstable-id=")
            && !bridge_stderr.contains("\tstable-id=-\t")
            && bridge_stderr.contains("\twritable=")
            && bridge_stderr.contains("\treason=cached-volume-report"),
        "{bridge_stderr}"
    );
    let bridge_stdout = String::from_utf8(bridge.stdout).unwrap();
    assert!(bridge_stdout.starts_with(
        "native-icon-bridge\tlaunchservices\tbackground-safe\tlaunchservices-document-icon\tdocument:extension:pdf\t"
    ));
    assert!(bridge_stdout.contains("\tdecision=use-native-bridge\t"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn native_icon_refuses_unreachable_network_volume_before_record_read_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-native-icon-unreachable-volume-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let path = root.join("Report.pdf");
    std::fs::write(&path, "pdf").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("native-icon")
        .arg(&path)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("native-icon\t"), "{stdout}");
    assert!(
        stderr.contains("native icon volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("security-worker-admission\tworker=native icon\t"),
        "{stderr}"
    );

    let bridge = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("native-icon-bridge")
        .arg(&path)
        .output()
        .unwrap();

    assert!(!bridge.status.success());
    let bridge_stdout = String::from_utf8_lossy(&bridge.stdout);
    let bridge_stderr = String::from_utf8_lossy(&bridge.stderr);
    assert!(
        !bridge_stdout.contains("native-icon-bridge\t"),
        "{bridge_stdout}"
    );
    assert!(
        bridge_stderr
            .contains("native icon bridge volume access blocked: unreachable volume network"),
        "{bridge_stderr}"
    );
    assert!(
        !bridge_stderr.contains("security-worker-admission\tworker=native icon bridge\t"),
        "{bridge_stderr}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_icloud_badges_in_native_icon_descriptor_from_binary() {
    let root = std::env::temp_dir().join(format!("gfm-native-icon-cloud-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let evicted = root.join("Remote.icloud-placeholder");
    let downloading = root.join("Asset.icloud-downloading.png");
    std::fs::write(&evicted, "placeholder").unwrap();
    mark_evicted_fixture(&evicted);
    std::fs::write(&downloading, "downloading").unwrap();
    xattr::set(&downloading, "com.apple.fileprovider.state", b"downloading").unwrap();

    let evicted_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("native-icon")
        .arg(&evicted)
        .output()
        .unwrap();
    assert!(
        evicted_output.status.success(),
        "{}",
        String::from_utf8_lossy(&evicted_output.stderr)
    );
    let evicted_stdout = String::from_utf8(evicted_output.stdout).unwrap();
    assert!(
        evicted_stdout.contains("\tbadges=cloud\n"),
        "{evicted_stdout}"
    );

    let downloading_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("native-icon")
        .arg(&downloading)
        .output()
        .unwrap();
    assert!(
        downloading_output.status.success(),
        "{}",
        String::from_utf8_lossy(&downloading_output.stderr)
    );
    let downloading_stdout = String::from_utf8(downloading_output.stdout).unwrap();
    assert!(
        downloading_stdout.contains("\tbadges=cloud,cloud-downloading\n"),
        "{downloading_stdout}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_native_icon_fileprovider_invalidation_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-native-icon-fileprovider-invalidation-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let evicted = root.join("Remote.icloud-placeholder");
    std::fs::write(&evicted, "placeholder").unwrap();
    mark_evicted_fixture(&evicted);

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("native-icon-fileprovider-invalidation")
        .arg("downloaded")
        .arg(&evicted)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(&stderr, "native icon fileprovider", &evicted);
    assert!(stdout.starts_with("native-icon-invalidation\t"));
    assert!(stdout.contains("\tprevious=downloaded\tcurrent=evicted\t"));
    assert!(stdout.contains("\tprevious-badges=cloud-available-offline\tcurrent-badges=cloud\t"));
    assert!(stdout.contains(
        "\tprevious-cache=fileprovider:downloaded:cloud-available-offline\tcurrent-cache=fileprovider:evicted:cloud\t"
    ));
    assert!(stdout.ends_with("invalidate-cache=true\treason=native-icon-badges-changed\n"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_custom_finder_icon_descriptor_from_binary() {
    const FINDER_INFO_XATTR: &str = "com.apple.FinderInfo";
    const FINDER_FLAG_CUSTOM_ICON: u16 = 0x0400;

    let root = std::env::temp_dir().join(format!("gfm-custom-native-icon-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let app = root.join("Custom.app");
    std::fs::create_dir_all(&app).unwrap();
    let mut finder_info = [0u8; 32];
    finder_info[8..10].copy_from_slice(&FINDER_FLAG_CUSTOM_ICON.to_be_bytes());
    xattr::set(&app, FINDER_INFO_XATTR, &finder_info).unwrap();

    let native = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("native-icon")
        .arg(&app)
        .output()
        .unwrap();
    assert!(
        native.status.success(),
        "{}",
        String::from_utf8_lossy(&native.stderr)
    );
    let native_stderr = String::from_utf8_lossy(&native.stderr);
    assert_worker_admitted(&native_stderr, "native icon", &app);
    let native_stdout = String::from_utf8(native.stdout).unwrap();
    assert!(native_stdout.starts_with("native-icon\tapplication\tfinder-custom-icon\t"));
    assert!(native_stdout.contains("\tbadges=package"));
    assert!(native_stdout.contains("\tcustom:"));

    let preview = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("icon-preview")
        .arg(&app)
        .output()
        .unwrap();
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stderr)
    );
    let preview_stdout = String::from_utf8(preview.stdout).unwrap();
    assert!(preview_stdout.contains("\tfinder-custom-icon\t"));
    assert!(preview_stdout.contains("\tbadges=package\t"));
    assert!(preview_stdout.contains("\tcache=refresh-memory-only\t"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn native_icon_and_icon_preview_report_volume_badges_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-native-icon-volume-badge-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "network-smb\n").unwrap();

    let native = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("native-icon")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        native.status.success(),
        "{}",
        String::from_utf8_lossy(&native.stderr)
    );
    let native_stderr = String::from_utf8_lossy(&native.stderr);
    assert_worker_admitted(&native_stderr, "native icon", &root);
    let native_stdout = String::from_utf8(native.stdout).unwrap();
    assert!(native_stdout.starts_with("native-icon\tfolder\tlaunchservices-folder-icon\t"));
    assert!(
        native_stdout.contains("\tfolder:public.folder:volume-network\t"),
        "{native_stdout}"
    );
    assert!(
        native_stdout.contains("\tbadges=volume-network\n"),
        "{native_stdout}"
    );

    let bridge = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("native-icon-bridge")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        bridge.status.success(),
        "{}",
        String::from_utf8_lossy(&bridge.stderr)
    );
    let bridge_stdout = String::from_utf8(bridge.stdout).unwrap();
    assert!(bridge_stdout.starts_with(
        "native-icon-bridge\tlaunchservices\tbackground-safe\tlaunchservices-folder-icon\t"
    ));
    assert!(
        bridge_stdout.contains("folder:public.folder:volume-network"),
        "{bridge_stdout}"
    );

    let preview = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("icon-preview")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stderr)
    );
    let preview_stderr = String::from_utf8_lossy(&preview.stderr);
    assert_worker_admitted(&preview_stderr, "icon preview", &root);
    let preview_stdout = String::from_utf8(preview.stdout).unwrap();
    assert!(preview_stdout.starts_with("icon-preview\t"));
    assert!(
        preview_stdout.contains("\tbadges=volume-network\t"),
        "{preview_stdout}"
    );
    assert!(
        preview_stdout.contains("\tcache=refresh-memory-only\t"),
        "{preview_stdout}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_spotlight_reconciliation_from_binary() {
    let root = std::env::temp_dir().join(format!("gfm-spotlight-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("Primary.md");
    let fixture = root.join("spotlight.tsv");
    std::fs::write(&path, "spotlight body").unwrap();
    std::fs::write(
        &fixture,
        "kMDItemDisplayName\tStale.md\nkMDItemKind\tMarkdown Document\nkMDItemFinderComment\tclient handoff\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("spotlight-reconcile")
        .arg(&path)
        .arg(&fixture)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(&stderr, "spotlight reconcile", &path);
    assert_worker_admitted(&stderr, "spotlight fixture", &fixture);
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("spotlight-reconciliation\t"));
    assert!(stdout.contains("\tprimary=filesystem\tspotlight=available\t"));
    assert!(stdout.contains("\tenrichments=2\tconflicts=1\t"));
    assert!(stdout.contains(
        "field\tdisplay-name\tprimary=Primary.md\tspotlight=Stale.md\tdecision=conflict-primary-wins"
    ));
    assert!(stdout.contains(
        "field\tfinder-comment\tprimary=-\tspotlight=client handoff\tdecision=enrich-from-spotlight"
    ));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn spotlight_reconcile_refuses_unreachable_network_volume_before_record_read_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-spotlight-unreachable-volume-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let path = root.join("Primary.md");
    let fixture = root.join("spotlight.tsv");
    std::fs::write(&path, "spotlight body").unwrap();
    std::fs::write(&fixture, "kMDItemDisplayName\tPrimary.md\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("spotlight-reconcile")
        .arg(&path)
        .arg(&fixture)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("spotlight-reconciliation\t"), "{stdout}");
    assert!(
        stderr.contains("spotlight reconcile volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(!stderr.contains("security-worker-admission\t"), "{stderr}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_icon_preview_contract_from_binary() {
    let root = std::env::temp_dir().join(format!("gfm-icon-preview-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("GFM.app")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("icon-preview")
        .arg(root.join("GFM.app"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(&stderr, "icon preview", &root.join("GFM.app"));
    assert!(
        stderr.contains(&format!(
            "preview-volume-access\tworker=icon preview\tpath={}\tintent=preview",
            root.join("GFM.app").display()
        )) && stderr.contains("\tstable-id=")
            && !stderr.contains("\tstable-id=-\t")
            && stderr.contains("\twritable=")
            && stderr.contains("\treason=cached-volume-report"),
        "{stderr}"
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout.trim(),
        format!(
            "icon-preview\t{}\tapplication\tlaunchservices-application-icon\tcom.apple.application-bundle\tbadges=package\tcache=refresh-memory-only\tinvalidate-memory=true\tinvalidate-disk=false",
            root.join("GFM.app").display()
        )
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn icon_preview_retries_transient_preview_failure_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-icon-preview-retry-probe-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("GFM.app")).unwrap();
    let app = root.join("GFM.app");
    let retry_probe = root.join("icon-preview-retry.state");
    let journal = root.join("jobs.gfmjournal");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .arg("icon-preview-retry-probe")
        .arg(&app)
        .arg(&retry_probe)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("icon-preview\t"), "{stdout}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains(&format!(
            "preview-volume-access\tworker=icon preview\tpath={}\tintent=preview",
            app.display()
        )) && stderr.contains("\tstable-id=")
            && !stderr.contains("\tstable-id=-\t")
            && stderr.contains("\twritable=")
            && stderr.contains("\treason=cached-volume-report"),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "preview-retry-volume-access\tworker=icon preview\tpath={}\tintent=write",
            retry_probe.parent().unwrap().display()
        )) && stderr.contains("\tstable-id=")
            && !stderr.contains("\tstable-id=-\t")
            && stderr.contains("\twritable=")
            && stderr.contains("\treason=cached-volume-report"),
        "{stderr}"
    );
    assert_eq!(std::fs::read_to_string(&retry_probe).unwrap(), "2");
    let journal_text = std::fs::read_to_string(&journal).unwrap();
    assert!(
        journal_text.contains("1\t1\tstarted\ticon preview"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t1\tfailed:temporary icon preview retry probe busy\ticon preview"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tstarted\ticon preview"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tcompleted\ticon preview"),
        "{journal_text}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn preview_volume_check_uses_descriptor_remote_truth_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-preview-volume-check-network-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "network\n").unwrap();
    let path = root.join("Installer.dmg");
    std::fs::write(&path, "disk image fixture").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("preview-volume-check")
        .arg(&path)
        .arg("quick-look")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("quick-look\tuntrusted\tfalse\ttrue\tdeny\ttrue\t"));
    assert!(stdout.contains(&path.display().to_string()), "{stdout}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn preview_volume_check_refuses_unreachable_volume_before_security_report_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-preview-volume-check-unreachable-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let path = root.join("Preview.pdf");
    std::fs::write(&path, "%PDF-1.7\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("preview-volume-check")
        .arg(&path)
        .arg("quick-look")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("quick-look\t"), "{stdout}");
    assert!(
        stderr.contains("preview volume check volume access blocked: unreachable volume network"),
        "{stderr}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn preview_volume_scheduling_throttles_network_prefetch_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-preview-volume-scheduling-network-prefetch-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "network-smb\n").unwrap();
    let path = root.join("Image.png");
    std::fs::write(&path, "png fixture").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("preview-volume-scheduling")
        .arg(&path)
        .arg("thumbnail")
        .arg("nominal")
        .arg("nominal")
        .arg("ac")
        .arg("idle")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("preview-volume-scheduling\tkind=thumbnail\t"));
    assert!(stdout.contains("\tvolume-kind=network\tremote=true\tslow=false\t"));
    assert!(
        stdout.contains("\tvolume-id=") && !stdout.contains("\tvolume-id=-\t"),
        "{stdout}"
    );
    assert!(
        stdout.contains("\tstable-id=fixture-marker:network-smb:dev:"),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!("\tvolume-root={}\t", root.display())),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "\tvolume-label={}\t",
            root.file_name().unwrap().to_string_lossy()
        )),
        "{stdout}"
    );
    assert!(stdout.contains("\tmax-visible=8\tmax-prefetch=0\t"));
    assert!(stdout.contains("\tcancel-offscreen=true\n"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn preview_volume_scheduling_refuses_unreachable_volume_before_policy_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-preview-volume-scheduling-unreachable-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let path = root.join("Preview.pdf");
    std::fs::write(&path, "%PDF-1.7\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("preview-volume-scheduling")
        .arg(&path)
        .arg("quick-look")
        .args(["nominal", "nominal", "ac", "idle"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("preview-volume-scheduling\t"), "{stdout}");
    assert!(
        stderr.contains(
            "preview volume scheduling volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn quicklook_and_thumbnail_generation_deny_descriptor_remote_untrusted_preview_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-preview-generation-network-security-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "network-smb\n").unwrap();
    let path = root.join("Installer.dmg");
    std::fs::write(&path, "disk image fixture").unwrap();

    let quicklook = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("quicklook-session")
        .arg(&path)
        .output()
        .unwrap();

    assert!(
        quicklook.status.success(),
        "{}",
        String::from_utf8_lossy(&quicklook.stderr)
    );
    let quicklook_stdout = String::from_utf8(quicklook.stdout).unwrap();
    assert!(
        quicklook_stdout.contains("\tdeny\tcloud=native-eligible\tdenied\t"),
        "{quicklook_stdout}"
    );
    assert!(
        quicklook_stdout.ends_with("schedule=cancelled:denied\n"),
        "{quicklook_stdout}"
    );

    let thumbnail = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("thumbnail-generation")
        .arg(&path)
        .output()
        .unwrap();

    assert!(
        thumbnail.status.success(),
        "{}",
        String::from_utf8_lossy(&thumbnail.stderr)
    );
    let thumbnail_stdout = String::from_utf8(thumbnail.stdout).unwrap();
    assert!(
        thumbnail_stdout.contains("\tdeny\tcloud=native-eligible\tdenied\t512px\t"),
        "{thumbnail_stdout}"
    );
    assert!(
        thumbnail_stdout.contains("\tcache=bypass\t"),
        "{thumbnail_stdout}"
    );
    assert!(
        thumbnail_stdout.ends_with("schedule=cancelled:denied\n"),
        "{thumbnail_stdout}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn adaptive_preview_generation_deny_descriptor_remote_untrusted_preview_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-adaptive-preview-generation-network-security-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "network-smb\n").unwrap();
    let path = root.join("Installer.dmg");
    std::fs::write(&path, "disk image fixture").unwrap();

    let quicklook = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "quicklook-session-adaptive",
            path.to_str().unwrap(),
            "nominal",
            "nominal",
            "ac",
            "idle",
        ])
        .output()
        .unwrap();

    assert!(
        quicklook.status.success(),
        "{}",
        String::from_utf8_lossy(&quicklook.stderr)
    );
    let quicklook_stdout = String::from_utf8(quicklook.stdout).unwrap();
    assert!(
        quicklook_stdout.contains("\tdeny\tcloud=native-eligible\tdenied\t"),
        "{quicklook_stdout}"
    );
    assert!(
        quicklook_stdout.contains("\tschedule=cancelled:denied\t"),
        "{quicklook_stdout}"
    );
    assert!(quicklook_stdout.ends_with("action=Run\tdeferred=false\n"));

    let thumbnail = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "thumbnail-generation-adaptive",
            path.to_str().unwrap(),
            "nominal",
            "nominal",
            "ac",
            "idle",
        ])
        .output()
        .unwrap();

    assert!(
        thumbnail.status.success(),
        "{}",
        String::from_utf8_lossy(&thumbnail.stderr)
    );
    let thumbnail_stdout = String::from_utf8(thumbnail.stdout).unwrap();
    assert!(
        thumbnail_stdout.contains("\tdeny\tcloud=native-eligible\tdenied\t512px\t"),
        "{thumbnail_stdout}"
    );
    assert!(
        thumbnail_stdout.contains("\tschedule=cancelled:denied\t"),
        "{thumbnail_stdout}"
    );
    assert!(thumbnail_stdout.ends_with("action=Run\tdeferred=false\n"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn preview_volume_scheduling_disables_prefetch_on_network_volume_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-preview-volume-scheduling-network-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "network\n").unwrap();
    let path = root.join("Preview.png");
    std::fs::write(&path, "image fixture").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("preview-volume-scheduling")
        .arg(&path)
        .arg("thumbnail")
        .arg("nominal")
        .arg("fair")
        .arg("ac")
        .arg("idle")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("preview-volume-scheduling\tkind=thumbnail\t"));
    assert!(stdout.contains("\tvolume-kind=network\tremote=true\tslow=false\t"));
    assert!(
        stdout.contains("\tvolume-id=") && !stdout.contains("\tvolume-id=-\t"),
        "{stdout}"
    );
    assert!(
        stdout.contains("\tstable-id=fixture-marker:network:dev:"),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!("\tvolume-root={}\t", root.display())),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "\tvolume-label={}\t",
            root.file_name().unwrap().to_string_lossy()
        )),
        "{stdout}"
    );
    assert!(stdout.contains("\tmax-visible=8\tmax-prefetch=0\tcancel-offscreen=true\n"));
    assert!(
        stdout.contains(&format!("\tpath={}\t", path.display())),
        "{stdout}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn icon_preview_refuses_unreachable_network_volume_before_record_read_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-icon-preview-unreachable-volume-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let path = root.join("GFM.app");
    std::fs::create_dir_all(&path).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("icon-preview")
        .arg(&path)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("icon-preview\t"), "{stdout}");
    assert!(
        stderr.contains("icon preview volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("security-worker-admission\tworker=icon preview\t"),
        "{stderr}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn spotlight_reconcile_refuses_unreachable_fixture_before_fixture_read_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-spotlight-fixture-unreachable-{}",
        std::process::id()
    ));
    let offline = std::env::temp_dir().join(format!(
        "gfm-spotlight-fixture-offline-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&offline);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&offline).unwrap();
    std::fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let path = root.join("Primary.md");
    let fixture = offline.join("spotlight.tsv");
    std::fs::write(&path, "spotlight body").unwrap();
    std::fs::write(&fixture, "kMDItemDisplayName\tPrimary.md\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("spotlight-reconcile")
        .arg(&path)
        .arg(&fixture)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("spotlight-reconciliation\t"), "{stdout}");
    assert!(
        stderr.contains("spotlight fixture volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("security-worker-admission\tworker=spotlight reconcile\t")
            && !stderr.contains("security-worker-admission\tworker=spotlight fixture\t"),
        "{stderr}"
    );

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(offline);
}

#[test]
fn reports_fileprovider_state_from_binary() {
    let root = std::env::temp_dir().join(format!("gfm-fileprovider-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let downloaded = root.join("Downloaded.icloud.md");
    let suffix_only = root.join("SuffixOnly.icloud");
    let evicted = root.join("Evicted.icloud-placeholder");
    let value_evicted = root.join("ValueEvicted.icloud.md");
    let value_current = root.join("Remote.icloud-placeholder");
    let value_downloaded_false = root.join("DownloadedFalse.icloud.md");
    let value_conflict_false = root.join("Clean.icloud.md");
    let value_false_prefix = root.join("FalsePrefix.icloud.md");
    let local_with_provider_xattr = root.join("Local.md");
    let conflict = root.join("Conflict.icloud-conflict.md");
    std::fs::write(&downloaded, "downloaded").unwrap();
    std::fs::write(&suffix_only, "suffix only").unwrap();
    std::fs::write(&evicted, "placeholder").unwrap();
    mark_evicted_fixture(&evicted);
    std::fs::write(&value_evicted, "remote").unwrap();
    xattr::set(
        &value_evicted,
        "com.apple.fileprovider.state",
        b"not-downloaded",
    )
    .unwrap();
    std::fs::write(&value_current, "current").unwrap();
    xattr::set(&value_current, "com.apple.fileprovider.state", b"current").unwrap();
    std::fs::write(&value_downloaded_false, "downloaded false").unwrap();
    xattr::set(
        &value_downloaded_false,
        "com.apple.fileprovider.state",
        b"isDownloaded=false; isDownloading=false",
    )
    .unwrap();
    std::fs::write(&value_conflict_false, "conflict false").unwrap();
    xattr::set(
        &value_conflict_false,
        "com.apple.fileprovider.state",
        b"hasUnresolvedConflicts=false",
    )
    .unwrap();
    std::fs::write(&value_false_prefix, "false prefix").unwrap();
    xattr::set(
        &value_false_prefix,
        "com.apple.fileprovider.state",
        b"isDownloaded=falsepositive",
    )
    .unwrap();
    std::fs::write(&local_with_provider_xattr, "local").unwrap();
    xattr::set(
        &local_with_provider_xattr,
        "com.apple.fileprovider.state",
        b"not-downloaded",
    )
    .unwrap();
    xattr::set(
        &local_with_provider_xattr,
        "com.apple.fileprovider.domain",
        b"com.example.drive",
    )
    .unwrap();
    std::fs::write(&conflict, "conflict").unwrap();
    xattr::set(&conflict, "com.apple.fileprovider.state", b"conflict").unwrap();

    let downloaded_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-state")
        .arg(&downloaded)
        .output()
        .unwrap();
    assert!(
        downloaded_output.status.success(),
        "{}",
        String::from_utf8_lossy(&downloaded_output.stderr)
    );
    let downloaded_stdout = String::from_utf8(downloaded_output.stdout).unwrap();
    let downloaded_stderr = String::from_utf8_lossy(&downloaded_output.stderr);
    assert_worker_admitted(&downloaded_stderr, "fileprovider state", &downloaded);
    assert!(downloaded_stdout.starts_with("fileprovider-state\t"));
    assert!(downloaded_stdout
        .contains("\tdomain=local\tstate=local-only\tmaterialization=not-provider-backed\t"));
    assert!(downloaded_stdout.contains("\tmaterialization-source=filesystem\t"));
    assert!(downloaded_stdout.contains("\tmaterialization-reason=not-fileprovider-backed\t"));
    assert!(downloaded_stdout.contains("\tbadges=\t"));
    assert!(downloaded_stdout.contains("\tdownload=hidden\tevict=hidden\t"));
    assert!(downloaded_stdout.contains("\treason=not-fileprovider-backed"));
    assert!(!downloaded_stdout.contains("nsfileprovidermanager"));

    let identity_state_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-state-with-identity")
        .arg(&downloaded)
        .output()
        .unwrap();
    assert!(
        identity_state_output.status.success(),
        "{}",
        String::from_utf8_lossy(&identity_state_output.stderr)
    );
    let identity_state_stdout = String::from_utf8(identity_state_output.stdout).unwrap();
    let identity_state_stderr = String::from_utf8_lossy(&identity_state_output.stderr);
    assert_worker_admitted(
        &identity_state_stderr,
        "fileprovider identity state",
        &downloaded,
    );
    assert!(identity_state_stdout.starts_with("fileprovider-state\t"));
    assert!(identity_state_stdout
        .contains("\tdomain=local\tstate=local-only\tmaterialization=not-provider-backed\t"));
    assert!(identity_state_stdout.contains("\tsource="));
    assert!(!identity_state_stdout.contains("nsfileprovidermanager"));

    let suffix_identity_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-state-with-identity")
        .arg(&suffix_only)
        .output()
        .unwrap();
    assert!(
        suffix_identity_output.status.success(),
        "{}",
        String::from_utf8_lossy(&suffix_identity_output.stderr)
    );
    let suffix_identity_stdout = String::from_utf8(suffix_identity_output.stdout).unwrap();
    let suffix_identity_stderr = String::from_utf8_lossy(&suffix_identity_output.stderr);
    assert_worker_admitted(
        &suffix_identity_stderr,
        "fileprovider identity state",
        &suffix_only,
    );
    assert!(suffix_identity_stdout.starts_with("fileprovider-state\t"));
    assert!(suffix_identity_stdout
        .contains("\tdomain=local\tstate=local-only\tmaterialization=not-provider-backed\t"));
    assert!(suffix_identity_stdout.contains("\tmaterialization-source=filesystem\t"));
    assert!(suffix_identity_stdout.contains("\tsource=fixture-name+icloud-extension\t"));
    assert!(
        !suffix_identity_stdout.contains("nsfileprovidermanager"),
        "{suffix_identity_stdout}"
    );

    let domain_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-domain")
        .arg(&downloaded)
        .output()
        .unwrap();
    assert!(
        domain_output.status.success(),
        "{}",
        String::from_utf8_lossy(&domain_output.stderr)
    );
    let domain_stdout = String::from_utf8(domain_output.stdout).unwrap();
    let domain_stderr = String::from_utf8_lossy(&domain_output.stderr);
    assert_worker_admitted(&domain_stderr, "fileprovider domain", &downloaded);
    assert!(domain_stdout.starts_with("fileprovider-domain\t"));
    assert!(domain_stdout.contains("\tdomain=local\t"));
    assert!(domain_stdout.contains("\tidentity-status=not-queried\t"));
    assert!(domain_stdout.contains("\tidentity-status="));
    assert!(domain_stdout.contains("\tmanager-status="));
    assert!(domain_stdout.contains("\tresource-status=available\t"));
    assert!(domain_stdout.contains("\tdomain-count="));
    assert!(domain_stdout.contains("\titem="));
    assert!(domain_stdout.contains("\tdomain-id="));
    assert!(domain_stdout.contains("\tmatched-display="));

    let domains_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-domains")
        .output()
        .unwrap();
    assert!(
        domains_output.status.success(),
        "{}",
        String::from_utf8_lossy(&domains_output.stderr)
    );
    let domains_stdout = String::from_utf8(domains_output.stdout).unwrap();
    assert!(domains_stdout.starts_with("fileprovider-domains\t"));
    assert!(domains_stdout.contains("\tcount="));
    assert!(domains_stdout.contains("\treason="));

    let cancelled_domains = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-domains-cancel-before-native")
        .output()
        .unwrap();
    assert!(!cancelled_domains.status.success());
    let cancelled_stdout = String::from_utf8_lossy(&cancelled_domains.stdout);
    let cancelled_stderr = String::from_utf8_lossy(&cancelled_domains.stderr);
    assert!(
        !cancelled_stdout.contains("fileprovider-domains\t"),
        "{cancelled_stdout}"
    );
    assert!(
        cancelled_stderr.contains("operation was cancelled"),
        "{cancelled_stderr}"
    );

    let evicted_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-state")
        .arg(&evicted)
        .output()
        .unwrap();
    assert!(
        evicted_output.status.success(),
        "{}",
        String::from_utf8_lossy(&evicted_output.stderr)
    );
    let evicted_stdout = String::from_utf8(evicted_output.stdout).unwrap();
    let evicted_stderr = String::from_utf8_lossy(&evicted_output.stderr);
    assert_worker_admitted(&evicted_stderr, "fileprovider state", &evicted);
    assert!(evicted_stdout.contains("\tstate=evicted\tmaterialization=remote-placeholder\t"));
    assert!(evicted_stdout.contains("\tmaterialization-source=xattr-fallback\t"));
    assert!(evicted_stdout.contains("\toffline=true\t"));
    assert!(evicted_stdout.contains("\tbadges=cloud\t"));
    assert!(evicted_stdout.contains("\tdownload=disabled\tevict=disabled\t"));
    assert!(evicted_stdout.contains("\treason=not-native-provider-backed"));

    let value_evicted_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-state")
        .arg(&value_evicted)
        .output()
        .unwrap();
    assert!(
        value_evicted_output.status.success(),
        "{}",
        String::from_utf8_lossy(&value_evicted_output.stderr)
    );
    let value_evicted_stdout = String::from_utf8(value_evicted_output.stdout).unwrap();
    let value_evicted_stderr = String::from_utf8_lossy(&value_evicted_output.stderr);
    assert_worker_admitted(&value_evicted_stderr, "fileprovider state", &value_evicted);
    assert!(value_evicted_stdout.contains("\tstate=evicted\tmaterialization=remote-placeholder\t"));
    assert!(value_evicted_stdout.contains("\tmaterialization-source=xattr-fallback\t"));
    assert!(value_evicted_stdout.contains("\tsource=fixture-name+xattr\t"));

    let value_current_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-state")
        .arg(&value_current)
        .output()
        .unwrap();
    assert!(
        value_current_output.status.success(),
        "{}",
        String::from_utf8_lossy(&value_current_output.stderr)
    );
    let value_current_stdout = String::from_utf8(value_current_output.stdout).unwrap();
    let value_current_stderr = String::from_utf8_lossy(&value_current_output.stderr);
    assert_worker_admitted(&value_current_stderr, "fileprovider state", &value_current);
    assert!(value_current_stdout.contains("\tstate=downloaded\tmaterialization=materialized\t"));
    assert!(value_current_stdout.contains("\tmaterialization-source=xattr-fallback\t"));
    assert!(value_current_stdout.contains("\tbadges=available-offline\t"));
    assert!(value_current_stdout.contains("\treason=not-native-provider-backed"));

    let value_downloaded_false_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-state")
        .arg(&value_downloaded_false)
        .output()
        .unwrap();
    assert!(
        value_downloaded_false_output.status.success(),
        "{}",
        String::from_utf8_lossy(&value_downloaded_false_output.stderr)
    );
    let value_downloaded_false_stdout =
        String::from_utf8(value_downloaded_false_output.stdout).unwrap();
    let value_downloaded_false_stderr =
        String::from_utf8_lossy(&value_downloaded_false_output.stderr);
    assert_worker_admitted(
        &value_downloaded_false_stderr,
        "fileprovider state",
        &value_downloaded_false,
    );
    assert!(value_downloaded_false_stdout
        .contains("\tstate=evicted\tmaterialization=remote-placeholder\t"));
    assert!(value_downloaded_false_stdout.contains("\tmaterialization-source=xattr-fallback\t"));
    assert!(value_downloaded_false_stdout.contains("\tbadges=cloud\t"));

    let value_conflict_false_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-state")
        .arg(&value_conflict_false)
        .output()
        .unwrap();
    assert!(
        value_conflict_false_output.status.success(),
        "{}",
        String::from_utf8_lossy(&value_conflict_false_output.stderr)
    );
    let value_conflict_false_stdout =
        String::from_utf8(value_conflict_false_output.stdout).unwrap();
    let value_conflict_false_stderr = String::from_utf8_lossy(&value_conflict_false_output.stderr);
    assert_worker_admitted(
        &value_conflict_false_stderr,
        "fileprovider state",
        &value_conflict_false,
    );
    assert!(
        value_conflict_false_stdout.contains("\tstate=downloaded\tmaterialization=materialized\t")
    );
    assert!(value_conflict_false_stdout.contains("\tconflict=false\t"));
    assert!(value_conflict_false_stdout.contains("\tmaterialization-source=xattr-fallback\t"));

    let value_false_prefix_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-state")
        .arg(&value_false_prefix)
        .output()
        .unwrap();
    assert!(
        value_false_prefix_output.status.success(),
        "{}",
        String::from_utf8_lossy(&value_false_prefix_output.stderr)
    );
    let value_false_prefix_stdout = String::from_utf8(value_false_prefix_output.stdout).unwrap();
    let value_false_prefix_stderr = String::from_utf8_lossy(&value_false_prefix_output.stderr);
    assert_worker_admitted(
        &value_false_prefix_stderr,
        "fileprovider state",
        &value_false_prefix,
    );
    assert!(
        value_false_prefix_stdout.contains("\tstate=downloaded\tmaterialization=materialized\t")
    );
    assert!(value_false_prefix_stdout.contains("\tmaterialization-source=xattr-fallback\t"));

    let local_with_provider_xattr_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-state")
        .arg(&local_with_provider_xattr)
        .output()
        .unwrap();
    assert!(
        local_with_provider_xattr_output.status.success(),
        "{}",
        String::from_utf8_lossy(&local_with_provider_xattr_output.stderr)
    );
    let local_with_provider_xattr_stdout =
        String::from_utf8(local_with_provider_xattr_output.stdout).unwrap();
    let local_with_provider_xattr_stderr =
        String::from_utf8_lossy(&local_with_provider_xattr_output.stderr);
    assert_worker_admitted(
        &local_with_provider_xattr_stderr,
        "fileprovider state",
        &local_with_provider_xattr,
    );
    assert!(local_with_provider_xattr_stdout
        .contains("\tdomain=local\tstate=local-only\tmaterialization=not-provider-backed\t"));
    assert!(local_with_provider_xattr_stdout.contains("\tprovider=-\t"));
    assert!(!local_with_provider_xattr_stdout.contains("xattr"));

    let conflict_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-state")
        .arg(&conflict)
        .output()
        .unwrap();
    assert!(
        conflict_output.status.success(),
        "{}",
        String::from_utf8_lossy(&conflict_output.stderr)
    );
    let conflict_stdout = String::from_utf8(conflict_output.stdout).unwrap();
    let conflict_stderr = String::from_utf8_lossy(&conflict_output.stderr);
    assert_worker_admitted(&conflict_stderr, "fileprovider state", &conflict);
    assert!(conflict_stdout.contains("\tstate=conflict\tmaterialization=conflict\t"));
    assert!(conflict_stdout.contains("\toffline=false\tconflict=true\t"));
    assert!(conflict_stdout.contains("\tbadges=conflict\t"));
    assert!(conflict_stdout.contains("\treveal-conflict=enabled\t"));

    let conflict_report = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-conflict")
        .arg(&conflict)
        .output()
        .unwrap();
    assert!(
        conflict_report.status.success(),
        "{}",
        String::from_utf8_lossy(&conflict_report.stderr)
    );
    let conflict_report_stdout = String::from_utf8(conflict_report.stdout).unwrap();
    let conflict_report_stderr = String::from_utf8_lossy(&conflict_report.stderr);
    assert_worker_admitted(&conflict_report_stderr, "fileprovider conflict", &conflict);
    assert!(conflict_report_stdout.starts_with("fileprovider-conflict\t"));
    assert!(conflict_report_stdout
        .contains("\tconflict=true\tstate=conflict\taffected=1\taffected-paths="));
    assert!(conflict_report_stdout.contains("\treveal=enabled\tblock-operations=true\t"));
    assert!(
        conflict_report_stdout.ends_with("reason=conflict-requires-user-resolution\n"),
        "{conflict_report_stdout}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_fileprovider_progress_from_binary() {
    let root =
        std::env::temp_dir().join(format!("gfm-fileprovider-progress-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let downloading = root.join("Downloading.icloud-downloading.md");
    std::fs::write(&downloading, "downloading").unwrap();
    xattr::set(&downloading, "com.apple.fileprovider.state", b"downloading").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-progress")
        .arg(&downloading)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(&stderr, "fileprovider progress", &downloading);
    assert!(stdout.starts_with("fileprovider-progress\t"));
    assert!(stdout.contains("\tstate=downloading\t"));
    assert!(stdout.contains("\tprogress-direction=download\tprogress-milli=-\t"));
    assert!(stdout.contains("\tprogress-requested=true\t"));
    assert!(stdout.contains("\tprogress-indeterminate=true\t"));
    assert!(stdout.ends_with("progress-reason=provider-progress-unavailable\n"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn publishes_fileprovider_progress_to_runtime_job_store_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-progress-job-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let downloading = root.join("Downloading.icloud-downloading.md");
    let progress = root.join("progress.gfmprogress");
    let catalog = root.join("payloads.gfmjobs");
    std::fs::write(&downloading, "downloading").unwrap();
    xattr::set(&downloading, "com.apple.fileprovider.state", b"downloading").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .arg("fileprovider-progress-job")
        .arg(&downloading)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(&stderr, "fileprovider progress job", &downloading);
    assert!(stdout.starts_with("fileprovider-progress\t"));
    assert!(stdout.contains("\tstate=downloading\t"));

    let progress_text = std::fs::read_to_string(&progress).unwrap();
    assert!(
        progress_text.contains("progress\t1\tvisible\tvisible\tfileprovider download"),
        "{progress_text}"
    );
    assert!(
        progress_text.contains("\trunning\t0\t1\tfileprovider:icloud-drive:downloading:download:state:-:provider-progress-unavailable\t"),
        "{progress_text}"
    );
    let catalog_text = std::fs::read_to_string(&catalog).unwrap();
    assert!(
        catalog_text.contains("payload\t1\toperation\tfileprovider download"),
        "{catalog_text}"
    );
    assert!(
        catalog_text.contains(&downloading.display().to_string()),
        "{catalog_text}"
    );

    let ui_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-progress-job-contract")
        .arg(&progress)
        .arg("1")
        .output()
        .unwrap();
    assert!(
        ui_output.status.success(),
        "{}",
        String::from_utf8_lossy(&ui_output.stderr)
    );
    let ui_stdout = String::from_utf8(ui_output.stdout).unwrap();
    assert!(ui_stdout.starts_with("dialog\tsurface=progress\tpresentation=progress-sheet"));
    assert!(ui_stdout.contains(
        "operation-progress\tjob=1\tlabel=fileprovider download\tstate=running\tcompleted=0\ttotal=1"
    ));
    assert!(ui_stdout.contains("operation-progress-command\tstop\tjob=1\tenabled=true"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn fileprovider_progress_job_retries_transient_provider_publish_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-progress-job-retry-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let downloading = root.join("Downloading.icloud-downloading.md");
    let progress = root.join("progress.gfmprogress");
    let catalog = root.join("payloads.gfmjobs");
    let journal = root.join("jobs.gfmjournal");
    let retry_probe = root.join("progress-retry.state");
    std::fs::write(&downloading, "downloading").unwrap();
    xattr::set(&downloading, "com.apple.fileprovider.state", b"downloading").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_FILEPROVIDER_PROGRESS_RETRY_PROBE", &retry_probe)
        .arg("fileprovider-progress-job")
        .arg(&downloading)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("fileprovider-progress\t"), "{stdout}");
    assert!(stdout.contains("\tstate=downloading\t"), "{stdout}");
    assert_eq!(std::fs::read_to_string(&retry_probe).unwrap(), "2");

    let journal_text = std::fs::read_to_string(&journal).unwrap();
    assert!(
        journal_text.contains("1\t1\tstarted\tfileprovider progress job"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains(
            "1\t1\tfailed:temporary fileprovider progress busy\tfileprovider progress job"
        ),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tstarted\tfileprovider progress job"),
        "{journal_text}"
    );
    assert!(
        journal_text.contains("1\t2\tcompleted\tfileprovider progress job"),
        "{journal_text}"
    );
    let progress_text = std::fs::read_to_string(&progress).unwrap();
    assert!(
        progress_text.contains("progress\t1\tvisible\tvisible\tfileprovider download"),
        "{progress_text}"
    );
    let catalog_text = std::fs::read_to_string(&catalog).unwrap();
    assert!(
        catalog_text.contains("payload\t1\toperation\tfileprovider download"),
        "{catalog_text}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn fileprovider_progress_job_refuses_unreachable_retry_probe_before_provider_read_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-progress-retry-probe-blocked-{}",
        std::process::id()
    ));
    let offline = std::env::temp_dir().join(format!(
        "gfm-fileprovider-progress-retry-probe-offline-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&offline);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&offline).unwrap();
    std::fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let downloading = root.join("Downloading.icloud-downloading.md");
    let progress = root.join("progress.gfmprogress");
    let catalog = root.join("payloads.gfmjobs");
    let journal = root.join("jobs.gfmjournal");
    let retry_probe = offline.join("progress-retry.state");
    std::fs::write(&downloading, "downloading").unwrap();
    xattr::set(&downloading, "com.apple.fileprovider.state", b"downloading").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_FILEPROVIDER_PROGRESS_RETRY_PROBE", &retry_probe)
        .arg("fileprovider-progress-job")
        .arg(&downloading)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("fileprovider-progress\t"), "{stdout}");
    assert!(
        stderr.contains(
            "fileprovider progress job volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert!(
        !stderr.contains("platform write path metadata unavailable"),
        "{stderr}"
    );
    assert!(!retry_probe.exists());
    assert!(!progress.exists());
    assert!(!catalog.exists());

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(offline);
}

#[test]
fn fileprovider_progress_job_cancel_after_access_stops_before_runtime_publish_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-progress-job-cancel-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let downloading = root.join("Downloading.icloud-downloading.md");
    let progress = root.join("progress.gfmprogress");
    let catalog = root.join("payloads.gfmjobs");
    std::fs::write(&downloading, "downloading").unwrap();
    xattr::set(&downloading, "com.apple.fileprovider.state", b"downloading").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .arg("fileprovider-progress-job-cancel-after-access")
        .arg(&downloading)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(&stderr, "fileprovider progress job", &downloading);
    assert_eq!(
        stdout,
        "fileprovider-progress\tstatus=cancelled\treason=cancelled-after-access\n"
    );
    assert!(!progress.exists());
    assert!(!catalog.exists());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn fileprovider_progress_job_refuses_unreachable_runtime_store_before_provider_read_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-progress-runtime-blocked-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("target")).unwrap();
    std::fs::create_dir_all(root.join("runtime")).unwrap();
    std::fs::write(
        root.join("runtime").join(".gfm-volume-kind"),
        "network-unreachable\n",
    )
    .unwrap();
    let downloading = root
        .join("target")
        .join("Downloading.icloud-downloading.md");
    let progress = root.join("runtime").join("progress.gfmprogress");
    let catalog = root.join("runtime").join("payloads.gfmjobs");
    std::fs::write(&downloading, "downloading").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .arg("fileprovider-progress-job")
        .arg(&downloading)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("fileprovider-progress\t"), "{stdout}");
    assert!(
        stderr.contains(
            "fileprovider progress job volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=fileprovider progress job\tpath={}",
            downloading.display()
        )),
        "{stderr}"
    );
    assert!(!progress.exists());
    assert!(!catalog.exists());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn fileprovider_state_controls_preview_generation_from_binary() {
    let root =
        std::env::temp_dir().join(format!("gfm-fileprovider-preview-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let evicted = root.join("Remote.icloud-placeholder.pdf");
    let downloading = root.join("Downloading.icloud-downloading.png");
    let unknown = root.join("Unknown.icloud.md");
    std::fs::write(&evicted, "%PDF-1.7\nplaceholder").unwrap();
    mark_evicted_fixture(&evicted);
    std::fs::write(&downloading, "png").unwrap();
    xattr::set(&downloading, "com.apple.fileprovider.state", b"downloading").unwrap();
    std::fs::write(&unknown, "unknown").unwrap();
    xattr::set(&unknown, "com.apple.fileprovider.state", b"unknown=true").unwrap();

    let quicklook = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("quicklook-session")
        .arg(&evicted)
        .output()
        .unwrap();
    assert!(
        quicklook.status.success(),
        "{}",
        String::from_utf8_lossy(&quicklook.stderr)
    );
    let quicklook_stderr = String::from_utf8_lossy(&quicklook.stderr);
    assert!(
        quicklook_stderr.contains("security-scope\t"),
        "{quicklook_stderr}"
    );
    assert!(
        quicklook_stderr.contains("\tintent=preview\t"),
        "{quicklook_stderr}"
    );
    assert_worker_admitted(&quicklook_stderr, "quicklook preview", &evicted);
    let quicklook_stdout = String::from_utf8(quicklook.stdout).unwrap();
    assert!(
        quicklook_stdout.contains("\tallow-native\tcloud=metadata-only\tmetadata-only\t"),
        "{quicklook_stdout}"
    );
    assert!(
        quicklook_stdout.ends_with("schedule=cancelled:metadata-only\n"),
        "{quicklook_stdout}"
    );

    let thumbnail = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("thumbnail-generation")
        .arg(&downloading)
        .output()
        .unwrap();
    assert!(
        thumbnail.status.success(),
        "{}",
        String::from_utf8_lossy(&thumbnail.stderr)
    );
    let thumbnail_stderr = String::from_utf8_lossy(&thumbnail.stderr);
    assert!(
        thumbnail_stderr.contains("security-scope\t"),
        "{thumbnail_stderr}"
    );
    assert!(
        thumbnail_stderr.contains("\tintent=preview\t"),
        "{thumbnail_stderr}"
    );
    assert_worker_admitted(&thumbnail_stderr, "thumbnail generation", &downloading);
    let thumbnail_stdout = String::from_utf8(thumbnail.stdout).unwrap();
    assert!(
        thumbnail_stdout.contains("\tallow-native\tcloud=defer\tmetadata-only\t512px\t"),
        "{thumbnail_stdout}"
    );
    assert!(
        thumbnail_stdout.ends_with("schedule=cancelled:fileprovider-in-flight\n"),
        "{thumbnail_stdout}"
    );

    let unknown_quicklook = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("quicklook-session")
        .arg(&unknown)
        .output()
        .unwrap();
    assert!(
        unknown_quicklook.status.success(),
        "{}",
        String::from_utf8_lossy(&unknown_quicklook.stderr)
    );
    let unknown_quicklook_stderr = String::from_utf8_lossy(&unknown_quicklook.stderr);
    assert_worker_admitted(&unknown_quicklook_stderr, "quicklook preview", &unknown);
    let unknown_quicklook_stdout = String::from_utf8(unknown_quicklook.stdout).unwrap();
    assert!(
        unknown_quicklook_stdout.contains("\tallow-native\tcloud=metadata-only\tmetadata-only\t"),
        "{unknown_quicklook_stdout}"
    );
    assert!(
        unknown_quicklook_stdout.ends_with("schedule=cancelled:metadata-only\n"),
        "{unknown_quicklook_stdout}"
    );

    let unknown_thumbnail = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("thumbnail-generation")
        .arg(&unknown)
        .output()
        .unwrap();
    assert!(
        unknown_thumbnail.status.success(),
        "{}",
        String::from_utf8_lossy(&unknown_thumbnail.stderr)
    );
    let unknown_thumbnail_stderr = String::from_utf8_lossy(&unknown_thumbnail.stderr);
    assert_worker_admitted(&unknown_thumbnail_stderr, "thumbnail generation", &unknown);
    let unknown_thumbnail_stdout = String::from_utf8(unknown_thumbnail.stdout).unwrap();
    assert!(
        unknown_thumbnail_stdout.contains("\tallow-native\tcloud=metadata-only\tmetadata-only\t"),
        "{unknown_thumbnail_stdout}"
    );
    assert!(
        unknown_thumbnail_stdout.ends_with("schedule=cancelled:metadata-only\n"),
        "{unknown_thumbnail_stdout}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn refuses_fileprovider_operations_without_native_provider_from_binary() {
    let root = std::env::temp_dir().join(format!("gfm-fileprovider-op-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let evicted = root.join("Remote.icloud-placeholder");
    let downloaded = root.join("Downloaded.icloud.md");
    std::fs::write(&evicted, "placeholder").unwrap();
    mark_evicted_fixture(&evicted);
    std::fs::write(&downloaded, "downloaded").unwrap();

    let download_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-operation")
        .arg("download")
        .arg(&evicted)
        .output()
        .unwrap();
    assert!(
        download_output.status.success(),
        "{}",
        String::from_utf8_lossy(&download_output.stderr)
    );
    let download_stdout = String::from_utf8(download_output.stdout).unwrap();
    let download_stderr = String::from_utf8_lossy(&download_output.stderr);
    assert_worker_admitted(&download_stderr, "fileprovider operation", &evicted);
    assert!(download_stdout.starts_with("fileprovider-operation\t"));
    assert!(download_stdout.contains("\toperation=download\tdisposition=refused\t"));
    assert!(download_stdout.contains("\tnative-status=-\t"));
    assert!(download_stdout.contains("\tbefore-domain=icloud-drive\t"));
    assert!(download_stdout.contains("\tbefore-state=evicted\t"));
    assert!(download_stdout.contains("\tbefore-materialization=remote-placeholder\t"));
    assert!(download_stdout.contains("\tbefore-materialization-source=xattr-fallback\t"));
    assert!(download_stdout.contains("\tbefore-materialization-confidence=xattr-fallback\t"));
    assert!(download_stdout.contains("\tbefore-offline=true\t"));
    assert!(download_stdout.contains("\tbefore-badges=cloud\t"));
    assert!(download_stdout.contains("\tbefore-download=disabled\tbefore-evict=disabled\t"));
    assert!(download_stdout.contains("\tafter-state=-\tafter-materialization=-\t"));
    assert!(download_stdout.ends_with("reason=not-native-provider-backed\n"));

    let evict_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-operation")
        .arg("evict")
        .arg(&downloaded)
        .output()
        .unwrap();
    assert!(
        evict_output.status.success(),
        "{}",
        String::from_utf8_lossy(&evict_output.stderr)
    );
    let evict_stdout = String::from_utf8(evict_output.stdout).unwrap();
    let evict_stderr = String::from_utf8_lossy(&evict_output.stderr);
    assert_worker_admitted(&evict_stderr, "fileprovider operation", &downloaded);
    assert!(evict_stdout.contains("\toperation=evict\tdisposition=refused\t"));
    assert!(evict_stdout.contains("\tnative-status=-\t"));
    assert!(evict_stdout.contains("\tbefore-domain=local\t"));
    assert!(evict_stdout.contains("\tbefore-state=local-only\t"));
    assert!(evict_stdout.contains("\tbefore-materialization=not-provider-backed\t"));
    assert!(evict_stdout.contains("\tafter-state=-\tafter-materialization=-\t"));
    assert!(evict_stdout.ends_with("reason=operation-disabled-for-current-state\n"));

    let conflict = root.join("Conflict.icloud-conflict.md");
    std::fs::write(&conflict, "conflict").unwrap();
    xattr::set(&conflict, "com.apple.fileprovider.state", b"conflict").unwrap();
    let conflict_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-operation")
        .arg("evict")
        .arg(&conflict)
        .output()
        .unwrap();
    assert!(
        conflict_output.status.success(),
        "{}",
        String::from_utf8_lossy(&conflict_output.stderr)
    );
    let conflict_stdout = String::from_utf8(conflict_output.stdout).unwrap();
    let conflict_stderr = String::from_utf8_lossy(&conflict_output.stderr);
    assert_worker_admitted(&conflict_stderr, "fileprovider operation", &conflict);
    assert!(conflict_stdout.contains("\toperation=evict\tdisposition=refused\t"));
    assert!(conflict_stdout.contains("\tnative-status=-\t"));
    assert!(conflict_stdout.contains("\tbefore-state=conflict\t"));
    assert!(conflict_stdout.contains("\tbefore-materialization=conflict\t"));
    assert!(conflict_stdout.contains("\tbefore-conflict=true\t"));
    assert!(conflict_stdout.contains("\tbefore-reveal-conflict=enabled\t"));
    assert!(conflict_stdout.contains("\tafter-state=-\tafter-materialization=-\t"));
    assert!(conflict_stdout.ends_with("reason=provider-conflict-requires-resolution\n"));

    let missing = std::fs::canonicalize(&root).unwrap().join("Missing.icloud");
    let missing_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-operation")
        .arg("download")
        .arg(&missing)
        .output()
        .unwrap();
    assert!(
        missing_output.status.success(),
        "{}",
        String::from_utf8_lossy(&missing_output.stderr)
    );
    let missing_stdout = String::from_utf8(missing_output.stdout).unwrap();
    let missing_stderr = String::from_utf8_lossy(&missing_output.stderr);
    assert_worker_admitted(&missing_stderr, "fileprovider operation", &missing);
    assert!(missing_stdout.contains("\toperation=download\tdisposition=missing\t"));
    assert!(missing_stdout.contains("\tnative-status=-\t"));
    assert!(missing_stdout.contains("\tbefore-state=unknown\t"));
    assert!(missing_stdout.contains("\tbefore-materialization=unknown\t"));
    assert!(
        missing_stdout.contains("\tbefore-materialization-source=native-url-resource:missing\t")
    );
    assert!(missing_stdout.contains("\tafter-state=-\tafter-materialization=-\t"));
    assert!(missing_stdout.ends_with("reason=native-url-resource-missing\n"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_fileprovider_invalidation_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-invalidation-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let evicted = root.join("Remote.icloud-placeholder");
    std::fs::write(&evicted, "placeholder").unwrap();
    mark_evicted_fixture(&evicted);

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-invalidation")
        .arg("downloaded")
        .arg(&evicted)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(&stderr, "fileprovider invalidation", &evicted);
    assert!(stdout.starts_with("fileprovider-invalidation\t"));
    assert!(stdout.contains("\tprevious=downloaded\tcurrent=evicted\t"));
    assert!(stdout.contains("\tchanged=true\t"));
    assert!(stdout.contains("\tcurrent-domain=icloud-drive\t"));
    assert!(stdout.contains("\tcurrent-state=evicted\t"));
    assert!(stdout.contains("\tcurrent-materialization=remote-placeholder\t"));
    assert!(stdout.contains("\tcurrent-materialization-source=xattr-fallback\t"));
    assert!(stdout.contains("\tcurrent-materialization-confidence=xattr-fallback\t"));
    assert!(stdout.contains("\tcurrent-offline=true\t"));
    assert!(stdout.contains("\tcurrent-badges=cloud\t"));
    assert!(stdout.contains("\ticon=true\tpreview-memory=true\tpreview-disk=true\t"));
    assert!(stdout.contains("\tsidebar=true\treindex-metadata=true\t"));
    assert!(stdout.ends_with("reason=fileprovider-state-changed\n"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_fileprovider_metadata_invalidation_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-metadata-invalidation-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let evicted = root.join("Remote.icloud-placeholder");
    std::fs::write(&evicted, "placeholder").unwrap();
    mark_evicted_fixture(&evicted);

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-metadata-invalidation")
        .arg("downloaded")
        .arg(&evicted)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(&stderr, "fileprovider metadata invalidation", &evicted);
    assert!(stdout.starts_with("provider-metadata-invalidation\t"));
    assert!(stdout.contains("\tprevious=downloaded\tcurrent=evicted\t"));
    assert!(stdout.contains("\treindex-metadata=true\tschedule-metadata-update=true\t"));
    assert!(stdout.contains("\tinvalidate-query-cache=true\t"));
    assert!(stdout.ends_with("reason=provider-metadata-state-changed\n"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_preview_cache_fileprovider_invalidation_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-preview-cache-fileprovider-invalidation-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let cache = root.join("cache");
    let evicted = root.join("Remote.icloud-placeholder");
    std::fs::write(&evicted, "placeholder").unwrap();
    mark_evicted_fixture(&evicted);
    let mut seeded_cache = PreviewCache::new(PreviewCacheConfig::new(&cache)).unwrap();
    let seeded_key = PreviewRequestKey::new(
        FileId::new(VolumeId(42), 9001),
        evicted.clone(),
        PreviewKind::Thumbnail,
    );
    let seeded_quicklook_key = PreviewRequestKey::new(
        FileId::new(VolumeId(42), 9002),
        evicted.clone(),
        PreviewKind::QuickLook,
    );
    seeded_cache
        .insert(PreviewEntry::new(seeded_key, b"cached thumbnail".to_vec()))
        .unwrap();
    seeded_cache
        .insert(PreviewEntry::new(
            seeded_quicklook_key,
            b"cached quicklook".to_vec(),
        ))
        .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("preview-cache-fileprovider-invalidation")
        .arg(&cache)
        .arg("downloaded")
        .arg(&evicted)
        .arg("thumbnail")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(&stderr, "preview cache root", &cache);
    assert_worker_admitted(&stderr, "preview cache", &evicted);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("preview-cache-invalidation\t"));
    assert!(stdout.contains("\tkind=thumbnail\treason=content-or-icloud\t"));
    assert!(stdout.contains("\tkind=quick-look\treason=content-or-icloud\t"));
    assert_eq!(
        stdout.matches("preview-cache-invalidation\t").count(),
        2,
        "{stdout}"
    );
    assert!(stdout.contains("\tinvalidate-memory=true\tinvalidate-disk=true\t"));
    assert_eq!(
        stdout
            .matches("\tremoved-memory=false\tremoved-disk=true")
            .count(),
        2,
        "{stdout}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_preview_cache_fileprovider_observed_invalidation_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-preview-cache-fileprovider-observed-invalidation-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let cache = root.join("cache");
    let state = root.join("fileprovider-state.tsv");
    let evicted = root.join("Remote.icloud-placeholder");
    std::fs::write(&evicted, "placeholder").unwrap();
    mark_evicted_fixture(&evicted);
    std::fs::write(
        &state,
        format!(
            "gfm-fileprovider-state-v1\ndownloaded\t{}\n",
            evicted.display()
        ),
    )
    .unwrap();
    let mut seeded_cache = PreviewCache::new(PreviewCacheConfig::new(&cache)).unwrap();
    let seeded_key = PreviewRequestKey::new(
        FileId::new(VolumeId(42), 9001),
        evicted.clone(),
        PreviewKind::Thumbnail,
    );
    seeded_cache
        .insert(PreviewEntry::new(seeded_key, b"cached thumbnail".to_vec()))
        .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("preview-cache-fileprovider-observed-invalidation")
        .arg(&cache)
        .arg(&state)
        .arg("thumbnail")
        .arg("metadata")
        .arg(&evicted)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with(
        "fileprovider-observed-invalidation\tevents=1\tevent-kinds=metadata\tpaths=1\n"
    ));
    assert!(stdout.contains(&format!(
        "fileprovider-observed-paths\tcount=1\tpaths={}\n",
        evicted.display()
    )));
    assert!(stdout.contains(
        "fileprovider-state-invalidation\tinitialized=false\tchanged=1\ticon=true\tpreview-memory=true\tpreview-disk=true\t"
    ));
    assert!(stdout.contains("preview-cache-invalidation\t"));
    assert!(stdout.contains("\tkind=thumbnail\treason=content-or-icloud\t"));
    assert!(stdout.contains("\tremoved-memory=false\tremoved-disk=true\n"));
    let state_text = std::fs::read_to_string(&state).unwrap();
    assert_fileprovider_state_entry(&state_text, "evicted", &evicted);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn preview_cache_fileprovider_observed_invalidation_removes_deleted_tracked_subtree_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-preview-cache-fileprovider-observed-subtree-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let cache = root.join("cache");
    let state = root.join("fileprovider-state.tsv");
    let removed_dir = root.join("Removed.icloud");
    let child = removed_dir.join("Child.icloud.md");
    std::fs::create_dir_all(&removed_dir).unwrap();
    std::fs::write(&child, "downloaded").unwrap();
    std::fs::write(
        &state,
        format!(
            "gfm-fileprovider-state-v1\ndownloaded\t{}\n",
            child.display()
        ),
    )
    .unwrap();
    let mut seeded_cache = PreviewCache::new(PreviewCacheConfig::new(&cache)).unwrap();
    let seeded_key = PreviewRequestKey::new(
        FileId::new(VolumeId(42), 9002),
        child.clone(),
        PreviewKind::Thumbnail,
    );
    seeded_cache
        .insert(PreviewEntry::new(seeded_key, b"cached thumbnail".to_vec()))
        .unwrap();
    std::fs::remove_dir_all(&removed_dir).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("preview-cache-fileprovider-observed-invalidation")
        .arg(&cache)
        .arg(&state)
        .arg("thumbnail")
        .arg("remove")
        .arg(&removed_dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with(
        "fileprovider-observed-invalidation\tevents=1\tevent-kinds=remove\tpaths=1\n"
    ));
    assert!(stdout.contains(&format!(
        "fileprovider-invalidation\t{}\tprevious=downloaded\tcurrent=removed\tchanged=true\t",
        child.display()
    )));
    assert!(stdout.contains("preview-cache-invalidation\t"));
    assert!(stdout.contains("\tkind=thumbnail\treason=removed\t"));
    assert!(stdout.contains("\tremoved-memory=false\tremoved-disk=true\n"));
    let state_text = std::fs::read_to_string(&state).unwrap();
    assert!(!state_text.contains(&child.display().to_string()));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn preview_cache_refuses_unreachable_network_volume_before_record_read_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-preview-cache-unreachable-volume-{}",
        std::process::id()
    ));
    let cache_root = std::env::temp_dir().join(format!(
        "gfm-preview-cache-unreachable-volume-cache-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&cache_root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&cache_root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let cache = cache_root.join("cache");
    let evicted = root.join("Remote.icloud-placeholder");
    std::fs::write(&evicted, "placeholder").unwrap();
    mark_evicted_fixture(&evicted);

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("preview-cache-fileprovider-invalidation")
        .arg(&cache)
        .arg("downloaded")
        .arg(&evicted)
        .arg("thumbnail")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("preview-cache-invalidation\t"), "{stdout}");
    assert!(
        stderr.contains("preview cache volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(!stderr.contains("security-worker-admission\t"), "{stderr}");

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(cache_root);
}

#[test]
fn preview_cache_refuses_unreachable_cache_root_before_invalidation_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-preview-cache-root-unreachable-source-{}",
        std::process::id()
    ));
    let offline = std::env::temp_dir().join(format!(
        "gfm-preview-cache-root-unreachable-cache-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&offline);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&offline).unwrap();
    std::fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let cache = offline.join(format!("{}.cache", "preview-cache-unavailable".repeat(12)));
    let evicted = root.join("Remote.icloud-placeholder");
    let state = root.join("fileprovider-state.tsv");
    std::fs::write(&evicted, "placeholder").unwrap();
    mark_evicted_fixture(&evicted);
    std::fs::write(
        &state,
        format!(
            "gfm-fileprovider-state-v1\ndownloaded\t{}\n",
            evicted.display()
        ),
    )
    .unwrap();

    for args in [
        vec![
            "preview-cache-fileprovider-invalidation".to_string(),
            cache.display().to_string(),
            "downloaded".to_string(),
            evicted.display().to_string(),
            "thumbnail".to_string(),
        ],
        vec![
            "preview-cache-fileprovider-observed-invalidation".to_string(),
            cache.display().to_string(),
            state.display().to_string(),
            "thumbnail".to_string(),
            "metadata".to_string(),
            evicted.display().to_string(),
        ],
        vec![
            "preview-cache-fileprovider-observer-probe".to_string(),
            cache.display().to_string(),
            state.display().to_string(),
            "thumbnail".to_string(),
            root.display().to_string(),
            evicted.display().to_string(),
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
            .args(args)
            .output()
            .unwrap();

        assert!(!output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stdout.contains("preview-cache-invalidation\t"), "{stdout}");
        assert!(
            stderr.contains("preview cache root volume access blocked: unreachable volume network")
                || stderr.contains(
                    "preview cache fileprovider observer cache volume access blocked: unreachable volume network"
                ),
            "{stderr}"
        );
        assert!(
            !stderr.contains("platform write path metadata unavailable"),
            "{stderr}"
        );
        assert!(!stderr.contains("security-worker-admission\t"), "{stderr}");
        assert!(!cache.exists());
    }

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(offline);
}

#[test]
fn preview_cache_surfaces_unreadable_disk_index_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-preview-cache-index-unavailable-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let cache = root.join("cache");
    std::fs::create_dir_all(cache.join("preview-cache-index.tsv")).unwrap();
    let evicted = root.join("Remote.icloud-placeholder");
    std::fs::write(&evicted, "placeholder").unwrap();
    mark_evicted_fixture(&evicted);

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("preview-cache-fileprovider-invalidation")
        .arg(&cache)
        .arg("downloaded")
        .arg(&evicted)
        .arg("thumbnail")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("preview-cache-invalidation\t"), "{stdout}");
    assert!(
        stderr.contains("preview disk cache index unavailable"),
        "{stderr}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn preview_cache_surfaces_corrupt_disk_index_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-preview-cache-index-corrupt-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let cache = root.join("cache");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(
        cache.join("preview-cache-index.tsv"),
        "not-a-valid-index-row\n",
    )
    .unwrap();
    let evicted = root.join("Remote.icloud-placeholder");
    std::fs::write(&evicted, "placeholder").unwrap();
    mark_evicted_fixture(&evicted);

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("preview-cache-fileprovider-invalidation")
        .arg(&cache)
        .arg("downloaded")
        .arg(&evicted)
        .arg("thumbnail")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("preview-cache-invalidation\t"), "{stdout}");
    assert!(
        stderr.contains("preview disk cache index corrupt at line 1"),
        "{stderr}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn preview_cache_surfaces_duplicate_disk_index_path_kind_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-preview-cache-index-duplicate-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let cache = root.join("cache");
    std::fs::create_dir_all(&cache).unwrap();
    let evicted = root.join("Remote.icloud-placeholder");
    std::fs::write(&evicted, "placeholder").unwrap();
    mark_evicted_fixture(&evicted);
    let path_hex = evicted
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    std::fs::write(
        cache.join("preview-cache-index.tsv"),
        format!("thumbnail\t1\t2\t256\t2000\t0\t{path_hex}\nthumbnail\t1\t3\t512\t2000\t1\t{path_hex}\n"),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("preview-cache-fileprovider-invalidation")
        .arg(&cache)
        .arg("downloaded")
        .arg(&evicted)
        .arg("thumbnail")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("preview-cache-invalidation\t"), "{stdout}");
    assert!(
        stderr.contains("preview disk cache index duplicate path/kind at line 2"),
        "{stderr}"
    );
    assert!(stderr.contains("thumbnail"), "{stderr}");
    assert!(stderr.contains(&evicted.display().to_string()), "{stderr}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn fileprovider_state_routes_refuse_unreachable_volume_before_native_read_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-state-unreachable-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let item = root.join("Remote.icloud-placeholder");
    std::fs::write(&item, "placeholder").unwrap();
    mark_evicted_fixture(&item);

    for (args, blocked_worker) in [
        (
            vec!["fileprovider-state".to_string(), item.display().to_string()],
            Some("fileprovider state"),
        ),
        (
            vec![
                "fileprovider-state-with-identity".to_string(),
                item.display().to_string(),
            ],
            Some("fileprovider identity state"),
        ),
        (
            vec![
                "fileprovider-domain".to_string(),
                item.display().to_string(),
            ],
            Some("fileprovider domain"),
        ),
        (
            vec![
                "fileprovider-progress".to_string(),
                item.display().to_string(),
            ],
            Some("fileprovider progress"),
        ),
        (
            vec![
                "fileprovider-conflict".to_string(),
                item.display().to_string(),
            ],
            Some("fileprovider conflict"),
        ),
        (
            vec![
                "fileprovider-progress-job".to_string(),
                item.display().to_string(),
            ],
            None,
        ),
        (
            vec![
                "fileprovider-invalidation".to_string(),
                "downloaded".to_string(),
                item.display().to_string(),
            ],
            Some("fileprovider invalidation"),
        ),
        (
            vec![
                "fileprovider-metadata-invalidation".to_string(),
                "downloaded".to_string(),
                item.display().to_string(),
            ],
            Some("fileprovider metadata invalidation"),
        ),
        (
            vec![
                "native-icon-fileprovider-invalidation".to_string(),
                "downloaded".to_string(),
                item.display().to_string(),
            ],
            Some("native icon fileprovider"),
        ),
        (
            vec![
                "fileprovider-operation".to_string(),
                "download".to_string(),
                item.display().to_string(),
            ],
            Some("fileprovider operation"),
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
            .args(&args)
            .output()
            .unwrap();

        assert!(!output.status.success(), "{args:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stdout.contains("fileprovider-"), "{args:?}: {stdout}");
        assert!(
            !stdout.contains("native-icon-invalidation\t"),
            "{args:?}: {stdout}"
        );
        assert!(
            stderr.contains("volume access blocked: unreachable volume network"),
            "{args:?}: {stderr}"
        );
        if let Some(worker) = blocked_worker {
            assert!(
                !stderr.contains(&format!(
                    "security-worker-admission\tworker={worker}\tpath={}",
                    item.display()
                )),
                "{args:?}: {stderr}"
            );
        }
    }

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn fileprovider_snapshot_routes_refuse_unreachable_volume_before_state_persistence_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-snapshot-unreachable-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let state = root.join("fileprovider-state-unavailable".repeat(16));
    let item = root.join("Remote.icloud-placeholder");
    std::fs::write(&item, "placeholder").unwrap();
    mark_evicted_fixture(&item);

    let scan = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-invalidation-scan")
        .arg(&state)
        .arg(&item)
        .output()
        .unwrap();

    assert!(!scan.status.success());
    let scan_stdout = String::from_utf8_lossy(&scan.stdout);
    let scan_stderr = String::from_utf8_lossy(&scan.stderr);
    assert!(
        !scan_stdout.contains("fileprovider-state-invalidation\t"),
        "{scan_stdout}"
    );
    assert!(
        scan_stderr.contains(
            "fileprovider invalidation scan volume access blocked: unreachable volume network"
        ),
        "{scan_stderr}"
    );
    assert!(
        !scan_stderr.contains("platform write path metadata unavailable"),
        "{scan_stderr}"
    );
    assert!(!state.exists());

    let event = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-invalidation-event")
        .arg(&state)
        .arg("metadata")
        .arg(&item)
        .output()
        .unwrap();

    assert!(!event.status.success());
    let event_stdout = String::from_utf8_lossy(&event.stdout);
    let event_stderr = String::from_utf8_lossy(&event.stderr);
    assert!(
        !event_stdout.contains("fileprovider-observed-invalidation\t"),
        "{event_stdout}"
    );
    assert!(
        event_stderr.contains(
            "fileprovider invalidation event volume access blocked: unreachable volume network"
        ),
        "{event_stderr}"
    );
    assert!(
        !event_stderr.contains("platform write path metadata unavailable"),
        "{event_stderr}"
    );
    assert!(!state.exists());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn fileprovider_snapshot_routes_report_state_probe_failure_before_worker_io_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-snapshot-probe-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join(format!(
        "{}.tsv",
        "fileprovider-state-unavailable".repeat(16)
    ));
    let item = root.join("Remote.icloud-placeholder");
    std::fs::write(&item, "placeholder").unwrap();
    mark_evicted_fixture(&item);

    let scan = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-invalidation-scan")
        .arg(&state)
        .arg(&item)
        .output()
        .unwrap();

    assert!(!scan.status.success());
    let scan_stdout = String::from_utf8_lossy(&scan.stdout);
    let scan_stderr = String::from_utf8_lossy(&scan.stderr);
    assert!(
        !scan_stdout.contains("fileprovider-state-invalidation\t"),
        "{scan_stdout}"
    );
    assert!(
        scan_stderr.contains("platform write path metadata unavailable"),
        "{scan_stderr}"
    );
    assert!(
        scan_stderr.contains("fileprovider-state-unavailable"),
        "{scan_stderr}"
    );
    assert!(
        !scan_stderr.contains("security-worker-admission\tworker=fileprovider invalidation scan"),
        "{scan_stderr}"
    );

    let event = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-invalidation-event")
        .arg(&state)
        .arg("metadata")
        .arg(&item)
        .output()
        .unwrap();

    assert!(!event.status.success());
    let event_stdout = String::from_utf8_lossy(&event.stdout);
    let event_stderr = String::from_utf8_lossy(&event.stderr);
    assert!(
        !event_stdout.contains("fileprovider-observed-invalidation\t"),
        "{event_stdout}"
    );
    assert!(
        event_stderr.contains("platform write path metadata unavailable"),
        "{event_stderr}"
    );
    assert!(
        event_stderr.contains("fileprovider-state-unavailable"),
        "{event_stderr}"
    );
    assert!(
        !event_stderr.contains("security-worker-admission\tworker=fileprovider invalidation event"),
        "{event_stderr}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_sidebar_fileprovider_invalidation_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-sidebar-fileprovider-invalidation-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let evicted = root.join("Remote.icloud-placeholder");
    std::fs::write(&evicted, "placeholder").unwrap();
    mark_evicted_fixture(&evicted);

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-sidebar-fileprovider-invalidation")
        .arg("downloaded")
        .arg(&evicted)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("sidebar-cloud-invalidation\ticloud-drive\tpath="));
    assert!(stdout.contains("\tprevious=available-offline\tcurrent=cloud-only\t"));
    assert!(stdout.contains(
        "\tprogress=0\tprogress-source=state\tprogress-reason=remote-placeholder\tinvalidate-row=true\t"
    ));
    assert!(stdout.ends_with("reason=sidebar-cloud-state-changed\n"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_sidebar_fileprovider_observed_invalidation_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-sidebar-fileprovider-observed-invalidation-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("fileprovider-state.tsv");
    let evicted = root.join("Remote.icloud-placeholder");
    std::fs::write(&evicted, "placeholder").unwrap();
    mark_evicted_fixture(&evicted);
    std::fs::write(
        &state,
        format!(
            "gfm-fileprovider-state-v1\ndownloaded\t{}\n",
            evicted.display()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-sidebar-fileprovider-observed-invalidation")
        .arg(&state)
        .arg("metadata")
        .arg(&evicted)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with(
        "fileprovider-observed-invalidation\tevents=1\tevent-kinds=metadata\tpaths=1\n"
    ));
    assert!(stdout.contains(
        "fileprovider-state-invalidation\tinitialized=false\tchanged=1\ticon=true\tpreview-memory=true\tpreview-disk=true\tsidebar=true\treindex-metadata=true\n"
    ));
    assert!(stdout.contains("\tprevious=downloaded\tcurrent=evicted\tchanged=true\t"));
    assert!(stdout.contains("\nsidebar-cloud-invalidation\ticloud-drive\tpath="));
    assert!(stdout.contains("\tprevious=available-offline\tcurrent=cloud-only\t"));
    assert!(stdout.contains(
        "\tprogress=0\tprogress-source=state\tprogress-reason=remote-placeholder\tinvalidate-row=true\t"
    ));
    assert!(stdout.ends_with("reason=sidebar-cloud-state-changed\n"));
    let state_text = std::fs::read_to_string(&state).unwrap();
    assert!(state_text.contains("\nevicted\t"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn sidebar_fileprovider_observed_invalidation_skips_snapshot_publish_for_local_only_noop() {
    let root = std::env::temp_dir().join(format!(
        "gfm-sidebar-fileprovider-observed-noop-publish-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("fileprovider-state.tsv");
    let local = root.join("Remote.icloud");
    std::fs::write(&local, "ordinary local file with provider-shaped extension").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-sidebar-fileprovider-observed-invalidation")
        .arg(&state)
        .arg("metadata")
        .arg(&local)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with(
        "fileprovider-observed-invalidation\tevents=1\tevent-kinds=metadata\tpaths=0"
    ));
    assert!(!stdout.contains("sidebar-cloud-invalidation\t"));
    assert!(!state.exists());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn sidebar_fileprovider_rename_deduplicates_repeated_event_path_admission_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-sidebar-fileprovider-rename-dedup-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("fileprovider-state.tsv");
    let item = root.join("Remote.icloud-placeholder");
    std::fs::write(&item, "placeholder").unwrap();
    mark_evicted_fixture(&item);
    std::fs::write(
        &state,
        format!("gfm-fileprovider-state-v1\nevicted\t{}\n", item.display()),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-sidebar-fileprovider-observed-invalidation")
        .arg(&state)
        .arg("rename")
        .arg(&item)
        .arg(&item)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        worker_admission_count_with_intent(
            &stderr,
            "ui fileprovider sidebar observed invalidation",
            &root,
            "read",
        ),
        1,
        "{stderr}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_sidebar_fileprovider_observer_probe_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-sidebar-fileprovider-observer-probe-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("fileprovider-state.tsv");
    let item = root.join("Remote.icloud-placeholder");
    std::fs::write(&item, "placeholder").unwrap();
    mark_evicted_fixture(&item);
    std::fs::write(
        &state,
        format!(
            "gfm-fileprovider-state-v1\ndownloaded\t{}\n",
            item.display()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-sidebar-fileprovider-observer-probe")
        .arg(&state)
        .arg(&root)
        .arg(&item)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(&stderr, "ui fileprovider sidebar observer root", &root);
    assert_worker_admitted(&stderr, "ui fileprovider sidebar observer target", &root);
    assert_worker_admitted(&stderr, "ui fileprovider sidebar observer state", &root);
    assert_worker_admitted(&stderr, "ui fileprovider sidebar observer state", &item);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("fileprovider-observed-invalidation\t"));
    assert!(stdout.contains("\tevent-kinds="));
    assert!(stdout.contains("\tpaths=1\n"));
    assert!(stdout.contains(
        "fileprovider-state-invalidation\tinitialized=false\tchanged=1\ticon=true\tpreview-memory=true\tpreview-disk=true\tsidebar=true\treindex-metadata=true\n"
    ));
    assert!(stdout.contains("\tprevious=downloaded\tcurrent=evicted\tchanged=true\t"));
    assert!(stdout.contains("\nsidebar-cloud-invalidation\ticloud-drive\tpath="));
    assert!(stdout.contains("\tprevious=available-offline\tcurrent=cloud-only\t"));
    assert!(stdout.contains(
        "\tprogress=0\tprogress-source=state\tprogress-reason=remote-placeholder\tinvalidate-row=true\t"
    ));
    assert!(stdout.ends_with("reason=sidebar-cloud-state-changed\n"));
    let state_text = std::fs::read_to_string(&state).unwrap();
    assert_fileprovider_state_entry(&state_text, "evicted", &item);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn sidebar_fileprovider_observed_invalidation_removes_deleted_tracked_subtree_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-sidebar-fileprovider-observed-subtree-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("fileprovider-state.tsv");
    let removed_dir = root.join("Removed.icloud");
    let child = removed_dir.join("Child.icloud.md");
    std::fs::create_dir_all(&removed_dir).unwrap();
    std::fs::write(&child, "downloaded").unwrap();
    std::fs::write(
        &state,
        format!(
            "gfm-fileprovider-state-v1\ndownloaded\t{}\n",
            child.display()
        ),
    )
    .unwrap();
    std::fs::remove_dir_all(&removed_dir).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-sidebar-fileprovider-observed-invalidation")
        .arg(&state)
        .arg("remove")
        .arg(&removed_dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with(
        "fileprovider-observed-invalidation\tevents=1\tevent-kinds=remove\tpaths=1\n"
    ));
    assert!(stdout.contains(&format!(
        "fileprovider-invalidation\t{}\tprevious=downloaded\tcurrent=removed\tchanged=true\t",
        child.display()
    )));
    assert!(stdout.contains(&format!(
        "sidebar-cloud-invalidation\ticloud-drive\tpath={}\tprevious=available-offline\tcurrent=unavailable\tprogress=-\tprogress-source=state\tprogress-reason=fileprovider-item-removed\tinvalidate-row=true\t",
        child.display()
    )));
    assert!(stdout.ends_with("reason=sidebar-cloud-state-changed\n"));
    let state_text = std::fs::read_to_string(&state).unwrap();
    assert!(!state_text.contains(&child.display().to_string()));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn persists_fileprovider_invalidation_scan_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-invalidation-scan-persist-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("fileprovider-state.tsv");
    let evicted = root.join("Remote.icloud-placeholder");
    std::fs::write(&evicted, "placeholder").unwrap();
    mark_evicted_fixture(&evicted);

    let first = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-invalidation-scan")
        .arg(&state)
        .arg(&evicted)
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first_stdout = String::from_utf8(first.stdout).unwrap();
    assert!(
        first_stdout.starts_with("fileprovider-state-invalidation\tinitialized=true\tchanged=1\t")
    );
    assert!(first_stdout.contains("\tcurrent=evicted\t"));
    assert!(state.is_file());

    let second = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-invalidation-scan")
        .arg(&state)
        .arg(&evicted)
        .output()
        .unwrap();
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let second_stdout = String::from_utf8(second.stdout).unwrap();
    assert!(second_stdout
        .starts_with("fileprovider-state-invalidation\tinitialized=false\tchanged=0\t"));
    assert!(!second_stdout.contains("\nfileprovider-invalidation\t"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn fileprovider_invalidation_scan_detects_v2_signature_changes_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-invalidation-scan-signature-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("fileprovider-state.tsv");
    let evicted = root.join("Remote.icloud-placeholder");
    std::fs::write(&evicted, "placeholder").unwrap();
    mark_evicted_fixture(&evicted);
    std::fs::write(
        &state,
        format!(
            "gfm-fileprovider-state-v2\nevicted\tstale-signature\t{}\n",
            evicted.display()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-invalidation-scan")
        .arg(&state)
        .arg(&evicted)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("fileprovider-state-invalidation\tinitialized=false\tchanged=1\t"));
    assert!(stdout.contains("\tprevious=evicted\tcurrent=evicted\tchanged=false\t"));
    assert!(stdout.contains("\tcurrent-materialization=remote-placeholder\t"));
    assert!(stdout.contains("\tcurrent-badges=cloud\t"));
    assert!(stdout.ends_with("reason=fileprovider-state-signature-changed\n"));
    let state_text = std::fs::read_to_string(&state).unwrap();
    assert!(state_text.starts_with("gfm-fileprovider-state-v2\n"));
    assert!(state_text.contains("badges=cloud"));
    assert!(!state_text.contains("stale-signature"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn fileprovider_invalidation_scan_preserves_unscanned_snapshot_entries_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-invalidation-scan-preserve-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("fileprovider-state.tsv");
    let unchanged = root.join("Unchanged.icloud-placeholder");
    let changed = root.join("Changed.icloud-placeholder");
    std::fs::write(&unchanged, "unchanged").unwrap();
    std::fs::write(&changed, "changed").unwrap();
    mark_evicted_fixture(&unchanged);
    mark_evicted_fixture(&changed);
    std::fs::write(
        &state,
        format!(
            "gfm-fileprovider-state-v1\nevicted\t{}\ndownloaded\t{}\n",
            unchanged.display(),
            changed.display()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-invalidation-scan")
        .arg(&state)
        .arg(&changed)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("fileprovider-state-invalidation\tinitialized=false\tchanged=1\t"));
    assert!(stdout.contains(&format!(
        "fileprovider-invalidation\t{}\tprevious=downloaded\tcurrent=evicted\tchanged=true\t",
        changed.display()
    )));
    let state_text = std::fs::read_to_string(&state).unwrap();
    assert!(state_text.starts_with("gfm-fileprovider-state-v2\n"));
    assert!(state_text.contains(&unchanged.display().to_string()));
    assert!(state_text.contains(&changed.display().to_string()));
    assert!(state_text.contains("badges=cloud"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn fileprovider_invalidation_scan_refuses_invalid_persisted_state_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-invalidation-scan-corrupt-state-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("fileprovider-state.tsv");
    let evicted = root.join("Remote.icloud-placeholder");
    std::fs::write(&evicted, "placeholder").unwrap();
    mark_evicted_fixture(&evicted);
    std::fs::write(
        &state,
        format!("gfm-fileprovider-state-v1\nbroken\t{}\n", evicted.display()),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-invalidation-scan")
        .arg(&state)
        .arg(&evicted)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("fileprovider-state-invalidation\t"),
        "{stdout}"
    );
    assert!(
        stderr.contains("invalid FileProvider state `broken`"),
        "{stderr}"
    );
    assert!(
        stderr.contains("unsupported FileProvider storage state `broken`"),
        "{stderr}"
    );
    let state_text = std::fs::read_to_string(&state).unwrap();
    assert!(state_text.contains("broken\t"), "{state_text}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn fileprovider_invalidation_scan_refuses_duplicate_persisted_state_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-invalidation-scan-duplicate-state-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("fileprovider-state.tsv");
    let evicted = root.join("Remote.icloud-placeholder");
    std::fs::write(&evicted, "placeholder").unwrap();
    mark_evicted_fixture(&evicted);
    std::fs::write(
        &state,
        format!(
            "gfm-fileprovider-state-v1\nevicted\t{}\ndownloaded\t{}\n",
            evicted.display(),
            evicted.display()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-invalidation-scan")
        .arg(&state)
        .arg(&evicted)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("fileprovider-state-invalidation\t"),
        "{stdout}"
    );
    assert!(
        stderr.contains("duplicate FileProvider state path"),
        "{stderr}"
    );
    assert!(stderr.contains(":3"), "{stderr}");
    let state_text = std::fs::read_to_string(&state).unwrap();
    assert!(state_text.contains("downloaded\t"), "{state_text}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn fileprovider_invalidation_scan_creates_nested_state_parent_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-invalidation-scan-parent-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("runtime").join("fileprovider-state.tsv");
    let evicted = root.join("Remote.icloud-placeholder");
    std::fs::write(&evicted, "placeholder").unwrap();
    mark_evicted_fixture(&evicted);

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-invalidation-scan")
        .arg(&state)
        .arg(&evicted)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("fileprovider-state-invalidation\tinitialized=true\tchanged=1\t"));
    assert!(state.is_file());
    assert!(std::fs::read_to_string(&state)
        .unwrap()
        .contains("evicted\t"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn fileprovider_invalidation_scan_skips_name_only_local_paths_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-invalidation-scan-local-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("fileprovider-state.tsv");
    let local = root.join("Downloaded.icloud.md");
    std::fs::write(&local, "local").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-invalidation-scan")
        .arg(&state)
        .arg(&local)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("fileprovider-state-invalidation\tinitialized=true\tchanged=0\t"));
    assert!(stdout.contains("\ticon=false\tpreview-memory=false\tpreview-disk=false\t"));
    assert!(stdout.ends_with("sidebar=false\treindex-metadata=false\n"));
    assert!(!state.exists());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn fileprovider_invalidation_scan_skips_native_local_icloud_extension_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-invalidation-scan-local-extension-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("fileprovider-state.tsv");
    let local = root.join("Remote.icloud");
    std::fs::write(&local, "ordinary local file with provider-shaped extension").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-invalidation-scan")
        .arg(&state)
        .arg(&local)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("fileprovider-state-invalidation\tinitialized=true\tchanged=0\t"));
    assert!(stdout.contains("\ticon=false\tpreview-memory=false\tpreview-disk=false\t"));
    assert!(stdout.ends_with("sidebar=false\treindex-metadata=false\n"));
    assert!(!state.exists());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn fileprovider_invalidation_scan_removes_tracked_entry_without_provider_evidence_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-invalidation-scan-untracked-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("fileprovider-state.tsv");
    let local = root.join("Downloaded.icloud.md");
    std::fs::write(&local, "local").unwrap();
    std::fs::write(
        &state,
        format!(
            "gfm-fileprovider-state-v1\ndownloaded\t{}\n",
            local.display()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-invalidation-scan")
        .arg(&state)
        .arg(&local)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("fileprovider-state-invalidation\tinitialized=false\tchanged=1\t"));
    assert!(stdout.contains("\tprevious=downloaded\tcurrent=local-only\tchanged=true\t"));
    assert!(stdout.contains("\ticon=true\tpreview-memory=true\tpreview-disk=true\t"));
    assert!(stdout
        .ends_with("sidebar=true\treindex-metadata=true\treason=fileprovider-state-changed\n"));
    assert_eq!(
        std::fs::read_to_string(&state).unwrap(),
        "gfm-fileprovider-state-v2\n"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn fileprovider_progress_job_refuses_unreachable_journal_before_provider_read_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-progress-journal-blocked-{}",
        std::process::id()
    ));
    let offline = std::env::temp_dir().join(format!(
        "gfm-fileprovider-progress-journal-offline-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&offline);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&offline).unwrap();
    std::fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let downloading = root.join("Downloading.icloud-downloading.md");
    let progress = root.join("progress.gfmprogress");
    let catalog = root.join("payloads.gfmjobs");
    let journal = offline.join(format!(
        "{}.gfmjournal",
        "fileprovider-progress-journal-unavailable".repeat(8)
    ));
    std::fs::write(&downloading, "downloading").unwrap();
    xattr::set(&downloading, "com.apple.fileprovider.state", b"downloading").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .env("GFM_JOB_PAYLOAD_CATALOG", &catalog)
        .env("GFM_JOB_JOURNAL", &journal)
        .arg("fileprovider-progress-job")
        .arg(&downloading)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("fileprovider-progress\t"), "{stdout}");
    assert!(
        stderr.contains(
            "fileprovider progress job volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert!(
        !stderr.contains("platform write path metadata unavailable"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(&format!(
            "security-worker-admission\tworker=fileprovider progress job\tpath={}",
            downloading.display()
        )),
        "{stderr}"
    );
    assert!(!progress.exists());
    assert!(!catalog.exists());
    assert!(!journal.exists());

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(offline);
}

#[test]
fn reports_fileprovider_invalidation_scan_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-invalidation-scan-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("fileprovider-state.tsv");
    let item = root.join("Remote.icloud-placeholder");
    std::fs::write(&item, "placeholder").unwrap();
    mark_evicted_fixture(&item);

    let initial = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-invalidation-scan")
        .arg(&state)
        .arg(&item)
        .output()
        .unwrap();
    assert!(
        initial.status.success(),
        "{}",
        String::from_utf8_lossy(&initial.stderr)
    );
    let initial_stdout = String::from_utf8(initial.stdout).unwrap();
    assert!(initial_stdout
        .starts_with("fileprovider-state-invalidation\tinitialized=true\tchanged=1\t"));
    assert!(initial_stdout.contains("\tcurrent=evicted\tchanged=true\t"));
    let state_text = std::fs::read_to_string(&state).unwrap();
    assert!(state_text.starts_with("gfm-fileprovider-state-v2\n"));
    assert!(state_text.contains("evicted\t"));
    assert!(state_text.contains("badges=cloud"));

    let unchanged = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-invalidation-scan")
        .arg(&state)
        .arg(&item)
        .output()
        .unwrap();
    assert!(
        unchanged.status.success(),
        "{}",
        String::from_utf8_lossy(&unchanged.stderr)
    );
    let unchanged_stdout = String::from_utf8(unchanged.stdout).unwrap();
    assert!(unchanged_stdout
        .starts_with("fileprovider-state-invalidation\tinitialized=false\tchanged=0\t"));

    let downloaded = root.join("Remote.icloud-downloaded");
    std::fs::rename(&item, &downloaded).unwrap();
    xattr::remove(&downloaded, "com.apple.icloud.placeholder").unwrap();
    xattr::set(&downloaded, "com.apple.fileprovider.state", b"downloaded").unwrap();
    let changed = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-invalidation-scan")
        .arg(&state)
        .arg(&downloaded)
        .output()
        .unwrap();
    assert!(
        changed.status.success(),
        "{}",
        String::from_utf8_lossy(&changed.stderr)
    );
    let changed_stdout = String::from_utf8(changed.stdout).unwrap();
    assert!(changed_stdout
        .starts_with("fileprovider-state-invalidation\tinitialized=false\tchanged=1\t"));
    assert!(changed_stdout.contains("\tcurrent=downloaded\tchanged=true\t"));
    assert!(changed_stdout.contains("\ticon=true\tpreview-memory=true\tpreview-disk=true\t"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_fileprovider_invalidation_event_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-invalidation-event-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("fileprovider-state.tsv");
    let item = root.join("Remote.icloud-placeholder");
    let untouched = root.join("Untouched.icloud-placeholder");
    std::fs::write(&item, "placeholder").unwrap();
    mark_evicted_fixture(&item);
    std::fs::write(&untouched, "placeholder").unwrap();
    mark_evicted_fixture(&untouched);
    std::fs::write(
        &state,
        format!(
            "gfm-fileprovider-state-v1\ndownloaded\t{}\nevicted\t{}\n",
            item.display(),
            untouched.display()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-invalidation-event")
        .arg(&state)
        .arg("metadata")
        .arg(&item)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with(
        "fileprovider-observed-invalidation\tevents=1\tevent-kinds=metadata\tpaths=1\n"
    ));
    assert!(
        stdout.contains("fileprovider-state-invalidation\tinitialized=false\tchanged=1\ticon=true")
    );
    assert!(stdout.contains("\tprevious=downloaded\tcurrent=evicted\tchanged=true\t"));
    let state_text = std::fs::read_to_string(&state).unwrap();
    assert!(state_text.contains("evicted\t"));
    assert!(state_text.contains(&untouched.display().to_string()));
    assert_eq!(
        state_text.matches("Untouched.icloud-placeholder").count(),
        1
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_observed_fileprovider_metadata_invalidation_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-observed-metadata-invalidation-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("fileprovider-state.tsv");
    let item = root.join("Remote.icloud-placeholder");
    std::fs::write(&item, "placeholder").unwrap();
    mark_evicted_fixture(&item);
    std::fs::write(
        &state,
        format!(
            "gfm-fileprovider-state-v1\ndownloaded\t{}\n",
            item.display()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-observed-metadata-invalidation")
        .arg(&state)
        .arg("metadata")
        .arg(&item)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with(
        "fileprovider-observed-invalidation\tevents=1\tevent-kinds=metadata\tpaths=1\n"
    ));
    assert!(stdout.contains(&format!(
        "fileprovider-observed-paths\tcount=1\tpaths={}\n",
        item.display()
    )));
    assert!(stdout.contains("provider-metadata-invalidation\t"));
    assert!(stdout.contains("\tprevious=downloaded\tcurrent=evicted\t"));
    assert!(stdout.contains(
        "\treindex-metadata=true\tschedule-metadata-update=true\tinvalidate-query-cache=true\t"
    ));
    assert!(stdout.ends_with("reason=provider-metadata-state-changed\n"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn observed_fileprovider_metadata_invalidation_schedules_same_state_provider_events_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-observed-metadata-same-state-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("fileprovider-state.tsv");
    let item = root.join("Remote.icloud-placeholder");
    std::fs::write(&item, "placeholder").unwrap();
    mark_evicted_fixture(&item);
    std::fs::write(
        &state,
        format!("gfm-fileprovider-state-v1\nevicted\t{}\n", item.display()),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-observed-metadata-invalidation")
        .arg(&state)
        .arg("metadata")
        .arg(&item)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with(
        "fileprovider-observed-invalidation\tevents=1\tevent-kinds=metadata\tpaths=1\n"
    ));
    assert!(stdout.contains(
        "fileprovider-state-invalidation\tinitialized=false\tchanged=1\ticon=true\tpreview-memory=true\tpreview-disk=false\tsidebar=true\treindex-metadata=true\n"
    ));
    assert!(stdout.contains("\tprevious=evicted\tcurrent=evicted\tchanged=false\t"));
    assert!(stdout.contains("\tcurrent-materialization=remote-placeholder\t"));
    assert!(stdout.contains("\tcurrent-badges=cloud\t"));
    assert!(stdout.contains("provider-metadata-invalidation\t"));
    assert!(stdout.contains("\tprevious=evicted\tcurrent=evicted\t"));
    assert!(stdout.contains(
        "\treindex-metadata=true\tschedule-metadata-update=true\tinvalidate-query-cache=true\t"
    ));
    assert!(stdout.ends_with("reason=fileprovider-observed-metadata-changed\n"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn observed_fileprovider_rename_deduplicates_repeated_event_path_admission_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-observed-rename-dedup-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("fileprovider-state.tsv");
    let item = root.join("Remote.icloud-placeholder");
    std::fs::write(&item, "placeholder").unwrap();
    mark_evicted_fixture(&item);
    std::fs::write(
        &state,
        format!("gfm-fileprovider-state-v1\nevicted\t{}\n", item.display()),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-observed-metadata-invalidation")
        .arg(&state)
        .arg("rename")
        .arg(&item)
        .arg(&item)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        worker_admission_count_with_intent(
            &stderr,
            "fileprovider observed metadata invalidation",
            &root,
            "read",
        ),
        1,
        "{stderr}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn fileprovider_invalidation_event_persists_renamed_tracked_provider_state_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-invalidation-rename-persist-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("fileprovider-state.tsv");
    let old_dir = root.join("Old.icloud");
    let new_dir = root.join("New.icloud");
    let old_child = old_dir.join("Child.icloud-placeholder");
    let new_child = new_dir.join("Child.icloud-placeholder");
    std::fs::create_dir_all(&old_dir).unwrap();
    std::fs::write(&old_child, "placeholder").unwrap();
    mark_evicted_fixture(&old_child);
    std::fs::write(
        &state,
        format!(
            "gfm-fileprovider-state-v1\ndownloaded\t{}\n",
            old_child.display()
        ),
    )
    .unwrap();
    std::fs::rename(&old_dir, &new_dir).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-invalidation-event")
        .arg(&state)
        .arg("rename")
        .arg(&old_dir)
        .arg(&new_dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with(
        "fileprovider-observed-invalidation\tevents=1\tevent-kinds=rename\tpaths=3\n"
    ));
    assert!(stdout.contains(&format!(
        "fileprovider-invalidation\t{}\tprevious=downloaded\tcurrent=removed\tchanged=true\t",
        old_child.display()
    )));
    assert!(stdout.contains(&format!(
        "fileprovider-invalidation\t{}\tprevious=local-only\tcurrent=evicted\tchanged=true\t",
        new_child.display()
    )));
    let state_text = std::fs::read_to_string(&state).unwrap();
    assert!(!state_text.contains(&old_child.display().to_string()));
    assert!(state_text.starts_with("gfm-fileprovider-state-v2\n"));
    assert!(state_text.contains(&new_child.display().to_string()));
    assert!(state_text.contains("badges=cloud"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn observed_fileprovider_metadata_invalidation_preserves_noop_events_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-observed-metadata-noop-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("fileprovider-state.tsv");
    let provider = root.join("Remote.icloud-placeholder");
    let local = root.join("Remote.fileprovider");
    std::fs::write(&provider, "placeholder").unwrap();
    mark_evicted_fixture(&provider);
    std::fs::write(&local, "local").unwrap();
    std::fs::write(
        &state,
        format!(
            "gfm-fileprovider-state-v1\nevicted\t{}\n",
            provider.display()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-observed-metadata-invalidation")
        .arg(&state)
        .arg("modify")
        .arg(&local)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout,
        "fileprovider-observed-invalidation\tevents=1\tevent-kinds=modify\tpaths=0\nfileprovider-state-invalidation\tinitialized=false\tchanged=0\ticon=false\tpreview-memory=false\tpreview-disk=false\tsidebar=false\treindex-metadata=false\n"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn fileprovider_invalidation_event_removes_deleted_tracked_item_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-invalidation-remove-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("fileprovider-state.tsv");
    let item = root.join("Downloaded.icloud.md");
    let untouched = root.join("Untouched.icloud-placeholder");
    std::fs::write(&item, "downloaded").unwrap();
    std::fs::write(&untouched, "placeholder").unwrap();
    mark_evicted_fixture(&untouched);
    std::fs::write(
        &state,
        format!(
            "gfm-fileprovider-state-v1\ndownloaded\t{}\nevicted\t{}\n",
            item.display(),
            untouched.display()
        ),
    )
    .unwrap();
    std::fs::remove_file(&item).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-invalidation-event")
        .arg(&state)
        .arg("remove")
        .arg(&item)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with(
        "fileprovider-observed-invalidation\tevents=1\tevent-kinds=remove\tpaths=1\n"
    ));
    assert!(stdout.contains("\tprevious=downloaded\tcurrent=removed\tchanged=true\t"));
    assert!(stdout.contains("\ticon=true\tpreview-memory=true\tpreview-disk=true\t"));
    assert!(stdout.contains("\tsidebar=true\treindex-metadata=true\t"));
    let state_text = std::fs::read_to_string(&state).unwrap();
    assert!(!state_text.contains(&item.display().to_string()));
    assert!(state_text.starts_with("gfm-fileprovider-state-v2\n"));
    assert!(state_text.contains(&untouched.display().to_string()));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn fileprovider_invalidation_event_removes_deleted_tracked_subtree_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-invalidation-remove-subtree-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("fileprovider-state.tsv");
    let removed_dir = root.join("Removed.icloud");
    let child = removed_dir.join("Child.icloud.md");
    let untouched = root.join("Untouched.icloud-placeholder");
    std::fs::create_dir_all(&removed_dir).unwrap();
    std::fs::write(&child, "downloaded").unwrap();
    std::fs::write(&untouched, "placeholder").unwrap();
    mark_evicted_fixture(&untouched);
    std::fs::write(
        &state,
        format!(
            "gfm-fileprovider-state-v1\ndownloaded\t{}\nevicted\t{}\n",
            child.display(),
            untouched.display()
        ),
    )
    .unwrap();
    std::fs::remove_dir_all(&removed_dir).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-invalidation-event")
        .arg(&state)
        .arg("remove")
        .arg(&removed_dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with(
        "fileprovider-observed-invalidation\tevents=1\tevent-kinds=remove\tpaths=1\n"
    ));
    assert!(stdout.contains(&format!(
        "fileprovider-invalidation\t{}\tprevious=downloaded\tcurrent=removed\tchanged=true\t",
        child.display()
    )));
    assert!(stdout.contains("\ticon=true\tpreview-memory=true\tpreview-disk=true\t"));
    assert!(stdout.contains("\tsidebar=true\treindex-metadata=true\t"));
    let state_text = std::fs::read_to_string(&state).unwrap();
    assert!(!state_text.contains(&child.display().to_string()));
    assert!(state_text.starts_with("gfm-fileprovider-state-v2\n"));
    assert!(state_text.contains(&untouched.display().to_string()));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn fileprovider_invalidation_event_ignores_existing_local_file_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-invalidation-local-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("fileprovider-state.tsv");
    let tracked = root.join("Remote.icloud-placeholder");
    let local = root.join("Notes.txt");
    std::fs::write(&tracked, "placeholder").unwrap();
    mark_evicted_fixture(&tracked);
    std::fs::write(&local, "ordinary local file").unwrap();
    std::fs::write(
        &state,
        format!(
            "gfm-fileprovider-state-v1\nevicted\t{}\n",
            tracked.display()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-invalidation-event")
        .arg(&state)
        .arg("modify")
        .arg(&local)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout,
        "fileprovider-observed-invalidation\tevents=1\tevent-kinds=modify\tpaths=0\nfileprovider-state-invalidation\tinitialized=false\tchanged=0\ticon=false\tpreview-memory=false\tpreview-disk=false\tsidebar=false\treindex-metadata=false\n"
    );
    let state_text = std::fs::read_to_string(&state).unwrap();
    assert_eq!(
        state_text,
        format!(
            "gfm-fileprovider-state-v1\nevicted\t{}\n",
            tracked.display()
        )
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn fileprovider_invalidation_event_ignores_native_local_icloud_extension_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-invalidation-local-extension-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("fileprovider-state.tsv");
    let tracked = root.join("Remote.icloud-placeholder");
    let local = root.join("Local.icloud");
    std::fs::write(&tracked, "placeholder").unwrap();
    mark_evicted_fixture(&tracked);
    std::fs::write(&local, "ordinary local file with provider-shaped extension").unwrap();
    std::fs::write(
        &state,
        format!(
            "gfm-fileprovider-state-v1\nevicted\t{}\n",
            tracked.display()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-invalidation-event")
        .arg(&state)
        .arg("modify")
        .arg(&local)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout,
        "fileprovider-observed-invalidation\tevents=1\tevent-kinds=modify\tpaths=0\nfileprovider-state-invalidation\tinitialized=false\tchanged=0\ticon=false\tpreview-memory=false\tpreview-disk=false\tsidebar=false\treindex-metadata=false\n"
    );
    let state_text = std::fs::read_to_string(&state).unwrap();
    assert_eq!(
        state_text,
        format!(
            "gfm-fileprovider-state-v1\nevicted\t{}\n",
            tracked.display()
        )
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_fileprovider_observer_probe_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-observer-probe-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("fileprovider-state.tsv");
    let item = root.join("Remote.icloud-placeholder");
    std::fs::write(&item, "placeholder").unwrap();
    mark_evicted_fixture(&item);
    std::fs::write(
        &state,
        format!(
            "gfm-fileprovider-state-v1\ndownloaded\t{}\n",
            item.display()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-observer-probe")
        .arg(&state)
        .arg(&root)
        .arg(&item)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(&stderr, "fileprovider observer root", &root);
    assert_worker_admitted(&stderr, "fileprovider observer target", &root);
    assert_worker_admitted(&stderr, "fileprovider observer state", &root);
    assert_worker_admitted(&stderr, "fileprovider observer state", &item);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("fileprovider-observed-invalidation\t"));
    assert!(stdout.contains("\tevent-kinds="));
    assert!(stdout.contains("\tpaths=1\n"));
    assert!(
        stdout.contains("fileprovider-state-invalidation\tinitialized=false\tchanged=1\ticon=true")
    );
    assert!(stdout.contains("\tprevious=downloaded\tcurrent=evicted\tchanged=true\t"));
    let state_text = std::fs::read_to_string(&state).unwrap();
    assert!(state_text.contains("evicted\t"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn fileprovider_observer_probe_publishes_same_state_metadata_refresh_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-observer-noop-publish-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("fileprovider-state.tsv");
    let item = root.join("Remote.icloud-placeholder");
    std::fs::write(&item, "placeholder").unwrap();
    mark_evicted_fixture(&item);
    let state_text = format!("gfm-fileprovider-state-v1\nevicted\t{}\n", item.display());
    std::fs::write(&state, &state_text).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-observer-probe")
        .arg(&state)
        .arg(&root)
        .arg(&item)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("fileprovider-observed-invalidation\t"));
    assert!(stdout.contains("\tpaths=1\n"));
    assert!(stdout.contains(
        "fileprovider-state-invalidation\tinitialized=false\tchanged=1\ticon=true\tpreview-memory=true\tpreview-disk=false\tsidebar=true\treindex-metadata=true"
    ));
    let refreshed_state_text = std::fs::read_to_string(&state).unwrap();
    assert!(refreshed_state_text.starts_with("gfm-fileprovider-state-v2\n"));
    assert_fileprovider_state_entry(&refreshed_state_text, "evicted", &item);
    assert!(refreshed_state_text.contains("badges=cloud"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_native_icon_fileprovider_observer_probe_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-native-icon-fileprovider-observer-probe-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("fileprovider-state.tsv");
    let item = root.join("Remote.icloud-placeholder");
    std::fs::write(&item, "placeholder").unwrap();
    mark_evicted_fixture(&item);
    std::fs::write(
        &state,
        format!(
            "gfm-fileprovider-state-v1\ndownloaded\t{}\n",
            item.display()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("native-icon-fileprovider-observer-probe")
        .arg(&state)
        .arg(&root)
        .arg(&item)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(&stderr, "native icon fileprovider observer root", &root);
    assert_worker_admitted(&stderr, "native icon fileprovider observer target", &root);
    assert_worker_admitted(&stderr, "native icon fileprovider observer state", &root);
    assert_worker_admitted(&stderr, "native icon fileprovider observer state", &item);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("fileprovider-observed-invalidation\t"));
    assert!(stdout.contains("\tevent-kinds="));
    assert!(stdout.contains("\tpaths=1\n"));
    assert!(stdout
        .contains("fileprovider-state-invalidation\tinitialized=false\tchanged=1\ticon=true\t"));
    assert!(stdout.contains("fileprovider-invalidation\t"));
    assert!(stdout.contains("native-icon-invalidation\t"));
    assert!(stdout.contains("\tprevious=downloaded\tcurrent=evicted\t"));
    assert!(stdout.contains("\tprevious-badges=cloud-available-offline\tcurrent-badges=cloud\t"));
    assert!(stdout.ends_with("invalidate-cache=true\treason=native-icon-badges-changed\n"));
    let state_text = std::fs::read_to_string(&state).unwrap();
    assert_fileprovider_state_entry(&state_text, "evicted", &item);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_fileprovider_observer_metadata_probe_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-observer-metadata-probe-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("fileprovider-state.tsv");
    let item = root.join("Remote.icloud-placeholder");
    std::fs::write(&item, "placeholder").unwrap();
    mark_evicted_fixture(&item);
    std::fs::write(
        &state,
        format!(
            "gfm-fileprovider-state-v1\ndownloaded\t{}\n",
            item.display()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-observer-metadata-probe")
        .arg(&state)
        .arg(&root)
        .arg(&item)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(&stderr, "fileprovider observer metadata root", &root);
    assert_worker_admitted(&stderr, "fileprovider observer metadata target", &root);
    assert_worker_admitted(&stderr, "fileprovider observer metadata state", &root);
    assert_worker_admitted(&stderr, "fileprovider observer metadata state", &item);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("fileprovider-observed-invalidation\t"));
    assert!(stdout.contains("\tpaths=1\n"));
    assert!(stdout.contains("provider-metadata-invalidation\t"));
    assert!(stdout.contains("\tprevious=downloaded\tcurrent=evicted\t"));
    assert!(stdout.contains(
        "\treindex-metadata=true\tschedule-metadata-update=true\tinvalidate-query-cache=true\t"
    ));
    assert!(stdout.ends_with("reason=provider-metadata-state-changed\n"));
    let state_text = std::fs::read_to_string(&state).unwrap();
    assert!(state_text.contains("evicted\t"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_preview_cache_fileprovider_observer_probe_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-preview-cache-fileprovider-observer-probe-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let cache = root.join("cache");
    let state = root.join("fileprovider-state.tsv");
    let item = root.join("Remote.icloud-placeholder");
    std::fs::write(&item, "placeholder").unwrap();
    mark_evicted_fixture(&item);
    std::fs::write(
        &state,
        format!(
            "gfm-fileprovider-state-v1\ndownloaded\t{}\n",
            item.display()
        ),
    )
    .unwrap();
    let mut seeded_cache = PreviewCache::new(PreviewCacheConfig::new(&cache)).unwrap();
    let seeded_key = PreviewRequestKey::new(
        FileId::new(VolumeId(42), 9001),
        item.clone(),
        PreviewKind::Thumbnail,
    );
    seeded_cache
        .insert(PreviewEntry::new(seeded_key, b"cached thumbnail".to_vec()))
        .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("preview-cache-fileprovider-observer-probe")
        .arg(&cache)
        .arg(&state)
        .arg("thumbnail")
        .arg(&root)
        .arg(&item)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_worker_admitted(&stderr, "preview cache fileprovider observer root", &root);
    assert_worker_admitted(&stderr, "preview cache fileprovider observer target", &root);
    assert_worker_admitted(&stderr, "preview cache fileprovider observer state", &root);
    assert_worker_admitted(&stderr, "preview cache fileprovider observer state", &item);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("fileprovider-observed-invalidation\t"));
    assert!(stdout.contains("\tevent-kinds="));
    assert!(stdout.contains("\tpaths=1\n"));
    assert!(stdout.contains(
        "fileprovider-state-invalidation\tinitialized=false\tchanged=1\ticon=true\tpreview-memory=true\tpreview-disk=true\t"
    ));
    assert!(stdout.contains("\tprevious=downloaded\tcurrent=evicted\tchanged=true\t"));
    assert!(stdout.contains("preview-cache-invalidation\t"));
    assert!(stdout.contains("\tkind=thumbnail\treason=content-or-icloud\t"));
    assert!(stdout.contains("\tinvalidate-memory=true\tinvalidate-disk=true\t"));
    assert!(stdout.contains("\tremoved-memory=false\tremoved-disk=true\n"));
    let state_text = std::fs::read_to_string(&state).unwrap();
    assert_fileprovider_state_entry(&state_text, "evicted", &item);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn fileprovider_sidebar_observed_rename_updates_snapshot_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-sidebar-rename-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("fileprovider-state.tsv");
    let old = root.join("Old.icloud-placeholder");
    let new = root.join("New.icloud-placeholder");
    std::fs::write(&new, "placeholder").unwrap();
    mark_evicted_fixture(&new);
    std::fs::write(
        &state,
        format!("gfm-fileprovider-state-v1\ndownloaded\t{}\n", old.display()),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-sidebar-fileprovider-observed-invalidation")
        .arg(&state)
        .arg("rename")
        .arg(&old)
        .arg(&new)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.starts_with(
            "fileprovider-observed-invalidation\tevents=1\tevent-kinds=rename\tpaths=2\n"
        ),
        "{stdout}"
    );
    assert!(stdout.contains(&format!(
        "fileprovider-invalidation\t{}\tprevious=downloaded\tcurrent=removed\tchanged=true\t",
        old.display()
    )));
    assert!(stdout.contains(&format!(
        "fileprovider-invalidation\t{}\tprevious=local-only\tcurrent=evicted\tchanged=true\t",
        new.display()
    )));
    assert!(stdout.contains(&format!(
        "sidebar-cloud-invalidation\ticloud-drive\tpath={}\tprevious=available-offline\tcurrent=unavailable\tprogress=-\tprogress-source=state\tprogress-reason=fileprovider-item-removed\tinvalidate-row=true\t",
        old.display()
    )));
    assert!(stdout.contains(&format!(
        "sidebar-cloud-invalidation\ticloud-drive\tpath={}\tprevious=none\tcurrent=cloud-only\tprogress=0\tprogress-source=state\tprogress-reason=remote-placeholder\tinvalidate-row=true\t",
        new.display()
    )));

    let state_text = std::fs::read_to_string(&state).unwrap();
    assert!(
        !state_text.contains(&old.display().to_string()),
        "{state_text}"
    );
    assert_fileprovider_state_entry(&state_text, "evicted", &new);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn fileprovider_observer_probe_refuses_unreachable_volume_before_watching_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-observer-unreachable-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let state = root.join("fileprovider-state.tsv");
    let item = root.join("Remote.icloud-placeholder");
    std::fs::write(&item, "placeholder").unwrap();
    mark_evicted_fixture(&item);

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-observer-probe")
        .arg(&state)
        .arg(&root)
        .arg(&item)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("fileprovider-observed-invalidation\t"),
        "{stdout}"
    );
    assert!(
        stderr.contains(
            "fileprovider observer root volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert!(!state.exists());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn fileprovider_observer_probe_refuses_unreachable_state_before_watching_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-observer-state-root-{}",
        std::process::id()
    ));
    let offline = std::env::temp_dir().join(format!(
        "gfm-fileprovider-observer-state-offline-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&offline);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&offline).unwrap();
    std::fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let state = offline.join("fileprovider-state.tsv");
    let target = root.join("Observed.icloud-placeholder");
    std::fs::write(&target, "local placeholder").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-observer-probe")
        .arg(&state)
        .arg(&root)
        .arg(&target)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("fileprovider-observed-invalidation\t"),
        "{stdout}"
    );
    assert!(
        stderr.contains(
            "fileprovider observer state volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "local placeholder"
    );
    assert!(!state.exists());

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(offline);
}

#[test]
fn fileprovider_observer_probe_refuses_unreachable_target_before_writing_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-fileprovider-observer-target-root-{}",
        std::process::id()
    ));
    let offline = std::env::temp_dir().join(format!(
        "gfm-fileprovider-observer-target-offline-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&offline);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&offline).unwrap();
    std::fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let state = root.join("fileprovider-state.tsv");
    let target = offline.join("Observed.icloud-placeholder");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("fileprovider-observer-probe")
        .arg(&state)
        .arg(&root)
        .arg(&target)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("fileprovider-observed-invalidation\t"),
        "{stdout}"
    );
    assert!(
        stderr.contains(
            "fileprovider observer target volume access blocked: unreachable volume network"
        ),
        "{stderr}"
    );
    assert!(!state.exists());
    assert!(!target.exists());

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(offline);
}

#[test]
fn reports_volume_discovery_from_binary() {
    let root = std::env::temp_dir().join(format!("gfm-volumes-{}", std::process::id()));
    let external = root.join("Work Drive");
    let network = root.join("Team Share");
    let offline = root.join("Offline Share");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&external).unwrap();
    std::fs::create_dir_all(&network).unwrap();
    std::fs::create_dir_all(&offline).unwrap();
    std::fs::write(external.join(".gfm-volume-kind"), "external-removable\n").unwrap();
    std::fs::write(network.join(".gfm-volume-kind"), "network-smb\n").unwrap();
    std::fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-discovery")
        .arg(&external)
        .arg(&network)
        .arg(&offline)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("volumes\tcount=3\n"));
    assert!(stdout.contains("\tWork Drive\t"));
    assert!(stdout.contains(
        "\tkind=external\tmount=mounted\tremovable=true\tnetwork=false\treachable=true\tejectable=true\t"
    ));
    assert!(stdout.contains("\teject=enabled\tmount=hidden\tunmount=enabled\t"));
    assert!(stdout.contains("\tTeam Share\t"));
    assert!(stdout.contains(
        "\tkind=network\tmount=mounted\tremovable=false\tnetwork=true\treachable=true\tejectable=true\t"
    ));
    assert!(stdout.contains("\tOffline Share\t"));
    assert!(stdout.contains(
        "\tkind=network\tmount=mounted\tremovable=false\tnetwork=true\treachable=false\tejectable=true\t"
    ));
    assert!(stdout.contains("\tstable-id="));
    assert!(!stdout.contains("\tnative-status=-\twritable="));
    assert!(stdout.contains("\tread-only="));
    assert!(!stdout.contains("\tresource-status=-"));
    assert!(!stdout.contains("\tmount-status=-\t"));
    assert!(stdout.contains("source=fixture-marker:network-smb"));
    assert!(stdout.contains("source=fixture-marker:network-unreachable"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn volume_discovery_surfaces_unavailable_explicit_path_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-volume-discovery-unavailable-{}",
        std::process::id()
    ));
    let invalid = root.join("volume-discovery-unavailable".repeat(16));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-discovery")
        .arg(&invalid)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("volumes\t"), "{stdout}");
    assert!(stderr.contains("File name too long"), "{stderr}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn volume_discovery_cancel_after_first_stops_before_second_descriptor_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-volume-discovery-cancel-after-first-{}",
        std::process::id()
    ));
    let external = root.join("Work Drive");
    let invalid = root.join("volume-discovery-cancelled-before-second".repeat(16));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&external).unwrap();
    std::fs::write(external.join(".gfm-volume-kind"), "external-removable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-discovery-cancel-after-first")
        .arg(&external)
        .arg(&invalid)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("volumes\t"), "{stdout}");
    assert!(stderr.contains("operation was cancelled"), "{stderr}");
    assert!(!stderr.contains("File name too long"), "{stderr}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_volume_index_policy_from_binary() {
    let root = std::env::temp_dir().join(format!("gfm-volume-policy-{}", std::process::id()));
    let external = root.join("Work Drive");
    let network = root.join("Team Share");
    let disk_image = root.join("Installer");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&external).unwrap();
    std::fs::create_dir_all(&network).unwrap();
    std::fs::create_dir_all(&disk_image).unwrap();
    std::fs::write(external.join(".gfm-volume-kind"), "external-removable\n").unwrap();
    std::fs::write(network.join(".gfm-volume-kind"), "network-smb\n").unwrap();
    std::fs::write(disk_image.join(".gfm-volume-kind"), "disk-image\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-index-policy")
        .arg("opt-in")
        .arg("opt-in")
        .arg(format!("opt-in:{}", external.display()))
        .arg(format!("opt-in:{}", disk_image.display()))
        .arg(&external)
        .arg(&network)
        .arg(&disk_image)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("volume-index-plan\tcount="));
    assert!(stdout.contains("\tid="));
    assert!(!stdout.contains("\tid=-\tpath="));
    assert!(stdout.contains("\tincluded=0\n"), "{stdout}");
    assert!(stdout.contains("\tmount=mounted\treachable=true\tstable-id="));
    assert!(!stdout.contains("\tstable-id=-\taction=api-unavailable\t"));
    assert!(stdout.contains("\taction=api-unavailable\t"));
    assert!(stdout.contains("\tthrottle=suspended\tmax-jobs=0\t"));
    assert!(stdout.contains("\tnative-status=unavailable\t"), "{stdout}");
    assert!(
        stdout.contains("\tnative-reason=DiskArbitration"),
        "{stdout}"
    );
    assert!(stdout.contains("\tresource-status=available\t"), "{stdout}");
    assert!(stdout.contains("\tmount-status=available\t"), "{stdout}");
    assert!(stdout.contains("\treason=native-volume-api-unavailable"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn volume_index_policy_surfaces_unavailable_explicit_path_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-volume-policy-unavailable-{}",
        std::process::id()
    ));
    let invalid = root.join("volume-index-policy-unavailable".repeat(16));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-index-policy")
        .arg("opt-in")
        .arg("opt-in")
        .arg(&invalid)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("volume-index-plan\t"), "{stdout}");
    assert!(stderr.contains("File name too long"), "{stderr}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_volume_invalidation_from_binary() {
    let root = std::env::temp_dir().join(format!("gfm-volume-invalidation-{}", std::process::id()));
    let external = root.join("Work Drive");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&external).unwrap();
    std::fs::write(external.join(".gfm-volume-kind"), "external-removable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-invalidation")
        .arg("external")
        .arg("stale")
        .arg(&external)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("volume-invalidation\tpath="));
    assert!(stdout.contains("\tprevious-class=external\tprevious-mount=stale\t"));
    assert!(stdout.contains("\tcurrent-class=external\tcurrent-mount=mounted\t"));
    assert!(stdout.contains("\tsidebar=true\toperation-policy=true\t"));
    assert!(stdout.contains("\tindex-admission=true\trescan-index=true\t"));
    assert!(stdout.contains("\tcancel-index-jobs=false\tclear-fsevents-cursor=true\t"));
    assert!(stdout.contains("\treason=mount-state-changed\n"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn volume_invalidation_accepts_previous_capability_snapshot_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-volume-invalidation-capability-{}",
        std::process::id()
    ));
    let external = root.join("Work Drive");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&external).unwrap();
    std::fs::write(external.join(".gfm-volume-kind"), "external-removable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-invalidation")
        .arg("external")
        .arg("mounted")
        .arg(&external)
        .args(["true", "false", "true", "false", "-", "-", "-"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("volume-invalidation\tpath="));
    assert!(stdout.contains("\tprevious-read-only=true\t"));
    assert!(stdout.contains("\tcurrent-read-only=false\t"));
    assert!(stdout.contains("\tprevious-writable=false\t"));
    assert!(stdout.contains("\tcurrent-writable=true\t"));
    assert!(stdout.contains("\toperation-policy=true\tindex-admission=true\t"));
    assert!(stdout.contains("\tcancel-index-jobs=true\tclear-fsevents-cursor=true\t"));
    assert!(stdout.contains("\treason=volume-read-only-changed\n"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn volume_invalidation_surfaces_unavailable_current_path_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-volume-invalidation-unavailable-{}",
        std::process::id()
    ));
    let invalid = root.join("volume-invalidation-unavailable".repeat(16));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-invalidation")
        .arg("external")
        .arg("mounted")
        .arg(&invalid)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("volume-invalidation\t"), "{stdout}");
    assert!(
        stderr.contains("volume invalidation current path existence unavailable")
            || stderr.contains("File name too long"),
        "{stderr}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn volume_invalidation_reports_unreachable_current_volume_before_existence_probe_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-volume-invalidation-unreachable-current-{}",
        std::process::id()
    ));
    let offline = root.join("Offline Share");
    let current = offline.join("Missing.md");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&offline).unwrap();
    std::fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-invalidation")
        .arg("network")
        .arg("mounted")
        .arg(&current)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.is_empty(), "{stderr}");
    assert!(stdout.starts_with("volume-invalidation\tpath="), "{stdout}");
    assert!(stdout.contains("\tprevious-class=network\tprevious-mount=mounted\t"));
    assert!(stdout.contains("\tcurrent-class=network\tcurrent-mount=mounted\t"));
    assert!(stdout.contains("\tcurrent-reachable=false\t"));
    assert!(stdout.contains("\toperation-policy=true\tindex-admission=true\t"));
    assert!(stdout.contains("\tcancel-index-jobs=true\tclear-fsevents-cursor=true\t"));
    assert!(stdout.contains("\treason=volume-path-changed\n"));
    assert!(!stdout.contains("volume invalidation current path existence unavailable"));
    assert!(!current.exists());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn volume_invalidation_matches_unprobeable_tmp_alias_current_volume_from_binary() {
    let root = std::path::PathBuf::from("/tmp").join(format!(
        "gfm-volume-invalidation-tmp-alias-{}",
        std::process::id()
    ));
    let offline = root.join("Offline Share");
    let current = offline.join("current-volume-unavailable".repeat(16));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&offline).unwrap();
    std::fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-invalidation")
        .arg("network")
        .arg("mounted")
        .arg(&current)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.is_empty(), "{stderr}");
    assert!(stdout.starts_with("volume-invalidation\tpath="), "{stdout}");
    assert!(stdout.contains("\tcurrent-class=network\tcurrent-mount=mounted\t"));
    assert!(stdout.contains("\tcurrent-reachable=false\t"));
    assert!(stdout.contains("\toperation-policy=true\tindex-admission=true\t"));
    assert!(!stdout.contains("volume invalidation current path existence unavailable"));
    assert!(!stderr.contains("File name too long"));
    assert!(!current.exists());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn volume_known_facts_lost_invalidates_index_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-known-facts-lost-invalidation")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("volume-invalidation\tpath=/Volumes/Known Facts Lost\t"));
    assert!(stdout.contains("\tprevious-read-only=false\t"));
    assert!(stdout.contains("\tcurrent-read-only=-\t"));
    assert!(stdout.contains("\tprevious-writable=true\t"));
    assert!(stdout.contains("\tcurrent-writable=-\t"));
    assert!(stdout.contains("\tprevious-ejectable=true\t"));
    assert!(stdout.contains("\tcurrent-ejectable=-\t"));
    assert!(stdout.contains("\tprevious-mountable=false\t"));
    assert!(stdout.contains("\tcurrent-mountable=-\t"));
    assert!(stdout.contains("\tprevious-case-sensitive=false\t"));
    assert!(stdout.contains("\tcurrent-case-sensitive=-\t"));
    assert!(stdout.contains("\tprevious-stable-id=diskarbitration:uuid:KNOWN-FACTS\t"));
    assert!(stdout.contains("\tcurrent-stable-id=-\t"));
    assert!(stdout.contains("\toperation-policy=true\tindex-admission=true\t"));
    assert!(stdout.contains("\tcancel-index-jobs=true\tclear-fsevents-cursor=true\t"));
    assert!(stdout.contains("\treason=volume-read-only-changed\n"));
    assert!(stdout.contains(
        "volume-event-index-invalidation\tkind=description-changed\tpath=/Volumes/Known Facts Lost\t"
    ));
    assert!(stdout.contains("\tidentity-changed=true\tfilesystem-changed=true\t"));
    assert!(stdout.contains("\tprevious-stable-id=diskarbitration:uuid:KNOWN-FACTS\t"));
    assert!(stdout.contains("\tcurrent-stable-id=-\t"));
    assert!(stdout.ends_with("reason=volume-event-identity-changed\n"));
}

#[test]
fn reports_volume_topology_diff_from_binary() {
    let root = std::env::temp_dir().join(format!("gfm-volume-topology-{}", std::process::id()));
    let previous = root.join("Work Drive");
    let current = root.join("Team Share");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&previous).unwrap();
    std::fs::create_dir_all(&current).unwrap();
    std::fs::write(previous.join(".gfm-volume-kind"), "external-removable\n").unwrap();
    std::fs::write(current.join(".gfm-volume-kind"), "network-smb\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-topology-diff")
        .arg(&previous)
        .arg("--")
        .arg(&current)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("volume-topology-diff\tcount=2\n"));
    assert!(stdout.contains("volume-topology\tdisconnected\t"));
    assert!(stdout.contains("volume-topology\tconnected\t"));
    assert!(stdout.contains("\tsidebar=true\toperation-policy=true\tindex-admission=true\t"));
    assert!(stdout.contains("\treason=volume-disconnected"));
    assert!(stdout.contains("\treason=volume-connected"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn volume_topology_diff_surfaces_unavailable_explicit_path_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-volume-topology-unavailable-{}",
        std::process::id()
    ));
    let current = root.join("Team Share");
    let invalid = root.join("volume-topology-unavailable".repeat(16));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&current).unwrap();
    std::fs::write(current.join(".gfm-volume-kind"), "network-smb\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-topology-diff")
        .arg(&invalid)
        .arg("--")
        .arg(&current)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("volume-topology-diff\t"), "{stdout}");
    assert!(stderr.contains("File name too long"), "{stderr}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_volume_topology_index_invalidation_from_binary() {
    let root =
        std::env::temp_dir().join(format!("gfm-volume-topology-index-{}", std::process::id()));
    let previous = root.join("Work Drive");
    let current = root.join("Team Share");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&previous).unwrap();
    std::fs::create_dir_all(&current).unwrap();
    std::fs::write(previous.join(".gfm-volume-kind"), "external-removable\n").unwrap();
    std::fs::write(current.join(".gfm-volume-kind"), "network-smb\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-topology-index-invalidation")
        .arg(&previous)
        .arg("--")
        .arg(&current)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("volume-topology-diff\tcount=2\n"));
    assert_eq!(
        stdout.matches("volume-event-index-invalidation\t").count(),
        2
    );
    assert!(stdout.contains("\tkind=disappeared\t"));
    assert!(stdout.contains("\tkind=appeared\t"));
    assert!(stdout.contains("\tprevious-stable-id="));
    assert!(stdout.contains("\tcurrent-stable-id="));
    assert!(stdout.contains("\tprevious-stable-id=-\t"));
    assert!(stdout.contains("\tcurrent-stable-id=-\t"));
    assert!(stdout.contains("\treason=volume-event-disconnected\n"));
    assert!(stdout.contains("\treason=volume-event-connected\n"));
    assert!(stdout.contains("\tcancel-index-jobs=true\t"));
    assert!(stdout.contains("\tcancel-index-jobs=false\t"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn volume_topology_index_invalidation_surfaces_unavailable_current_path_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-volume-topology-index-unavailable-{}",
        std::process::id()
    ));
    let previous = root.join("Work Drive");
    let invalid = root.join("volume-topology-index-unavailable".repeat(16));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&previous).unwrap();
    std::fs::write(previous.join(".gfm-volume-kind"), "external-removable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-topology-index-invalidation")
        .arg(&previous)
        .arg("--")
        .arg(&invalid)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("volume-topology-diff\t"), "{stdout}");
    assert!(
        !stdout.contains("volume-event-index-invalidation\t"),
        "{stdout}"
    );
    assert!(stderr.contains("File name too long"), "{stderr}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_volume_topology_case_sensitivity_diff_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-topology-case-sensitivity")
        .arg("false")
        .arg("true")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("volume-topology-diff\tcount=1\n"));
    assert!(stdout.contains("volume-topology\tchanged\t"));
    assert!(stdout.contains("\tstable-id=diskarbitration:uuid:CASE-TOPOLOGY\t"));
    assert!(stdout.contains("\tprevious-case-sensitive=false\t"));
    assert!(stdout.contains("\tcurrent-case-sensitive=true\t"));
    assert!(stdout.contains("\tsidebar=true\toperation-policy=true\tindex-admission=true\t"));
    assert!(stdout.ends_with("reason=volume-case-sensitivity-changed\n"));
}

#[test]
fn reports_volume_topology_removable_media_diff_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-topology-removable-media")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("volume-topology-diff\tcount=1\n"));
    assert!(stdout.contains("volume-topology\tchanged\t"));
    assert!(stdout.contains("\tstable-id=diskarbitration:uuid:REMOVABLE-TOPOLOGY\t"));
    assert!(stdout.contains("\tprevious-kind=external\tcurrent-kind=external\t"));
    assert!(stdout.contains("\tprevious-removable=false\tcurrent-removable=true\t"));
    assert!(stdout.contains("\tprevious-ejectable=true\tcurrent-ejectable=true\t"));
    assert!(stdout.contains("\tsidebar=true\toperation-policy=true\tindex-admission=true\t"));
    assert!(stdout.ends_with("reason=volume-media-truth-changed\n"));
}

#[test]
fn reports_volume_topology_api_status_diff_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-topology-api-status")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("volume-topology-diff\tcount=1\n"));
    assert!(stdout.contains("volume-topology\tchanged\t"));
    assert!(stdout.contains("\tstable-id=diskarbitration:uuid:API-STATUS\t"));
    assert!(stdout.contains("\tprevious-native-status=unavailable\t"));
    assert!(stdout.contains(
        "\tprevious-native-reason=DiskArbitration unavailable during topology refresh\t"
    ));
    assert!(stdout.contains("\tcurrent-native-status=available\t"));
    assert!(stdout.contains("\tcurrent-native-reason=-\t"));
    assert!(stdout.contains("\tprevious-resource-status=unavailable\t"));
    assert!(stdout.contains(
        "\tprevious-resource-reason=URL resource values unavailable during topology refresh\t"
    ));
    assert!(stdout.contains("\tcurrent-resource-status=available\t"));
    assert!(stdout.contains("\tcurrent-resource-reason=-\t"));
    assert!(stdout.contains("\tprevious-mount-status=unavailable\t"));
    assert!(stdout
        .contains("\tprevious-mount-reason=mount table unavailable during topology refresh\t"));
    assert!(stdout.contains("\tcurrent-mount-status=available\t"));
    assert!(stdout.contains("\tcurrent-mount-reason=-\t"));
    assert!(stdout.contains("\tsidebar=true\toperation-policy=true\tindex-admission=true\t"));
    assert!(stdout.ends_with("reason=volume-api-status-changed\n"));
}

#[test]
fn reports_api_status_volume_invalidation_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-api-status-invalidation")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout
        .starts_with("volume-invalidation\tpath=/Volumes/API Status\tprevious-class=external\t"));
    assert!(stdout.contains("\tcurrent-class=external\tcurrent-mount=mounted\t"));
    assert!(stdout.contains("\tprevious-native-status=unavailable\t"));
    assert!(stdout.contains(
        "\tprevious-native-reason=DiskArbitration unavailable for volume invalidation\t"
    ));
    assert!(stdout.contains("\tprevious-resource-status=unavailable\t"));
    assert!(stdout.contains(
        "\tprevious-resource-reason=URL resource values unavailable for volume invalidation\t"
    ));
    assert!(stdout.contains("\tprevious-mount-status=unavailable\t"));
    assert!(stdout
        .contains("\tprevious-mount-reason=mount table unavailable for volume invalidation\t"));
    assert!(stdout.contains("\tcurrent-native-status=available\t"));
    assert!(stdout.contains("\tcurrent-resource-status=available\t"));
    assert!(stdout.contains("\tcurrent-mount-status=available\t"));
    assert!(stdout.contains("\tapi-status-changed=true\t"));
    assert!(stdout.contains("\tindex-admission=true\trescan-index=true\t"));
    assert!(stdout.contains("\tcancel-index-jobs=true\tclear-fsevents-cursor=true\t"));
    assert!(stdout.ends_with("reason=volume-api-status-changed\n"));
}

#[test]
fn reports_apfs_metadata_volume_invalidation_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-apfs-metadata-invalidation")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with(
        "volume-invalidation\tpath=/Volumes/APFS Metadata\tprevious-class=external\t"
    ));
    assert!(stdout.contains("\tcurrent-class=external\tcurrent-mount=mounted\t"));
    assert!(stdout.contains("\tapi-status-changed=false\t"));
    assert!(stdout.contains("\tapfs-metadata-changed=true\t"));
    assert!(stdout.contains("\tfilesystem-identity-changed=true\t"));
    assert!(stdout.contains("\tindex-admission=true\trescan-index=true\t"));
    assert!(stdout.contains("\tcancel-index-jobs=true\tclear-fsevents-cursor=true\t"));
    assert!(stdout.ends_with("reason=volume-apfs-metadata-changed\n"));
}

#[test]
fn probes_volume_event_stream_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-events-probe")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("volume-events\tattached="));
    assert!(stdout.contains("\tpending="));
}

#[test]
fn probes_volume_event_stream_shutdown_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-events-shutdown-probe")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("volume-events-shutdown\tattached-before="));
    assert!(stdout.contains("\tstop-requested=true\t"));
    assert!(stdout.contains("\tthread-joined=true\t"));
    assert!(stdout.contains("\tpending-before="));
}

#[test]
fn volume_event_stream_cancel_before_recv_stops_before_event_poll_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-events-cancel-before-recv")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("operation was cancelled"), "{stderr}");
}

#[test]
fn drains_volume_event_stream_with_explicit_bound_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-events-drain-probe")
        .arg("4")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("volume-events-drain\tattached="));
    assert!(stdout.contains("\tmax=4\t"));
    assert!(stdout.contains("\tcount="));
}

#[test]
fn drains_volume_event_stream_into_state_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-volume-events-state-drain-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "external-removable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-events-state-drain-probe")
        .arg("0")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("volume-events-state-drain\tattached="));
    assert!(stdout.contains("\tmax=0\tinput=0\tapplied=0\tresulting-volumes=1\t"));
    assert!(stdout.contains("\tsidebar=false\toperation-policy=false\t"));
    assert!(stdout.contains(
        "\tindex-admission=false\trescan-index=false\tcancel-index-jobs=false\tclear-fsevents-cursor=false\n"
    ));
    assert!(
        stdout.contains("\nvolume-event-state-batch\tinput=0\tapplied=0\tresulting-volumes=1\t")
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_volume_event_invalidation_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-volume-event-invalidation-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "external-removable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-event-invalidation")
        .arg("description-changed")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.starts_with("volume-event-invalidation\tkind=description-changed\tnative-status=")
    );
    assert!(stdout.contains("\tcurrent-native-status="));
    assert!(stdout.contains("\tcurrent-kind=external\tcurrent-mount=mounted\t"));
    assert!(stdout.contains("\tprevious-kind=-\tprevious-mount=-\t"));
    assert!(stdout.contains("\tsidebar=true\toperation-policy=true\t"));
    assert!(stdout.contains("\tindex-admission=true\trescan-index=true\t"));
    if stdout.contains("\tnative-status=unavailable\t") {
        assert!(!stdout.ends_with("reason=-\n"), "{stdout}");
        assert!(
            !stdout.ends_with("reason=volume-event-description-changed\n"),
            "{stdout}"
        );
    } else {
        assert!(stdout.ends_with("reason=volume-event-description-changed\n"));
    }

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn sidebar_volume_invalidation_reports_unavailable_description_probe_from_binary() {
    let path = std::env::temp_dir().join("gfm-sidebar-volume-invalid".repeat(64));

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-sidebar-volume-invalidation")
        .arg("description-changed")
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("sidebar-volume-invalidation\trow=-\t"));
    assert!(stdout.contains(&format!("\tpath={}\t", path.display())));
    assert!(stdout.contains("\tkind=description-changed\t"));
    assert!(stdout.contains("\tprevious-kind=-\t"));
    assert!(stdout.contains("\tcurrent-kind=-\t"));
    assert!(stdout.contains("\tcurrent-native-status=-\t"));
    assert!(stdout.contains("\tinvalidate-row=false\t"));
    assert!(stdout.contains("\tinvalidate-section=true\t"));
    assert!(
        stdout.contains("\treason=volume path state unavailable: "),
        "{stdout}"
    );
}

#[test]
fn reports_volume_event_transition_label_change_as_sidebar_only_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-volume-event-transition-label-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "external-removable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-event-transition-invalidation")
        .arg("description-changed")
        .arg(&root)
        .arg("External")
        .arg("Renamed Drive")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with(
        "volume-event-invalidation\tkind=description-changed\tnative-status=available\t"
    ));
    assert!(stdout.contains("\tprevious-kind=external\tprevious-mount=mounted\t"));
    assert!(stdout.contains("\tcurrent-kind=external\tcurrent-mount=mounted\t"));
    assert!(stdout.contains(
        "\tsidebar=true\toperation-policy=false\tindex-admission=false\trescan-index=false\t"
    ));
    assert!(stdout.ends_with("reason=volume-label-changed\n"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_volume_event_transition_case_sensitivity_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-event-transition-case-sensitivity")
        .arg("false")
        .arg("true")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with(
        "volume-event-invalidation\tkind=description-changed\tnative-status=available\t"
    ));
    assert!(stdout.contains("\tpath=/Volumes/Case Event\t"));
    assert!(stdout.contains("\tprevious-case-sensitive=false\t"));
    assert!(stdout.contains("\tcurrent-case-sensitive=true\t"));
    assert!(stdout.contains("\tsidebar=true\toperation-policy=true\tindex-admission=true\t"));
    assert!(stdout.ends_with("reason=volume-case-sensitivity-changed\n"));
}

#[test]
fn reports_volume_event_transition_removable_media_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-event-transition-removable-media")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with(
        "volume-event-invalidation\tkind=description-changed\tnative-status=available\t"
    ));
    assert!(stdout.contains("\tpath=/Volumes/Removable Event\t"));
    assert!(stdout.contains("\tprevious-kind=external\tprevious-mount=mounted\t"));
    assert!(stdout.contains("\tcurrent-kind=external\tcurrent-mount=mounted\t"));
    assert!(stdout.contains("\tprevious-removable=false\t"));
    assert!(stdout.contains("\tcurrent-removable=true\t"));
    assert!(stdout.contains("\tprevious-case-preserving=true\t"));
    assert!(stdout.contains("\tcurrent-case-preserving=true\t"));
    assert!(stdout.contains("\tsidebar=true\toperation-policy=true\tindex-admission=true\t"));
    assert!(stdout.ends_with("reason=volume-media-truth-changed\n"));
}

#[test]
fn reports_volume_event_transition_api_status_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-event-transition-api-status")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with(
        "volume-event-invalidation\tkind=description-changed\tnative-status=available\t"
    ));
    assert!(stdout.contains("\tpath=/Volumes/API Event\t"));
    assert!(stdout.contains("\tprevious-native-status=unavailable\t"));
    assert!(stdout
        .contains("\tprevious-native-reason=DiskArbitration unavailable during event refresh\t"));
    assert!(stdout.contains("\tcurrent-native-status=available\t"));
    assert!(stdout.contains("\tcurrent-native-reason=-\t"));
    assert!(stdout.contains("\tprevious-resource-status=unavailable\t"));
    assert!(stdout.contains(
        "\tprevious-resource-reason=URL resource values unavailable during event refresh\t"
    ));
    assert!(stdout.contains("\tcurrent-resource-status=available\t"));
    assert!(stdout.contains("\tcurrent-resource-reason=-\t"));
    assert!(stdout.contains("\tprevious-mount-status=unavailable\t"));
    assert!(
        stdout.contains("\tprevious-mount-reason=mount table unavailable during event refresh\t")
    );
    assert!(stdout.contains("\tcurrent-mount-status=available\t"));
    assert!(stdout.contains("\tcurrent-mount-reason=-\t"));
    assert!(stdout.contains("\tsidebar=true\toperation-policy=true\tindex-admission=true\t"));
    assert!(stdout.ends_with("reason=volume-api-status-changed\n"));
}

#[test]
fn reports_volume_description_event_api_status_reason_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-event-description-api-status")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with(
        "volume-event-invalidation\tkind=description-changed\tnative-status=unavailable\t"
    ));
    assert!(stdout.contains("\tpath=/Volumes/API Description\t"));
    assert!(stdout.contains("\tcurrent-kind=external\tcurrent-mount=mounted\t"));
    assert!(stdout.contains("\tcurrent-native-status=unavailable\t"));
    assert!(stdout.contains("\tcurrent-native-reason=DiskArbitration unavailable during refresh\t"));
    assert!(stdout.contains("\tcurrent-resource-status=unavailable\t"));
    assert!(stdout
        .contains("\tcurrent-resource-reason=URL resource values unavailable during refresh\t"));
    assert!(stdout.contains("\tcurrent-mount-status=unavailable\t"));
    assert!(stdout.contains("\tcurrent-mount-reason=mount table unavailable during refresh\t"));
    assert!(stdout.contains("\tsidebar=true\toperation-policy=true\t"));
    assert!(stdout.contains("\tindex-admission=true\trescan-index=true\t"));
    assert!(stdout.ends_with("reason=DiskArbitration unavailable during refresh\n"));
}

#[test]
fn sidebar_volume_description_event_api_status_reason_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-sidebar-volume-api-status-invalidation")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("sidebar-volume-invalidation\trow=volume-"));
    assert!(stdout.contains("\tpath=/Volumes/UI API Description\t"));
    assert!(stdout.contains("\tkind=description-changed\t"));
    assert!(stdout.contains("\tcurrent-kind=external\tcurrent-mount=mounted\t"));
    assert!(stdout.contains("\tcase-sensitive=true\t"));
    assert!(stdout.contains("\tcase-preserving=true\t"));
    assert!(stdout.contains("\tcurrent-native-status=unavailable\t"));
    assert!(stdout.contains("\tcurrent-native-reason=DiskArbitration unavailable during refresh\t"));
    assert!(stdout.contains("\tcurrent-resource-status=unavailable\t"));
    assert!(stdout
        .contains("\tcurrent-resource-reason=URL resource values unavailable during refresh\t"));
    assert!(stdout.contains("\tcurrent-mount-status=unavailable\t"));
    assert!(stdout.contains("\tcurrent-mount-reason=mount table unavailable during refresh\t"));
    assert!(stdout.contains("\tinvalidate-row=true\t"));
    assert!(stdout.contains("\tinvalidate-section=true\t"));
    assert!(stdout.ends_with("reason=DiskArbitration unavailable during refresh\n"));
}

#[test]
fn reports_unavailable_volume_event_invalidation_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-event-invalidation")
        .arg("unavailable")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with(
        "volume-event-invalidation\tkind=unavailable\tnative-status=unavailable\tpath=-\t"
    ));
    assert!(stdout.contains("\tprevious-kind=-\tprevious-mount=-\t"));
    assert!(stdout.contains("\tcurrent-kind=-\tcurrent-mount=-\t"));
    assert!(stdout.contains("\tsidebar=true\toperation-policy=true\t"));
    assert!(stdout.contains("\tindex-admission=true\trescan-index=true\t"));
}

#[test]
fn reports_disappeared_volume_event_with_previous_descriptor_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-volume-event-disappeared-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "external-removable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-event-invalidation")
        .arg("disappeared")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("volume-event-invalidation\tkind=disappeared\tnative-status="));
    assert!(stdout.contains("\tprevious-native-status="));
    assert!(stdout.contains("\tprevious-kind=external\tprevious-mount=mounted\t"));
    assert!(stdout.contains("\tcurrent-kind=-\tcurrent-mount=unmounted\t"));
    assert!(stdout.contains("\tsidebar=true\toperation-policy=true\t"));
    assert!(stdout.contains("\tindex-admission=true\trescan-index=true\t"));
    assert!(stdout.ends_with("reason=volume-event-disappeared\n"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_disappeared_volume_event_for_missing_path_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-volume-event-missing-disappeared-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-event-invalidation")
        .arg("disappeared")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.starts_with("volume-event-invalidation\tkind=disappeared\tnative-status=missing\t")
    );
    assert!(stdout.contains(&format!("\tpath={}\t", root.display())));
    assert!(stdout.contains("\tprevious-kind=-\tprevious-mount=-\t"));
    assert!(stdout.contains("\tcurrent-kind=-\tcurrent-mount=unmounted\t"));
    assert!(stdout.contains("\tsidebar=true\toperation-policy=true\t"));
    assert!(stdout.contains("\tindex-admission=true\trescan-index=true\t"));
    assert!(stdout.ends_with("reason=volume-event-disappeared\n"));
}

#[test]
fn reports_volume_event_index_invalidation_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-volume-event-index-invalidation-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "external-removable\n").unwrap();

    let connected = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-event-index-invalidation")
        .arg("appeared")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        connected.status.success(),
        "{}",
        String::from_utf8_lossy(&connected.stderr)
    );
    let connected_stdout = String::from_utf8(connected.stdout).unwrap();
    assert!(connected_stdout.starts_with("volume-event-index-invalidation\tkind=appeared\t"));
    assert!(connected_stdout.contains("\tprevious-volume=-\t"));
    assert!(connected_stdout.contains("\tcurrent-volume="));
    assert!(connected_stdout.contains("\tcurrent-class=external\tcurrent-mount=mounted\t"));
    assert!(connected_stdout.contains("\tprevious-stable-id=-\t"));
    assert!(connected_stdout.contains("\tcurrent-stable-id="));
    assert!(!connected_stdout.contains("\tcurrent-stable-id=-\t"));
    assert!(connected_stdout.contains("\tindex-admission=true\trescan-index=true\t"));
    assert!(connected_stdout.contains("\tcancel-index-jobs=false\t"));
    assert!(connected_stdout.ends_with("reason=volume-event-connected\n"));

    let unavailable = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-event-index-invalidation")
        .arg("unavailable")
        .output()
        .unwrap();
    assert!(
        unavailable.status.success(),
        "{}",
        String::from_utf8_lossy(&unavailable.stderr)
    );
    let unavailable_stdout = String::from_utf8(unavailable.stdout).unwrap();
    assert!(unavailable_stdout
        .starts_with("volume-event-index-invalidation\tkind=unavailable\tpath=-\t"));
    assert!(unavailable_stdout.contains("\tprevious-volume=-\t"));
    assert!(unavailable_stdout.contains("\tcurrent-volume=-\t"));
    assert!(unavailable_stdout.contains("\tprevious-stable-id=-\t"));
    assert!(unavailable_stdout.contains("\tcurrent-stable-id=-\t"));
    assert!(unavailable_stdout.contains("\tcancel-index-jobs=true\t"));
    assert!(unavailable_stdout.contains("\tclear-fsevents-cursor=true\t"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_volume_event_state_index_invalidation_with_previous_snapshot_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-volume-event-state-index-invalidation-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "external-removable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-event-state-index-invalidation")
        .arg(&root)
        .arg("--")
        .arg("disappeared")
        .arg(&root)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("volume-event-index-invalidation\tkind=disappeared\t"));
    assert!(stdout.contains(&format!("\tpath={}\t", root.display())));
    assert!(stdout.contains("\tprevious-volume="));
    assert!(stdout.contains("\tprevious-class=external\tprevious-mount=mounted\t"));
    assert!(stdout.contains("\tcurrent-volume=-\tcurrent-class=-\tcurrent-mount=-\t"));
    assert!(stdout.contains("\tindex-admission=true\trescan-index=true\t"));
    assert!(stdout.contains("\tcancel-index-jobs=true\tclear-fsevents-cursor=true\t"));
    assert!(stdout.ends_with("reason=volume-event-disconnected\n"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn volume_event_index_invalidation_reports_unavailable_path_probe_from_binary() {
    let path = std::env::temp_dir().join("gfm-volume-event-index-invalid".repeat(64));

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-event-index-invalidation")
        .arg("description-changed")
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("volume-event-index-invalidation\tkind=description-changed\t"));
    assert!(stdout.contains(&format!("\tpath={}\t", path.display())));
    assert!(stdout.contains("\tprevious-volume=-\t"));
    assert!(stdout.contains("\tcurrent-volume=-\t"));
    assert!(stdout.contains("\tindex-admission=true\trescan-index=true\t"));
    assert!(stdout.contains("\tcancel-index-jobs=true\tclear-fsevents-cursor=true\t"));
    assert!(stdout.ends_with("reason=volume-event-description-unavailable\n"));
}

#[test]
fn stateful_volume_event_index_invalidation_preserves_missing_disappearance_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-volume-event-state-index-gone-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "network-smb\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-event-state-index-invalidation")
        .arg(&root)
        .arg("--")
        .arg("disappeared")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("volume-event-index-invalidation\tkind=disappeared\t"));
    assert!(stdout.contains(&format!("\tpath={}\t", root.display())));
    assert!(stdout.contains("\tprevious-volume="));
    assert!(stdout.contains("\tprevious-class=network\tprevious-mount=mounted\t"));
    assert!(stdout.contains("\tcurrent-volume=-\tcurrent-class=-\t"));
    assert!(stdout.contains("\tcancel-index-jobs=true\tclear-fsevents-cursor=true\t"));
    assert!(stdout.ends_with("reason=volume-event-disconnected\n"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn stateful_volume_event_index_invalidation_suppresses_unchanged_description_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-volume-event-state-index-label-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "external-removable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-event-state-index-invalidation")
        .arg(&root)
        .arg("--")
        .arg("description-changed")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("volume-event-index-invalidation\tkind=description-changed\t"));
    assert!(stdout.contains("\tprevious-volume="));
    assert!(stdout.contains("\tcurrent-volume="));
    assert!(stdout.contains("\tindex-admission=false\trescan-index=false\t"));
    assert!(stdout.contains("\tcancel-index-jobs=false\tclear-fsevents-cursor=false\t"));
    assert!(stdout.ends_with("reason=volume-event-state-unchanged\n"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_case_sensitivity_volume_event_index_invalidation_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-case-sensitivity-invalidation")
        .arg("false")
        .arg("true")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with(
        "volume-event-index-invalidation\tkind=description-changed\tpath=/Volumes/Case Test\t"
    ));
    assert!(stdout.contains("\tprevious-case-sensitive=false\t"));
    assert!(stdout.contains("\tcurrent-case-sensitive=true\t"));
    assert!(stdout.contains("\tcase-sensitive-changed=true\t"));
    assert!(stdout.contains("\tindex-admission=true\trescan-index=true\t"));
    assert!(stdout.contains("\tcancel-index-jobs=true\tclear-fsevents-cursor=true\t"));
    assert!(stdout.ends_with("reason=volume-event-case-sensitivity-changed\n"));
}

#[test]
fn reports_removable_media_volume_event_index_invalidation_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-removable-media-invalidation")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with(
        "volume-event-index-invalidation\tkind=description-changed\tpath=/Volumes/Removable Test\t"
    ));
    assert!(stdout.contains("\tprevious-volume=43\tprevious-class=external\t"));
    assert!(stdout.contains("\tcurrent-volume=43\tcurrent-class=external\t"));
    assert!(stdout.contains("\tprevious-ejectable=true\t"));
    assert!(stdout.contains("\tcurrent-ejectable=true\t"));
    assert!(stdout.contains("\tprevious-removable=false\t"));
    assert!(stdout.contains("\tcurrent-removable=true\t"));
    assert!(stdout.contains("\tfilesystem-changed=true\t"));
    assert!(stdout.contains("\tejectable-changed=false\t"));
    assert!(stdout.contains("\tremovable-changed=true\t"));
    assert!(stdout.contains("\tindex-admission=true\trescan-index=true\t"));
    assert!(stdout.contains("\tcancel-index-jobs=true\tclear-fsevents-cursor=true\t"));
    assert!(stdout.ends_with("reason=volume-event-removable-media-changed\n"));
}

#[test]
fn reports_api_status_volume_event_index_invalidation_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-api-status-index-invalidation")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with(
        "volume-event-index-invalidation\tkind=description-changed\tpath=/Volumes/API Status Test\t"
    ));
    assert!(stdout.contains("\tprevious-volume=46\tprevious-class=external\t"));
    assert!(stdout.contains("\tcurrent-volume=46\tcurrent-class=external\t"));
    assert!(stdout.contains("\tprevious-native-status=unavailable\t"));
    assert!(stdout
        .contains("\tprevious-native-reason=DiskArbitration unavailable for index invalidation\t"));
    assert!(stdout.contains("\tprevious-resource-status=unavailable\t"));
    assert!(stdout.contains(
        "\tprevious-resource-reason=URL resource values unavailable for index invalidation\t"
    ));
    assert!(stdout.contains("\tprevious-mount-status=unavailable\t"));
    assert!(
        stdout.contains("\tprevious-mount-reason=mount table unavailable for index invalidation\t")
    );
    assert!(stdout.contains("\tcurrent-native-status=available\t"));
    assert!(stdout.contains("\tcurrent-resource-status=available\t"));
    assert!(stdout.contains("\tcurrent-mount-status=available\t"));
    assert!(stdout.contains("\tapi-status-changed=true\t"));
    assert!(stdout.contains("\tindex-admission=true\trescan-index=true\t"));
    assert!(stdout.contains("\tcancel-index-jobs=true\tclear-fsevents-cursor=true\t"));
    assert!(stdout.ends_with("reason=volume-event-api-status-changed\n"));
}

#[test]
fn reports_root_filesystem_volume_event_index_invalidation_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-root-filesystem-invalidation")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with(
        "volume-event-index-invalidation\tkind=description-changed\tpath=/Volumes/Root Filesystem Test\t"
    ));
    assert!(stdout.contains("\tprevious-volume=44\tprevious-class=system\t"));
    assert!(stdout.contains("\tcurrent-volume=44\tcurrent-class=system\t"));
    assert!(stdout.contains("\tfilesystem-changed=true\t"));
    assert!(stdout.contains("\tindex-admission=true\trescan-index=true\t"));
    assert!(stdout.contains("\tcancel-index-jobs=true\tclear-fsevents-cursor=true\t"));
    assert!(stdout.ends_with("reason=volume-event-filesystem-changed\n"));
}

#[test]
fn reports_bsd_identity_volume_event_index_invalidation_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-bsd-identity-invalidation")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with(
        "volume-event-index-invalidation\tkind=description-changed\tpath=/Volumes/BSD Identity Test\t"
    ));
    assert!(stdout.contains("\tprevious-volume=45\tprevious-class=external\t"));
    assert!(stdout.contains("\tcurrent-volume=45\tcurrent-class=external\t"));
    assert!(stdout.contains("\tidentity-changed=false\tfilesystem-changed=true\t"));
    assert!(stdout.contains("\tindex-admission=true\trescan-index=true\t"));
    assert!(stdout.contains("\tcancel-index-jobs=true\tclear-fsevents-cursor=true\t"));
    assert!(stdout.ends_with("reason=volume-event-filesystem-changed\n"));
}

#[test]
fn reports_apfs_metadata_volume_event_index_invalidation_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-apfs-metadata-index-invalidation")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with(
        "volume-event-index-invalidation\tkind=description-changed\tpath=/Volumes/APFS Metadata Test\t"
    ));
    assert!(stdout.contains("\tprevious-volume=79\tprevious-class=external\t"));
    assert!(stdout.contains("\tcurrent-volume=79\tcurrent-class=external\t"));
    assert!(stdout.contains("\tidentity-changed=false\tfilesystem-changed=true\t"));
    assert!(stdout.contains("\tapfs-metadata-changed=true\t"));
    assert!(stdout.contains("\tfilesystem-identity-changed=true\t"));
    assert!(stdout.contains("\tindex-admission=true\trescan-index=true\t"));
    assert!(stdout.contains("\tcancel-index-jobs=true\tclear-fsevents-cursor=true\t"));
    assert!(stdout.ends_with("reason=volume-event-apfs-metadata-changed\n"));
}

#[test]
fn reports_volume_event_state_batch_from_binary() {
    let previous = std::env::temp_dir().join(format!(
        "gfm-volume-event-state-batch-previous-{}",
        std::process::id()
    ));
    let appeared = std::env::temp_dir().join(format!(
        "gfm-volume-event-state-batch-appeared-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&previous);
    let _ = std::fs::remove_dir_all(&appeared);
    std::fs::create_dir_all(&previous).unwrap();
    std::fs::create_dir_all(&appeared).unwrap();
    std::fs::write(previous.join(".gfm-volume-kind"), "external-removable\n").unwrap();
    std::fs::write(appeared.join(".gfm-volume-kind"), "network-smb\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-event-state-batch")
        .arg(&previous)
        .arg("--")
        .arg("appeared")
        .arg(&appeared)
        .arg("disappeared")
        .arg(&previous)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.starts_with("volume-event-state-batch\tinput=2\tapplied=2\tresulting-volumes=1\t")
    );
    assert!(stdout.contains("\tsidebar=true\toperation-policy=true\tindex-admission=true\t"));
    assert!(stdout.contains("\trescan-index=true\tcancel-index-jobs=true\t"));
    assert!(stdout.contains("\tclear-fsevents-cursor=true"));
    assert!(stdout.contains("\nvolume-event-invalidation\tkind=appeared\t"));
    assert!(stdout.contains("\nvolume-event-invalidation\tkind=disappeared\t"));
    assert!(stdout.contains("\tcurrent-mount=unmounted\t"));

    let _ = std::fs::remove_dir_all(previous);
    let _ = std::fs::remove_dir_all(appeared);
}

#[test]
fn volume_event_state_batch_suppresses_duplicate_events_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-volume-event-state-batch-duplicate-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "external-removable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-event-state-batch")
        .arg(&root)
        .arg("--")
        .arg("appeared")
        .arg(&root)
        .arg("description-changed")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(
        stdout.starts_with("volume-event-state-batch\tinput=2\tapplied=0\tresulting-volumes=1\t")
    );
    assert!(stdout.contains(
        "\tsidebar=false\toperation-policy=false\tindex-admission=false\trescan-index=false\t"
    ));
    assert!(stdout.contains("\tcancel-index-jobs=false\tclear-fsevents-cursor=false"));
    assert_eq!(
        stdout
            .matches("reason=volume-event-state-unchanged")
            .count(),
        2
    );
    assert!(!stdout.contains("\treason=volume-event-appeared\n"));
    assert!(!stdout.contains("\treason=volume-event-description-changed\n"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_volume_event_runtime_cancellation_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-volume-event-runtime-invalidation-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "external-removable\n").unwrap();

    let cancelled = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-event-runtime-invalidation")
        .arg("description-changed")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        cancelled.status.success(),
        "{}",
        String::from_utf8_lossy(&cancelled.stderr)
    );
    let cancelled_stdout = String::from_utf8(cancelled.stdout).unwrap();
    assert!(
        cancelled_stdout.starts_with("volume-event-index-invalidation\tkind=description-changed\t")
    );
    assert!(cancelled_stdout.contains("\tcancel-index-jobs=true\t"));
    assert!(cancelled_stdout.contains("\nvolume-job-cancellation\tvolume="));
    assert!(cancelled_stdout.contains("\tclass=background\tcancelled=1\n"));
    assert!(cancelled_stdout
        .contains("cancelled-job\t1\tbackground\tbackground\tindex invalidated volume"));
    assert!(!cancelled_stdout.contains("render visible volume previews"));
    assert!(!cancelled_stdout.contains("index unrelated volume"));

    let disappeared = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-event-runtime-invalidation")
        .arg("disappeared")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        disappeared.status.success(),
        "{}",
        String::from_utf8_lossy(&disappeared.stderr)
    );
    let disappeared_stdout = String::from_utf8(disappeared.stdout).unwrap();
    assert!(disappeared_stdout.starts_with("volume-event-index-invalidation\tkind=disappeared\t"));
    assert!(disappeared_stdout.contains("\tprevious-volume="));
    assert!(disappeared_stdout.contains("\tcurrent-volume=-\t"));
    assert!(disappeared_stdout.contains("\tcancel-index-jobs=true\t"));
    assert!(disappeared_stdout.contains("\nvolume-job-cancellation\tvolume="));
    assert!(disappeared_stdout.contains("\tclass=background\tcancelled=1\n"));

    let kept = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-event-runtime-invalidation")
        .arg("appeared")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        kept.status.success(),
        "{}",
        String::from_utf8_lossy(&kept.stderr)
    );
    let kept_stdout = String::from_utf8(kept.stdout).unwrap();
    assert!(kept_stdout.contains("\tcancel-index-jobs=false\t"));
    assert!(kept_stdout.ends_with(
        "volume-job-cancellation\tvolume=-\tclass=background\tcancelled=0\treason=index-jobs-still-valid\n"
    ));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_volume_event_runtime_fanout_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-volume-event-runtime-fanout-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "external-removable\n").unwrap();

    let disappeared = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-event-runtime-fanout")
        .arg(&root)
        .arg("--")
        .arg("disappeared")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        disappeared.status.success(),
        "{}",
        String::from_utf8_lossy(&disappeared.stderr)
    );
    let stdout = String::from_utf8(disappeared.stdout).unwrap();

    assert!(stdout.starts_with("volume-event-runtime-fanout\tkind=disappeared\t"));
    assert!(stdout.contains("\tsidebar=true\t"));
    assert!(stdout.contains("\toperation-policy=true\t"));
    assert!(stdout.contains("\tindex-admission=true\t"));
    assert!(stdout.contains("\trescan-index=true\t"));
    assert!(stdout.contains("\tcancel-index-jobs=true\t"));
    assert!(stdout.contains("\tclear-fsevents-cursor=true\t"));
    assert!(stdout.contains("\tsidebar-row=true\tsidebar-section=true\t"));
    assert!(stdout.contains("\treason=volume-event-disappeared\n"));
    assert!(stdout.contains("volume-event-index-invalidation\tkind=disappeared\t"));
    assert!(stdout.contains("\tcurrent-volume=-\t"));
    assert!(stdout.contains("sidebar-volume-invalidation\trow=volume-"));
    assert!(stdout.contains("\tkind=disappeared\t"));
    assert!(stdout.contains("\tprevious-case-sensitive=false\t"));
    assert!(stdout.contains("\tcase-sensitive=-\t"));
    assert!(stdout.contains("\tremove-row=true\t"));
    assert!(stdout.contains("volume-event-operation-policy-invalidation\tkind=disappeared\t"));
    assert!(stdout.contains("\tprevious-class="));
    assert!(stdout.contains("\tprevious-stable-id="));
    assert!(!stdout.contains("\tprevious-stable-id=-\t"));
    assert!(stdout.contains("\tprevious-mount="));
    assert!(stdout.contains("\tprevious-writable=true\t"));
    assert!(stdout.contains("\tprevious-read-only=false\t"));
    assert!(stdout.contains("\tprevious-network=false\t"));
    assert!(stdout.contains("\tprevious-reachable=true\t"));
    assert!(stdout.contains("\tprevious-removable=true\t"));
    assert!(stdout.contains("\tprevious-case-sensitive=false\t"));
    assert!(stdout.contains("\tprevious-case-preserving=true\t"));
    assert!(stdout.contains("\tprevious-slow="));
    assert!(stdout.contains("\tcurrent-class=-\tcurrent-mount=-\t"));
    assert!(stdout.contains("\tcurrent-stable-id=-\t"));
    assert!(stdout.contains("\tcurrent-writable=-\t"));
    assert!(stdout.contains("\tcurrent-read-only=-\t"));
    assert!(stdout.contains("\tcurrent-network=-\t"));
    assert!(stdout.contains("\tcurrent-reachable=-\t"));
    assert!(stdout.contains("\tcurrent-removable=-\t"));
    assert!(stdout.contains("\tcurrent-case-sensitive=-\t"));
    assert!(stdout.contains("\tcurrent-case-preserving=-\t"));
    assert!(stdout.contains("\tcurrent-slow=-\t"));
    assert!(stdout.contains("\tinvalidate-policy=true\treason=volume-event-disappeared\n"));
    assert!(stdout.contains("\nvolume-job-cancellation\tvolume="));
    assert!(stdout.contains("\tclass=background\tcancelled=1\n"));
    assert!(stdout.contains("cancelled-job\t1\tbackground\tbackground\tindex invalidated volume"));
    assert!(!stdout.contains("render visible volume previews"));
    assert!(!stdout.contains("index unrelated volume"));

    let kept = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-event-runtime-fanout")
        .arg(&root)
        .arg("--")
        .arg("appeared")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        kept.status.success(),
        "{}",
        String::from_utf8_lossy(&kept.stderr)
    );
    let kept_stdout = String::from_utf8(kept.stdout).unwrap();
    assert!(kept_stdout.starts_with("volume-event-runtime-fanout\tkind=appeared\t"));
    assert!(kept_stdout.contains("\tsidebar=false\t"));
    assert!(kept_stdout.contains("\toperation-policy=false\t"));
    assert!(kept_stdout.contains("\tindex-admission=false\t"));
    assert!(kept_stdout.contains("\trescan-index=false\t"));
    assert!(kept_stdout.contains("\tcancel-index-jobs=false\t"));
    assert!(kept_stdout.contains("\tclear-fsevents-cursor=false\t"));
    assert!(kept_stdout.contains("sidebar-volume-invalidation\trow=volume-"));
    assert!(kept_stdout.contains("\tremove-row=false\t"));
    assert!(kept_stdout.contains("volume-event-operation-policy-invalidation\tkind=appeared\t"));
    assert!(kept_stdout.contains("\tprevious-class="));
    assert!(kept_stdout.contains("\tprevious-stable-id="));
    assert!(!kept_stdout.contains("\tprevious-stable-id=-\t"));
    assert!(kept_stdout.contains("\tprevious-mount="));
    assert!(kept_stdout.contains("\tprevious-writable=true\t"));
    assert!(kept_stdout.contains("\tprevious-read-only=false\t"));
    assert!(kept_stdout.contains("\tprevious-network=false\t"));
    assert!(kept_stdout.contains("\tprevious-reachable=true\t"));
    assert!(kept_stdout.contains("\tprevious-slow=false\t"));
    assert!(kept_stdout.contains("\tcurrent-class="));
    assert!(kept_stdout.contains("\tcurrent-stable-id="));
    assert!(!kept_stdout.contains("\tcurrent-stable-id=-\t"));
    assert!(kept_stdout.contains("\tcurrent-mount=mounted\t"));
    assert!(kept_stdout.contains("\tcurrent-writable=true\t"));
    assert!(kept_stdout.contains("\tcurrent-read-only=false\t"));
    assert!(kept_stdout.contains("\tcurrent-network=false\t"));
    assert!(kept_stdout.contains("\tcurrent-reachable=true\t"));
    assert!(kept_stdout.contains("\tcurrent-removable=true\t"));
    assert!(kept_stdout.contains("\tcurrent-case-sensitive=false\t"));
    assert!(kept_stdout.contains("\tcurrent-case-preserving=true\t"));
    assert!(kept_stdout.contains("\tcurrent-slow=false\t"));
    assert!(kept_stdout.contains("\tindex-admission=false\trescan-index=false\t"));
    assert!(kept_stdout.contains("\tinvalidate-row=false\t"));
    assert!(
        kept_stdout.contains("\tinvalidate-policy=false\treason=volume-event-state-unchanged\n")
    );
    assert!(kept_stdout.ends_with(
        "volume-job-cancellation\tvolume=-\tclass=background\tcancelled=0\treason=index-jobs-still-valid\n"
    ));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_volume_operation_refusal_from_binary() {
    let root = std::env::temp_dir().join(format!("gfm-volume-operation-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "external-removable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-operation")
        .arg("eject")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("volume-operation\teject\t"));
    assert!(stdout.contains("\tdisposition=refused\tnative-status=-\tdissenter-status=-\t"));
    assert!(stdout.contains("\tcommand-state=enabled\t"));
    assert!(stdout.contains("\tvolume-kind=external\tmount=mounted\t"));
    assert!(stdout.contains("\tvolume-writable=true\t"));
    assert!(stdout.contains("\tvolume-read-only=false\t"));
    assert!(stdout.contains("\tvolume-network=false\t"));
    assert!(stdout.contains("\tvolume-reachable=true\t"));
    assert!(stdout.contains("\tvolume-ejectable=true\t"));
    assert!(stdout.contains("\tvolume-removable=true\t"));
    assert!(stdout.contains("\tvolume-mountable="));
    assert!(stdout.contains("\tvolume-native-status="));
    assert!(stdout.contains("\tvolume-native-reason="));
    assert!(stdout.contains("\tvolume-resource-status="));
    assert!(stdout.contains("\tvolume-resource-reason="));
    assert!(stdout.contains("\tvolume-mount-status="));
    assert!(stdout.contains("\tvolume-mount-reason="));
    assert!(stdout.contains("\treason=fixture-volume-native-operation-disabled\n"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_volume_unmount_operation_refusal_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-volume-unmount-operation-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "external-removable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-operation")
        .arg("unmount")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("volume-operation\tunmount\t"));
    assert!(stdout.contains("\tdisposition=refused\tnative-status=-\tdissenter-status=-\t"));
    assert!(stdout.contains("\tcommand-state=enabled\t"));
    assert!(stdout.contains("\tvolume-kind=external\tmount=mounted\t"));
    assert!(stdout.contains("\tvolume-ejectable=true\t"));
    assert!(stdout.contains("\tvolume-removable=true\t"));
    assert!(stdout.contains("\treason=fixture-volume-native-operation-disabled\n"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_volume_operation_policy_refusal_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-volume-operation-policy-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "internal\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-operation")
        .arg("eject")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("volume-operation\teject\t"));
    assert!(stdout.contains("\tdisposition=refused\tnative-status=-\tdissenter-status=-\t"));
    assert!(stdout.contains("\tcommand-state=hidden\t"));
    assert!(stdout.contains("\tvolume-kind=internal\tmount=mounted\t"));
    assert!(stdout.contains("\treason=internal-volume-not-ejectable\n"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn preview_retry_routes_refuse_unreachable_attempt_state_before_probe_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-preview-retry-unreachable-inputs-{}",
        std::process::id()
    ));
    let offline = std::env::temp_dir().join(format!(
        "gfm-preview-retry-unreachable-state-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&offline);
    std::fs::create_dir_all(root.join("GFM.app")).unwrap();
    std::fs::create_dir_all(&offline).unwrap();
    std::fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let app = root.join("GFM.app");
    let pdf = root.join("Retry.pdf");
    let image = root.join("Retry.png");
    std::fs::write(&pdf, b"%PDF-1.7\nretry quicklook").unwrap();
    std::fs::write(&image, b"\x89PNG\r\n\x1a\nretry thumbnail").unwrap();

    for (command, path, worker, forbidden) in [
        (
            "icon-preview-retry-probe",
            app.as_path(),
            "icon preview",
            "icon-preview\t",
        ),
        (
            "quicklook-session-retry-probe",
            pdf.as_path(),
            "quicklook preview",
            "quicklook-session\t",
        ),
        (
            "thumbnail-generation-retry-probe",
            image.as_path(),
            "thumbnail generation",
            "thumbnail-generation\t",
        ),
    ] {
        let attempt_state = offline.join(format!(
            "{}.state",
            format!("{}-attempt-state-unavailable", command).repeat(8)
        ));
        let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
            .arg(command)
            .arg(path)
            .arg(&attempt_state)
            .output()
            .unwrap();

        assert!(!output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stdout.contains(forbidden), "{stdout}");
        assert!(
            stderr.contains(&format!(
                "{worker} volume access blocked: unreachable volume network"
            )),
            "{stderr}"
        );
        assert!(
            !stderr.contains("platform write path metadata unavailable"),
            "{stderr}"
        );
        assert!(!attempt_state.exists());
    }

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(offline);
}

#[test]
fn volume_operation_refuses_nested_volume_path_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-volume-operation-nested-{}",
        std::process::id()
    ));
    let nested = root.join("Project").join("Preview.pdf");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(nested.parent().unwrap()).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "external-removable\n").unwrap();
    std::fs::write(&nested, "%PDF-1.7\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-operation")
        .arg("eject")
        .arg(&nested)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("volume-operation\teject\t"));
    assert!(stdout.contains(&format!("path={}", nested.display())));
    assert!(stdout.contains("\tdisposition=refused\tnative-status=-\tdissenter-status=-\t"));
    assert!(stdout.contains("\tvolume-kind=external\tmount=mounted\t"));
    assert!(stdout.contains("\treason=native-volume-operation-requires-volume-root\n"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn volume_operation_reports_missing_path_from_binary() {
    let missing = std::env::temp_dir().join(format!(
        "gfm-volume-operation-missing-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&missing);

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-operation")
        .arg("eject")
        .arg(&missing)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("volume-operation\teject\t"));
    assert!(stdout.contains("\tdisposition=missing\tnative-status=-\t"));
    assert!(stdout.contains("\tvolume-kind=-\tmount=-\t"));
    assert!(stdout.contains("\tvolume-writable=-\t"));
    assert!(stdout.contains("\tvolume-removable=-\t"));
    assert!(stdout.contains("\tvolume-case-preserving=-\t"));
    assert!(stdout.contains("\treason=volume-path-missing\n"));
}

#[test]
fn volume_operation_reports_unavailable_path_probe_from_binary() {
    let path = std::env::temp_dir().join("gfm-volume-operation-invalid".repeat(64));

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-operation")
        .arg("eject")
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("volume-operation\teject\t"));
    assert!(stdout.contains("\tdisposition=unavailable\tnative-status=-\t"));
    assert!(stdout.contains("\tvolume-kind=-\tmount=-\t"));
    assert!(stdout.contains("\treason=volume-path-existence-unavailable: "));
}

#[test]
fn volume_operation_refuses_unreachable_volume_before_descriptor_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-volume-operation-unreachable-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-operation")
        .arg("eject")
        .arg(&root)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("volume-operation\t"), "{stdout}");
    assert!(
        stderr.contains("volume operation volume access blocked: unreachable volume network"),
        "{stderr}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn volume_operation_refuses_missing_child_on_unreachable_volume_before_missing_report_from_binary()
{
    let root = std::env::temp_dir().join(format!(
        "gfm-volume-operation-unreachable-missing-child-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let missing_child = root.join("Missing");

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-operation")
        .arg("eject")
        .arg(&missing_child)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("volume-operation\t"), "{stdout}");
    assert!(
        stderr.contains("volume operation volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(!stderr.contains("volume-path-missing"), "{stderr}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn volume_operation_cancel_after_access_stops_before_native_execution_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-volume-operation-cancel-after-access-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "external-removable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-operation-cancel-after-access")
        .arg("eject")
        .arg(&root)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("volume-operation\t"), "{stdout}");
    assert!(stderr.contains("operation was cancelled"), "{stderr}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn volume_unmount_cancel_after_access_stops_before_native_execution_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-volume-unmount-cancel-after-access-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "external-removable\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-operation-cancel-after-access")
        .arg("unmount")
        .arg(&root)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("volume-operation\t"), "{stdout}");
    assert!(stderr.contains("operation was cancelled"), "{stderr}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn volume_operation_cancel_before_probe_stops_before_path_existence_from_binary() {
    let path = std::env::temp_dir().join("gfm-volume-operation-cancel-probe".repeat(64));

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-operation-cancel-before-probe")
        .arg("eject")
        .arg(&path)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("volume-operation\t"), "{stdout}");
    assert!(stderr.contains("operation was cancelled"), "{stderr}");
    assert!(
        !stderr.contains("volume-path-existence-unavailable"),
        "{stderr}"
    );
}

#[test]
fn operation_access_refuses_unavailable_volume_api_state_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-operation-access-api-unavailable-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let source_root = root.join("Source");
    let volume = root.join("TeamShare");
    std::fs::create_dir_all(&source_root).unwrap();
    std::fs::create_dir_all(&volume).unwrap();
    let source = source_root.join("source.txt");
    let destination = volume.join("copy.txt");
    std::fs::write(&source, "content").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("operation-access-unavailable-volume-api")
        .arg(&source)
        .arg(&destination)
        .arg(&volume)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("operation-access\tcopy\taction=deny\t"));
    assert!(stdout.contains("unavailable volume network"), "{stdout}");
    assert!(stdout.contains("native-status=unavailable"), "{stdout}");
    assert!(
        stdout.contains("native-reason=DiskArbitration unavailable for operation volume"),
        "{stdout}"
    );
    assert!(stdout.contains("resource-status=unavailable"), "{stdout}");
    assert!(
        stdout.contains("resource-reason=URL resource values unavailable for operation volume"),
        "{stdout}"
    );
    assert!(stdout.contains("mount-status=unavailable"), "{stdout}");
    assert!(
        stdout.contains("mount-reason=mount table unavailable for operation volume"),
        "{stdout}"
    );
    assert!(
        stdout.contains("refresh-on-permission-change=true"),
        "{stdout}"
    );
    assert!(stdout.contains("probe=unavailable"), "{stdout}");
    assert!(stdout.contains("probe-path="), "{stdout}");
    assert!(!destination.exists());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_volume_mount_bsd_refusal_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-mount-bsd")
        .arg("not/a/disk")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("volume-mount-bsd\tbsd-name=not/a/disk\t"));
    assert!(stdout.contains("\tdisposition=unsupported\tnative-status=unsupported\t"));
    assert!(stdout.contains("\tdissenter-status=-\t"));
    assert!(stdout.contains("\treason=diskarbitration-mount-requires-bsd-name\n"));

    let malformed = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-mount-bsd")
        .arg("notadisk")
        .output()
        .unwrap();
    assert!(
        malformed.status.success(),
        "{}",
        String::from_utf8_lossy(&malformed.stderr)
    );
    let malformed_stdout = String::from_utf8(malformed.stdout).unwrap();
    assert!(malformed_stdout.starts_with("volume-mount-bsd\tbsd-name=notadisk\t"));
    assert!(malformed_stdout.contains("\tdisposition=unsupported\tnative-status=unsupported\t"));
    assert!(malformed_stdout.contains("\tdissenter-status=-\t"));
    assert!(malformed_stdout.contains("\treason=diskarbitration-mount-requires-bsd-name\n"));

    let native_refusal = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-mount-bsd")
        .arg("disk999999s999")
        .output()
        .unwrap();
    assert!(
        native_refusal.status.success(),
        "{}",
        String::from_utf8_lossy(&native_refusal.stderr)
    );
    let native_refusal_stdout = String::from_utf8(native_refusal.stdout).unwrap();
    assert!(native_refusal_stdout.starts_with("volume-mount-bsd\tbsd-name=disk999999s999\t"));
    assert!(native_refusal_stdout.contains("\tdisposition=refused\tnative-status=bad-argument\t"));
    assert!(native_refusal_stdout.contains("\tdissenter-status=0xf8da0003\t"));
    assert!(native_refusal_stdout.contains("\treason=diskarbitration-bad-argument:0xf8da0003"));

    let cancelled = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-mount-bsd-cancel-before-native")
        .arg("disk999999s999")
        .output()
        .unwrap();
    assert!(!cancelled.status.success());
    let cancelled_stdout = String::from_utf8_lossy(&cancelled.stdout);
    let cancelled_stderr = String::from_utf8_lossy(&cancelled.stderr);
    assert!(
        !cancelled_stdout.contains("volume-mount-bsd\t"),
        "{cancelled_stdout}"
    );
    assert!(
        cancelled_stderr.contains("operation was cancelled"),
        "{cancelled_stderr}"
    );
}

fn mark_evicted_fixture(path: impl AsRef<std::path::Path>) {
    xattr::set(path.as_ref(), "com.apple.icloud.placeholder", b"1").unwrap();
}

fn assert_worker_admitted(stderr: &str, worker: &str, path: &std::path::Path) {
    assert!(worker_admission_count(stderr, worker, path) > 0, "{stderr}");
}

fn worker_admission_count(stderr: &str, worker: &str, path: &std::path::Path) -> usize {
    worker_admission_count_with_intent(stderr, worker, path, "-")
}

fn worker_admission_count_with_intent(
    stderr: &str,
    worker: &str,
    path: &std::path::Path,
    intent: &str,
) -> usize {
    let expected = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let expected = format!(
        "security-worker-admission\tworker={worker}\tpath={}",
        expected.display()
    );
    let expected_intent = format!("intent={intent}");
    stderr
        .lines()
        .filter(|line| {
            line.contains(&expected)
                && (intent == "-" || line.split('\t').any(|field| field == expected_intent))
        })
        .count()
}

fn seed_stale_permission_state(state: &std::path::Path) {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("permission-invalidation")
        .arg(state)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = std::fs::read_to_string(state).unwrap();
    let mut lines = text.lines().map(str::to_string).collect::<Vec<_>>();
    let first_scope = lines
        .iter_mut()
        .skip(1)
        .find(|line| !line.trim().is_empty())
        .expect("permission snapshot should include at least one scope");
    let mut fields = first_scope
        .split('\t')
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert!(fields.len() >= 4, "{first_scope}");
    fields[1] = "granted".to_string();
    fields[3] = "stale platform admission fixture".to_string();
    *first_scope = fields.join("\t");
    std::fs::write(state, format!("{}\n", lines.join("\n"))).unwrap();
}
