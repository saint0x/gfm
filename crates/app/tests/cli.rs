use gfm_store::{read_records, write_content_postings, MetadataField, MetadataPosting};
use gfm_types::{ContentPosting, FileId, VolumeId};
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
fn inspects_archive_schema_from_binary() {
    let root = unique_temp_dir("gfm-cli-archive-schema-root");
    let index = unique_temp_path("gfm-cli-archive-schema", "gfmidx");
    let prefixes = unique_temp_path("gfm-cli-archive-schema", "gfmprefix");
    let unsupported = unique_temp_path("gfm-cli-archive-schema", "gfmidx");
    fs::write(root.join("InstantSearch.md"), "alpha").unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), index.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );

    let records_schema = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["archive-schema", "records", index.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        records_schema.status.success(),
        "{}",
        String::from_utf8_lossy(&records_schema.stderr)
    );
    let records_stdout = String::from_utf8(records_schema.stdout).unwrap();
    assert!(
        records_stdout
            .contains("archive-schema\tkind=records\tstatus=current\tschema=gfm-store-v3"),
        "{records_stdout}"
    );

    let prefixes_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-prefixes",
            index.to_str().unwrap(),
            prefixes.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        prefixes_output.status.success(),
        "{}",
        String::from_utf8_lossy(&prefixes_output.stderr)
    );

    let prefixes_schema = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["archive-schema", "prefixes", prefixes.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        prefixes_schema.status.success(),
        "{}",
        String::from_utf8_lossy(&prefixes_schema.stderr)
    );
    let prefixes_stdout = String::from_utf8(prefixes_schema.stdout).unwrap();
    assert!(
        prefixes_stdout
            .contains("archive-schema\tkind=prefixes\tstatus=current\tschema=gfm-prefix-v1"),
        "{prefixes_stdout}"
    );

    fs::write(&unsupported, "not-gfm\n").unwrap();
    let unsupported_schema = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["archive-schema", "records", unsupported.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(unsupported_schema.status.success());
    let unsupported_stdout = String::from_utf8(unsupported_schema.stdout).unwrap();
    assert!(
        unsupported_stdout.contains("\tstatus=unsupported\t"),
        "{unsupported_stdout}"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(index).unwrap();
    fs::remove_file(prefixes).unwrap();
    fs::remove_file(unsupported).unwrap();
}

#[test]
fn migrates_legacy_record_archive_from_binary() {
    let records = unique_temp_path("gfm-cli-records-migrate", "gfmidx");
    let backup = unique_temp_dir("gfm-cli-records-migrate-backup");
    fs::write(
        &records,
        "gfm-store-v1\n1\t2\t0\tf\t1\t0\t0\t0\t0\t/tmp/legacy.txt\n",
    )
    .unwrap();

    let plan = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["records-migration-plan", records.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        plan.status.success(),
        "{}",
        String::from_utf8_lossy(&plan.stderr)
    );
    let plan_stdout = String::from_utf8(plan.stdout).unwrap();
    assert!(
        plan_stdout.contains("record-archive-migration-plan\taction=migrate"),
        "{plan_stdout}"
    );

    let migration = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "records-migrate",
            records.to_str().unwrap(),
            backup.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        migration.status.success(),
        "{}",
        String::from_utf8_lossy(&migration.stderr)
    );
    let migration_stdout = String::from_utf8(migration.stdout).unwrap();
    assert!(
        migration_stdout.contains(
            "record-archive-migration\tmigrated-records=1\tbefore-status=legacy\tafter-status=current"
        ),
        "{migration_stdout}"
    );

    let schema = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["archive-schema", "records", records.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(schema.status.success());
    let schema_stdout = String::from_utf8(schema.stdout).unwrap();
    assert!(
        schema_stdout.contains("\tstatus=current\tschema=gfm-store-v3"),
        "{schema_stdout}"
    );
    assert!(backup.read_dir().unwrap().next().is_some());

    fs::remove_file(records).unwrap();
    fs::remove_dir_all(backup).unwrap();
}

#[test]
fn migrates_legacy_content_archive_from_binary() {
    let content = unique_temp_path("gfm-cli-content-migrate", "gfmcontent");
    let backup = unique_temp_dir("gfm-cli-content-migrate-backup");
    write_legacy_content_archive(
        &content,
        &[ContentPosting {
            term: "legacyneedle".to_string(),
            ids: vec![FileId::new(VolumeId(7), 11)],
            positions: Vec::new(),
        }],
    );

    let plan = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["content-migration-plan", content.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        plan.status.success(),
        "{}",
        String::from_utf8_lossy(&plan.stderr)
    );
    let plan_stdout = String::from_utf8(plan.stdout).unwrap();
    assert!(
        plan_stdout.contains("content-archive-migration-plan\taction=migrate"),
        "{plan_stdout}"
    );

    let migration = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-migrate",
            content.to_str().unwrap(),
            backup.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        migration.status.success(),
        "{}",
        String::from_utf8_lossy(&migration.stderr)
    );
    let migration_stdout = String::from_utf8(migration.stdout).unwrap();
    assert!(
        migration_stdout.contains(
            "content-archive-migration\tmigrated-postings=1\tbefore-status=legacy\tafter-status=current"
        ),
        "{migration_stdout}"
    );

    let verify = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["content-verify", content.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        verify.status.success(),
        "{}",
        String::from_utf8_lossy(&verify.stderr)
    );
    let verify_stdout = String::from_utf8(verify.stdout).unwrap();
    assert!(
        verify_stdout.contains("\tchecksum=verified"),
        "{verify_stdout}"
    );
    assert!(backup.read_dir().unwrap().next().is_some());

    fs::remove_file(content).unwrap();
    fs::remove_dir_all(backup).unwrap();
}

