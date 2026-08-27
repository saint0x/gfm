use crate::{
    access::{preflight_access_scope, preflight_volume_access_scope, ScopedAccessGuard},
    detect_volume_id, parse_u64_arg, parse_usize_arg, required_path,
    runtime::run_volume_task_cancellable,
};
use gfm_fs::read_directory;
use gfm_index::{
    EventBackpressureQueue, EventPriority, FseventsCursor, FseventsCursorHealth, IndexVolumeState,
    Indexer, LiveIndex,
};
use gfm_jobs::Priority;
use gfm_mac::{AccessIntent, FileEventStream, WatchRoot};
use gfm_types::{FileEvent, FileEventKind, FileKind, GfmError, Result};
use std::io::Write;
use std::path::{Path, PathBuf};

pub(crate) fn run(command: &str, args: &mut impl Iterator<Item = String>) -> Result<bool> {
    match command {
        "list" => {
            let path = args
                .next()
                .map(PathBuf::from)
                .unwrap_or(std::env::current_dir().unwrap());
            preflight_volume_access_scope(&path, AccessIntent::Read, "directory listing")?;
            let volume = detect_volume_id(&path).ok();
            let page = run_volume_task_cancellable(
                volume,
                Priority::Visible,
                "directory listing",
                move |cancellation| {
                    cancellation.check()?;
                    let _access = preflight_index_read(&path, "directory listing")?;
                    cancellation.check()?;
                    read_directory(path)
                },
            )?;
            for record in page.entries {
                println!(
                    "{}\t{}\t{}",
                    marker(record.kind),
                    record.len,
                    record.path.display()
                );
            }
            for issue in page.inaccessible {
                eprintln!("inaccessible\t{}\t{}", issue.path.display(), issue.reason);
            }
        }
        "index" => {
            let root = required_path(args.next(), "index requires a root path")?;
            let output = required_path(args.next(), "index requires an output path")?;
            preflight_index_volume_access(&root)?;
            let _output_access = preflight_index_write(&output, "index records")?;
            let volume = detect_volume_id(&root).ok();
            let (record_count, inaccessible_count) = run_volume_task_cancellable(
                volume,
                Priority::Visible,
                "index",
                move |cancellation| {
                    let _root_access = enforce_index_access(&root)?;
                    let snapshot = Indexer::default().build_cancellable(root, &cancellation)?;
                    let record_count = snapshot.records.len();
                    let inaccessible_count = snapshot.inaccessible.len();
                    snapshot.save(output)?;
                    Ok((record_count, inaccessible_count))
                },
            )?;
            eprintln!("indexed {record_count} records; {inaccessible_count} inaccessible");
        }
        "index-state" => {
            let root = required_path(args.next(), "index-state requires a root path")?;
            let records = required_path(args.next(), "index-state requires a records path")?;
            let state = required_path(args.next(), "index-state requires a state path")?;
            preflight_index_volume_access(&root)?;
            let _records_access = preflight_index_write(&records, "index records")?;
            let _state_access = preflight_index_write(&state, "index state")?;
            let volume = detect_volume_id(&root).ok();
            let state = run_volume_task_cancellable(
                volume,
                Priority::Visible,
                "index",
                move |cancellation| {
                    let _root_access = enforce_index_access(&root)?;
                    Indexer::default().build_persistent_cancellable(
                        root,
                        records,
                        state,
                        &cancellation,
                    )
                },
            )?;
            println!("{}", state.as_tsv());
        }
        "index-state-inspect" => {
            let state = required_path(
                args.next(),
                "index-state-inspect requires an index state path",
            )?;
            let _state_access = preflight_index_read(&state, "index state inspect")?;
            println!("{}", IndexVolumeState::read(state)?.as_tsv());
        }
        "scan-progress" => {
            let root = required_path(args.next(), "scan-progress requires a root path")?;
            let records = required_path(args.next(), "scan-progress requires a records path")?;
            let progress = required_path(
                args.next(),
                "scan-progress requires a progress checkpoint path",
            )?;
            preflight_index_volume_access(&root)?;
            let _records_access = preflight_index_write(&records, "scan progress records")?;
            let _progress_access = preflight_index_write(&progress, "scan progress checkpoint")?;
            let volume = detect_volume_id(&root).ok();
            let checkpoint = run_volume_task_cancellable(
                volume,
                Priority::Visible,
                "index",
                move |cancellation| {
                    let _root_access = enforce_index_access(&root)?;
                    Indexer::default().build_with_progress_cancellable(
                        root,
                        records,
                        progress,
                        &cancellation,
                    )
                },
            )?;
            println!("{}", checkpoint.as_tsv());
        }
        "scan-progress-inspect" => {
            let progress = required_path(
                args.next(),
                "scan-progress-inspect requires a progress checkpoint path",
            )?;
            let _progress_access =
                preflight_index_read(&progress, "scan progress checkpoint inspect")?;
            println!("{}", Indexer::default().scan_progress(progress)?.as_tsv());
        }
        "fair-scan" => {
            let root = required_path(args.next(), "fair-scan requires a root path")?;
            let visible_burst =
                parse_usize_arg(args.next(), "fair-scan requires a visible burst size")?;
            let visible_roots = args.map(PathBuf::from).collect::<Vec<_>>();
            preflight_index_volume_access(&root)?;
            visible_roots
                .iter()
                .map(|visible_root| preflight_index_volume_access(visible_root))
                .collect::<Result<Vec<_>>>()?;
            let volume = detect_volume_id(&root).ok();
            let report = run_volume_task_cancellable(
                volume,
                Priority::Visible,
                "index",
                move |cancellation| {
                    let _root_access = enforce_index_access(&root)?;
                    let _visible_accesses = visible_roots
                        .iter()
                        .map(|visible_root| enforce_index_access(visible_root))
                        .collect::<Result<Vec<_>>>()?;
                    Indexer::default().build_fair_cancellable(
                        root,
                        &visible_roots,
                        visible_burst,
                        &cancellation,
                    )
                },
            )?;
            println!("{}", report.as_tsv());
        }
        "rename-correlation" => {
            let from = required_path(args.next(), "rename-correlation requires a source path")?;
            let to = required_path(
                args.next(),
                "rename-correlation requires a destination path",
            )?;
            let root = from
                .parent()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            preflight_index_volume_access(&root)?;
            preflight_index_write_volume(&from, "rename correlation source")?;
            preflight_index_write_volume(&to, "rename correlation destination")?;
            let volume = detect_volume_id(&root).ok();
            let report = run_volume_task_cancellable(
                volume,
                Priority::Visible,
                "index",
                move |cancellation| {
                    let _root_access = enforce_index_access(&root)?;
                    let _from_access = preflight_index_write(&from, "rename correlation source")?;
                    let _to_access = preflight_index_write(&to, "rename correlation destination")?;
                    let snapshot = Indexer::default().build_cancellable(root, &cancellation)?;
                    cancellation.check()?;
                    std::fs::rename(&from, &to).map_err(|err| GfmError::io(&from, err))?;
                    let mut live = LiveIndex::from_records(snapshot.records);
                    live.apply_rename(&from, &to)
                },
            )?;
            println!("{}", report.as_tsv());
        }
        "metadata-update" => {
            let path = required_path(args.next(), "metadata-update requires a path")?;
            let root = path
                .parent()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            let append = args.next();
            preflight_index_volume_access(&root)?;
            if append.is_some() {
                preflight_index_write_volume(&path, "metadata update")?;
            }
            let volume = detect_volume_id(&root).ok();
            let report = run_volume_task_cancellable(
                volume,
                Priority::Visible,
                "index",
                move |cancellation| {
                    let _root_access = enforce_index_access(&root)?;
                    let _path_access = if append.is_some() {
                        Some(preflight_index_write(&path, "metadata update")?)
                    } else {
                        None
                    };
                    let snapshot = Indexer::default().build_cancellable(root, &cancellation)?;
                    if let Some(append) = append {
                        cancellation.check()?;
                        let mut file = std::fs::OpenOptions::new()
                            .append(true)
                            .open(&path)
                            .map_err(|err| GfmError::io(&path, err))?;
                        file.write_all(append.as_bytes())
                            .map_err(|err| GfmError::io(&path, err))?;
                    }
                    let mut live = LiveIndex::from_records(snapshot.records);
                    live.apply_metadata_update(&path)
                },
            )?;
            println!("{}", report.as_tsv());
        }
        "event-backpressure" => {
            let capacity = parse_usize_arg(args.next(), "event-backpressure requires a capacity")?;
            let visible_burst = parse_usize_arg(
                args.next(),
                "event-backpressure requires a visible burst size",
            )?;
            let background = parse_usize_arg(
                args.next(),
                "event-backpressure requires a background event count",
            )?;
            let visible = args
                .next()
                .map(|value| parse_usize(&value, "visible event count"))
                .transpose()?
                .unwrap_or(1);
            let mut queue = EventBackpressureQueue::new(capacity, visible_burst);
            for index in 0..background {
                queue.enqueue(
                    EventPriority::Background,
                    FileEvent::new(
                        format!("/tmp/gfm-background-{index}.md"),
                        FileEventKind::Modify,
                    ),
                );
            }
            for index in 0..visible {
                queue.enqueue(
                    EventPriority::Visible,
                    FileEvent::new(
                        format!("/tmp/gfm-visible-{index}.md"),
                        FileEventKind::Modify,
                    ),
                );
            }
            println!("{}", queue.snapshot().as_tsv());
        }
        "fsevents-cursor-checkpoint" => {
            let state = required_path(
                args.next(),
                "fsevents-cursor-checkpoint requires an index state path",
            )?;
            let cursor = required_path(
                args.next(),
                "fsevents-cursor-checkpoint requires a cursor path",
            )?;
            let event_id = parse_u64_arg(
                args.next(),
                "fsevents-cursor-checkpoint requires a last event id",
            )?;
            let health = args
                .next()
                .map(|value| FseventsCursorHealth::parse(&value))
                .transpose()?
                .unwrap_or(FseventsCursorHealth::Clean);
            let _state_access = preflight_index_read(&state, "fsevents cursor checkpoint state")?;
            let _cursor_access = preflight_index_write(&cursor, "fsevents cursor checkpoint")?;
            let cursor =
                Indexer::default().checkpoint_fsevents_cursor(state, cursor, event_id, health)?;
            println!("{}", cursor.as_tsv());
        }
        "fsevents-cursor-inspect" => {
            let cursor = required_path(
                args.next(),
                "fsevents-cursor-inspect requires a cursor path",
            )?;
            let _cursor_access = preflight_index_read(&cursor, "fsevents cursor inspect")?;
            println!("{}", FseventsCursor::read(cursor)?.as_tsv());
        }
        "fsevents-cursor-resume" => {
            let state = required_path(
                args.next(),
                "fsevents-cursor-resume requires an index state path",
            )?;
            let cursor =
                required_path(args.next(), "fsevents-cursor-resume requires a cursor path")?;
            let _state_access = preflight_index_read(&state, "fsevents cursor resume state")?;
            let _cursor_access = preflight_index_read(&cursor, "fsevents cursor resume")?;
            println!(
                "{}",
                Indexer::default()
                    .fsevents_resume_plan(state, cursor)?
                    .as_tsv()
            );
        }
        "fsevents-repair-schedule" => {
            let state = required_path(
                args.next(),
                "fsevents-repair-schedule requires an index state path",
            )?;
            let cursor = required_path(
                args.next(),
                "fsevents-repair-schedule requires a cursor path",
            )?;
            let event_ids = args.next().ok_or_else(|| {
                GfmError::Format(
                    "fsevents-repair-schedule requires observed event ids or `-`".to_string(),
                )
            })?;
            let observed_event_ids = parse_event_ids(&event_ids)?;
            let reason = args
                .next()
                .and_then(|value| (value != "-").then_some(value));
            let dropped_roots: Vec<PathBuf> = args.map(PathBuf::from).collect();
            let _state_access = preflight_index_read(&state, "fsevents repair schedule state")?;
            let _cursor_access = preflight_index_read(&cursor, "fsevents repair schedule cursor")?;
            let _dropped_access = dropped_roots
                .iter()
                .filter(|root| root.exists())
                .map(|root| preflight_index_read(root, "fsevents repair schedule dropped root"))
                .collect::<Result<Vec<_>>>()?;
            println!(
                "{}",
                Indexer::default()
                    .repair_schedule(
                        state,
                        cursor,
                        &observed_event_ids,
                        &dropped_roots,
                        reason.as_deref(),
                    )?
                    .as_tsv()
            );
        }
        "watch-once" => {
            let root = required_path(args.next(), "watch-once requires a root path")?;
            let _root_access = enforce_index_access(&root)?;
            let stream = FileEventStream::watch(&[WatchRoot::tree(root)])?;
            let event = stream.recv()?;
            println!("{}\t{}", event_marker(&event.kind), event.path.display());
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn parse_event_ids(value: &str) -> Result<Vec<u64>> {
    if value == "-" || value.trim().is_empty() {
        return Ok(Vec::new());
    }
    value
        .split(',')
        .map(|part| {
            part.parse().map_err(|_| {
                GfmError::Format(format!("observed event id `{part}` must be unsigned"))
            })
        })
        .collect()
}

fn enforce_index_access(root: &Path) -> Result<ScopedAccessGuard> {
    preflight_access_scope(root, AccessIntent::Index, "index")
}

fn preflight_index_volume_access(root: &Path) -> Result<()> {
    preflight_volume_access_scope(root, AccessIntent::Index, "index")
}

fn preflight_index_read(path: &Path, worker: &str) -> Result<ScopedAccessGuard> {
    preflight_access_scope(path, AccessIntent::Read, worker)
}

fn preflight_index_write(path: &Path, worker: &str) -> Result<ScopedAccessGuard> {
    preflight_access_scope(write_probe_path(path), AccessIntent::Write, worker)
}

fn preflight_index_write_volume(path: &Path, worker: &str) -> Result<()> {
    preflight_volume_access_scope(write_probe_path(path), AccessIntent::Write, worker)
}

fn write_probe_path(path: &Path) -> &Path {
    if path.is_dir() {
        return path;
    }
    path.parent().unwrap_or(path)
}

fn parse_usize(value: &str, message: &str) -> Result<usize> {
    value
        .parse()
        .map_err(|_| GfmError::Format(format!("{message}; got `{value}`")))
}

fn marker(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Directory => "dir",
        FileKind::File => "file",
        FileKind::Symlink => "link",
        FileKind::Other => "other",
    }
}

fn event_marker(kind: &FileEventKind) -> &'static str {
    match kind {
        FileEventKind::Create => "create",
        FileEventKind::Metadata => "metadata",
        FileEventKind::Modify => "modify",
        FileEventKind::Remove => "remove",
        FileEventKind::Rename { .. } => "rename",
        FileEventKind::Rescan => "rescan",
        FileEventKind::Other => "other",
    }
}
