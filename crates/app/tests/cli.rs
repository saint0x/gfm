use std::fs;
use std::io::{Cursor, Write};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use zip::write::SimpleFileOptions;

#[test]
fn indexes_and_searches_real_files_from_binary() {
    let root = unique_temp_dir("gfm-cli-root");
    let index = unique_temp_path("gfm-cli-index", "gfmidx");
    fs::create_dir_all(root.join("Reports")).unwrap();
    fs::write(root.join("Reports").join("QuarterlyPlan.md"), "alpha").unwrap();
    fs::write(root.join("notes.txt"), "beta").unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), index.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["search-index", index.to_str().unwrap(), "quarterly"])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );

    let stdout = String::from_utf8(search_output.stdout).unwrap();
    assert!(stdout.contains("QuarterlyPlan.md"), "{stdout}");

    let mmap_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["search-index-mmap", index.to_str().unwrap(), "quarterly"])
        .output()
        .unwrap();
    assert!(
        mmap_output.status.success(),
        "{}",
        String::from_utf8_lossy(&mmap_output.stderr)
    );
    assert_eq!(String::from_utf8(mmap_output.stdout).unwrap(), stdout);

    let verify_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["records-verify", index.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        verify_output.status.success(),
        "{}",
        String::from_utf8_lossy(&verify_output.stderr)
    );
    let verify_stdout = String::from_utf8(verify_output.stdout).unwrap();
    assert!(
        verify_stdout.contains("\tchecksum=verified"),
        "{verify_stdout}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(index).unwrap();
}

#[test]
fn persists_volume_index_state_from_binary() {
    let root = unique_temp_dir("gfm-cli-index-state-root");
    let index = unique_temp_path("gfm-cli-index-state-records", "gfmidx");
    let state = unique_temp_path("gfm-cli-index-state", "gfmstate");
    fs::create_dir_all(root.join("Projects")).unwrap();
    fs::write(root.join("Projects").join("StatefulSearch.md"), "alpha").unwrap();

    let first = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-state",
            root.to_str().unwrap(),
            index.to_str().unwrap(),
            state.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first_stdout = String::from_utf8(first.stdout).unwrap();
    assert!(first_stdout.starts_with("index-state\t"), "{first_stdout}");
    assert!(first_stdout.contains("\tschema=1\t"), "{first_stdout}");
    assert!(first_stdout.contains("\tscan-epoch=1\t"), "{first_stdout}");
    assert!(
        first_stdout.contains("\trecord-count=3\t"),
        "{first_stdout}"
    );

    let second = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-state",
            root.to_str().unwrap(),
            index.to_str().unwrap(),
            state.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let second_stdout = String::from_utf8(second.stdout).unwrap();
    assert!(
        second_stdout.contains("\tscan-epoch=2\t"),
        "{second_stdout}"
    );

    let inspect = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index-state-inspect", state.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        inspect.status.success(),
        "{}",
        String::from_utf8_lossy(&inspect.stderr)
    );
    let inspect_stdout = String::from_utf8(inspect.stdout).unwrap();
    assert_eq!(inspect_stdout, second_stdout);

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["search-index", index.to_str().unwrap(), "stateful"])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );
    let search_stdout = String::from_utf8(search_output.stdout).unwrap();
    assert!(
        search_stdout.contains("StatefulSearch.md"),
        "{search_stdout}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(index).unwrap();
    fs::remove_file(state).unwrap();
}

#[test]
fn reports_scan_progress_from_binary() {
    let root = unique_temp_dir("gfm-cli-scan-progress-root");
    let records = unique_temp_path("gfm-cli-scan-progress-records", "gfmidx");
    let progress = unique_temp_path("gfm-cli-scan-progress", "gfmprogress");
    fs::write(root.join("Progress.md"), "alpha").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "scan-progress",
            root.to_str().unwrap(),
            records.to_str().unwrap(),
            progress.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("scan-progress\t"), "{stdout}");
    assert!(stdout.contains("\tcompleted=true"), "{stdout}");

    let inspect = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["scan-progress-inspect", progress.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        inspect.status.success(),
        "{}",
        String::from_utf8_lossy(&inspect.stderr)
    );
    assert_eq!(String::from_utf8(inspect.stdout).unwrap(), stdout);

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(progress).unwrap();
}

