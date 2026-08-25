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

    assert!(stdout.starts_with("mac-bridges\timplemented=4\trequired=6\ttotal=10"));
    assert!(stdout.contains(
        "bridge\tfoundation-host-profile\tfoundation\tcrates/mac\tsw-vers-uname-sysctl-host-profile\tbackground-safe\timplemented"
    ));
    assert!(stdout.contains(
        "bridge\tfsevents-file-event-stream\tfile-events\tcrates/mac\ttyped-create-modify-remove-rename-rescan-events\tdedicated-worker\timplemented"
    ));
    assert!(stdout.contains(
        "bridge\tquicklook-preview\tquicklook\tcrates/preview\tquicklook-preview-controller-thumbnail-generator\tmain-thread\trequired"
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
fn reports_ui_menu_contract_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-menu-contract")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let mut lines = stdout.lines();

    assert_eq!(
        lines.next(),
        Some("menus\tGFM,File,Edit,View,Go,Window,Help\tservices=true")
    );
    assert!(lines.any(|line| line == "command\tFile\tNew Window\tgfm::NewWindow\tcmd-n\tglobal"));
    assert!(stdout.contains("command\tGFM\tServices\tsystem::Services\t-\tsystem"));
    assert!(stdout.contains("command\tEdit\tCopy\tsystem::Copy\tcmd-c\tsystem"));
}

#[test]
fn reports_ui_context_menu_contract_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "ui-context-menu-contract",
            "search-result",
            "1",
            "true",
            "false",
            "true",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("context-menu\tsurface=search-result\tselection=1\titems="));
    assert!(stdout.contains("item\topen\tOpen\tgfm::Open\tcommand\tenabled=true"));
    assert!(stdout.contains(
        "item\tshow-original\tShow in Enclosing Folder\tgfm::EnclosingFolder\tcommand\tenabled=true"
    ));
    assert!(stdout.contains(
        "item\tmove-to-trash\tMove to Trash\tgfm::MoveToTrash\tcommand\tenabled=true\tdestructive=true"
    ));
}

#[test]
fn reports_ui_dialog_contract_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["ui-dialog-contract", "conflict"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("dialog\tsurface=conflict\tpresentation=window-sheet"));
    assert!(stdout.contains("field\tapply-to-all\tApply to All\tcheckbox"));
    assert!(stdout.contains("button\treplace\tReplace\tdestructive\tenabled=true"));
    assert!(stdout.contains("button\tkeep-both\tKeep Both\tdefault\tenabled=true"));
    assert!(stdout.contains("button\tstop\tStop\tcancel\tenabled=true"));
}

#[test]
fn reports_ui_titlebar_contract_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["ui-titlebar-contract", "/tmp/gfm"])
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
        "titlebar\tGFM\theight=54\ttraffic-light=20x20\tmaterial=transparent-system-titlebar\tfocus=system-active-inactive\tfull-screen=native-macos-zoom-and-full-screen\ttabs=gfm-main-window"
    );
}

#[test]
fn reports_ui_session_contract_from_binary() {
    let store = std::env::temp_dir().join(format!(
        "gfm-missing-window-session-contract-{}.tsv",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&store);

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-session-contract")
        .arg("/tmp/gfm")
        .arg(&store)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with(
        "session\trestore=restore-last-window-bounds\tplacement-policy=persisted-or-centered"
    ));
    assert!(stdout.contains("tab-policy=native-macos-tab-group"));
    assert!(stdout.contains("activation=activate-app-and-focus-new-window"));
    assert!(stdout.contains("tabs=gfm-main-window"));
    assert!(stdout.contains("placement=centered"));
    assert!(
        stdout.contains("focus=true\tshow=true\tmovable=true\tresizable=true\tminimizable=true")
    );
    let _ = std::fs::remove_file(store);
}

#[test]
fn reports_ui_toolbar_contract_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["ui-toolbar-contract", "/tmp/gfm"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let mut lines = stdout.lines();

    assert_eq!(
        lines.next(),
        Some("toolbar\theight=54\ttraffic-light-gutter=96")
    );
    assert!(stdout.contains(
        "control\tlocation\tpath-title\tgfm\tcurrent-folder-title\tpath-title\t220px\tenabled=true\tselected=false"
    ));
    assert!(stdout.contains(
        "control\tview\ticon-view\tgrid\tview-as-icons\tsegmented-button\t34px\tenabled=true\tselected=true"
    ));
    assert!(stdout.contains(
        "control\tsearch\tsearch-field\tSearch\tmachine-search\tsearch-field\t232px\tenabled=true\tselected=false"
    ));
}

