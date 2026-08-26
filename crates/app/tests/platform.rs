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
    assert!(downloaded_stdout.contains("\tdomain=icloud-drive\tstate=downloaded\t"));
    assert!(downloaded_stdout.contains("\tbadges=available-offline\t"));
    assert!(downloaded_stdout.contains("\tdownload=disabled\tevict=enabled\t"));

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
    assert!(evicted_stdout.contains("\tstate=evicted\toffline=true\t"));
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
    assert!(conflict_stdout.contains("\tstate=conflict\toffline=false\tconflict=true\t"));
    assert!(conflict_stdout.contains("\tbadges=conflict\t"));
    assert!(conflict_stdout.contains("\treveal-conflict=enabled\t"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_volume_discovery_from_binary() {
    let root = std::env::temp_dir().join(format!("gfm-volumes-{}", std::process::id()));
    let external = root.join("Work Drive");
    let network = root.join("Team Share");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&external).unwrap();
    std::fs::create_dir_all(&network).unwrap();
    std::fs::write(external.join(".gfm-volume-kind"), "external-removable\n").unwrap();
    std::fs::write(network.join(".gfm-volume-kind"), "network-smb\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("volume-discovery")
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

    assert!(stdout.starts_with("volumes\tcount=2\n"));
    assert!(stdout.contains("\tWork Drive\t"));
    assert!(stdout.contains(
        "\tkind=external\tmount=mounted\tremovable=true\tnetwork=false\tejectable=true\t"
    ));
    assert!(stdout.contains("\teject=enabled\tmount=hidden\tunmount=enabled\t"));
    assert!(stdout.contains("\tTeam Share\t"));
    assert!(stdout.contains(
        "\tkind=network\tmount=mounted\tremovable=false\tnetwork=true\tejectable=true\t"
    ));
    assert!(stdout.contains("source=fixture-marker:network-smb"));

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
    assert!(stdout.contains("\tclass=external\tmount=mounted\taction=include\t"));
    assert!(stdout.contains("\tthrottle=external\tmax-jobs=2\t"));
    assert!(stdout.contains("\treason=opted-in"));
    assert!(stdout.contains("\tTeam Share\t"));
    assert!(stdout.contains("\tclass=network\tmount=mounted\taction=deferred-opt-in\t"));
    assert!(stdout.contains("\tthrottle=suspended\tmax-jobs=0\t"));
    assert!(stdout.contains("\treason=requires-opt-in"));

    let _ = std::fs::remove_dir_all(root);
}