#[test]
fn reports_fair_scan_from_binary() {
    let root = unique_temp_dir("gfm-cli-fair-scan-root");
    let visible = root.join("Visible");
    let background = root.join("Background");
    fs::create_dir_all(&visible).unwrap();
    fs::create_dir_all(&background).unwrap();
    fs::write(visible.join("Needle.md"), "visible").unwrap();
    fs::write(background.join("Bulk.md"), "background").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "fair-scan",
            root.to_str().unwrap(),
            "2",
            visible.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("fair-scan\t"), "{stdout}");
    assert!(stdout.contains("\tvisible-records="), "{stdout}");
    assert!(stdout.contains("\tbackground-records="), "{stdout}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reports_rename_correlation_from_binary() {
    let root = unique_temp_dir("gfm-cli-rename-root");
    let from = root.join("RenameOld.md");
    let to = root.join("RenameNew.md");
    fs::write(&from, "rename identity").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "rename-correlation",
            from.to_str().unwrap(),
            to.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("rename-correlation\t"), "{stdout}");
    assert!(stdout.contains("\tremoved=1\t"), "{stdout}");
    assert!(stdout.contains("\tinserted=1\t"), "{stdout}");
    assert!(stdout.contains("\tpreserved=1"), "{stdout}");
    assert!(!from.exists());
    assert!(to.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reports_metadata_update_from_binary() {
    let root = unique_temp_dir("gfm-cli-metadata-root");
    let path = root.join("Metadata.md");
    fs::write(&path, "metadata").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "metadata-update",
            path.to_str().unwrap(),
            " appended content",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("metadata-update\t"), "{stdout}");
    assert!(stdout.contains("\texisted=true\t"), "{stdout}");
    assert!(stdout.contains("size"), "{stdout}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reports_event_backpressure_from_binary() {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["event-backpressure", "5", "2", "8", "2"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("event-backpressure\t"), "{stdout}");
    assert!(stdout.contains("\tvisible=2\t"), "{stdout}");
    assert!(stdout.contains("repair-required=true"), "{stdout}");
}

#[test]
fn persists_fsevents_cursor_from_binary() {
    let root = unique_temp_dir("gfm-cli-fsevents-root");
    let index = unique_temp_path("gfm-cli-fsevents-records", "gfmidx");
    let state = unique_temp_path("gfm-cli-fsevents-state", "gfmstate");
    let cursor = unique_temp_path("gfm-cli-fsevents-cursor", "gfmcursor");
    fs::write(root.join("CursorSearch.md"), "alpha").unwrap();

    let index_state = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-state",
            root.to_str().unwrap(),
            index.to_str().unwrap(),
            state.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        index_state.status.success(),
        "{}",
        String::from_utf8_lossy(&index_state.stderr)
    );

    let checkpoint = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "fsevents-cursor-checkpoint",
            state.to_str().unwrap(),
            cursor.to_str().unwrap(),
            "123",
        ])
        .output()
        .unwrap();
    assert!(
        checkpoint.status.success(),
        "{}",
        String::from_utf8_lossy(&checkpoint.stderr)
    );
    let checkpoint_stdout = String::from_utf8(checkpoint.stdout).unwrap();
    assert!(
        checkpoint_stdout.starts_with("fsevents-cursor\t"),
        "{checkpoint_stdout}"
    );
    assert!(
        checkpoint_stdout.contains("\tlast-event-id=123\thealth=clean"),
        "{checkpoint_stdout}"
    );

    let inspect = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["fsevents-cursor-inspect", cursor.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        inspect.status.success(),
        "{}",
        String::from_utf8_lossy(&inspect.stderr)
    );
    let inspect_stdout = String::from_utf8(inspect.stdout).unwrap();
    assert_eq!(inspect_stdout, checkpoint_stdout);

    let resume = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "fsevents-cursor-resume",
            state.to_str().unwrap(),
            cursor.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        resume.status.success(),
        "{}",
        String::from_utf8_lossy(&resume.stderr)
    );
    let resume_stdout = String::from_utf8(resume.stdout).unwrap();
    assert_eq!(
        resume_stdout.trim(),
        "fsevents-resume\taction=continue\tfrom-event-id=124\treason=cursor-clean"
    );

    let reindex = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-state",
            root.to_str().unwrap(),
            index.to_str().unwrap(),
            state.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        reindex.status.success(),
        "{}",
        String::from_utf8_lossy(&reindex.stderr)
    );
    let stale = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "fsevents-cursor-resume",
            state.to_str().unwrap(),
            cursor.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        stale.status.success(),
        "{}",
        String::from_utf8_lossy(&stale.stderr)
    );
    let stale_stdout = String::from_utf8(stale.stdout).unwrap();
    assert_eq!(
        stale_stdout.trim(),
        "fsevents-resume\taction=rescan\tfrom-event-id=-\treason=scan-epoch-changed"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(index).unwrap();
    fs::remove_file(state).unwrap();
    fs::remove_file(cursor).unwrap();
}

