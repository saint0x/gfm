use std::os::unix::fs::PermissionsExt;
use std::process::Command;

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
        "window\tGFM\t/tmp/gfm\t1040x720\tmin=640x420\ttransparent-titlebar=true\tactivate=true\ttabs=gfm-main-window\tpermission-dialog=permission\npermission-prompt\tkind=general\tsurface=permission"
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
fn reports_permission_onboarding_dialog_contract_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-permission-onboarding-contract")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("dialog\tsurface=permission\tpresentation=window-sheet"));
    assert!(stdout.contains("button\topen-settings\tOpen Settings\tdefault\tenabled=true"));
    assert!(stdout.contains("button\tnot-now\tNot Now\tcancel\tenabled=true"));
    assert!(
        stdout.contains("\npermission-onboarding\taction="),
        "{stdout}"
    );
    assert!(stdout.contains("\tprompt-kind="), "{stdout}");
    assert!(
        stdout.contains("\tprompt-mode=defer-until-needed\t"),
        "{stdout}"
    );
    assert!(stdout.contains("\tfinder-parity-default="), "{stdout}");
    assert!(stdout.contains("\tmachine-search-ready="), "{stdout}");
    assert!(
        stdout.contains("\npermission-scope\tdesktop\tstate="),
        "{stdout}"
    );
    assert!(
        stdout.contains("\npermission-scope\tfull-disk-access\tstate="),
        "{stdout}"
    );
}

