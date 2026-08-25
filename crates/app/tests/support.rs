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