#[test]
fn schedules_fsevents_repair_from_binary() {
    let root = unique_temp_dir("gfm-cli-repair-root");
    let index = unique_temp_path("gfm-cli-repair-records", "gfmidx");
    let state = unique_temp_path("gfm-cli-repair-state", "gfmstate");
    let cursor = unique_temp_path("gfm-cli-repair-cursor", "gfmcursor");
    fs::create_dir_all(root.join("Projects").join("Nested")).unwrap();
    fs::write(
        root.join("Projects").join("Nested").join("Repair.md"),
        "alpha",
    )
    .unwrap();

    let index_state = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-state",
            root.to_str().unwrap(),
            index.to_str().unwrap(),
            state.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        index_state.status.success(),
        "{}",
        String::from_utf8_lossy(&index_state.stderr)
    );

    let checkpoint = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "fsevents-cursor-checkpoint",
            state.to_str().unwrap(),
            cursor.to_str().unwrap(),
            "200",
        ])
        .output()
        .unwrap();
    assert!(
        checkpoint.status.success(),
        "{}",
        String::from_utf8_lossy(&checkpoint.stderr)
    );

    let gap = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "fsevents-repair-schedule",
            state.to_str().unwrap(),
            cursor.to_str().unwrap(),
            "201,204",
            "-",
        ])
        .output()
        .unwrap();
    assert!(
        gap.status.success(),
        "{}",
        String::from_utf8_lossy(&gap.stderr)
    );
    let gap_stdout = String::from_utf8(gap.stdout).unwrap();
    assert!(gap_stdout.starts_with("repair-schedule\t"), "{gap_stdout}");
    assert!(gap_stdout.contains("\tjobs=1\t"), "{gap_stdout}");
    assert!(
        gap_stdout.contains("reason=event-id-gap:202-204"),
        "{gap_stdout}"
    );

    let explicit = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "fsevents-repair-schedule",
            state.to_str().unwrap(),
            cursor.to_str().unwrap(),
            "201",
            "kernel-dropped",
            "Projects",
            "Projects/Nested",
        ])
        .output()
        .unwrap();
    assert!(
        explicit.status.success(),
        "{}",
        String::from_utf8_lossy(&explicit.stderr)
    );
    let explicit_stdout = String::from_utf8(explicit.stdout).unwrap();
    assert!(explicit_stdout.contains("\tjobs=1\t"), "{explicit_stdout}");
    assert!(
        explicit_stdout.contains("reason=explicit-drop:kernel-dropped"),
        "{explicit_stdout}"
    );
    assert!(explicit_stdout.contains("Projects"), "{explicit_stdout}");

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(index).unwrap();
    fs::remove_file(state).unwrap();
    fs::remove_file(cursor).unwrap();
}