#[test]
fn migrates_legacy_metadata_archive_from_binary() {
    let metadata = unique_temp_path("gfm-cli-metadata-migrate", "gfmmeta");
    let backup = unique_temp_dir("gfm-cli-metadata-migrate-backup");
    write_legacy_metadata_archive(
        &metadata,
        &[MetadataPosting {
            field: MetadataField::Tag,
            term: "important".to_string(),
            ids: vec![FileId::new(VolumeId(7), 11)],
        }],
    );

    let plan = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["metadata-migration-plan", metadata.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        plan.status.success(),
        "{}",
        String::from_utf8_lossy(&plan.stderr)
    );
    let plan_stdout = String::from_utf8(plan.stdout).unwrap();
    assert!(
        plan_stdout.contains("metadata-archive-migration-plan\taction=migrate"),
        "{plan_stdout}"
    );

    let migration = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "metadata-migrate",
            metadata.to_str().unwrap(),
            backup.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        migration.status.success(),
        "{}",
        String::from_utf8_lossy(&migration.stderr)
    );
    let migration_stdout = String::from_utf8(migration.stdout).unwrap();
    assert!(
        migration_stdout.contains(
            "metadata-archive-migration\tmigrated-postings=1\tbefore-status=legacy\tafter-status=current"
        ),
        "{migration_stdout}"
    );

    let verify = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["metadata-verify", metadata.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        verify.status.success(),
        "{}",
        String::from_utf8_lossy(&verify.stderr)
    );
    let verify_stdout = String::from_utf8(verify.stdout).unwrap();
    assert!(
        verify_stdout.contains("\tchecksum=verified"),
        "{verify_stdout}"
    );
    assert!(backup.read_dir().unwrap().next().is_some());

    fs::remove_file(metadata).unwrap();
    fs::remove_dir_all(backup).unwrap();
}

