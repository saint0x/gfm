use gfm_preview::{PreviewCache, PreviewCacheConfig, PreviewEntry, PreviewKind, PreviewRequestKey};
use gfm_types::{FileId, VolumeId};
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
        first_stdout.contains("\npermission-change\tdesktop\t"),
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
fn permission_invalidation_refuses_unreachable_state_before_persisting_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-permission-invalidation-offline-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let state = root.join("permission-state.tsv");

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
    assert!(!state.exists());

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
fn reports_security_worker_admission_from_binary() {
    let root = std::env::temp_dir().join(format!("gfm-security-worker-{}", std::process::id()));
    let path = root.join("plain.md");
    let missing = root.join("missing.md");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(&path, "plain").unwrap();

    let allowed = Command::new(env!("CARGO_BIN_EXE_gfm"))
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
    assert!(allowed_stdout.starts_with("security-worker-admission\t"));
    assert!(allowed_stdout.contains("\tintent=index\tscope=none\tprobe=granted\t"));
    assert!(allowed_stdout.contains("\tworker-action=start\t"));
    assert!(allowed_stdout.contains("\tcan-touch-filesystem=true\t"));

    let denied = Command::new(env!("CARGO_BIN_EXE_gfm"))
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
    assert!(denied_stdout.contains("\tintent=preview\tscope=none\tprobe=missing\t"));
    assert!(denied_stdout.contains("\tworker-action=deny\t"));
    assert!(denied_stdout.contains("\tcan-touch-filesystem=false\t"));

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
    std::fs::write(&downloading, "downloading").unwrap();

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

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(offline);
}