#[test]
fn searches_with_structured_filters_from_binary() {
    let root = unique_temp_dir("gfm-cli-filter-root");
    fs::create_dir_all(root.join("Desktop").join("Client Work")).unwrap();
    fs::write(
        root.join("Desktop")
            .join("Client Work")
            .join("final notes.md"),
        "approved",
    )
    .unwrap();
    fs::write(
        root.join("Desktop")
            .join("Client Work")
            .join("draft notes.md"),
        "draft",
    )
    .unwrap();
    fs::write(
        root.join("Desktop").join("Client Work").join("notes.pdf"),
        "pdf",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search",
            root.to_str().unwrap(),
            r#""Client Work" notes kind:file ext:md -draft size:>1b"#,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("final notes.md"), "{stdout}");
    assert!(!stdout.contains("draft notes.md"), "{stdout}");
    assert!(!stdout.contains("notes.pdf"), "{stdout}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn searches_with_date_filters_from_binary() {
    let root = unique_temp_dir("gfm-cli-date-filter-root");
    fs::write(root.join("current.md"), "fresh").unwrap();

    let after_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search",
            root.to_str().unwrap(),
            "current ext:md modified:>=2020-01-01",
        ])
        .output()
        .unwrap();
    assert!(
        after_output.status.success(),
        "{}",
        String::from_utf8_lossy(&after_output.stderr)
    );
    let after_stdout = String::from_utf8(after_output.stdout).unwrap();
    assert!(after_stdout.contains("current.md"), "{after_stdout}");

    let before_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search",
            root.to_str().unwrap(),
            "current ext:md modified:<2020-01-01",
        ])
        .output()
        .unwrap();
    assert!(
        before_output.status.success(),
        "{}",
        String::from_utf8_lossy(&before_output.stderr)
    );
    let before_stdout = String::from_utf8(before_output.stdout).unwrap();
    assert!(!before_stdout.contains("current.md"), "{before_stdout}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn searches_with_boolean_groups_from_binary() {
    let root = unique_temp_dir("gfm-cli-boolean-filter-root");
    fs::write(root.join("report.md"), "report").unwrap();
    fs::write(root.join("invoice.md"), "invoice").unwrap();
    fs::write(root.join("draft-report.md"), "draft").unwrap();
    fs::write(root.join("image.png"), "image").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search",
            root.to_str().unwrap(),
            "(report OR invoice) NOT draft ext:md",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("report.md"), "{stdout}");
    assert!(stdout.contains("invoice.md"), "{stdout}");
    assert!(!stdout.contains("draft-report.md"), "{stdout}");
    assert!(!stdout.contains("image.png"), "{stdout}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn streams_hot_then_deep_search_batches_from_binary() {
    let root = unique_temp_dir("gfm-cli-stream-root");
    fs::write(root.join("needle.md"), "hot").unwrap();
    fs::write(root.join("needl"), "fuzzy").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["search-stream", root.to_str().unwrap(), "needle"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    let hot = stdout.find("batch\thot").expect(&stdout);
    let deep = stdout.find("batch\tdeep").expect(&stdout);
    assert!(hot < deep, "{stdout}");
    assert!(stdout.contains("needle.md"), "{stdout}");
    assert!(stdout.contains("needl"), "{stdout}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reports_package_traversal_policy_from_binary() {
    let root = unique_temp_dir("gfm-cli-package-root");
    fs::create_dir_all(root.join("GFMFixture.app").join("Contents")).unwrap();
    fs::write(
        root.join("GFMFixture.app")
            .join("Contents")
            .join("Info.plist"),
        "plist",
    )
    .unwrap();
    fs::create_dir_all(root.join("Proposal.pages").join("Data")).unwrap();
    fs::write(
        root.join("Proposal.pages").join("Data").join("Index.zip"),
        "zip",
    )
    .unwrap();

    let opaque = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["package-traversal", root.to_str().unwrap(), "opaque"])
        .output()
        .unwrap();
    assert!(
        opaque.status.success(),
        "{}",
        String::from_utf8_lossy(&opaque.stderr)
    );
    let opaque_stdout = String::from_utf8(opaque.stdout).unwrap();
    assert!(
        opaque_stdout.contains("package-traversal\tmode=opaque"),
        "{opaque_stdout}"
    );
    assert!(
        opaque_stdout.contains("package\tapplication\tfalse\tGFMFixture.app"),
        "{opaque_stdout}"
    );
    assert!(
        opaque_stdout.contains("package\tdocument-package\tfalse\tProposal.pages"),
        "{opaque_stdout}"
    );

    let traverse = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["package-traversal", root.to_str().unwrap(), "traverse"])
        .output()
        .unwrap();
    assert!(
        traverse.status.success(),
        "{}",
        String::from_utf8_lossy(&traverse.stderr)
    );
    let traverse_stdout = String::from_utf8(traverse.stdout).unwrap();
    assert!(
        traverse_stdout.contains("package-traversal\tmode=traverse"),
        "{traverse_stdout}"
    );
    assert!(
        traverse_stdout.contains("package\tapplication\ttrue\tGFMFixture.app"),
        "{traverse_stdout}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reports_finder_metadata_from_binary() {
    let root = unique_temp_dir("gfm-cli-finder-metadata");
    let app = root.join("GFMFixture.app");
    fs::create_dir_all(&app).unwrap();

    let app_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("finder-metadata")
        .arg(&app)
        .output()
        .unwrap();
    assert!(
        app_output.status.success(),
        "{}",
        String::from_utf8_lossy(&app_output.stderr)
    );
    let app_stdout = String::from_utf8(app_output.stdout).unwrap();
    assert!(app_stdout.starts_with("finder-metadata\t"), "{app_stdout}");
    assert!(
        app_stdout.contains("\tdisplay=GFMFixture\t"),
        "{app_stdout}"
    );
    assert!(app_stdout.contains("\tkind=Application\t"), "{app_stdout}");
    assert!(app_stdout.contains("\ttype=application\t"), "{app_stdout}");
    assert!(app_stdout.contains("\text-hidden=true\t"), "{app_stdout}");

    let target = root.join("target.txt");
    let link = root.join("target link");
    fs::write(&target, "target").unwrap();
    make_symlink(&target, &link);
    let link_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .arg("finder-metadata")
        .arg(&link)
        .output()
        .unwrap();
    assert!(
        link_output.status.success(),
        "{}",
        String::from_utf8_lossy(&link_output.stderr)
    );
    let link_stdout = String::from_utf8(link_output.stdout).unwrap();
    assert!(link_stdout.contains("\tkind=Alias\t"), "{link_stdout}");
    assert!(link_stdout.contains("\ttype=symlink\t"), "{link_stdout}");
    assert!(link_stdout.contains("\tlink=symlink\t"), "{link_stdout}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn searches_persisted_tags_from_binary() {
    let index = unique_temp_path("gfm-cli-tags", "gfmidx");
    let metadata = unique_temp_path("gfm-cli-tags", "gfmmeta");
    let dictionary = unique_temp_path("gfm-cli-tags", "gfmdict");
    let columns = unique_temp_path("gfm-cli-tags", "gfmcols");
    fs::write(
        &index,
        "gfm-store-v2\n1\t1\t0\tf\t5\t0\t0\t0\t0\tImportant,Client\t/tmp/tagged.md\n2\t2\t0\tf\t5\t0\t0\t0\t0\tLater\t/tmp/other.md\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["search-index", index.to_str().unwrap(), "tag:Important"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("tagged.md"), "{stdout}");
    assert!(!stdout.contains("other.md"), "{stdout}");

    let metadata_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-metadata",
            index.to_str().unwrap(),
            metadata.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        metadata_output.status.success(),
        "{}",
        String::from_utf8_lossy(&metadata_output.stderr)
    );

    let ids_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "metadata-ids-mmap",
            metadata.to_str().unwrap(),
            "tag",
            "Important",
        ])
        .output()
        .unwrap();
    assert!(
        ids_output.status.success(),
        "{}",
        String::from_utf8_lossy(&ids_output.stderr)
    );
    assert_eq!(String::from_utf8(ids_output.stdout).unwrap(), "1\t1\n");

    let block_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "metadata-id-block-mmap",
            metadata.to_str().unwrap(),
            "tag",
            "Important",
            "0",
        ])
        .output()
        .unwrap();
    assert!(
        block_output.status.success(),
        "{}",
        String::from_utf8_lossy(&block_output.stderr)
    );
    assert_eq!(String::from_utf8(block_output.stdout).unwrap(), "1\t1\n");

    let verify_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["metadata-verify", metadata.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        verify_output.status.success(),
        "{}",
        String::from_utf8_lossy(&verify_output.stderr)
    );
    let verify_stdout = String::from_utf8(verify_output.stdout).unwrap();
    assert!(
        verify_stdout.contains("\tchecksum=verified"),
        "{verify_stdout}"
    );

    let dictionary_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-dictionary",
            index.to_str().unwrap(),
            dictionary.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        dictionary_output.status.success(),
        "{}",
        String::from_utf8_lossy(&dictionary_output.stderr)
    );

    let lookup_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "dictionary-lookup",
            dictionary.to_str().unwrap(),
            "Important",
        ])
        .output()
        .unwrap();
    assert!(
        lookup_output.status.success(),
        "{}",
        String::from_utf8_lossy(&lookup_output.stderr)
    );
    let lookup_stdout = String::from_utf8(lookup_output.stdout).unwrap();
    assert!(
        lookup_stdout.starts_with("dictionary\tfound\t"),
        "{lookup_stdout}"
    );

    let dictionary_verify = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["dictionary-verify", dictionary.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        dictionary_verify.status.success(),
        "{}",
        String::from_utf8_lossy(&dictionary_verify.stderr)
    );
    let dictionary_verify_stdout = String::from_utf8(dictionary_verify.stdout).unwrap();
    assert!(
        dictionary_verify_stdout.contains("\tchecksum=verified"),
        "{dictionary_verify_stdout}"
    );

    let columns_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-columns",
            index.to_str().unwrap(),
            columns.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        columns_output.status.success(),
        "{}",
        String::from_utf8_lossy(&columns_output.stderr)
    );

    let columns_verify = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["columns-verify", columns.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        columns_verify.status.success(),
        "{}",
        String::from_utf8_lossy(&columns_verify.stderr)
    );
    let columns_verify_stdout = String::from_utf8(columns_verify.stdout).unwrap();
    assert!(
        columns_verify_stdout.contains("\tchecksum=verified"),
        "{columns_verify_stdout}"
    );

    let columns_lookup = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["columns-lookup", columns.to_str().unwrap(), "1", "1"])
        .output()
        .unwrap();
    assert!(
        columns_lookup.status.success(),
        "{}",
        String::from_utf8_lossy(&columns_lookup.stderr)
    );
    let columns_lookup_stdout = String::from_utf8(columns_lookup.stdout).unwrap();
    assert!(
        columns_lookup_stdout.contains("\tname=tagged.md\t"),
        "{columns_lookup_stdout}"
    );
    assert!(
        columns_lookup_stdout.contains("\ttags=client,important\t"),
        "{columns_lookup_stdout}"
    );

    let column_search = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-index-columns",
            index.to_str().unwrap(),
            columns.to_str().unwrap(),
            "tag:Important",
        ])
        .output()
        .unwrap();
    assert!(
        column_search.status.success(),
        "{}",
        String::from_utf8_lossy(&column_search.stderr)
    );
    let column_search_stdout = String::from_utf8(column_search.stdout).unwrap();
    assert!(
        column_search_stdout.contains("tagged.md"),
        "{column_search_stdout}"
    );
    assert!(
        !column_search_stdout.contains("other.md"),
        "{column_search_stdout}"
    );

    fs::remove_file(index).unwrap();
    fs::remove_file(metadata).unwrap();
    fs::remove_file(dictionary).unwrap();
    fs::remove_file(columns).unwrap();
}