#[test]
fn reports_ui_sidebar_contract_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["ui-sidebar-contract", "/tmp/gfm"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let mut lines = stdout.lines();

    assert_eq!(
        lines.next(),
        Some(
            "sidebar\twidth=188\trow-height=28\tsection-header-height=26\tsections=Favorites,iCloud,Locations,Tags"
        )
    );
    assert!(stdout.contains("row\tFavorites\thome\t"));
    assert!(stdout.contains("row\tiCloud\ticloud-drive\tiCloud Drive\ticloud-drive\tcloud"));
    assert!(stdout.contains("row\tLocations\tcomputer\tComputer\tcomputer\tlocation"));
    assert!(stdout.contains("row\tTags\ttag-red\tRed\tfinder-tag\ttag"));
    assert!(stdout.contains("row\tTags\ttag-all\tAll Tags...\tfinder-tag\ttag"));
}

#[test]
fn reports_ui_icon_view_contract_from_binary() {
    let root = std::env::temp_dir().join(format!("gfm-icon-view-contract-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Folder")).unwrap();
    std::fs::write(root.join("Note.txt"), "note").unwrap();
    std::fs::write(root.join(".hidden"), "hidden").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-icon-view-contract")
        .arg(&root)
        .args(["2", "2", "0"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("icon-view\tsort=finder-name\ticon=64px"));
    assert!(stdout.contains("\ttotal=2\t"));
    assert!(stdout.contains("hidden-filtered=1"));
    assert!(stdout.contains("cell\t0\t"));
    assert!(stdout.contains("\tdir\t0x0\tFolder\t"));
    assert!(stdout.contains("\tfile\t112x0\tNote.txt\t"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_ui_virtualization_contract_from_binary() {
    let list = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "ui-virtualization-contract",
            "list-rows",
            "250000",
            "32",
            "199990",
        ])
        .output()
        .unwrap();
    assert!(
        list.status.success(),
        "{}",
        String::from_utf8_lossy(&list.stderr)
    );
    let list_stdout = String::from_utf8(list.stdout).unwrap();
    assert_eq!(
        list_stdout.trim(),
        "virtualization\tlist-rows\tunit=row\ttotal=250000\tvisible=199990..200022\trendered=32\tcapacity=32\tbounded=true"
    );

    let icon = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "ui-virtualization-contract",
            "icon-grid",
            "400000",
            "4",
            "40000",
            "6",
        ])
        .output()
        .unwrap();
    assert!(
        icon.status.success(),
        "{}",
        String::from_utf8_lossy(&icon.stderr)
    );
    let icon_stdout = String::from_utf8(icon.stdout).unwrap();
    assert_eq!(
        icon_stdout.trim(),
        "virtualization\ticon-grid\tunit=cell\ttotal=400000\tvisible=240000..240024\trendered=24\tcapacity=24\tbounded=true"
    );
}

