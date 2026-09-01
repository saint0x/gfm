use crate::{
    access::{
        preflight_access_scope_checked, preflight_access_scope_checked_with_volume_report,
        preflight_volume_access_scope_with_report, ScopedAccessGuard,
    },
    index_volume_descriptor, parse_u64_arg, parse_usize_arg, required_path, required_string,
    runtime::{
        run_retriable_volume_task_cancellable_with_payload_path, run_volume_task_cancellable,
    },
};
use gfm_fs::read_directory_checked;
use gfm_index::{
    parse_volume_indexing_policy, EventBackpressureQueue, EventPriority, FseventsCursor,
    FseventsCursorHealth, IndexVolumeState, Indexer, LiveIndex, VolumeIndexPolicy,
};
use gfm_jobs::{Cancellation, Priority};
use gfm_mac::{AccessIntent, FileEventStream, VolumeDiscoveryReport, WatchRoot};
use gfm_store::atomic_write_checked;
use gfm_types::{FileEvent, FileEventKind, FileKind, GfmError, Result};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

pub(crate) fn run(command: &str, args: &mut impl Iterator<Item = String>) -> Result<bool> {
    match command {
        "list" => {
            let path = args
                .next()
                .map(PathBuf::from)
                .unwrap_or(std::env::current_dir().unwrap());
            let volume_report =
                VolumeDiscoveryReport::for_containing_path_checked(&path, || Ok(()))?;
            preflight_volume_access_scope_with_report(
                &path,
                AccessIntent::Read,
                "directory listing",
                &volume_report,
            )?;
            let volume = volume_report.volume_for_path(&path).map(|volume| volume.id);
            let page = run_volume_task_cancellable(
                volume,
                Priority::Visible,
                "directory listing",
                move |cancellation| {
                    cancellation.check()?;
                    let _access = preflight_index_read_checked_with_volume_report(
                        &path,
                        "directory listing",
                        &volume_report,
                        || cancellation.check(),
                    )?;
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
        "index" | "index-retry-probe" => {
            let root = required_path(args.next(), "index requires a root path")?;
            let output = required_path(args.next(), "index requires an output path")?;
            let retry_probe = if command == "index-retry-probe" {
                Some(required_path(
                    args.next(),
                    "index-retry-probe requires an attempt state path",
                )?)
            } else {
                None
            };
            let access_reports =
                IndexBuildAccessReports::for_root_and_output_checked(&root, &output, || Ok(()))?;
            access_reports.preflight_volumes()?;
            let _output_access = access_reports.preflight_output_access_checked(|| Ok(()))?;
            if let Some(retry_probe) = retry_probe.as_ref() {
                preflight_index_write(retry_probe, "index")?;
            }
            let volume = access_reports.first_volume();
            let (record_count, inaccessible_count) =
                run_retriable_volume_task_cancellable_with_payload_path(
                    volume,
                    Priority::Visible,
                    "index",
                    output.clone(),
                    move |cancellation| {
                        let root = root.clone();
                        let output = output.clone();
                        let retry_probe = retry_probe.clone();
                        if let Some(retry_probe) = retry_probe.as_ref() {
                            fail_first_index_retry_probe_attempt(
                                retry_probe,
                                "index",
                                &cancellation,
                            )?;
                        }
                        let _root_access =
                            access_reports.enforce_root_access_checked(|| cancellation.check())?;
                        cancellation.check()?;
                        let _output_access = access_reports
                            .preflight_output_access_checked(|| cancellation.check())?;
                        cancellation.check()?;
                        let snapshot = Indexer::default().build_cancellable(root, &cancellation)?;
                        let record_count = snapshot.records.len();
                        let inaccessible_count = snapshot.inaccessible.len();
                        snapshot.save_checked(output, || cancellation.check())?;
                        Ok((record_count, inaccessible_count))
                    },
                )?;
            eprintln!("indexed {record_count} records; {inaccessible_count} inaccessible");
        }
        "index-state" => {
            let root = required_path(args.next(), "index-state requires a root path")?;
            let records = required_path(args.next(), "index-state requires a records path")?;
            let state = required_path(args.next(), "index-state requires a state path")?;
            let access_reports = IndexRootWriteAccessReports::for_root_and_writes_checked(
                &root,
                &[(&records, "index records"), (&state, "index state")],
                || Ok(()),
            )?;
            access_reports.preflight_volumes()?;
            let _write_accesses = access_reports.write_accesses_checked(|| Ok(()))?;
            let volume = access_reports.first_volume();
            let state = run_volume_task_cancellable(
                volume,
                Priority::Visible,
                "index",
                move |cancellation| {
                    let _root_access =
                        access_reports.root_access_checked(|| cancellation.check())?;
                    cancellation.check()?;
                    let _write_accesses =
                        access_reports.write_accesses_checked(|| cancellation.check())?;
                    cancellation.check()?;
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
        "index-admission-state" => {
            let external = parse_volume_indexing_policy(&required_string(
                args.next(),
                "index-admission-state requires an external policy",
            )?)?;
            let network = parse_volume_indexing_policy(&required_string(
                args.next(),
                "index-admission-state requires a network policy",
            )?)?;
            let root = required_path(args.next(), "index-admission-state requires a root path")?;
            let records =
                required_path(args.next(), "index-admission-state requires a records path")?;
            let state = required_path(args.next(), "index-admission-state requires a state path")?;
            let opted_in = args
                .map(|arg| {
                    arg.strip_prefix("opt-in:")
                        .map(PathBuf::from)
                        .ok_or_else(|| {
                            GfmError::Format(format!(
                                "index-admission-state unsupported argument `{arg}`; expected opt-in:<path>"
                            ))
                        })
                })
                .collect::<Result<Vec<_>>>()?;
            let state_access = IndexPathAccessReport::write_probe_checked(
                &state,
                "index admission state",
                || Ok(()),
            )?;
            state_access.preflight_volume()?;
            let root_report = VolumeDiscoveryReport::for_containing_path_checked(&root, || Ok(()))?;
            let volume = root_report.volume_for_path(&root).cloned().ok_or_else(|| {
                GfmError::Format(format!(
                    "index-admission-state could not resolve containing volume for {}",
                    root.display()
                ))
            })?;
            let descriptor = index_volume_descriptor(&volume);
            let policy = VolumeIndexPolicy::new(external, network).with_opted_in_roots(opted_in);
            let decision = policy.decide(&descriptor);
            let volume_id = Some(volume.id).or_else(|| state_access.volume());
            let persisted = run_volume_task_cancellable(
                volume_id,
                Priority::Visible,
                "index admission state",
                move |cancellation| {
                    cancellation.check()?;
                    let _state_access = state_access.access_checked(|| cancellation.check())?;
                    cancellation.check()?;
                    Indexer::default().write_volume_decision_state_cancellable(
                        &decision,
                        records,
                        state,
                        &cancellation,
                    )
                },
            )?;
            println!("{}", persisted.as_tsv());
        }
        "scan-progress" => {
            let root = required_path(args.next(), "scan-progress requires a root path")?;
            let records = required_path(args.next(), "scan-progress requires a records path")?;
            let progress = required_path(
                args.next(),
                "scan-progress requires a progress checkpoint path",
            )?;
            let access_reports = IndexRootWriteAccessReports::for_root_and_writes_checked(
                &root,
                &[
                    (&records, "scan progress records"),
                    (&progress, "scan progress checkpoint"),
                ],
                || Ok(()),
            )?;
            access_reports.preflight_volumes()?;
            let _write_accesses = access_reports.write_accesses_checked(|| Ok(()))?;
            let volume = access_reports.first_volume();
            let checkpoint = run_volume_task_cancellable(
                volume,
                Priority::Visible,
                "index",
                move |cancellation| {
                    let _root_access =
                        access_reports.root_access_checked(|| cancellation.check())?;
                    cancellation.check()?;
                    let _write_accesses =
                        access_reports.write_accesses_checked(|| cancellation.check())?;
                    cancellation.check()?;
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
            let access_reports = IndexRootReadAccessReports::for_root_and_reads_checked(
                &root,
                &visible_roots,
                || Ok(()),
            )?;
            access_reports.preflight_volumes()?;
            let volume = access_reports.first_volume();
            let report = run_volume_task_cancellable(
                volume,
                Priority::Visible,
                "index",
                move |cancellation| {
                    let _root_access =
                        access_reports.root_access_checked(|| cancellation.check())?;
                    let _visible_accesses =
                        access_reports.read_accesses_checked(|| cancellation.check())?;
                    cancellation.check()?;
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
            let access_reports = IndexRootWriteAccessReports::for_root_and_writes_checked(
                &root,
                &[
                    (&from, "rename correlation source"),
                    (&to, "rename correlation destination"),
                ],
                || Ok(()),
            )?;
            access_reports.preflight_volumes()?;
            let volume = access_reports.first_volume();
            let report = run_volume_task_cancellable(
                volume,
                Priority::Visible,
                "index",
                move |cancellation| {
                    let _root_access =
                        access_reports.root_access_checked(|| cancellation.check())?;
                    let _write_accesses =
                        access_reports.write_accesses_checked(|| cancellation.check())?;
                    cancellation.check()?;
                    let snapshot = Indexer::default().build_cancellable(root, &cancellation)?;
                    cancellation.check()?;
                    std::fs::rename(&from, &to).map_err(|err| GfmError::io(&from, err))?;
                    let mut live = LiveIndex::from_records(snapshot.records);
                    live.apply_rename_cancellable(&from, &to, &cancellation)
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
            let access_reports = if append.is_some() {
                IndexRootWriteAccessReports::for_root_and_writes_checked(
                    &root,
                    &[(&path, "metadata update")],
                    || Ok(()),
                )?
            } else {
                IndexRootWriteAccessReports::for_root_and_writes_checked(&root, &[], || Ok(()))?
            };
            access_reports.preflight_volumes()?;
            let volume = access_reports.first_volume();
            let report = run_volume_task_cancellable(
                volume,
                Priority::Visible,
                "index",
                move |cancellation| {
                    let _root_access =
                        access_reports.root_access_checked(|| cancellation.check())?;
                    let _path_access = if append.is_some() {
                        Some(access_reports.write_accesses_checked(|| cancellation.check())?)
                    } else {
                        None
                    };
                    cancellation.check()?;
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
                    live.apply_metadata_update_cancellable(&path, &cancellation)
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
        "fsevents-repair-schedule" | "fsevents-repair-schedule-retry-probe" => {
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
            let retry_probe = if command == "fsevents-repair-schedule-retry-probe" {
                Some(required_path(
                    args.next(),
                    "fsevents-repair-schedule-retry-probe requires an attempt state path",
                )?)
            } else {
                None
            };
            let dropped_roots: Vec<PathBuf> = args.map(PathBuf::from).collect();
            println!(
                "{}",
                run_fsevents_repair_schedule(
                    state,
                    cursor,
                    observed_event_ids,
                    reason,
                    dropped_roots,
                    retry_probe
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
        |path, cancellation| Indexer::default().scan_progress_cancellable(path, cancellation),
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
    let access_reports =
        FseventsCursorCheckpointAccessReports::for_state_and_cursor(&state, &cursor)?;
    access_reports.preflight_volumes()?;
    let volume = access_reports.first_volume();
    run_volume_task_cancellable(
        volume,
        Priority::Visible,
        "fsevents cursor checkpoint",
        move |cancellation| {
            cancellation.check()?;
            let _state_access = access_reports
                .state
                .access_checked(|| cancellation.check())?;
            let _cursor_access = access_reports
                .cursor
                .access_checked(|| cancellation.check())?;
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
    let access_reports = FseventsCursorResumeAccessReports::for_state_and_cursor(&state, &cursor);
    access_reports.preflight_volumes()?;
    let volume = access_reports.first_volume();
    run_volume_task_cancellable(
        volume,
        Priority::Visible,
        "fsevents cursor resume",
        move |cancellation| {
            cancellation.check()?;
            let _state_access = access_reports
                .state
                .access_checked(|| cancellation.check())?;
            let _cursor_access = access_reports
                .cursor
                .access_checked(|| cancellation.check())?;
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
    retry_probe: Option<PathBuf>,
) -> Result<gfm_index::RepairSchedule> {
    let access_reports =
        FseventsRepairScheduleAccessReports::for_paths(&state, &cursor, &dropped_roots);
    access_reports.preflight_state_and_cursor_volumes()?;
    if let Some(retry_probe) = retry_probe.as_ref() {
        preflight_index_write(retry_probe, "fsevents repair schedule")?;
    }
    let volume = access_reports.first_volume();
    run_retriable_volume_task_cancellable_with_payload_path(
        volume,
        Priority::Visible,
        "fsevents repair schedule",
        cursor.clone(),
        move |cancellation| {
            let state = state.clone();
            let cursor = cursor.clone();
            let observed_event_ids = observed_event_ids.clone();
            let reason = reason.clone();
            let retry_probe = retry_probe.clone();
            let access_reports = access_reports.clone();
            cancellation.check()?;
            if let Some(retry_probe) = retry_probe.as_ref() {
                fail_first_index_retry_probe_attempt(
                    retry_probe,
                    "fsevents repair schedule",
                    &cancellation,
                )?;
            }
            let existing_dropped_root_reports =
                access_reports.existing_dropped_roots_checked(|| cancellation.check())?;
            let existing_dropped_roots = existing_dropped_root_reports
                .iter()
                .map(|entry| entry.path.clone())
                .collect::<Vec<_>>();
            cancellation.check()?;
            let _state_access = access_reports
                .state
                .access_checked(|| cancellation.check())?;
            let _cursor_access = access_reports
                .cursor
                .access_checked(|| cancellation.check())?;
            let _dropped_access = existing_dropped_root_reports
                .iter()
                .map(|root| {
                    cancellation.check()?;
                    root.access_checked(|| cancellation.check())
                })
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

fn fsevents_state_access_report_checked(
    path: &Path,
    worker: &'static str,
    check_control: impl FnMut() -> Result<()>,
) -> Result<IndexPathAccessReport> {
    IndexPathAccessReport::new_checked(
        path.to_path_buf(),
        AccessIntent::Read,
        worker,
        check_control,
    )
}

#[derive(Clone)]
struct FseventsCursorCheckpointAccessReports {
    state: IndexPathAccessReport,
    cursor: IndexPathAccessReport,
}

impl FseventsCursorCheckpointAccessReports {
    fn for_state_and_cursor(state: &Path, cursor: &Path) -> Result<Self> {
        Self::for_state_and_cursor_checked(state, cursor, || Ok(()))
    }

    fn for_state_and_cursor_checked(
        state: &Path,
        cursor: &Path,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        Ok(Self {
            state: fsevents_state_access_report_checked(
                state,
                "fsevents cursor checkpoint state",
                &mut check_control,
            )?,
            cursor: IndexPathAccessReport::write_probe_checked(
                cursor,
                "fsevents cursor checkpoint",
                &mut check_control,
            )?,
        })
    }

    fn preflight_volumes(&self) -> Result<()> {
        self.state.preflight_volume()?;
        self.cursor.preflight_volume()
    }

    fn first_volume(&self) -> Option<gfm_types::VolumeId> {
        first_access_report_volume([&self.state, &self.cursor])
    }
}

#[derive(Clone)]
struct FseventsCursorResumeAccessReports {
    state: IndexPathAccessReport,
    cursor: IndexPathAccessReport,
}

impl FseventsCursorResumeAccessReports {
    fn for_state_and_cursor(state: &Path, cursor: &Path) -> Self {
        Self::for_state_and_cursor_checked(state, cursor, || Ok(()))
            .expect("uncancelled fsevents cursor resume access report cannot be cancelled")
    }

    fn for_state_and_cursor_checked(
        state: &Path,
        cursor: &Path,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        Ok(Self {
            state: fsevents_state_access_report_checked(
                state,
                "fsevents cursor resume state",
                &mut check_control,
            )?,
            cursor: IndexPathAccessReport::new_checked(
                cursor.to_path_buf(),
                AccessIntent::Read,
                "fsevents cursor resume",
                &mut check_control,
            )?,
        })
    }

    fn preflight_volumes(&self) -> Result<()> {
        self.state.preflight_volume()?;
        self.cursor.preflight_volume()
    }

    fn first_volume(&self) -> Option<gfm_types::VolumeId> {
        first_access_report_volume([&self.state, &self.cursor])
    }
}

#[derive(Clone)]
struct FseventsRepairScheduleAccessReports {
    state: IndexPathAccessReport,
    cursor: IndexPathAccessReport,
    dropped_roots: Vec<IndexPathAccessReport>,
}

impl FseventsRepairScheduleAccessReports {
    fn for_paths(state: &Path, cursor: &Path, dropped_roots: &[PathBuf]) -> Self {
        Self::for_paths_checked(state, cursor, dropped_roots, || Ok(()))
            .expect("uncancelled fsevents repair access report cannot be cancelled")
    }

    fn for_paths_checked(
        state: &Path,
        cursor: &Path,
        dropped_roots: &[PathBuf],
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        Ok(Self {
            state: fsevents_state_access_report_checked(
                state,
                "fsevents repair schedule state",
                &mut check_control,
            )?,
            cursor: IndexPathAccessReport::new_checked(
                cursor.to_path_buf(),
                AccessIntent::Read,
                "fsevents repair schedule cursor",
                &mut check_control,
            )?,
            dropped_roots: dropped_roots
                .iter()
                .map(|root| {
                    IndexPathAccessReport::new_checked(
                        root.clone(),
                        AccessIntent::Read,
                        "fsevents repair schedule dropped root",
                        &mut check_control,
                    )
                })
                .collect::<Result<Vec<_>>>()?,
        })
    }

    fn preflight_state_and_cursor_volumes(&self) -> Result<()> {
        self.state.preflight_volume()?;
        self.cursor.preflight_volume()
    }

    fn existing_dropped_roots_checked(
        &self,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Vec<IndexPathAccessReport>> {
        existing_dropped_root_reports_checked(&self.dropped_roots, &mut check_control)
    }

    fn first_volume(&self) -> Option<gfm_types::VolumeId> {
        first_access_report_volume(
            [&self.state, &self.cursor]
                .into_iter()
                .chain(self.dropped_roots.iter()),
        )
    }
}

fn existing_dropped_root_reports_checked(
    dropped_roots: &[IndexPathAccessReport],
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<IndexPathAccessReport>> {
    let mut existing = Vec::new();
    for root in dropped_roots {
        check_control()?;
        root.preflight_volume()?;
        check_control()?;
        if !root.path.try_exists().map_err(|err| {
            GfmError::io(
                &root.path,
                format!("fsevents repair dropped root existence unavailable: {err}"),
            )
        })? {
            continue;
        }
        check_control()?;
        existing.push(root.clone());
    }
    check_control()?;
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
    run_index_read_task_checked(path, worker, read, || Ok(()))
}

fn run_index_read_task_checked<T>(
    path: PathBuf,
    worker: &'static str,
    read: impl FnOnce(PathBuf, &Cancellation) -> Result<T> + Send + 'static,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<T>
where
    T: Send + 'static,
{
    check_control()?;
    let volume_report =
        VolumeDiscoveryReport::for_containing_path_checked(&path, &mut check_control)?;
    check_control()?;
    preflight_volume_access_scope_with_report(&path, AccessIntent::Read, worker, &volume_report)?;
    check_control()?;
    let volume = volume_report.volume_for_path(&path).map(|volume| volume.id);
    run_volume_task_cancellable(volume, Priority::Visible, worker, move |cancellation| {
        cancellation.check()?;
        let _access =
            preflight_index_read_checked_with_volume_report(&path, worker, &volume_report, || {
                cancellation.check()
            })?;
        cancellation.check()?;
        read(path, &cancellation)
    })
}

#[derive(Clone)]
struct IndexPathAccessReport {
    path: PathBuf,
    intent: AccessIntent,
    worker: &'static str,
    volume_report: VolumeDiscoveryReport,
}

impl IndexPathAccessReport {
    fn new_checked(
        path: PathBuf,
        intent: AccessIntent,
        worker: &'static str,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        check_control()?;
        let volume_report =
            VolumeDiscoveryReport::for_containing_path_checked(&path, &mut check_control)?;
        check_control()?;
        Ok(Self {
            path,
            intent,
            worker,
            volume_report,
        })
    }

    fn write_probe_checked(
        path: &Path,
        worker: &'static str,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        check_control()?;
        preflight_write_target_volume_checked(path, worker, &mut check_control)?;
        check_control()?;
        let probe_path = write_probe_path(path)?.to_path_buf();
        check_control()?;
        Self::new_checked(probe_path, AccessIntent::Write, worker, check_control)
    }

    fn preflight_volume(&self) -> Result<()> {
        preflight_volume_access_scope_with_report(
            &self.path,
            self.intent,
            self.worker,
            &self.volume_report,
        )
    }

    fn access_checked(
        &self,
        check_control: impl FnMut() -> Result<()>,
    ) -> Result<ScopedAccessGuard> {
        preflight_access_scope_checked_with_volume_report(
            &self.path,
            self.intent,
            self.worker,
            &self.volume_report,
            check_control,
        )
    }

    fn volume(&self) -> Option<gfm_types::VolumeId> {
        self.volume_report
            .volume_for_path(&self.path)
            .map(|volume| volume.id)
    }
}

fn first_access_report_volume<'a>(
    reports: impl IntoIterator<Item = &'a IndexPathAccessReport>,
) -> Option<gfm_types::VolumeId> {
    reports.into_iter().find_map(IndexPathAccessReport::volume)
}

#[derive(Clone)]
struct IndexBuildAccessReports {
    root: IndexPathAccessReport,
    output: IndexPathAccessReport,
}

#[derive(Clone)]
struct IndexRootWriteAccessReports {
    root: IndexPathAccessReport,
    writes: Vec<IndexPathAccessReport>,
}

#[derive(Clone)]
struct IndexRootReadAccessReports {
    root: IndexPathAccessReport,
    reads: Vec<IndexPathAccessReport>,
}

impl IndexRootReadAccessReports {
    fn for_root_and_reads_checked(
        root: &Path,
        reads: &[PathBuf],
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        Ok(Self {
            root: IndexPathAccessReport::new_checked(
                root.to_path_buf(),
                AccessIntent::Index,
                "index",
                &mut check_control,
            )?,
            reads: reads
                .iter()
                .map(|path| {
                    IndexPathAccessReport::new_checked(
                        path.clone(),
                        AccessIntent::Index,
                        "index",
                        &mut check_control,
                    )
                })
                .collect::<Result<Vec<_>>>()?,
        })
    }

    fn preflight_volumes(&self) -> Result<()> {
        self.root.preflight_volume()?;
        for read in &self.reads {
            read.preflight_volume()?;
        }
        Ok(())
    }

    fn root_access_checked(
        &self,
        check_control: impl FnMut() -> Result<()>,
    ) -> Result<ScopedAccessGuard> {
        self.root.access_checked(check_control)
    }

    fn read_accesses_checked(
        &self,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Vec<ScopedAccessGuard>> {
        let mut guards = Vec::with_capacity(self.reads.len());
        for read in &self.reads {
            check_control()?;
            guards.push(read.access_checked(&mut check_control)?);
        }
        check_control()?;
        Ok(guards)
    }

    fn first_volume(&self) -> Option<gfm_types::VolumeId> {
        first_access_report_volume(std::iter::once(&self.root).chain(self.reads.iter()))
    }
}

impl IndexRootWriteAccessReports {
    fn for_root_and_writes_checked(
        root: &Path,
        writes: &[(&PathBuf, &'static str)],
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        Ok(Self {
            root: IndexPathAccessReport::new_checked(
                root.to_path_buf(),
                AccessIntent::Index,
                "index",
                &mut check_control,
            )?,
            writes: writes
                .iter()
                .map(|(path, worker)| {
                    IndexPathAccessReport::write_probe_checked(path, worker, &mut check_control)
                })
                .collect::<Result<Vec<_>>>()?,
        })
    }

    fn preflight_volumes(&self) -> Result<()> {
        self.root.preflight_volume()?;
        for write in &self.writes {
            write.preflight_volume()?;
        }
        Ok(())
    }

    fn root_access_checked(
        &self,
        check_control: impl FnMut() -> Result<()>,
    ) -> Result<ScopedAccessGuard> {
        self.root.access_checked(check_control)
    }

    fn write_accesses_checked(
        &self,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Vec<ScopedAccessGuard>> {
        let mut guards = Vec::with_capacity(self.writes.len());
        for write in &self.writes {
            check_control()?;
            guards.push(write.access_checked(&mut check_control)?);
        }
        check_control()?;
        Ok(guards)
    }

    fn first_volume(&self) -> Option<gfm_types::VolumeId> {
        first_access_report_volume(std::iter::once(&self.root).chain(self.writes.iter()))
    }
}

impl IndexBuildAccessReports {
    fn for_root_and_output_checked(
        root: &Path,
        output: &Path,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        check_control()?;
        preflight_write_target_volume_checked(output, "index records", &mut check_control)?;
        check_control()?;
        let output_probe = write_probe_path(output)?.to_path_buf();
        check_control()?;
        Ok(Self {
            root: Self::entry_checked(
                root.to_path_buf(),
                AccessIntent::Index,
                "index",
                &mut check_control,
            )?,
            output: Self::entry_checked(
                output_probe,
                AccessIntent::Write,
                "index records",
                &mut check_control,
            )?,
        })
    }

    fn entry_checked(
        path: PathBuf,
        intent: AccessIntent,
        worker: &'static str,
        check_control: impl FnMut() -> Result<()>,
    ) -> Result<IndexPathAccessReport> {
        IndexPathAccessReport::new_checked(path, intent, worker, check_control)
    }

    fn preflight_volumes(&self) -> Result<()> {
        for entry in [&self.root, &self.output] {
            entry.preflight_volume()?;
        }
        Ok(())
    }

    fn enforce_root_access_checked(
        &self,
        check_control: impl FnMut() -> Result<()>,
    ) -> Result<ScopedAccessGuard> {
        self.root.access_checked(check_control)
    }

    fn preflight_output_access_checked(
        &self,
        check_control: impl FnMut() -> Result<()>,
    ) -> Result<ScopedAccessGuard> {
        self.output.access_checked(check_control)
    }

    fn first_volume(&self) -> Option<gfm_types::VolumeId> {
        first_access_report_volume([&self.root, &self.output])
    }
}

fn enforce_index_access(root: &Path) -> Result<ScopedAccessGuard> {
    enforce_index_access_checked(root, || Ok(()))
}

fn enforce_index_access_checked(
    root: &Path,
    check_control: impl FnMut() -> Result<()>,
) -> Result<ScopedAccessGuard> {
    preflight_access_scope_checked(root, AccessIntent::Index, "index", check_control)
}

fn preflight_index_read_checked_with_volume_report(
    path: &Path,
    worker: &str,
    volume_report: &VolumeDiscoveryReport,
    check_control: impl FnMut() -> Result<()>,
) -> Result<ScopedAccessGuard> {
    preflight_access_scope_checked_with_volume_report(
        path,
        AccessIntent::Read,
        worker,
        volume_report,
        check_control,
    )
}

fn preflight_index_write(path: &Path, worker: &str) -> Result<ScopedAccessGuard> {
    preflight_index_write_checked(path, worker, || Ok(()))
}

fn preflight_index_write_checked(
    path: &Path,
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<ScopedAccessGuard> {
    check_control()?;
    preflight_write_target_volume_checked(path, worker, &mut check_control)?;
    check_control()?;
    let probe_path = write_probe_path(path)?;
    check_control()?;
    preflight_access_scope_checked(probe_path, AccessIntent::Write, worker, check_control)
}

fn preflight_write_target_volume_checked(
    path: &Path,
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    check_control()?;
    let volume_path = crate::parent_or_cwd(path);
    let volume_report =
        VolumeDiscoveryReport::for_containing_path_checked(volume_path, &mut check_control)?;
    check_control()?;
    preflight_volume_access_scope_with_report(
        volume_path,
        AccessIntent::Write,
        worker,
        &volume_report,
    )
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

fn fail_first_index_retry_probe_attempt(
    attempt_state: &Path,
    worker: &str,
    cancellation: &Cancellation,
) -> Result<()> {
    cancellation.check()?;
    let _access = preflight_index_write_checked(attempt_state, worker, || cancellation.check())?;
    cancellation.check()?;
    let attempts = read_index_retry_probe_attempt_checked(attempt_state, || cancellation.check())?;
    cancellation.check()?;
    write_index_retry_probe_attempt_checked(attempt_state, attempts + 1, || cancellation.check())?;
    cancellation.check()?;
    if attempts == 0 {
        return Err(GfmError::Format(format!(
            "temporary {worker} retry probe busy"
        )));
    }
    Ok(())
}

fn read_index_retry_probe_attempt_checked(
    path: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<usize> {
    check_control()?;
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(GfmError::io(path, err)),
    };
    check_control()?;
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        check_control()?;
        let read = file
            .read(&mut buffer)
            .map_err(|err| GfmError::io(path, err))?;
        check_control()?;
        if read == 0 {
            break;
        }
        if bytes.len().saturating_add(read) > 4096 {
            return Ok(0);
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    let Ok(value) = std::str::from_utf8(&bytes) else {
        return Ok(0);
    };
    Ok(value.trim().parse::<usize>().unwrap_or(0))
}

fn write_index_retry_probe_attempt_checked(
    path: &Path,
    attempt: usize,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    check_control()?;
    let encoded = attempt.to_string();
    atomic_write_checked(path, &mut check_control, |writer, check_control| {
        for chunk in encoded.as_bytes().chunks(4096) {
            check_control()?;
            writer
                .write_all(chunk)
                .map_err(|err| GfmError::io(path, err))?;
            check_control()?;
        }
        Ok(())
    })?;
    check_control()?;
    Ok(())
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

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn index_read_task_checked_honors_pre_cancelled_control_before_volume_discovery() {
        let path = std::env::temp_dir().join(format!(
            "gfm-index-read-volume-pre-cancel-{}.gfmstate",
            std::process::id()
        ));

        let result = run_index_read_task_checked(
            path.clone(),
            "index read cancellation token",
            |_path, _cancellation| Ok(()),
            || Err(GfmError::Cancelled),
        );

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!path.exists());
    }

    #[test]
    fn enforce_index_access_checked_honors_pre_cancelled_control() {
        let root =
            std::env::temp_dir().join(format!("gfm-index-root-pre-cancel-{}", std::process::id()));

        let result = enforce_index_access_checked(&root, || Err(GfmError::Cancelled));

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!root.exists());
    }

    #[test]
    fn index_build_access_checked_honors_pre_cancelled_control() {
        let root = std::env::temp_dir().join(format!(
            "gfm-index-build-access-pre-cancel-{}",
            std::process::id()
        ));
        let output = root.join("records.gfmidx");
        let reports =
            IndexBuildAccessReports::for_root_and_output_checked(&root, &output, || Ok(()))
                .unwrap();

        let result = reports.enforce_root_access_checked(|| Err(GfmError::Cancelled));

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!root.exists());
    }

    #[test]
    fn index_path_access_report_checked_honors_pre_cancelled_control_before_volume_discovery() {
        let path = std::env::temp_dir()
            .join(format!(
                "gfm-index-path-access-pre-cancel-{}",
                std::process::id()
            ))
            .join("root-that-should-not-be-probed");

        let result = IndexPathAccessReport::new_checked(
            path.clone(),
            AccessIntent::Read,
            "index path access",
            || Err(GfmError::Cancelled),
        );

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!path.exists());
    }

    #[test]
    fn index_path_write_probe_checked_honors_pre_cancelled_control_before_probe() {
        let path = std::env::temp_dir()
            .join(format!(
                "gfm-index-path-write-pre-cancel-{}",
                std::process::id()
            ))
            .join("records.gfmidx");

        let result = IndexPathAccessReport::write_probe_checked(&path, "index write probe", || {
            Err(GfmError::Cancelled)
        });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!path.exists());
    }

    #[test]
    fn index_root_read_access_reports_checked_can_cancel_between_visible_roots() {
        let root = std::env::temp_dir().join(format!(
            "gfm-index-root-read-report-cancel-{}",
            std::process::id()
        ));
        let first = root.join("first");
        let second = root.join("second");
        let visible_roots = vec![first, second];
        let mut checks = 0;

        let result =
            IndexRootReadAccessReports::for_root_and_reads_checked(&root, &visible_roots, || {
                checks += 1;
                if checks > 5 {
                    Err(GfmError::Cancelled)
                } else {
                    Ok(())
                }
            });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!root.exists());
    }

    #[test]
    fn index_root_write_access_reports_checked_can_cancel_before_write_probe() {
        let root = std::env::temp_dir().join(format!(
            "gfm-index-root-write-report-cancel-{}",
            std::process::id()
        ));
        let output = root.join("records.gfmidx");
        let mut checks = 0;

        let result = IndexRootWriteAccessReports::for_root_and_writes_checked(
            &root,
            &[(&output, "index records")],
            || {
                checks += 1;
                if checks > 3 {
                    Err(GfmError::Cancelled)
                } else {
                    Ok(())
                }
            },
        );

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!root.exists());
    }

    #[test]
    fn index_build_access_reports_checked_can_cancel_before_output_probe() {
        let root = std::env::temp_dir().join(format!(
            "gfm-index-build-report-cancel-{}",
            std::process::id()
        ));
        let output = root.join("records.gfmidx");

        let result = IndexBuildAccessReports::for_root_and_output_checked(&root, &output, || {
            Err(GfmError::Cancelled)
        });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!root.exists());
    }

    #[test]
    fn preflight_index_write_checked_can_cancel_before_write_probe() {
        let path = std::env::temp_dir().join(format!(
            "gfm-index-write-pre-cancel-{}.gfmidx",
            std::process::id()
        ));

        let result =
            preflight_index_write_checked(&path, "index records", || Err(GfmError::Cancelled));

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!path.exists());
    }

    #[test]
    fn preflight_index_write_refuses_unreachable_volume_before_write_probe() {
        let root = std::env::temp_dir().join(format!(
            "gfm-index-write-unreachable-before-probe-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
        let path = root.join(format!("{}.gfmidx", "records-unavailable".repeat(16)));

        let err = match preflight_index_write_checked(&path, "index records", || Ok(())) {
            Ok(_) => panic!("unreachable write target was admitted before volume preflight"),
            Err(err) => err,
        };

        assert!(err
            .to_string()
            .contains("index records volume access blocked: unreachable volume network"));
        assert!(!err
            .to_string()
            .contains("index write path metadata unavailable"));
        assert!(!path.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dropped_repair_root_filter_honors_pre_cancelled_control_before_probe() {
        let path = std::env::temp_dir()
            .join(format!("gfm-repair-root-cancel-{}", std::process::id()))
            .join("root-that-should-not-be-probed");
        let report = IndexPathAccessReport::new_checked(
            path,
            AccessIntent::Read,
            "fsevents repair schedule dropped root",
            || Ok(()),
        )
        .unwrap();

        let result = existing_dropped_root_reports_checked(&[report], || Err(GfmError::Cancelled));

        assert_eq!(result.err(), Some(GfmError::Cancelled));
    }

    #[test]
    fn dropped_repair_root_filter_refuses_unreachable_volume_before_existence_probe() {
        let root = std::env::temp_dir().join(format!(
            "gfm-repair-root-filter-unreachable-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
        let report = IndexPathAccessReport::new_checked(
            root.clone(),
            AccessIntent::Read,
            "fsevents repair schedule dropped root",
            || Ok(()),
        )
        .unwrap();

        let err = match existing_dropped_root_reports_checked(&[report], || Ok(())) {
            Ok(_) => panic!("unreachable dropped root was admitted before volume preflight"),
            Err(err) => err,
        };
        assert!(err
            .to_string()
            .contains("fsevents repair schedule dropped root volume access blocked"));
        assert!(err.to_string().contains("unreachable volume network"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dropped_repair_root_filter_can_cancel_between_roots() {
        let root = std::env::temp_dir().join(format!(
            "gfm-repair-root-filter-cancel-{}",
            std::process::id()
        ));
        let first = root.join("first");
        let second = root.join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        let reports = [first, second]
            .into_iter()
            .map(|path| {
                IndexPathAccessReport::new_checked(
                    path,
                    AccessIntent::Read,
                    "fsevents repair schedule dropped root",
                    || Ok(()),
                )
            })
            .collect::<Result<Vec<_>>>()
            .unwrap();
        let mut checks = 0;

        let result = existing_dropped_root_reports_checked(&reports, || {
            checks += 1;
            if checks > 3 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        fs::remove_dir_all(root).unwrap();
    }
}