#[test]
fn reports_fileprovider_state_from_binary() {
    let root = std::env::temp_dir().join(format!("gfm-fileprovider-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let downloaded = root.join("Downloaded.icloud.md");
    let evicted = root.join("Evicted.icloud-placeholder");
    let conflict = root.join("Conflict.icloud-conflict.md");
    std::fs::write(&downloaded, "downloaded").unwrap();
    std::fs::write(&evicted, "placeholder").unwrap();
    std::fs::write(&conflict, "conflict").unwrap();

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
    assert!(downloaded_stdout.starts_with("fileprovider-state\t"));
    assert!(downloaded_stdout.contains("\tdomain=icloud-drive\tstate=unknown\t"));
    assert!(downloaded_stdout.contains("\tmaterialization=unknown\t"));
    assert!(downloaded_stdout.contains("\tmaterialization-source=path-fallback\t"));
    assert!(downloaded_stdout.contains("\tbadges=waiting\t"));
    assert!(downloaded_stdout.contains("\tdownload=disabled\tevict=disabled\t"));
    assert!(downloaded_stdout.contains("\treason=unknown-provider-state"));
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
    assert!(identity_state_stdout.starts_with("fileprovider-state\t"));
    assert!(identity_state_stdout.contains("\tdomain=icloud-drive\tstate=unknown\t"));
    assert!(identity_state_stdout.contains("\tmaterialization=unknown\t"));
    assert!(identity_state_stdout.contains("\tsource="));

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
    assert!(domain_stdout.starts_with("fileprovider-domain\t"));
    assert!(domain_stdout.contains("\tdomain=icloud-drive\t"));
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
    assert!(evicted_stdout.contains("\tstate=evicted\tmaterialization=remote-placeholder\t"));
    assert!(evicted_stdout.contains("\tmaterialization-source=path-fallback\t"));
    assert!(evicted_stdout.contains("\toffline=true\t"));
    assert!(evicted_stdout.contains("\tbadges=cloud\t"));
    assert!(evicted_stdout.contains("\tdownload=enabled\tevict=disabled\t"));

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
    assert!(stdout.starts_with("fileprovider-progress\t"));
    assert!(stdout.contains("\tstate=downloading\t"));

    let progress_text = std::fs::read_to_string(&progress).unwrap();
    assert!(
        progress_text.contains("progress\t1\tvisible\tvisible\tfileprovider download"),
        "{progress_text}"
    );
    assert!(
        progress_text.contains("\trunning\t0\t1\tfileprovider:icloud-drive:downloading:download:provider-progress-unavailable\t"),
        "{progress_text}"
    );
    let catalog_text = std::fs::read_to_string(&catalog).unwrap();
    assert!(
        catalog_text.contains("payload\t1\toperation\tfileprovider download"),
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
fn fileprovider_state_controls_preview_generation_from_binary() {
    let root =
        std::env::temp_dir().join(format!("gfm-fileprovider-preview-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let evicted = root.join("Remote.icloud-placeholder.pdf");
    let downloading = root.join("Downloading.icloud-downloading.png");
    std::fs::write(&evicted, "%PDF-1.7\nplaceholder").unwrap();
    std::fs::write(&downloading, "png").unwrap();

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
    let quicklook_stdout = String::from_utf8(quicklook.stdout).unwrap();
    assert!(
        quicklook_stdout.contains("\tallow-native\tcloud=metadata-only\tmetadata-only\t"),
        "{quicklook_stdout}"
    );
    assert!(
        quicklook_stdout.ends_with("schedule=scheduled:visible\n"),
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
    let thumbnail_stdout = String::from_utf8(thumbnail.stdout).unwrap();
    assert!(
        thumbnail_stdout.contains("\tallow-native\tcloud=defer\tmetadata-only\t512px\t"),
        "{thumbnail_stdout}"
    );
    assert!(
        thumbnail_stdout.ends_with("schedule=cancelled:fileprovider-in-flight\n"),
        "{thumbnail_stdout}"
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
    assert!(download_stdout.starts_with("fileprovider-operation\t"));
    assert!(download_stdout.contains("\toperation=download\tdisposition=refused\t"));
    assert!(download_stdout.contains("\tbefore-state=evicted\tafter-state=-\t"));
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
    assert!(evict_stdout.contains("\toperation=evict\tdisposition=refused\t"));
    assert!(evict_stdout.contains("\tbefore-state=unknown\tafter-state=-\t"));
    assert!(evict_stdout.ends_with("reason=operation-disabled-for-current-state\n"));

    let conflict = root.join("Conflict.icloud-conflict.md");
    std::fs::write(&conflict, "conflict").unwrap();
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
    assert!(conflict_stdout.contains("\toperation=evict\tdisposition=refused\t"));
    assert!(conflict_stdout.contains("\tbefore-state=conflict\tafter-state=-\t"));
    assert!(conflict_stdout.ends_with("reason=provider-conflict-requires-resolution\n"));

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
    assert!(stdout.starts_with("fileprovider-invalidation\t"));
    assert!(stdout.contains("\tprevious=downloaded\tcurrent=evicted\tchanged=true\t"));
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
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("preview-cache-invalidation\t"));
    assert!(stdout.contains("\tkind=thumbnail\treason=content-or-icloud\t"));
    assert!(stdout.contains("\tinvalidate-memory=true\tinvalidate-disk=true\t"));
    assert!(stdout.contains("\tremoved-memory=false\tremoved-disk=true\n"));

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
    let cache = offline.join("cache");
    let evicted = root.join("Remote.icloud-placeholder");
    std::fs::write(&evicted, "placeholder").unwrap();

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
        stderr.contains("preview cache root volume access blocked: unreachable volume network"),
        "{stderr}"
    );
    assert!(!cache.exists());

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(offline);
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

    for args in [
        vec!["fileprovider-state".to_string(), item.display().to_string()],
        vec![
            "fileprovider-state-with-identity".to_string(),
            item.display().to_string(),
        ],
        vec![
            "fileprovider-domain".to_string(),
            item.display().to_string(),
        ],
        vec![
            "fileprovider-progress".to_string(),
            item.display().to_string(),
        ],
        vec![
            "fileprovider-conflict".to_string(),
            item.display().to_string(),
        ],
        vec![
            "fileprovider-progress-job".to_string(),
            item.display().to_string(),
        ],
        vec![
            "fileprovider-invalidation".to_string(),
            "downloaded".to_string(),
            item.display().to_string(),
        ],
        vec![
            "fileprovider-metadata-invalidation".to_string(),
            "downloaded".to_string(),
            item.display().to_string(),
        ],
        vec![
            "native-icon-fileprovider-invalidation".to_string(),
            "downloaded".to_string(),
            item.display().to_string(),
        ],
        vec![
            "fileprovider-operation".to_string(),
            "download".to_string(),
            item.display().to_string(),
        ],
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
    let state = root.join("fileprovider-state.tsv");
    let item = root.join("Remote.icloud-placeholder");
    std::fs::write(&item, "placeholder").unwrap();

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
    assert!(!state.exists());

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
    assert!(stdout.contains("\tprogress=0\tinvalidate-row=true\t"));
    assert!(stdout.ends_with("reason=sidebar-cloud-state-changed\n"));

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
    assert!(state_text.starts_with("gfm-fileprovider-state-v1\n"));
    assert!(state_text.contains("evicted\t"));

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
    assert!(changed_stdout.contains("\tcurrent=unknown\tchanged=true\t"));
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
    std::fs::write(&item, "placeholder").unwrap();
    std::fs::write(
        &state,
        format!(
            "gfm-fileprovider-state-v1\ndownloaded\t{}\n",
            item.display()
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
    assert!(stdout.contains("\tprevious=downloaded\tcurrent=local-only\tchanged=true\t"));
    assert!(stdout.contains("\ticon=true\tpreview-memory=true\tpreview-disk=true\t"));
    assert!(stdout.contains("\tsidebar=true\treindex-metadata=true\t"));
    let state_text = std::fs::read_to_string(&state).unwrap();
    assert!(!state_text.contains(&item.display().to_string()));
    assert!(state_text.contains(&format!("evicted\t{}\n", untouched.display())));

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
fn reports_volume_index_policy_from_binary() {
    let root = std::env::temp_dir().join(format!("gfm-volume-policy-{}", std::process::id()));
    let external = root.join("Work Drive");
    let network = root.join("Team Share");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&external).unwrap();
    std::fs::create_dir_all(&network).unwrap();
    std::fs::write(external.join(".gfm-volume-kind"), "external-removable\n").unwrap();
    std::fs::write(network.join(".gfm-volume-kind"), "network-smb\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-index-policy")
        .arg("opt-in")
        .arg("opt-in")
        .arg(format!("opt-in:{}", external.display()))
        .arg(&external)
        .arg(&network)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("volume-index-plan\tcount=2\tincluded=1\n"));
    assert!(stdout.contains("\tWork Drive\t"));
    assert!(stdout.contains("\tid="));
    assert!(!stdout.contains("\tid=-\tpath="));
    assert!(stdout.contains("\tclass=external\tmount=mounted\treachable=true\taction=include\t"));
    assert!(stdout.contains("\tthrottle=external\tmax-jobs=2\t"));
    assert!(stdout.contains("\treason=opted-in"));
    assert!(stdout.contains("\tTeam Share\t"));
    assert!(
        stdout.contains("\tclass=network\tmount=mounted\treachable=true\taction=deferred-opt-in\t")
    );
    assert!(stdout.contains("\tthrottle=suspended\tmax-jobs=0\t"));
    assert!(stdout.contains("\treason=requires-opt-in"));

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
    assert!(stdout.starts_with(
        "volume-event-invalidation\tkind=description-changed\tnative-status=available\t"
    ));
    assert!(stdout.contains("\tcurrent-kind=external\tcurrent-mount=mounted\t"));
    assert!(stdout.contains("\tprevious-kind=-\tprevious-mount=-\t"));
    assert!(stdout.contains("\tsidebar=true\toperation-policy=true\t"));
    assert!(stdout.contains("\tindex-admission=true\trescan-index=true\t"));
    assert!(stdout.ends_with("reason=volume-event-description-changed\n"));

    let _ = std::fs::remove_dir_all(root);
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
    assert!(stdout
        .starts_with("volume-event-invalidation\tkind=disappeared\tnative-status=available\t"));
    assert!(stdout.contains("\tprevious-kind=external\tprevious-mount=mounted\t"));
    assert!(stdout.contains("\tcurrent-kind=-\tcurrent-mount=unmounted\t"));
    assert!(stdout.contains("\tsidebar=true\toperation-policy=true\t"));
    assert!(stdout.contains("\tindex-admission=true\trescan-index=true\t"));
    assert!(stdout.ends_with("reason=volume-event-disappeared\n"));

    let _ = std::fs::remove_dir_all(root);
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
    assert!(unavailable_stdout.contains("\tcancel-index-jobs=true\t"));
    assert!(unavailable_stdout.contains("\tclear-fsevents-cursor=true\t"));

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
    assert!(stdout.contains("\tvolume-kind=external\tmount=mounted\t"));
    assert!(stdout.contains("\treason=fixture-volume-native-operation-disabled\n"));

    let _ = std::fs::remove_dir_all(root);
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
}