#[test]
fn searches_with_scope_prefixes_from_binary() {
    let root = unique_temp_dir("gfm-cli-scope-root");
    fs::create_dir_all(root.join("Desktop")).unwrap();
    fs::create_dir_all(root.join("Downloads")).unwrap();
    fs::write(root.join("Desktop").join("report.md"), "desktop").unwrap();
    fs::write(root.join("Downloads").join("report.md"), "downloads").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["search", root.to_str().unwrap(), "report @desktop"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Desktop/report.md"), "{stdout}");
    assert!(!stdout.contains("Downloads/report.md"), "{stdout}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn performs_journaled_copy_move_and_delete_from_binary() {
    let root = unique_temp_dir("gfm-cli-ops-root");
    let journal = root.join("ops.journal");
    let source = root.join("source.txt");
    let copy = root.join("copy.txt");
    let moved = root.join("moved.txt");
    fs::write(&source, "hello ops").unwrap();

    run_gfm(
        &journal,
        ["copy", source.to_str().unwrap(), copy.to_str().unwrap()],
    );
    assert_eq!(fs::read_to_string(&copy).unwrap(), "hello ops");

    run_gfm(
        &journal,
        ["move", copy.to_str().unwrap(), moved.to_str().unwrap()],
    );
    assert!(!copy.exists());
    assert_eq!(fs::read_to_string(&moved).unwrap(), "hello ops");

    run_gfm(&journal, ["delete", moved.to_str().unwrap()]);
    assert!(!moved.exists());

    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(journal_text.contains("copy"));
    assert!(journal_text.contains("move"));
    assert!(journal_text.contains("delete"));
    assert!(journal_text.contains("completed"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn searches_text_content_from_binary() {
    let root = unique_temp_dir("gfm-cli-content-root");
    fs::write(root.join("journal.md"), "the body contains superneedle").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["search-content", root.to_str().unwrap(), "superneedle"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("journal.md"), "{stdout}");
    assert!(stdout.contains("[[superneedle]]"), "{stdout}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn searches_pdf_content_from_binary() {
    let root = unique_temp_dir("gfm-cli-pdf-content-root");
    fs::write(root.join("brief.pdf"), minimal_pdf("pdfneedle lives here")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["search-content", root.to_str().unwrap(), "pdfneedle"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("brief.pdf"), "{stdout}");
    assert!(stdout.contains("[[pdfneedle]]"), "{stdout}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn searches_docx_content_from_binary() {
    let root = unique_temp_dir("gfm-cli-docx-content-root");
    fs::write(
        root.join("brief.docx"),
        ooxml_package(&[(
            "word/document.xml",
            "<w:document><w:body><w:p><w:r><w:t>docxneedle lives here</w:t></w:r></w:p></w:body></w:document>",
        )]),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["search-content", root.to_str().unwrap(), "docxneedle"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("brief.docx"), "{stdout}");
    assert!(stdout.contains("[[docxneedle]]"), "{stdout}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn searches_zip_archive_metadata_from_binary() {
    let root = unique_temp_dir("gfm-cli-zip-content-root");
    fs::write(
        root.join("bundle.zip"),
        ooxml_package(&[("docs/zipneedle.txt", "payload")]),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["search-content", root.to_str().unwrap(), "zipneedle"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("bundle.zip"), "{stdout}");
    assert!(stdout.contains("[[zipneedle]]"), "{stdout}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn searches_json_content_from_binary() {
    let root = unique_temp_dir("gfm-cli-json-content-root");
    fs::write(
        root.join("data.json"),
        br#"{"client":"Aperture","marker":"jsonneedle"}"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["search-content", root.to_str().unwrap(), "jsonneedle"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("data.json"), "{stdout}");
    assert!(stdout.contains("[[jsonneedle]]"), "{stdout}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn search_content_skips_disguised_binary_from_binary() {
    let root = unique_temp_dir("gfm-cli-disguised-binary-root");
    fs::write(
        root.join("fake.txt"),
        b"\x89PNG\r\n\x1a\nsuperneedle hidden in binary payload",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["search-content", root.to_str().unwrap(), "superneedle"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.is_empty(), "{stdout}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reports_compressed_pdf_extraction_from_binary() {
    let root = unique_temp_dir("gfm-cli-extract-report-root");
    let path = root.join("Compressed.pdf");
    fs::write(&path, compressed_pdf("compressed report needle")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["extract-report", path.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("extract\tpath="), "{stdout}");
    assert!(
        stdout.contains("\tformat=pdf\tstatus=extracted\t"),
        "{stdout}"
    );
    assert!(stdout.contains("\tversion=2\t"), "{stdout}");
    assert!(stdout.contains("quarantine\tallow"), "{stdout}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reports_extraction_cache_hits_from_binary() {
    let root = unique_temp_dir("gfm-cli-extract-cache-root");
    let path = root.join("Cache.md");
    fs::write(&path, "cached cli needle").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["extract-cache", path.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 2, "{stdout}");
    assert!(
        lines[0].starts_with("extract-cache\tstatus=miss\t"),
        "{stdout}"
    );
    assert!(
        lines[1].starts_with("extract-cache\tstatus=hit\t"),
        "{stdout}"
    );
    assert!(stdout.contains("\tversion=2\t"), "{stdout}");
    assert!(stdout.contains("\tmetadata-epoch="), "{stdout}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reports_extraction_quarantine_from_binary() {
    let root = unique_temp_dir("gfm-cli-extract-quarantine-root");
    let path = root.join("Slow.pdf");
    let store = root.join("quarantine.gfmquarantine");
    fs::write(&path, minimal_pdf("slow worker")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "extract-quarantine",
            path.to_str().unwrap(),
            store.to_str().unwrap(),
            "timeout",
            "2",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 2, "{stdout}");
    assert!(lines[0].starts_with("quarantine\tblocked\t"), "{stdout}");
    assert!(lines[1].starts_with("quarantine\tblocked\t"), "{stdout}");
    assert!(stdout.contains("\treason=worker-timeout\t"), "{stdout}");
    assert!(store.is_file());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn searches_persisted_text_content_from_binary() {
    let root = unique_temp_dir("gfm-cli-durable-content-root");
    let records = unique_temp_path("gfm-cli-durable-records", "gfmidx");
    let content = unique_temp_path("gfm-cli-durable-content", "gfmcontent");
    fs::write(root.join("archive.md"), "the body contains durablemarker").unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-content",
            root.to_str().unwrap(),
            records.to_str().unwrap(),
            content.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-index",
            records.to_str().unwrap(),
            content.to_str().unwrap(),
            "durablemarker",
        ])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );

    let stdout = String::from_utf8(search_output.stdout).unwrap();
    assert!(stdout.contains("archive.md"), "{stdout}");

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(content).unwrap();
}

#[test]
fn searches_persisted_content_phrases_from_binary() {
    let root = unique_temp_dir("gfm-cli-durable-phrase-root");
    let records = unique_temp_path("gfm-cli-durable-phrase-records", "gfmidx");
    let content = unique_temp_path("gfm-cli-durable-phrase-content", "gfmcontent");
    fs::write(
        root.join("keep.md"),
        "this body has a durable phrase marker",
    )
    .unwrap();
    fs::write(
        root.join("skip.md"),
        "this durable body phrase marker is not adjacent",
    )
    .unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-content",
            root.to_str().unwrap(),
            records.to_str().unwrap(),
            content.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-index",
            records.to_str().unwrap(),
            content.to_str().unwrap(),
            r#""durable phrase marker""#,
        ])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );

    let stdout = String::from_utf8(search_output.stdout).unwrap();
    assert!(stdout.contains("keep.md"), "{stdout}");
    assert!(!stdout.contains("skip.md"), "{stdout}");
    assert!(stdout.contains("[[durable phrase marker]]"), "{stdout}");

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(content).unwrap();
}

#[test]
fn searches_persisted_content_proximity_from_binary() {
    let root = unique_temp_dir("gfm-cli-durable-proximity-root");
    let records = unique_temp_path("gfm-cli-durable-proximity-records", "gfmidx");
    let content = unique_temp_path("gfm-cli-durable-proximity-content", "gfmcontent");
    fs::write(root.join("keep.md"), "alpha one two beta").unwrap();
    fs::write(root.join("skip.md"), "alpha one two three four beta").unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-content",
            root.to_str().unwrap(),
            records.to_str().unwrap(),
            content.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-index",
            records.to_str().unwrap(),
            content.to_str().unwrap(),
            "near:3:alpha,beta",
        ])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );

    let stdout = String::from_utf8(search_output.stdout).unwrap();
    assert!(stdout.contains("keep.md"), "{stdout}");
    assert!(!stdout.contains("skip.md"), "{stdout}");

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(content).unwrap();
}

#[test]
fn resolves_content_ids_from_archive_directory() {
    let root = unique_temp_dir("gfm-cli-content-ids-root");
    let records = unique_temp_path("gfm-cli-content-ids-records", "gfmidx");
    let content = unique_temp_path("gfm-cli-content-ids", "gfmcontent");
    fs::write(root.join("archive.md"), "the body contains directmarker").unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-content",
            root.to_str().unwrap(),
            records.to_str().unwrap(),
            content.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );

    let ids_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["content-ids", content.to_str().unwrap(), "directmarker"])
        .output()
        .unwrap();
    assert!(
        ids_output.status.success(),
        "{}",
        String::from_utf8_lossy(&ids_output.stderr)
    );

    let stdout = String::from_utf8(ids_output.stdout).unwrap();
    assert_eq!(stdout.lines().count(), 1, "{stdout}");
    assert!(stdout.lines().all(|line| line.split('\t').count() == 2));

    let mmap_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-ids-mmap",
            content.to_str().unwrap(),
            "directmarker",
        ])
        .output()
        .unwrap();
    assert!(
        mmap_output.status.success(),
        "{}",
        String::from_utf8_lossy(&mmap_output.stderr)
    );

    let mmap_stdout = String::from_utf8(mmap_output.stdout).unwrap();
    assert_eq!(mmap_stdout, stdout);

    let block_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-id-block-mmap",
            content.to_str().unwrap(),
            "directmarker",
            "0",
        ])
        .output()
        .unwrap();
    assert!(
        block_output.status.success(),
        "{}",
        String::from_utf8_lossy(&block_output.stderr)
    );

    let block_stdout = String::from_utf8(block_output.stdout).unwrap();
    assert_eq!(block_stdout, stdout);

    let verify_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["content-verify", content.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        verify_output.status.success(),
        "{}",
        String::from_utf8_lossy(&verify_output.stderr)
    );
    let verify_stdout = String::from_utf8(verify_output.stdout).unwrap();
    assert!(
        verify_stdout.contains("\tchecksum=verified"),
        "{verify_stdout}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(content).unwrap();
}

#[test]
fn compacts_content_segments_from_binary() {
    let root = unique_temp_dir("gfm-cli-segment-content-root");
    let records = unique_temp_path("gfm-cli-segment-records", "gfmidx");
    let segment = unique_temp_path("gfm-cli-segment-content", "gfmseg");
    let content = unique_temp_path("gfm-cli-segment-compact", "gfmcontent");
    fs::write(root.join("segment.md"), "the body contains segmentmarker").unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), records.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );

    let segment_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-content-segment",
            root.to_str().unwrap(),
            segment.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        segment_output.status.success(),
        "{}",
        String::from_utf8_lossy(&segment_output.stderr)
    );

    let compact_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "compact-content",
            content.to_str().unwrap(),
            segment.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        compact_output.status.success(),
        "{}",
        String::from_utf8_lossy(&compact_output.stderr)
    );

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-index",
            records.to_str().unwrap(),
            content.to_str().unwrap(),
            "segmentmarker",
        ])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );

    let stdout = String::from_utf8(search_output.stdout).unwrap();
    assert!(stdout.contains("segment.md"), "{stdout}");

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(segment).unwrap();
    fs::remove_file(content).unwrap();
}