#[test]
fn rebuilds_columns_archive_from_binary() {
    let root = unique_temp_dir("gfm-cli-columns-rebuild-root");
    let records = unique_temp_path("gfm-cli-columns-rebuild-records", "gfmidx");
    let columns = unique_temp_path("gfm-cli-columns-rebuild-columns", "gfmcols");
    let backup = unique_temp_dir("gfm-cli-columns-rebuild-backup");
    fs::write(root.join("ColumnNeedle.md"), "alpha").unwrap();

    let index = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), records.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        index.status.success(),
        "{}",
        String::from_utf8_lossy(&index.stderr)
    );

    let plan = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "columns-rebuild-plan",
            records.to_str().unwrap(),
            columns.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        plan.status.success(),
        "{}",
        String::from_utf8_lossy(&plan.stderr)
    );
    let plan_stdout = String::from_utf8(plan.stdout).unwrap();
    assert!(
        plan_stdout.contains(
            "columns-archive-rebuild-plan\taction=rebuild\trecords-status=current\tcolumns-status=missing",
        ),
        "{plan_stdout}"
    );

    let rebuild = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "columns-rebuild",
            records.to_str().unwrap(),
            columns.to_str().unwrap(),
            backup.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        rebuild.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuild.stderr)
    );
    let rebuild_stdout = String::from_utf8(rebuild.stdout).unwrap();
    assert!(
        rebuild_stdout.contains(
            "columns-archive-rebuild\trebuilt-records=2\trecords-status=current\tbefore-status=missing\tafter-status=current",
        ),
        "{rebuild_stdout}"
    );

    let verify = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["columns-verify", columns.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        verify.status.success(),
        "{}",
        String::from_utf8_lossy(&verify.stderr)
    );
    let verify_stdout = String::from_utf8(verify.stdout).unwrap();
    assert!(
        verify_stdout.contains("\tchecksum=verified"),
        "{verify_stdout}"
    );
    assert!(backup.read_dir().unwrap().next().is_none());

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(columns).unwrap();
    fs::remove_dir_all(backup).unwrap();
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
    let prefixes = unique_temp_path("gfm-cli-tags", "gfmprefix");
    let fuzzy = unique_temp_path("gfm-cli-tags", "gfmfuzzy");
    let content = unique_temp_path("gfm-cli-tags", "gfmcontent");
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
    let dictionary_stderr = String::from_utf8(dictionary_output.stderr).unwrap();
    assert!(
        dictionary_stderr.contains("dictionary-indexed\tterms=")
            && dictionary_stderr.contains("\tpaths=")
            && dictionary_stderr.contains("\tpath-prefixes=1")
            && dictionary_stderr.contains("\textensions=1")
            && dictionary_stderr.contains("\ttags=3")
            && dictionary_stderr.contains("\tkinds=1")
            && dictionary_stderr.contains("\tmetadata-keys=6"),
        "{dictionary_stderr}"
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

    let prefix_lookup_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["dictionary-lookup", dictionary.to_str().unwrap(), "/tmp"])
        .output()
        .unwrap();
    assert!(
        prefix_lookup_output.status.success(),
        "{}",
        String::from_utf8_lossy(&prefix_lookup_output.stderr)
    );
    let prefix_lookup_stdout = String::from_utf8(prefix_lookup_output.stdout).unwrap();
    assert!(
        prefix_lookup_stdout.starts_with("dictionary\tfound\t"),
        "{prefix_lookup_stdout}"
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

    let prefixes_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-prefixes",
            index.to_str().unwrap(),
            prefixes.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        prefixes_output.status.success(),
        "{}",
        String::from_utf8_lossy(&prefixes_output.stderr)
    );

    let prefix_ids = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["prefix-ids-mmap", prefixes.to_str().unwrap(), "tag"])
        .output()
        .unwrap();
    assert!(
        prefix_ids.status.success(),
        "{}",
        String::from_utf8_lossy(&prefix_ids.stderr)
    );
    assert_eq!(String::from_utf8(prefix_ids.stdout).unwrap(), "1\t1\n");

    let prefix_block = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "prefix-id-block-mmap",
            prefixes.to_str().unwrap(),
            "tag",
            "0",
        ])
        .output()
        .unwrap();
    assert!(
        prefix_block.status.success(),
        "{}",
        String::from_utf8_lossy(&prefix_block.stderr)
    );
    assert_eq!(String::from_utf8(prefix_block.stdout).unwrap(), "1\t1\n");

    let prefix_verify = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["prefix-verify", prefixes.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        prefix_verify.status.success(),
        "{}",
        String::from_utf8_lossy(&prefix_verify.stderr)
    );
    let prefix_verify_stdout = String::from_utf8(prefix_verify.stdout).unwrap();
    assert!(
        prefix_verify_stdout.contains("\tchecksum=true"),
        "{prefix_verify_stdout}"
    );

    let fuzzy_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-fuzzy",
            index.to_str().unwrap(),
            fuzzy.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        fuzzy_output.status.success(),
        "{}",
        String::from_utf8_lossy(&fuzzy_output.stderr)
    );

    let fuzzy_terms = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["fuzzy-terms-mmap", fuzzy.to_str().unwrap(), "tagge"])
        .output()
        .unwrap();
    assert!(
        fuzzy_terms.status.success(),
        "{}",
        String::from_utf8_lossy(&fuzzy_terms.stderr)
    );
    assert_eq!(String::from_utf8(fuzzy_terms.stdout).unwrap(), "tagged\n");

    let fuzzy_verify = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["fuzzy-verify", fuzzy.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        fuzzy_verify.status.success(),
        "{}",
        String::from_utf8_lossy(&fuzzy_verify.stderr)
    );
    let fuzzy_verify_stdout = String::from_utf8(fuzzy_verify.stdout).unwrap();
    assert!(
        fuzzy_verify_stdout.contains("\tchecksum=true"),
        "{fuzzy_verify_stdout}"
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
    write_content_postings(
        &content,
        &[
            ContentPosting {
                term: "bodymarker".to_string(),
                ids: vec![FileId::new(VolumeId(1), 1)],
                positions: Vec::new(),
            },
            ContentPosting {
                term: "coldmarker".to_string(),
                ids: vec![FileId::new(VolumeId(1), 2)],
                positions: Vec::new(),
            },
        ],
    )
    .unwrap();

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

    let sidecar_search = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-index-sidecars",
            index.to_str().unwrap(),
            columns.to_str().unwrap(),
            metadata.to_str().unwrap(),
            prefixes.to_str().unwrap(),
            fuzzy.to_str().unwrap(),
            content.to_str().unwrap(),
            "tag:Important",
        ])
        .output()
        .unwrap();
    assert!(
        sidecar_search.status.success(),
        "{}",
        String::from_utf8_lossy(&sidecar_search.stderr)
    );
    let sidecar_search_stdout = String::from_utf8(sidecar_search.stdout).unwrap();
    assert!(
        sidecar_search_stdout.contains("tagged.md"),
        "{sidecar_search_stdout}"
    );
    let sidecar_search_stderr = String::from_utf8(sidecar_search.stderr).unwrap();
    assert!(
        sidecar_search_stderr.contains("metadata-keys 1")
            && sidecar_search_stderr.contains("prefix-keys 0")
            && sidecar_search_stderr.contains("fuzzy-keys 0")
            && sidecar_search_stderr.contains("prefix-archive-keys")
            && sidecar_search_stderr.contains("fuzzy-archive-keys")
            && sidecar_search_stderr.contains("content-keys 0"),
        "{sidecar_search_stderr}"
    );

    let sidecar_prefix_search = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-index-sidecars",
            index.to_str().unwrap(),
            columns.to_str().unwrap(),
            metadata.to_str().unwrap(),
            prefixes.to_str().unwrap(),
            fuzzy.to_str().unwrap(),
            content.to_str().unwrap(),
            "tag",
        ])
        .output()
        .unwrap();
    assert!(
        sidecar_prefix_search.status.success(),
        "{}",
        String::from_utf8_lossy(&sidecar_prefix_search.stderr)
    );
    let sidecar_prefix_stdout = String::from_utf8(sidecar_prefix_search.stdout).unwrap();
    assert!(
        sidecar_prefix_stdout.contains("tagged.md"),
        "{sidecar_prefix_stdout}"
    );
    let sidecar_prefix_stderr = String::from_utf8(sidecar_prefix_search.stderr).unwrap();
    assert!(
        sidecar_prefix_stderr.contains("prefix-keys 0")
            && sidecar_prefix_stderr.contains("prefix-archive-keys"),
        "{sidecar_prefix_stderr}"
    );
    let sidecar_budget_search = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-index-sidecars-budget",
            index.to_str().unwrap(),
            columns.to_str().unwrap(),
            metadata.to_str().unwrap(),
            prefixes.to_str().unwrap(),
            fuzzy.to_str().unwrap(),
            content.to_str().unwrap(),
            "1",
            "1",
            "1",
            "1",
            "tag",
        ])
        .output()
        .unwrap();
    assert!(
        sidecar_budget_search.status.success(),
        "{}",
        String::from_utf8_lossy(&sidecar_budget_search.stderr)
    );
    let sidecar_budget_stderr = String::from_utf8(sidecar_budget_search.stderr).unwrap();
    assert!(
        sidecar_budget_stderr.contains("sidecar-budget")
            && sidecar_budget_stderr.contains("\tprefix-terms=2")
            && sidecar_budget_stderr.contains("\tprefix-lookup-requests=")
            && sidecar_budget_stderr.contains("\tprefix-lookup-ids=")
            && sidecar_budget_stderr.contains("\tprefix-candidate-ids=")
            && sidecar_budget_stderr.contains("\tprefix-cache-misses=")
            && sidecar_budget_stderr.contains("\tprefix-cutoff-terms=")
            && sidecar_budget_stderr.contains("\tfuzzy-terms=2")
            && sidecar_budget_stderr.contains("\tfuzzy-lookup-requests=")
            && sidecar_budget_stderr.contains("\tfuzzy-cache-misses=")
            && sidecar_budget_stderr.contains("\tfuzzy-term-truncated-keys=2"),
        "{sidecar_budget_stderr}"
    );
    let sidecar_content_search = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-index-sidecars",
            index.to_str().unwrap(),
            columns.to_str().unwrap(),
            metadata.to_str().unwrap(),
            prefixes.to_str().unwrap(),
            fuzzy.to_str().unwrap(),
            content.to_str().unwrap(),
            "bodymarker",
        ])
        .output()
        .unwrap();
    assert!(
        sidecar_content_search.status.success(),
        "{}",
        String::from_utf8_lossy(&sidecar_content_search.stderr)
    );
    let sidecar_content_stdout = String::from_utf8(sidecar_content_search.stdout).unwrap();
    assert!(
        sidecar_content_stdout.contains("tagged.md"),
        "{sidecar_content_stdout}"
    );
    let sidecar_content_stderr = String::from_utf8(sidecar_content_search.stderr).unwrap();
    assert!(
        sidecar_content_stderr.contains("content-keys 1"),
        "{sidecar_content_stderr}"
    );

    fs::remove_file(index).unwrap();
    fs::remove_file(metadata).unwrap();
    fs::remove_file(dictionary).unwrap();
    fs::remove_file(columns).unwrap();
    fs::remove_file(prefixes).unwrap();
    fs::remove_file(fuzzy).unwrap();
    fs::remove_file(content).unwrap();
}