#[test]
fn reports_ui_list_view_contract_from_binary() {
    let root = std::env::temp_dir().join(format!("gfm-list-view-contract-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Folder")).unwrap();
    std::fs::write(root.join("Note.txt"), "note").unwrap();
    std::fs::write(root.join(".hidden"), "hidden").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-list-view-contract")
        .arg(&root)
        .args(["6", "0"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("list-view\tsort=finder-name\trow-height=22px"));
    assert!(stdout.contains("\ttotal=2\t"));
    assert!(stdout.contains("hidden-filtered=1"));
    assert!(stdout.contains("column\tname\tName\t260px\tmin=120px"));
    assert!(stdout.contains("row\t0\t"));
    assert!(stdout.contains("\tdir\t0px\tdepth=0\texpandable=true"));
    assert!(stdout.contains("\tFolder\tname=Folder"));
    assert!(stdout.contains("\tfile\t22px\tdepth=0\texpandable=false"));
    assert!(stdout.contains("\tNote.txt\tname=Note.txt"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_ui_column_view_contract_from_binary() {
    let root =
        std::env::temp_dir().join(format!("gfm-column-view-contract-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Folder")).unwrap();
    std::fs::write(root.join("Folder").join("Child.txt"), "child").unwrap();
    std::fs::write(root.join("Note.txt"), "note").unwrap();
    std::fs::write(root.join(".hidden"), "hidden").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-column-view-contract")
        .arg(&root)
        .args(["6", "0", "Folder"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("column-view\tsort=finder-name\tcolumn-width=220px"));
    assert!(stdout.contains("\tcolumns=2\tpreview=true\t"));
    assert!(stdout.contains("hidden-filtered=1"));
    assert!(stdout.contains("keyboard=finder-left-right-column-navigation"));
    assert!(stdout.contains("column\t0\t"));
    assert!(stdout.contains("column\t1\t"));
    assert!(stdout.contains("\tdir\t0px\tFolder\t"));
    assert!(
        stdout.contains("selected=true\texpandable=true\tpreviewable=false\tbranch-loaded=true")
    );
    assert!(stdout.contains("\tfile\t0px\tChild.txt\t"));
    assert!(stdout.contains("preview\t2\t"));
    assert!(stdout.contains("\tfolder-summary\tFolder\t"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_ui_gallery_view_contract_from_binary() {
    let root =
        std::env::temp_dir().join(format!("gfm-gallery-view-contract-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Folder")).unwrap();
    std::fs::write(root.join("Image.png"), "image").unwrap();
    std::fs::write(root.join("Note.txt"), "note").unwrap();
    std::fs::write(root.join(".hidden"), "hidden").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-gallery-view-contract")
        .arg(&root)
        .args(["6", "0", "Image.png"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("gallery-view\tsort=finder-name\tpreview=720x420"));
    assert!(stdout.contains("\ttotal=3\t"));
    assert!(stdout.contains("hidden-filtered=1"));
    assert!(stdout.contains("keyboard=finder-left-right-filmstrip-navigation"));
    assert!(stdout.contains("preview\t"));
    assert!(stdout.contains("\timage-preview\tImage.png"));
    assert!(stdout.contains("metadata\t"));
    assert!(stdout.contains("quick-action\trotate-left\tRotate Left\tenabled=true"));
    assert!(stdout.contains("quick-action\tmarkup\tMarkup\tenabled=true"));
    assert!(stdout.contains("filmstrip\t"));
    assert!(stdout.contains("\tImage.png\tselected=true"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_ui_search_results_contract_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-search-results-contract-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Folder")).unwrap();
    std::fs::write(root.join("PLAN.md"), "plan").unwrap();
    std::fs::write(root.join("Folder").join("PLAN-notes.txt"), "notes").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-search-results-contract")
        .arg(&root)
        .args(["PLAN", "6", "0"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout
        .starts_with("search-results\tquery=PLAN\tscope=this-mac\tgrouping=kind\trow-height=24px"));
    assert!(stdout.contains("\ttotal=2\t"));
    assert!(stdout.contains("group\tfile\tDocuments\tcount=2"));
    assert!(stdout.contains("row\t0\t"));
    assert!(stdout.contains("\tPLAN.md\t"));
    assert!(stdout.contains("reason="));
    assert!(stdout.contains("stage="));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_ui_trash_view_contract_from_binary() {
    let root = std::env::temp_dir().join(format!("gfm-trash-view-contract-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("Note.txt"), "note").unwrap();
    std::fs::write(root.join("Locked.txt"), "locked").unwrap();
    let metadata = root.join("restore.tsv");
    std::fs::write(
        &metadata,
        "Note.txt\t/Users/me/Documents/Note.txt\t200\ttrue\ttrue\t\nLocked.txt\t/Users/me/Desktop/Locked.txt\t100\tfalse\tfalse\tfull-disk-access-required\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-trash-view-contract")
        .arg(&root)
        .arg(&metadata)
        .args(["6", "0"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("trash-view\tsort=deleted-newest\trow-height=24px"));
    assert!(stdout.contains("\ttotal=3\t"));
    assert!(stdout.contains(
        "command\tempty-trash\tEmpty Trash\tenabled=false\tdestructive=true\tdisabled-reason=permission-blocked"
    ));
    assert!(stdout.contains("row\t0\t"));
    assert!(stdout.contains("\tNote.txt\t"));
    assert!(stdout.contains("original=/Users/me/Documents/Note.txt\tdeleted-at=200"));
    assert!(stdout.contains("\tLocked.txt\t"));
    assert!(stdout.contains("restore=false\tdelete=false\tpermission=full-disk-access-required"));

    let _ = std::fs::remove_dir_all(root);
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
fn reports_quicklook_session_from_binary() {
    let path = std::env::temp_dir().join(format!("gfm-quicklook-{}.pdf", std::process::id()));
    std::fs::write(&path, b"%PDF-1.7\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("quicklook-session")
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("quicklook-session\tquick-look\t"));
    assert!(stdout.contains("\tallow-native\tnative-preview-controller\t"));
    assert!(stdout.contains("\tinvalidate-memory=true\tinvalidate-disk=true\t"));
    assert!(stdout.ends_with("schedule=scheduled:visible\n"));

    let _ = std::fs::remove_file(path);
}

#[test]
fn reports_thumbnail_generation_from_binary() {
    let path = std::env::temp_dir().join(format!("gfm-thumbnail-{}.png", std::process::id()));
    std::fs::write(&path, b"png").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("thumbnail-generation")
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("thumbnail-generation\t"));
    assert!(stdout.contains("\tallow-native\tquicklook-thumbnailing\t512px\tscale=2000m\t"));
    assert!(stdout.contains("\tcache=refresh-memory-only\t"));
    assert!(stdout.contains("\tinvalidate-memory=true\tinvalidate-disk=false\t"));
    assert!(stdout.ends_with("schedule=scheduled:visible\n"));

    let _ = std::fs::remove_file(path);
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