#[test]
fn runs_background_content_indexer_from_binary() {
    let root = unique_temp_dir("gfm-cli-background-content-root");
    let segments = unique_temp_dir("gfm-cli-background-content-segments");
    let records = unique_temp_path("gfm-cli-background-records", "gfmidx");
    let content = unique_temp_path("gfm-cli-background-content", "gfmcontent");
    let journal = unique_temp_path("gfm-cli-background-jobs", "journal");
    let spec = unique_temp_path("gfm-cli-background-content", "job");
    fs::write(root.join("worker.md"), "the body contains workermarker").unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_JOB_JOURNAL", &journal)
        .env("GFM_CONTENT_JOB", &spec)
        .args([
            "index-content-background",
            root.to_str().unwrap(),
            segments.to_str().unwrap(),
            records.to_str().unwrap(),
            content.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-index",
            records.to_str().unwrap(),
            content.to_str().unwrap(),
            "workermarker",
        ])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );

    let stdout = String::from_utf8(search_output.stdout).unwrap();
    assert!(stdout.contains("worker.md"), "{stdout}");
    assert!(segments.join("content-00000000.gfmseg").exists());
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(journal_text.contains("started"));
    assert!(journal_text.contains("completed"));
    assert!(fs::read_to_string(&spec)
        .unwrap()
        .contains("gfm-content-job-v1"));

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(segments).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(content).unwrap();
    fs::remove_file(journal).unwrap();
    fs::remove_file(spec).unwrap();
}