#[test]
fn recovers_missing_and_corrupt_sidecars_from_binary() {
    let root = unique_temp_dir("gfm-cli-sidecar-recovery-root");
    let records = unique_temp_path("gfm-cli-sidecar-recovery", "gfmidx");
    let prefixes = unique_temp_path("gfm-cli-sidecar-recovery", "gfmprefix");
    let dictionary = unique_temp_path("gfm-cli-sidecar-recovery", "gfmdict");
    let quarantine = unique_temp_dir("gfm-cli-sidecar-recovery-quarantine");
    fs::write(root.join("ProjectPlan.md"), "sidecar").unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), records.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );
    fs::write(&dictionary, "not-a-dictionary").unwrap();

    let plan_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "sidecar-recovery-plan",
            records.to_str().unwrap(),
            "-",
            "-",
            prefixes.to_str().unwrap(),
            "-",
            dictionary.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        plan_output.status.success(),
        "{}",
        String::from_utf8_lossy(&plan_output.stderr)
    );
    let plan_stdout = String::from_utf8(plan_output.stdout).unwrap();
    assert!(
        plan_stdout.contains("action=rebuild")
            && plan_stdout.contains("reason=missing-sidecar")
            && plan_stdout.contains("invalid=2"),
        "{plan_stdout}"
    );

    let recover_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "sidecar-recover",
            records.to_str().unwrap(),
            quarantine.to_str().unwrap(),
            "-",
            "-",
            prefixes.to_str().unwrap(),
            "-",
            dictionary.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        recover_output.status.success(),
        "{}",
        String::from_utf8_lossy(&recover_output.stderr)
    );
    let recover_stdout = String::from_utf8(recover_output.stdout).unwrap();
    assert!(
        recover_stdout.contains("sidecar-recovery\trebuilt=2\tquarantined=1")
            && recover_stdout.contains("action=ready"),
        "{recover_stdout}"
    );

    let prefix_verify = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["prefix-verify", prefixes.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(prefix_verify.status.success());
    let dictionary_verify = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["dictionary-verify", dictionary.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(dictionary_verify.status.success());

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(prefixes).unwrap();
    fs::remove_file(dictionary).unwrap();
    fs::remove_dir_all(quarantine).unwrap();
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
fn searches_persisted_content_across_mmap_archive_set_from_binary() {
    let root = unique_temp_dir("gfm-cli-content-set-root");
    let records = unique_temp_path("gfm-cli-content-set-records", "gfmidx");
    let first_content = unique_temp_path("gfm-cli-content-set-first", "gfmcontent");
    let second_content = unique_temp_path("gfm-cli-content-set-second", "gfmcontent");
    let third_content = unique_temp_path("gfm-cli-content-set-third", "gfmcontent");
    let manifest = unique_temp_path("gfm-cli-content-set", "gfmmanifest");
    fs::write(root.join("left.md"), "metadata only").unwrap();
    fs::write(root.join("right.md"), "metadata only").unwrap();

    let index_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["index", root.to_str().unwrap(), records.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        index_output.status.success(),
        "{}",
        String::from_utf8_lossy(&index_output.stderr)
    );

    let indexed_records = read_records(&records).unwrap();
    let left = indexed_records
        .iter()
        .find(|record| record.path.ends_with("left.md"))
        .unwrap()
        .id;
    let right = indexed_records
        .iter()
        .find(|record| record.path.ends_with("right.md"))
        .unwrap()
        .id;
    write_content_postings(
        &first_content,
        &[ContentPosting {
            term: "setneedle".to_string(),
            ids: vec![left],
            positions: vec![],
        }],
    )
    .unwrap();
    write_content_postings(
        &second_content,
        &[ContentPosting {
            term: "setneedle".to_string(),
            ids: vec![left, right],
            positions: vec![],
        }],
    )
    .unwrap();
    write_content_postings(
        &third_content,
        &[ContentPosting {
            term: "promotedneedle".to_string(),
            ids: vec![right],
            positions: vec![],
        }],
    )
    .unwrap();

    let ids_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-ids-mmap-set",
            "setneedle",
            first_content.to_str().unwrap(),
            second_content.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        ids_output.status.success(),
        "{}",
        String::from_utf8_lossy(&ids_output.stderr)
    );
    let ids_stdout = String::from_utf8(ids_output.stdout).unwrap();
    assert_eq!(ids_stdout.lines().count(), 2, "{ids_stdout}");

    let manifest_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-manifest-write",
            manifest.to_str().unwrap(),
            &format!("hot:{}", first_content.display()),
            &format!("warm:{}", second_content.display()),
        ])
        .output()
        .unwrap();
    assert!(
        manifest_output.status.success(),
        "{}",
        String::from_utf8_lossy(&manifest_output.stderr)
    );

    let manifest_inspect = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["content-manifest-inspect", manifest.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        manifest_inspect.status.success(),
        "{}",
        String::from_utf8_lossy(&manifest_inspect.stderr)
    );
    let inspect_stdout = String::from_utf8(manifest_inspect.stdout).unwrap();
    assert!(
        inspect_stdout.contains("content-manifest\tarchives=2")
            && inspect_stdout.contains("\tterms=2")
            && inspect_stdout.contains("archive\thot\t")
            && inspect_stdout.contains("archive\twarm\t"),
        "{inspect_stdout}"
    );

    let search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-index-set",
            records.to_str().unwrap(),
            "setneedle",
            first_content.to_str().unwrap(),
            second_content.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&search_output.stderr)
    );
    let stdout = String::from_utf8(search_output.stdout).unwrap();
    assert!(stdout.contains("left.md"), "{stdout}");
    assert!(stdout.contains("right.md"), "{stdout}");
    let stderr = String::from_utf8(search_output.stderr).unwrap();
    assert!(
        stderr.contains("content-archives 2") && stderr.contains("content-keys 1"),
        "{stderr}"
    );

    let manifest_ids_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-ids-mmap-manifest",
            manifest.to_str().unwrap(),
            "setneedle",
        ])
        .output()
        .unwrap();
    assert!(
        manifest_ids_output.status.success(),
        "{}",
        String::from_utf8_lossy(&manifest_ids_output.stderr)
    );
    assert_eq!(
        String::from_utf8(manifest_ids_output.stdout).unwrap(),
        ids_stdout
    );

    let manifest_search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-index-manifest",
            records.to_str().unwrap(),
            manifest.to_str().unwrap(),
            "setneedle",
        ])
        .output()
        .unwrap();
    assert!(
        manifest_search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&manifest_search_output.stderr)
    );
    assert_eq!(
        String::from_utf8(manifest_search_output.stdout).unwrap(),
        stdout
    );
    let manifest_stderr = String::from_utf8(manifest_search_output.stderr).unwrap();
    assert!(
        manifest_stderr.contains("content-manifest-keys 1"),
        "{manifest_stderr}"
    );

    let promote_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-manifest-promote",
            manifest.to_str().unwrap(),
            &format!("warm:{}", third_content.display()),
            first_content.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        promote_output.status.success(),
        "{}",
        String::from_utf8_lossy(&promote_output.stderr)
    );
    let promote_stderr = String::from_utf8(promote_output.stderr).unwrap();
    assert!(
        promote_stderr.contains("content-manifest-promoted")
            && promote_stderr.contains("archives=2")
            && promote_stderr.contains("retired=1"),
        "{promote_stderr}"
    );
    let promote_stdout = String::from_utf8(promote_output.stdout).unwrap();
    assert!(
        promote_stdout.contains(&format!("retire\t{}", first_content.display())),
        "{promote_stdout}"
    );

    let promoted_ids_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-ids-mmap-manifest",
            manifest.to_str().unwrap(),
            "promotedneedle",
        ])
        .output()
        .unwrap();
    assert!(
        promoted_ids_output.status.success(),
        "{}",
        String::from_utf8_lossy(&promoted_ids_output.stderr)
    );
    let promoted_ids_stdout = String::from_utf8(promoted_ids_output.stdout).unwrap();
    assert_eq!(
        promoted_ids_stdout.lines().count(),
        1,
        "{promoted_ids_stdout}"
    );

    let cleanup_plan_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-cleanup-plan",
            manifest.to_str().unwrap(),
            "1",
            "0",
            "1",
            first_content.to_str().unwrap(),
            second_content.to_str().unwrap(),
            &format!("{}.missing", first_content.display()),
        ])
        .output()
        .unwrap();
    assert!(
        cleanup_plan_output.status.success(),
        "{}",
        String::from_utf8_lossy(&cleanup_plan_output.stderr)
    );
    let cleanup_plan_stderr = String::from_utf8(cleanup_plan_output.stderr).unwrap();
    assert!(
        cleanup_plan_stderr.contains("content-cleanup-plan")
            && cleanup_plan_stderr.contains("action=Cleanup")
            && cleanup_plan_stderr.contains("cleanup=1")
            && cleanup_plan_stderr.contains("active=1")
            && cleanup_plan_stderr.contains("missing=1"),
        "{cleanup_plan_stderr}"
    );
    let cleanup_plan_stdout = String::from_utf8(cleanup_plan_output.stdout).unwrap();
    assert!(
        cleanup_plan_stdout.contains(&format!("cleanup\t{}", first_content.display()))
            && cleanup_plan_stdout.contains(&format!("active\t{}", second_content.display()))
            && cleanup_plan_stdout.contains("missing\t"),
        "{cleanup_plan_stdout}"
    );
    assert!(first_content.exists());
    assert!(second_content.exists());

    let cleanup_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-manifest-cleanup",
            manifest.to_str().unwrap(),
            first_content.to_str().unwrap(),
            second_content.to_str().unwrap(),
            &format!("{}.missing", first_content.display()),
        ])
        .output()
        .unwrap();
    assert!(
        cleanup_output.status.success(),
        "{}",
        String::from_utf8_lossy(&cleanup_output.stderr)
    );
    let cleanup_stderr = String::from_utf8(cleanup_output.stderr).unwrap();
    assert!(
        cleanup_stderr.contains("content-manifest-cleanup")
            && cleanup_stderr.contains("removed=1")
            && cleanup_stderr.contains("active=1")
            && cleanup_stderr.contains("missing=1"),
        "{cleanup_stderr}"
    );
    let cleanup_stdout = String::from_utf8(cleanup_output.stdout).unwrap();
    assert!(
        cleanup_stdout.contains(&format!("removed\t{}", first_content.display()))
            && cleanup_stdout.contains(&format!("active\t{}", second_content.display()))
            && cleanup_stdout.contains("missing\t"),
        "{cleanup_stdout}"
    );
    assert!(!first_content.exists());
    assert!(second_content.exists());

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(second_content).unwrap();
    fs::remove_file(third_content).unwrap();
    fs::remove_file(manifest).unwrap();
}

