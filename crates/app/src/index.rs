use crate::{parse_u64_arg, parse_usize_arg, required_path};
use gfm_fs::read_directory;
use gfm_index::{
    EventBackpressureQueue, EventPriority, FseventsCursor, FseventsCursorHealth, IndexVolumeState,
    Indexer, LiveIndex,
};
use gfm_mac::{FileEventStream, WatchRoot};
use gfm_types::{FileEvent, FileEventKind, FileKind, GfmError, Result};
use std::io::Write;
use std::path::PathBuf;

pub(crate) fn run(command: &str, args: &mut impl Iterator<Item = String>) -> Result<bool> {
    match command {
        "list" => {
            let path = args
                .next()
                .map(PathBuf::from)
                .unwrap_or(std::env::current_dir().unwrap());
            let page = read_directory(path)?;
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
            let snapshot = Indexer::default().build(root)?;
            snapshot.save(output)?;
            eprintln!(
                "indexed {} records; {} inaccessible",
                snapshot.records.len(),
                snapshot.inaccessible.len()
            );
        }
        "index-state" => {
            let root = required_path(args.next(), "index-state requires a root path")?;
            let records = required_path(args.next(), "index-state requires a records path")?;
            let state = required_path(args.next(), "index-state requires a state path")?;
            let state = Indexer::default().build_persistent(root, records, state)?;
            println!("{}", state.as_tsv());
        }
        "index-state-inspect" => {
            let state = required_path(
                args.next(),
                "index-state-inspect requires an index state path",
            )?;
            println!("{}", IndexVolumeState::read(state)?.as_tsv());
        }
        "scan-progress" => {
            let root = required_path(args.next(), "scan-progress requires a root path")?;
            let records = required_path(args.next(), "scan-progress requires a records path")?;
            let progress = required_path(
                args.next(),
                "scan-progress requires a progress checkpoint path",
            )?;
            let checkpoint = Indexer::default().build_with_progress(root, records, progress)?;
            println!("{}", checkpoint.as_tsv());
        }
        "scan-progress-inspect" => {
            let progress = required_path(
                args.next(),
                "scan-progress-inspect requires a progress checkpoint path",
            )?;
            println!("{}", Indexer::default().scan_progress(progress)?.as_tsv());
        }
        "fair-scan" => {
            let root = required_path(args.next(), "fair-scan requires a root path")?;
            let visible_burst =
                parse_usize_arg(args.next(), "fair-scan requires a visible burst size")?;
            let visible_roots = args.map(PathBuf::from).collect::<Vec<_>>();
            let report = Indexer::default().build_fair(root, &visible_roots, visible_burst)?;
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
            let snapshot = Indexer::default().build(root)?;
            std::fs::rename(&from, &to).map_err(|err| GfmError::io(&from, err))?;
            let mut live = LiveIndex::from_records(snapshot.records);
            let report = live.apply_rename(&from, &to)?;
            println!("{}", report.as_tsv());
        }
        "metadata-update" => {
            let path = required_path(args.next(), "metadata-update requires a path")?;
            let root = path
                .parent()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            let snapshot = Indexer::default().build(root)?;
            if let Some(append) = args.next() {
                let mut file = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .map_err(|err| GfmError::io(&path, err))?;
                file.write_all(append.as_bytes())
                    .map_err(|err| GfmError::io(&path, err))?;
            }
            let mut live = LiveIndex::from_records(snapshot.records);
            let report = live.apply_metadata_update(&path)?;
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
            let cursor =
                Indexer::default().checkpoint_fsevents_cursor(state, cursor, event_id, health)?;
            println!("{}", cursor.as_tsv());
        }
        "fsevents-cursor-inspect" => {
            let cursor = required_path(
                args.next(),
                "fsevents-cursor-inspect requires a cursor path",
            )?;
            println!("{}", FseventsCursor::read(cursor)?.as_tsv());
        }
        "fsevents-cursor-resume" => {
            let state = required_path(
                args.next(),
                "fsevents-cursor-resume requires an index state path",
            )?;
            let cursor =
                required_path(args.next(), "fsevents-cursor-resume requires a cursor path")?;
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
        FileEventKind::Modify => "modify",
        FileEventKind::Remove => "remove",
        FileEventKind::Rename { .. } => "rename",
        FileEventKind::Rescan => "rescan",
        FileEventKind::Other => "other",
    }
}