#[test]
fn resumes_content_index_job_from_binary() {
    let root = unique_temp_dir("gfm-cli-resume-content-root");
    let segments = unique_temp_dir("gfm-cli-resume-content-segments");
    let records = unique_temp_path("gfm-cli-resume-records", "gfmidx");
    let content = unique_temp_path("gfm-cli-resume-content", "gfmcontent");
    let journal = unique_temp_path("gfm-cli-resume-jobs", "journal");
    let spec = unique_temp_path("gfm-cli-resume-content", "job");
    fs::write(root.join("resume.md"), "the body contains resumemarker").unwrap();
    fs::write(
        &spec,
        format!(
            "gfm-content-job-v1\nroot\t{}\nsegment_dir\t{}\nrecords_path\t{}\ncontent_path\t{}\nbatch_size\t1024\n",
            root.display(),
            segments.display(),
            records.display(),
            content.display()
        ),
    )
    .unwrap();
    fs::write(&journal, "99\t1\tstarted\tbackground content index\n").unwrap();

    let resume_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "resume-content-background",
            spec.to_str().unwrap(),
            journal.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        resume_output.status.success(),
        "{}",
        String::from_utf8_lossy(&resume_output.stderr)
    );

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-index",
            records.to_str().unwrap(),
            content.to_str().unwrap(),
            "resumemarker",
        ])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );

    let stdout = String::from_utf8(search_output.stdout).unwrap();
    assert!(stdout.contains("resume.md"), "{stdout}");
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(journal_text.contains("99\t1\tstarted"));
    assert!(journal_text.contains("completed"));

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(segments).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(content).unwrap();
    fs::remove_file(journal).unwrap();
    fs::remove_file(spec).unwrap();
}

