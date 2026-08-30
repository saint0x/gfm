use crate::{
    access::{preflight_access_scope, preflight_volume_access_scope, ScopedAccessGuard},
    parse_u64_arg, parse_usize_arg, path_volume, required_path,
    runtime::run_volume_task_cancellable,
};
use gfm_fs::read_directory_checked;
use gfm_index::{
    EventBackpressureQueue, EventPriority, FseventsCursor, FseventsCursorHealth, IndexVolumeState,
    Indexer, LiveIndex,
};
use gfm_jobs::{Cancellation, Priority};
use gfm_mac::{AccessIntent, FileEventStream, WatchRoot};
use gfm_types::{FileEvent, FileEventKind, FileKind, GfmError, Result};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub(crate) fn run(command: &str, args: &mut impl Iterator<Item = String>) -> Result<bool> {
    match command {
        "list" => {
            let path = args
                .next()
                .map(PathBuf::from)
                .unwrap_or(std::env::current_dir().unwrap());
            preflight_volume_access_scope(&path, AccessIntent::Read, "directory listing")?;
            let volume = path_volume(&path);
            let page = run_volume_task_cancellable(
                volume,
                Priority::Visible,
                "directory listing",
                move |cancellation| {
                    cancellation.check()?;
                    let _access = preflight_index_read(&path, "directory listing")?;
                    cancellation.check()?;
                    read_directory_checked(path, || cancellation.check())
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
            let output_probe = write_probe_path(&output)?.to_path_buf();
            let volume = path_volume(&root).or_else(|| path_volume(&output_probe));
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
            let records_probe = write_probe_path(&records)?.to_path_buf();
            let state_probe = write_probe_path(&state)?.to_path_buf();
            let volume = path_volume(&root)
                .or_else(|| path_volume(&records_probe))
                .or_else(|| path_volume(&state_probe));
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
            println!("{}", run_index_state_inspect(state)?.as_tsv());
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
            let records_probe = write_probe_path(&records)?.to_path_buf();
            let progress_probe = write_probe_path(&progress)?.to_path_buf();
            let volume = path_volume(&root)
                .or_else(|| path_volume(&records_probe))
                .or_else(|| path_volume(&progress_probe));
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
            println!("{}", run_scan_progress_inspect(progress)?.as_tsv());
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
            let volume = path_volume(&root)
                .or_else(|| visible_roots.iter().find_map(|root| path_volume(root)));
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
            let from_probe = write_probe_path(&from)?.to_path_buf();
            let to_probe = write_probe_path(&to)?.to_path_buf();
            let volume = path_volume(&root)
                .or_else(|| path_volume(&from_probe))
                .or_else(|| path_volume(&to_probe));
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
            let volume = path_volume(&root).or_else(|| path_volume(&path));
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
            let cursor = run_fsevents_cursor_checkpoint(state, cursor, event_id, health)?;
            println!("{}", cursor.as_tsv());
        }
        "fsevents-cursor-inspect" => {
            let cursor = required_path(
                args.next(),
                "fsevents-cursor-inspect requires a cursor path",
            )?;
            println!("{}", run_fsevents_cursor_inspect(cursor)?.as_tsv());
        }
        "fsevents-cursor-resume" => {
            let state = required_path(
                args.next(),
                "fsevents-cursor-resume requires an index state path",
            )?;
            let cursor =
                required_path(args.next(), "fsevents-cursor-resume requires a cursor path")?;
            println!("{}", run_fsevents_cursor_resume(state, cursor)?.as_tsv());
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
            println!(
                "{}",
                run_fsevents_repair_schedule(
                    state,
                    cursor,
                    observed_event_ids,
                    reason,
                    dropped_roots
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

fn run_index_state_inspect(state: PathBuf) -> Result<IndexVolumeState> {
    run_index_read_task(state, "index state inspect", |path, cancellation| {
        let state = IndexVolumeState::read_checked(path, || cancellation.check())?;
        cancellation.check()?;
        Ok(state)
    })
}

fn run_scan_progress_inspect(progress: PathBuf) -> Result<gfm_index::ScanProgressCheckpoint> {
    run_index_read_task(
        progress,
        "scan progress checkpoint inspect",
        |path, cancellation| {
            let checkpoint = Indexer::default().scan_progress(path)?;
            cancellation.check()?;
            Ok(checkpoint)
        },
    )
}

fn run_fsevents_cursor_inspect(cursor: PathBuf) -> Result<FseventsCursor> {
    run_index_read_task(cursor, "fsevents cursor inspect", |path, cancellation| {
        let cursor = FseventsCursor::read_checked(path, || cancellation.check())?;
        cancellation.check()?;
        Ok(cursor)
    })
}

fn run_fsevents_cursor_checkpoint(
    state: PathBuf,
    cursor: PathBuf,
    event_id: u64,
    health: FseventsCursorHealth,
) -> Result<FseventsCursor> {
    preflight_volume_access_scope(
        &state,
        AccessIntent::Read,
        "fsevents cursor checkpoint state",
    )?;
    preflight_index_write_volume(&cursor, "fsevents cursor checkpoint")?;
    let cursor_probe = write_probe_path(&cursor)?.to_path_buf();
    let volume = path_volume(&state).or_else(|| path_volume(&cursor_probe));
    run_volume_task_cancellable(
        volume,
        Priority::Visible,
        "fsevents cursor checkpoint",
        move |cancellation| {
            cancellation.check()?;
            let _state_access = preflight_index_read(&state, "fsevents cursor checkpoint state")?;
            let _cursor_access = preflight_index_write(&cursor, "fsevents cursor checkpoint")?;
            cancellation.check()?;
            Indexer::default().checkpoint_fsevents_cursor_cancellable(
                state,
                cursor,
                event_id,
                health,
                &cancellation,
            )
        },
    )
}

fn run_fsevents_cursor_resume(
    state: PathBuf,
    cursor: PathBuf,
) -> Result<gfm_index::FseventsResumePlan> {
    preflight_volume_access_scope(&state, AccessIntent::Read, "fsevents cursor resume state")?;
    preflight_volume_access_scope(&cursor, AccessIntent::Read, "fsevents cursor resume")?;
    let volume = path_volume(&state).or_else(|| path_volume(&cursor));
    run_volume_task_cancellable(
        volume,
        Priority::Visible,
        "fsevents cursor resume",
        move |cancellation| {
            cancellation.check()?;
            let _state_access = preflight_index_read(&state, "fsevents cursor resume state")?;
            let _cursor_access = preflight_index_read(&cursor, "fsevents cursor resume")?;
            cancellation.check()?;
            Indexer::default().fsevents_resume_plan_cancellable(state, cursor, &cancellation)
        },
    )
}

fn run_fsevents_repair_schedule(
    state: PathBuf,
    cursor: PathBuf,
    observed_event_ids: Vec<u64>,
    reason: Option<String>,
    dropped_roots: Vec<PathBuf>,
) -> Result<gfm_index::RepairSchedule> {
    preflight_volume_access_scope(&state, AccessIntent::Read, "fsevents repair schedule state")?;
    preflight_volume_access_scope(
        &cursor,
        AccessIntent::Read,
        "fsevents repair schedule cursor",
    )?;
    let existing_dropped_roots = existing_dropped_roots(&dropped_roots)?;
    let volume = path_volume(&state)
        .or_else(|| path_volume(&cursor))
        .or_else(|| {
            existing_dropped_roots
                .iter()
                .find_map(|root| path_volume(root))
        });
    run_volume_task_cancellable(
        volume,
        Priority::Visible,
        "fsevents repair schedule",
        move |cancellation| {
            cancellation.check()?;
            let _state_access = preflight_index_read(&state, "fsevents repair schedule state")?;
            let _cursor_access = preflight_index_read(&cursor, "fsevents repair schedule cursor")?;
            let _dropped_access = existing_dropped_roots
                .iter()
                .map(|root| preflight_index_read(root, "fsevents repair schedule dropped root"))
                .collect::<Result<Vec<_>>>()?;
            cancellation.check()?;
            Indexer::default().repair_schedule_cancellable(
                state,
                cursor,
                &observed_event_ids,
                &existing_dropped_roots,
                reason.as_deref(),
                &cancellation,
            )
        },
    )
}

fn existing_dropped_roots(dropped_roots: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut existing = Vec::new();
    for root in dropped_roots {
        if !root.try_exists().map_err(|err| {
            GfmError::io(
                root,
                format!("fsevents repair dropped root existence unavailable: {err}"),
            )
        })? {
            continue;
        }
        preflight_volume_access_scope(
            root,
            AccessIntent::Read,
            "fsevents repair schedule dropped root",
        )?;
        existing.push(root.clone());
    }
    Ok(existing)
}

fn run_index_read_task<T>(
    path: PathBuf,
    worker: &'static str,
    read: impl FnOnce(PathBuf, &Cancellation) -> Result<T> + Send + 'static,
) -> Result<T>
where
    T: Send + 'static,
{
    preflight_volume_access_scope(&path, AccessIntent::Read, worker)?;
    let volume = path_volume(&path);
    run_volume_task_cancellable(volume, Priority::Visible, worker, move |cancellation| {
        cancellation.check()?;
        let _access = preflight_index_read(&path, worker)?;
        cancellation.check()?;
        read(path, &cancellation)
    })
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
    preflight_access_scope(write_probe_path(path)?, AccessIntent::Write, worker)
}

fn preflight_index_write_volume(path: &Path, worker: &str) -> Result<()> {
    preflight_volume_access_scope(write_probe_path(path)?, AccessIntent::Write, worker)
}

fn write_probe_path(path: &Path) -> Result<&Path> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(path),
        Ok(_) => Ok(crate::parent_or_cwd(path)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(crate::parent_or_cwd(path)),
        Err(err) => Err(GfmError::io(
            path,
            format!("index write path metadata unavailable: {err}"),
        )),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_read_task_passes_runtime_token_to_reader() {
        let path = std::env::temp_dir().join(format!(
            "gfm-index-read-cancellation-token-{}.gfmstate",
            std::process::id()
        ));
        fs::write(&path, b"token-probe").unwrap();

        let result = run_index_read_task(
            path.clone(),
            "index read cancellation token",
            |_path, cancellation| {
                cancellation.cancel();
                cancellation.check()
            },
        );

        assert_eq!(result, Err(GfmError::Cancelled));
        fs::remove_file(path).unwrap();
    }
}