#[test]
fn permission_onboarding_contract_uses_full_disk_access_prompt_from_binary() {
    let root = std::env::temp_dir().join(format!("gfm-permission-fda-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let home = root.join("home");
    let mail = home.join("Library").join("Mail");
    std::fs::create_dir_all(&mail).unwrap();
    std::fs::set_permissions(&mail, std::fs::Permissions::from_mode(0o000)).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("HOME", &home)
        .arg("ui-permission-onboarding-contract")
        .output()
        .unwrap();

    let _ = std::fs::set_permissions(&mail, std::fs::Permissions::from_mode(0o700));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("dialog\tsurface=permission\tpresentation=window-sheet"));
    assert!(
        stdout.contains("\ttitle=Allow Full Disk Access\t"),
        "{stdout}"
    );
    assert!(
        stdout.contains("\tprompt-kind=full-disk-access\t"),
        "{stdout}"
    );
    assert!(
        stdout.contains("button\topen-settings\tOpen Settings\tdefault\tenabled=true"),
        "{stdout}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_permission_ui_refresh_in_onboarding_contract_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-ui-permission-refresh-onboarding-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("permission-state.tsv");
    seed_stale_permission_state(&state);

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_PERMISSION_STATE", &state)
        .arg("ui-permission-onboarding-contract")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("dialog\tsurface=permission\tpresentation=window-sheet"));
    assert!(
        stdout.contains("\npermission-refresh\taudience=ui\tinitialized=false\tchanged=1\t"),
        "{stdout}"
    );
    assert!(stdout.contains("\trefresh-ui=true\t"), "{stdout}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_permission_ui_refresh_in_lifecycle_contract_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-ui-permission-refresh-lifecycle-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("permission-state.tsv");
    seed_stale_permission_state(&state);

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_PERMISSION_STATE", &state)
        .args(["ui-contract", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("window\tGFM\t"));
    assert!(
        stdout.contains("\tpermission-dialog=permission"),
        "{stdout}"
    );
    assert!(stdout.contains("\npermission-prompt\tkind="), "{stdout}");
    assert!(
        stdout.contains("\npermission-refresh\taudience=ui\tinitialized=false\tchanged=1\t"),
        "{stdout}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_restorable_progress_surfaces_in_lifecycle_contract_from_binary() {
    let progress = std::env::temp_dir().join(format!(
        "gfm-ui-lifecycle-progress-{}.gfmprogress",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&progress);

    let seed = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("jobs-progress-snapshot")
        .arg(&progress)
        .output()
        .unwrap();
    assert!(
        seed.status.success(),
        "{}",
        String::from_utf8_lossy(&seed.stderr)
    );

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_PROGRESS_STORE", &progress)
        .args(["ui-contract", "/tmp/gfm"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("window\tGFM\t/tmp/gfm\t"), "{stdout}");
    assert!(
        stdout.contains(
            "\noperation-progress\tjob=1\tlabel=copy selected files\tstate=running\tcompleted=42\ttotal=100\tpercent=42\t"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "\noperation-progress\tjob=2\tlabel=index content\tstate=paused\tcompleted=128\ttotal=250\tpercent=51\t"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains("\noperation-progress-command\tpause\tjob=1\tenabled=true"),
        "{stdout}"
    );
    assert!(
        stdout.contains("\noperation-progress-command\tresume\tjob=2\tenabled=true"),
        "{stdout}"
    );
    assert!(!stdout.contains("compact content segments"), "{stdout}");

    let _ = std::fs::remove_file(progress);
}

#[test]
fn reports_operation_conflict_surfaces_in_lifecycle_contract_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-ui-lifecycle-operation-conflict-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("source.txt");
    let target = root.join("target.txt");
    let source_dir = root.join("source-dir");
    let target_dir = root.join("target-dir");
    let conflicts = root.join("operation-conflicts.tsv");
    let journal = root.join("ops.journal");
    std::fs::write(&source, "new").unwrap();
    std::fs::write(&target, "old").unwrap();
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::create_dir_all(&target_dir).unwrap();
    std::fs::write(source_dir.join("child.txt"), "new").unwrap();
    std::fs::write(target_dir.join("child.txt"), "old").unwrap();

    let failed_copy = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPERATION_CONFLICT_STORE", &conflicts)
        .env("GFM_OPS_JOURNAL", &journal)
        .arg("copy")
        .arg(&source)
        .arg(&target)
        .output()
        .unwrap();
    assert!(
        !failed_copy.status.success(),
        "{}",
        String::from_utf8_lossy(&failed_copy.stdout)
    );
    assert!(
        String::from_utf8_lossy(&failed_copy.stderr).contains("destination already exists"),
        "{}",
        String::from_utf8_lossy(&failed_copy.stderr)
    );
    let failed_move = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPERATION_CONFLICT_STORE", &conflicts)
        .env("GFM_OPS_JOURNAL", &journal)
        .arg("move")
        .arg(&source_dir)
        .arg(&target_dir)
        .output()
        .unwrap();
    assert!(
        !failed_move.status.success(),
        "{}",
        String::from_utf8_lossy(&failed_move.stdout)
    );
    assert!(
        String::from_utf8_lossy(&failed_move.stderr).contains("destination already exists"),
        "{}",
        String::from_utf8_lossy(&failed_move.stderr)
    );
    assert!(conflicts.is_file());
    let conflict_store = std::fs::read_to_string(&conflicts).unwrap();
    assert!(
        conflict_store.contains(
            "\texists=true\tkind=file\tpolicy=fail\tavailable=replace,keep-both,skip\tblocks-operation=true\t"
        ),
        "{conflict_store}"
    );
    assert!(
        conflict_store.contains(&format!("\tsource={}\t", source.display())),
        "{conflict_store}"
    );
    assert!(
        conflict_store.contains(
            "\texists=true\tkind=directory\tpolicy=fail\tavailable=replace,keep-both,merge,skip\tblocks-operation=true\t"
        ),
        "{conflict_store}"
    );
    assert!(
        conflict_store.contains(&format!("\tsource={}\t", source_dir.display())),
        "{conflict_store}"
    );

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPERATION_CONFLICT_STORE", &conflicts)
        .args(["ui-contract", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("window\tGFM\t"), "{stdout}");
    assert!(
        stdout.contains(
            "\noperation-conflict-ui\toperation=batch\ttarget=2 items\tkind=mixed\tpolicy=fail\tavailable=replace,keep-both,skip\t"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "\tfocus=keep-both\tdefault-action=keep-both\tcancel-action=stop\tkeyboard=finder-conflict-sheet-return-default-escape-cancel-tab-cycle\t"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "\noperation-conflict-row\t0\toperation=copy\tsource={}\t",
            source.display()
        )),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "\noperation-conflict-row\t1\toperation=move\tsource={}\t",
            source_dir.display()
        )),
        "{stdout}"
    );
    assert!(stdout.contains("\tblocks-operation=true\t"), "{stdout}");
    assert!(
        stdout.contains("button\tmerge\tMerge\talternate\tenabled=false"),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "\noperation-conflict-action\tpolicy=replace\tcommand=operation-conflict-apply-all\tstore={}\ttarget=-\tapply-to-all=true",
            conflicts.display()
        )),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "\noperation-conflict-action\tpolicy=keep-both\tcommand=operation-conflict-apply-all\tstore={}\ttarget=-\tapply-to-all=true",
            conflicts.display()
        )),
        "{stdout}"
    );
    assert!(
        !stdout.contains("policy=merge\tcommand=operation-conflict-apply-all"),
        "{stdout}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn resolves_operation_conflict_surface_before_next_lifecycle_contract() {
    let root = std::env::temp_dir().join(format!(
        "gfm-ui-resolve-operation-conflict-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("source.txt");
    let target = root.join("target.txt");
    let conflicts = root.join("operation-conflicts.tsv");
    let journal = root.join("ops.journal");
    std::fs::write(&source, "new").unwrap();
    std::fs::write(&target, "old").unwrap();

    let failed_copy = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPERATION_CONFLICT_STORE", &conflicts)
        .env("GFM_OPS_JOURNAL", &journal)
        .arg("copy")
        .arg(&source)
        .arg(&target)
        .output()
        .unwrap();
    assert!(
        !failed_copy.status.success(),
        "{}",
        String::from_utf8_lossy(&failed_copy.stdout)
    );

    let invalid_resolve = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-operation-conflict-resolve")
        .arg(&conflicts)
        .arg(target.to_str().unwrap())
        .arg("merge")
        .output()
        .unwrap();
    assert!(!invalid_resolve.status.success());
    let still_blocking = std::fs::read_to_string(&conflicts).unwrap();
    assert!(
        still_blocking.contains("\tblocks-operation=true\t"),
        "{still_blocking}"
    );

    let resolve = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-operation-conflict-resolve")
        .arg(&conflicts)
        .arg(target.to_str().unwrap())
        .arg("keep-both")
        .output()
        .unwrap();
    assert!(
        resolve.status.success(),
        "{}",
        String::from_utf8_lossy(&resolve.stderr)
    );
    let resolve_stdout = String::from_utf8(resolve.stdout).unwrap();
    assert!(
        resolve_stdout.contains("operation-conflict-control\tresolve\t"),
        "{resolve_stdout}"
    );
    assert!(
        resolve_stdout.contains("\tpolicy=keep-both\tblocks-operation=false\t"),
        "{resolve_stdout}"
    );
    assert!(
        resolve_stdout.contains("\noperation-conflict-ui\toperation=copy\t"),
        "{resolve_stdout}"
    );
    assert!(
        resolve_stdout.contains(&format!(
            "\noperation-conflict-row\t0\toperation=copy\tsource={}\t",
            source.display()
        )),
        "{resolve_stdout}"
    );

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPERATION_CONFLICT_STORE", &conflicts)
        .args(["ui-contract", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("window\tGFM\t"), "{stdout}");
    assert!(!stdout.contains("\noperation-conflict-ui\t"), "{stdout}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_progress_dialog_pause_resume_contract_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["ui-dialog-contract", "progress", "paused", "true"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("dialog\tsurface=progress\tpresentation=progress-sheet"));
    assert!(stdout.contains("field\tprogress\tProgress\tprogress"));
    assert!(stdout.contains("button\tpause\tPause\talternate\tenabled=false"));
    assert!(stdout.contains("button\tresume\tResume\tdefault\tenabled=true"));
    assert!(stdout.contains("button\tstop\tStop\tcancel\tenabled=true"));
}

#[test]
fn reports_progress_dialog_from_job_progress_store() {
    let progress = std::env::temp_dir().join(format!(
        "gfm-ui-progress-job-contract-{}.gfmprogress",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&progress);

    let seed = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("jobs-progress-snapshot")
        .arg(&progress)
        .output()
        .unwrap();
    assert!(
        seed.status.success(),
        "{}",
        String::from_utf8_lossy(&seed.stderr)
    );

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-progress-job-contract")
        .arg(&progress)
        .arg("2")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("dialog\tsurface=progress\tpresentation=progress-sheet"));
    assert!(stdout.contains("button\tpause\tPause\talternate\tenabled=false"));
    assert!(stdout.contains("button\tresume\tResume\tdefault\tenabled=true"));
    assert!(stdout.contains("button\tstop\tStop\tcancel\tenabled=true"));
    assert!(stdout.contains(
        "operation-progress\tjob=2\tlabel=index content\tstate=paused\tcompleted=128\ttotal=250\tpercent=51\tdetail=pressure:throttled"
    ));
    assert!(stdout.contains("operation-progress-command\tresume\tjob=2\tenabled=true"));

    let _ = std::fs::remove_file(progress);
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
fn reports_fileprovider_state_in_ui_sidebar_contract_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-ui-sidebar-fileprovider-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let downloading = root.join("Downloading.icloud-downloading");
    std::fs::write(&downloading, "downloading").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-sidebar-fileprovider-contract")
        .arg(&downloading)
        .arg(&downloading)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.contains("row\tiCloud\ticloud-drive\tiCloud Drive\ticloud-drive\tcloud"));
    assert!(stdout.contains("\tselected=true\t"));
    assert!(stdout.contains("\tcloud=downloading\tcloud-progress=-"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_volume_invalidation_in_ui_sidebar_contract_from_binary() {
    let root = std::env::temp_dir().join(format!("gfm-ui-sidebar-volume-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "network-smb\n").unwrap();

    let changed = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-sidebar-volume-invalidation")
        .arg("description-changed")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        changed.status.success(),
        "{}",
        String::from_utf8_lossy(&changed.stderr)
    );
    let changed_stdout = String::from_utf8(changed.stdout).unwrap();
    assert!(changed_stdout.starts_with("sidebar-volume-invalidation\trow=volume-"));
    assert!(changed_stdout.contains(&format!("\tpath={}\t", root.display())));
    assert!(changed_stdout.contains("\tkind=description-changed\tcurrent-kind=network\t"));
    assert!(changed_stdout
        .contains("\tcurrent-mount=mounted\tread-only=false\tnetwork=true\treachable=true\t"));
    assert!(changed_stdout.contains(
        "\tinvalidate-row=true\tinvalidate-section=true\tremove-row=false\tdisable-row=false\t"
    ));

    let missing = root.join("gone");
    let disappeared = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-sidebar-volume-invalidation")
        .arg("disappeared")
        .arg(&missing)
        .output()
        .unwrap();
    assert!(
        disappeared.status.success(),
        "{}",
        String::from_utf8_lossy(&disappeared.stderr)
    );
    let disappeared_stdout = String::from_utf8(disappeared.stdout).unwrap();
    assert_eq!(
        disappeared_stdout,
        format!(
            "sidebar-volume-invalidation\trow=-\tpath={}\tkind=disappeared\tcurrent-kind=-\tcurrent-mount=-\tread-only=-\tnetwork=-\treachable=-\tinvalidate-row=true\tinvalidate-section=true\tremove-row=true\tdisable-row=false\treason=sidebar-volume-disappeared\n",
            missing.display()
        )
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_fileprovider_conflict_in_ui_dialog_contract_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-ui-fileprovider-conflict-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let conflict = root.join("Conflict.icloud-conflict.md");
    std::fs::write(&conflict, "conflict").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-fileprovider-conflict-contract")
        .arg(&conflict)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("dialog\tsurface=conflict\tpresentation=window-sheet"));
    assert!(stdout.contains("button\treveal-conflict\tReveal Conflict\tdefault\tenabled=true"));
    assert!(stdout.contains("button\tstop\tStop\tcancel\tenabled=true"));
    assert!(stdout.contains("\nprovider-conflict\tpath="));
    assert!(stdout.contains("\tconflict=true\taffected=1\taffected-paths="));
    assert!(stdout.contains(
        "\treveal=true\toperations-blocked=true\treason=conflict-requires-user-resolution"
    ));

    let _ = std::fs::remove_dir_all(root);
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
fn ui_list_view_refuses_unreachable_network_volume_before_reading_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-list-view-unreachable-volume-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    std::fs::write(root.join("Visible.txt"), "should not be listed").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-list-view-contract")
        .arg(&root)
        .args(["6", "0"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("list-view\t"), "{stdout}");
    assert!(
        stderr.contains("ui list view volume access blocked: unreachable volume network"),
        "{stderr}"
    );

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
fn reports_operation_conflict_sheet_from_existing_destination() {
    let root =
        std::env::temp_dir().join(format!("gfm-ui-operation-conflict-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("source.txt");
    let target = root.join("target.txt");
    std::fs::write(&source, "new").unwrap();
    std::fs::write(&target, "old").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-operation-conflict-contract")
        .arg("copy")
        .arg(&source)
        .arg(&target)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("dialog\tsurface=conflict\tpresentation=window-sheet"));
    assert!(stdout.contains("button\treplace\tReplace\tdestructive\tenabled=true"));
    assert!(stdout.contains("button\tkeep-both\tKeep Both\tdefault\tenabled=true"));
    assert!(stdout.contains("button\tmerge\tMerge\talternate\tenabled=false"));
    assert!(stdout.contains("\noperation-conflict-ui\toperation=copy\t"));
    assert!(stdout.contains("\tfocus=keep-both\tdefault-action=keep-both\tcancel-action=stop\t"));
    assert!(stdout.contains(&format!(
        "\noperation-conflict-row\t0\toperation=copy\tsource={}\t",
        source.display()
    )));
    assert!(!stdout.contains("\texists=true"));
    assert!(stdout.contains(
        "\tkind=file\tpolicy=fail\tavailable=replace,keep-both,skip\tblocks-operation=true\t"
    ));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_directory_operation_conflict_with_merge_resolution() {
    let root = std::env::temp_dir().join(format!(
        "gfm-ui-directory-operation-conflict-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let source = root.join("source");
    let target = root.join("target");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(&target).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("ui-operation-conflict-contract")
        .arg("move")
        .arg(&source)
        .arg(&target)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.contains("button\tmerge\tMerge\talternate\tenabled=true"));
    assert!(stdout.contains("\tfocus=keep-both\tdefault-action=keep-both\tcancel-action=stop\t"));
    assert!(stdout.contains(&format!(
        "\noperation-conflict-row\t0\toperation=move\tsource={}\t",
        source.display()
    )));
    assert!(stdout.contains("\tkind=directory\tpolicy=fail\tavailable=replace,keep-both,merge,skip\tblocks-operation=true\t"));

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
    assert!(stdout.contains("\tallow-native\tcloud=native-eligible\tnative-preview-controller\t"));
    assert!(stdout.contains("\tinvalidate-memory=true\tinvalidate-disk=true\t"));
    assert!(stdout.ends_with("schedule=scheduled:visible\n"));

    let _ = std::fs::remove_file(path);
}

#[test]
fn quicklook_refuses_unreachable_network_volume_before_preview_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-quicklook-unreachable-volume-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Project")).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let path = root.join("Project").join("Preview.pdf");
    std::fs::write(&path, b"%PDF-1.7\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("quicklook-session")
        .arg(&path)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("quicklook-session\t"), "{stdout}");
    assert!(
        stderr.contains("quicklook preview volume access blocked: unreachable volume network"),
        "{stderr}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn adaptive_quicklook_session_stays_visible_under_pressure_from_binary() {
    let path =
        std::env::temp_dir().join(format!("gfm-quicklook-adaptive-{}.pdf", std::process::id()));
    std::fs::write(&path, b"%PDF-1.7\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "quicklook-session-adaptive",
            path.to_str().unwrap(),
            "saturated",
            "critical",
            "low",
            "active",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.starts_with("quicklook-session\tquick-look\t"));
    assert!(stdout.contains("\tschedule=scheduled:visible\taction=Run\tdeferred=false\n"));

    let _ = std::fs::remove_file(path);
}

#[test]
fn cancelled_quicklook_session_stops_before_planning_from_binary() {
    let path =
        std::env::temp_dir().join(format!("gfm-quicklook-cancel-{}.pdf", std::process::id()));
    std::fs::write(&path, b"%PDF-1.7\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("quicklook-session-cancel")
        .arg(&path)
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
        "quicklook-session\tstatus=cancelled\treason=cancelled-before-plan\n"
    );

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
    assert!(stdout.contains(
        "\tallow-native\tcloud=native-eligible\tquicklook-thumbnailing\t512px\tscale=2000m\t"
    ));
    assert!(stdout.contains("\tcache=refresh-memory-only\t"));
    assert!(stdout.contains("\tinvalidate-memory=true\tinvalidate-disk=false\t"));
    assert!(stdout.ends_with("schedule=scheduled:visible\n"));

    let _ = std::fs::remove_file(path);
}

#[test]
fn thumbnail_refuses_unreachable_network_volume_before_generation_from_binary() {
    let root = std::env::temp_dir().join(format!(
        "gfm-thumbnail-unreachable-volume-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Project")).unwrap();
    std::fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
    let path = root.join("Project").join("Preview.png");
    std::fs::write(&path, b"png").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("thumbnail-generation")
        .arg(&path)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("thumbnail-generation\t"), "{stdout}");
    assert!(
        stderr.contains("thumbnail generation volume access blocked: unreachable volume network"),
        "{stderr}"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn adaptive_thumbnail_generation_defers_under_saturated_pressure_from_binary() {
    let path =
        std::env::temp_dir().join(format!("gfm-thumbnail-adaptive-{}.png", std::process::id()));
    std::fs::write(&path, b"png").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "thumbnail-generation-adaptive",
            path.to_str().unwrap(),
            "saturated",
            "critical",
            "low",
            "active",
        ])
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
        "thumbnail-generation\tstatus=deferred\taction=Defer\tdeferred=true\n"
    );

    let _ = std::fs::remove_file(path);
}

#[test]
fn cancelled_thumbnail_generation_stops_before_planning_from_binary() {
    let path =
        std::env::temp_dir().join(format!("gfm-thumbnail-cancel-{}.png", std::process::id()));
    std::fs::write(&path, b"png").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("thumbnail-generation-cancel")
        .arg(&path)
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
        "thumbnail-generation\tstatus=cancelled\treason=cancelled-before-plan\n"
    );

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
    assert_eq!(fields.len(), 4, "{first_scope}");
    fields[1] = if fields[1] == "unknown" {
        "granted".to_string()
    } else {
        "unknown".to_string()
    };
    *first_scope = fields.join("\t");
    std::fs::write(state, format!("{}\n", lines.join("\n"))).unwrap();
}