#[test]
fn reports_recoverable_jobs_from_binary() {
    let journal = unique_temp_path("gfm-cli-recovery-jobs", "journal");
    fs::write(
        &journal,
        "10\t1\tstarted\tinterrupted job\n11\t1\tstarted\tretry job\n11\t1\tfailed:temporary\tretry job\n12\t1\tstarted\tdone job\n12\t1\tcompleted\tdone job\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["jobs-recover", journal.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("10\t1\tinterrupted\tinterrupted job"),
        "{stdout}"
    );
    assert!(
        stdout.contains("11\t1\tretryable-failure\tretry job"),
        "{stdout}"
    );
    assert!(!stdout.contains("done job"), "{stdout}");

    fs::remove_file(journal).unwrap();
}

fn run_gfm<const N: usize>(journal: &std::path::Path, args: [&str; N]) {
    let output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .env("GFM_OPS_JOURNAL", journal)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
    let path = unique_temp_path(prefix, "");
    fs::create_dir_all(&path).unwrap();
    path
}

fn unique_temp_path(prefix: &str, extension: &str) -> std::path::PathBuf {
    let mut name = format!(
        "{}-{}",
        prefix,
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    if !extension.is_empty() {
        name.push('.');
        name.push_str(extension);
    }
    std::env::temp_dir().join(name)
}

#[cfg(unix)]
fn make_symlink(target: &std::path::Path, link: &std::path::Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[cfg(not(unix))]
fn make_symlink(_target: &std::path::Path, link: &std::path::Path) {
    fs::write(link, "link").unwrap();
}

fn minimal_pdf(text: &str) -> Vec<u8> {
    format!(
        "%PDF-1.4
1 0 obj
<< /Type /Page /Contents 2 0 R >>
endobj
2 0 obj
<< /Length {} >>
stream
BT /F1 12 Tf 72 720 Td ({}) Tj ET
endstream
endobj
%%EOF",
        text.len() + 31,
        text
    )
    .into_bytes()
}

fn compressed_pdf(text: &str) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    let stream = format!("BT /F1 12 Tf 72 720 Td ({text}) Tj ET");
    encoder.write_all(stream.as_bytes()).unwrap();
    let compressed = encoder.finish().unwrap();
    let mut pdf = b"%PDF-1.4
1 0 obj
<< /Type /Page /Contents 2 0 R >>
endobj
2 0 obj
<< /Length "
        .to_vec();
    pdf.extend(compressed.len().to_string().as_bytes());
    pdf.extend(
        b" /Filter /FlateDecode >>
stream
",
    );
    pdf.extend(compressed);
    pdf.extend(
        b"
endstream
endobj
%%EOF",
    );
    pdf
}

fn ooxml_package(parts: &[(&str, &str)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, text) in parts {
        writer.start_file(*name, options).unwrap();
        writer.write_all(text.as_bytes()).unwrap();
    }
    writer.finish().unwrap().into_inner()
}
