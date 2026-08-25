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

    fs::remove_dir_all(root).unwrap();
    fs::remove_file(index).unwrap();
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
fn searches_persisted_tags_from_binary() {
    let index = unique_temp_path("gfm-cli-tags", "gfmidx");
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

    fs::remove_file(index).unwrap();
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

fn ooxml_package(parts: &[(&str, &str)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, text) in parts {
        writer.start_file(*name, options).unwrap();
        writer.write_all(text.as_bytes()).unwrap();
    }
    writer.finish().unwrap().into_inner()
}