#[test]
fn recovers_corrupt_content_manifest_from_binary() {
    let manifest = unique_temp_path("gfm-cli-content-manifest-recovery", "gfmmanifest");
    let content = unique_temp_path("gfm-cli-content-manifest-recovery", "gfmcontent");
    let quarantine = unique_temp_dir("gfm-cli-content-manifest-recovery-quarantine");
    write_content_postings(
        &content,
        &[ContentPosting {
            term: "recoverneedle".to_string(),
            ids: vec![FileId::new(VolumeId(1), 7)],
            positions: Vec::new(),
        }],
    )
    .unwrap();
    fs::write(&manifest, "not-a-content-manifest").unwrap();

    let plan_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-manifest-recovery-plan",
            manifest.to_str().unwrap(),
            &format!("hot:{}", content.display()),
        ])
        .output()
        .unwrap();
    assert!(
        plan_output.status.success(),
        "{}",
        String::from_utf8_lossy(&plan_output.stderr)
    );
    let plan_stdout = String::from_utf8(plan_output.stdout).unwrap();
    assert!(
        plan_stdout.contains("action=quarantine-manifest-and-write-discovered")
            && plan_stdout.contains("reason=unreadable-manifest")
            && plan_stdout.contains("valid=1"),
        "{plan_stdout}"
    );

    let recover_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-manifest-recover",
            manifest.to_str().unwrap(),
            quarantine.to_str().unwrap(),
            &format!("hot:{}", content.display()),
        ])
        .output()
        .unwrap();
    assert!(
        recover_output.status.success(),
        "{}",
        String::from_utf8_lossy(&recover_output.stderr)
    );
    let recover_stdout = String::from_utf8(recover_output.stdout).unwrap();
    assert!(
        recover_stdout.contains("wrote-manifest=true") && recover_stdout.contains("action=ready"),
        "{recover_stdout}"
    );
    assert!(quarantine.read_dir().unwrap().any(|entry| entry
        .unwrap()
        .path()
        .extension()
        .is_some()));

    let inspect_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args(["content-manifest-inspect", manifest.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        inspect_output.status.success(),
        "{}",
        String::from_utf8_lossy(&inspect_output.stderr)
    );
    let inspect_stdout = String::from_utf8(inspect_output.stdout).unwrap();
    assert!(
        inspect_stdout.contains("content-manifest\tarchives=1"),
        "{inspect_stdout}"
    );

    fs::remove_file(manifest).unwrap();
    fs::remove_file(content).unwrap();
    fs::remove_dir_all(quarantine).unwrap();
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
    let tiered_content = unique_temp_path("gfm-cli-segment-tiered-compact", "gfmcontent");
    let tiered_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "compact-content-tiered",
            tiered_content.to_str().unwrap(),
            segment.to_str().unwrap(),
            segment.to_str().unwrap(),
            segment.to_str().unwrap(),
            segment.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        tiered_output.status.success(),
        "{}",
        String::from_utf8_lossy(&tiered_output.stderr)
    );
    let tiered_stderr = String::from_utf8(tiered_output.stderr).unwrap();
    assert!(
        tiered_stderr.contains("tiered-compacted")
            && tiered_stderr.contains("merged 4")
            && tiered_stderr.contains("retained 0"),
        "{tiered_stderr}"
    );
    let manifest = unique_temp_path("gfm-cli-segment-maintenance", "gfmmanifest");
    let maintained_content = unique_temp_path("gfm-cli-segment-maintained", "gfmcontent");
    let manifest_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-manifest-write",
            manifest.to_str().unwrap(),
            &format!("hot:{}", content.display()),
        ])
        .output()
        .unwrap();
    assert!(
        manifest_output.status.success(),
        "{}",
        String::from_utf8_lossy(&manifest_output.stderr)
    );
    let footprint_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-footprint",
            records.to_str().unwrap(),
            "-",
            "-",
            "-",
            "-",
            manifest.to_str().unwrap(),
            segment.to_str().unwrap(),
            segment.to_str().unwrap(),
            segment.to_str().unwrap(),
            segment.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        footprint_output.status.success(),
        "{}",
        String::from_utf8_lossy(&footprint_output.stderr)
    );
    let footprint_stderr = String::from_utf8(footprint_output.stderr).unwrap();
    assert!(
        footprint_stderr.contains("index-footprint")
            && footprint_stderr.contains("compaction-scheduled=true")
            && footprint_stderr.contains("reason=TierPressure"),
        "{footprint_stderr}"
    );
    let footprint_stdout = String::from_utf8(footprint_output.stdout).unwrap();
    assert!(
        footprint_stdout.contains("records\tcount=")
            && footprint_stdout.contains("content\tarchives=1")
            && footprint_stdout.contains("compaction\tscheduled=true"),
        "{footprint_stdout}"
    );
    let adaptive_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "index-compaction-plan",
            records.to_str().unwrap(),
            manifest.to_str().unwrap(),
            "saturated",
            "nominal",
            "ac",
            "idle",
            segment.to_str().unwrap(),
            segment.to_str().unwrap(),
            segment.to_str().unwrap(),
            segment.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        adaptive_output.status.success(),
        "{}",
        String::from_utf8_lossy(&adaptive_output.stderr)
    );
    let adaptive_stderr = String::from_utf8(adaptive_output.stderr).unwrap();
    assert!(
        adaptive_stderr.contains("index-compaction-plan")
            && adaptive_stderr.contains("action=Defer")
            && adaptive_stderr.contains("scheduled=false"),
        "{adaptive_stderr}"
    );
    let adaptive_stdout = String::from_utf8(adaptive_output.stdout).unwrap();
    assert!(
        adaptive_stdout.contains("compaction\taction=Defer")
            && adaptive_stdout.contains("effective-max-bytes=0"),
        "{adaptive_stdout}"
    );
    let maintenance_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "content-maintain-segments",
            manifest.to_str().unwrap(),
            maintained_content.to_str().unwrap(),
            segment.to_str().unwrap(),
            segment.to_str().unwrap(),
            segment.to_str().unwrap(),
            segment.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        maintenance_output.status.success(),
        "{}",
        String::from_utf8_lossy(&maintenance_output.stderr)
    );
    let maintenance_stderr = String::from_utf8(maintenance_output.stderr).unwrap();
    assert!(
        maintenance_stderr.contains("content-maintenance")
            && maintenance_stderr.contains("scheduled=true")
            && maintenance_stderr.contains("merged=4")
            && maintenance_stderr.contains("manifest-archives=2"),
        "{maintenance_stderr}"
    );
    let maintenance_stdout = String::from_utf8(maintenance_output.stdout).unwrap();
    assert!(
        maintenance_stdout.contains(&format!("published\t{}", maintained_content.display())),
        "{maintenance_stdout}"
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
    let manifest_search_output = Command::new(env!("CARGO_BIN_EXE_gfm"))
        .args([
            "search-content-index-manifest",
            records.to_str().unwrap(),
            manifest.to_str().unwrap(),
            "segmentmarker",
        ])
        .output()
        .unwrap();
    assert!(
        manifest_search_output.status.success(),
        "{}",
        String::from_utf8_lossy(&manifest_search_output.stderr)
    );
    let manifest_stdout = String::from_utf8(manifest_search_output.stdout).unwrap();
    assert!(manifest_stdout.contains("segment.md"), "{manifest_stdout}");

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(records).unwrap();
    fs::remove_file(segment).unwrap();
    fs::remove_file(content).unwrap();
    fs::remove_file(tiered_content).unwrap();
    fs::remove_file(maintained_content).unwrap();
    fs::remove_file(manifest).unwrap();
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

fn write_legacy_content_archive(path: &std::path::Path, postings: &[ContentPosting]) {
    let mut bytes = Vec::new();
    bytes.extend(b"gfm-content-v1\n");
    push_varint(&mut bytes, postings.len() as u64);
    for posting in postings {
        push_varint(&mut bytes, posting.term.len() as u64);
        bytes.extend(posting.term.as_bytes());
        write_legacy_file_ids(&mut bytes, &posting.ids);
    }
    fs::write(path, bytes).unwrap();
}

fn write_legacy_metadata_archive(path: &std::path::Path, postings: &[MetadataPosting]) {
    let mut bytes = Vec::new();
    bytes.extend(b"gfm-metadata-v1\n");
    push_varint(&mut bytes, postings.len() as u64);
    let mut directory = Vec::new();
    let mut postings = postings.to_vec();
    postings.sort_by(|left, right| {
        (metadata_field_code(left.field), left.term.as_str())
            .cmp(&(metadata_field_code(right.field), right.term.as_str()))
    });
    for posting in &postings {
        let offset = bytes.len() as u64;
        write_legacy_metadata_posting(&mut bytes, posting);
        directory.push((
            posting.field,
            posting.term.clone(),
            offset,
            bytes.len() as u64 - offset,
        ));
    }
    let directory_offset = bytes.len() as u64;
    push_varint(&mut bytes, directory.len() as u64);
    for (field, term, offset, len) in directory {
        bytes.push(metadata_field_code(field));
        push_varint(&mut bytes, term.len() as u64);
        bytes.extend(term.as_bytes());
        push_varint(&mut bytes, offset);
        push_varint(&mut bytes, len);
    }
    bytes.extend(directory_offset.to_le_bytes());
    bytes.extend(b"gfm-metadata-index-v1\n");
    fs::write(path, bytes).unwrap();
}

fn write_legacy_metadata_posting(bytes: &mut Vec<u8>, posting: &MetadataPosting) {
    bytes.push(metadata_field_code(posting.field));
    push_varint(bytes, posting.term.len() as u64);
    bytes.extend(posting.term.as_bytes());
    write_legacy_file_ids(bytes, &posting.ids);
}

fn metadata_field_code(field: MetadataField) -> u8 {
    match field {
        MetadataField::Tag => b't',
        MetadataField::Comment => b'c',
    }
}

fn write_legacy_file_ids(bytes: &mut Vec<u8>, ids: &[FileId]) {
    let mut ids = ids.to_vec();
    ids.sort();
    push_varint(bytes, ids.len() as u64);
    let mut previous = FileId::new(VolumeId(0), 0);
    for id in ids {
        push_varint(bytes, id.volume.0.saturating_sub(previous.volume.0));
        let node_delta = if id.volume == previous.volume {
            id.node.saturating_sub(previous.node)
        } else {
            id.node
        };
        push_varint(bytes, node_delta);
        previous = id;
    }
}

fn push_varint(bytes: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        bytes.push(((value as u8) & 0x7f) | 0x80);
        value >>= 7;
    }
    bytes.push(value as u8);
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
