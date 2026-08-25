use gfm_types::{FileEvent, FileEventKind, GfmError, Result};
use notify::event::{CreateKind, ModifyKind, RemoveKind, RenameMode};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchDepth {
    Directory,
    Tree,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchRoot {
    pub path: PathBuf,
    pub depth: WatchDepth,
}

impl WatchRoot {
    pub fn tree(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            depth: WatchDepth::Tree,
        }
    }

    pub fn directory(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            depth: WatchDepth::Directory,
        }
    }
}

pub struct FileEventStream {
    _watcher: RecommendedWatcher,
    receiver: Receiver<Result<FileEvent>>,
}

impl FileEventStream {
    pub fn watch(roots: &[WatchRoot]) -> Result<Self> {
        let (tx, rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |event| {
            for mapped in map_notify_result(event) {
                let _ = tx.send(mapped);
            }
        })
        .map_err(|err| GfmError::Format(format!("failed to create watcher: {err}")))?;

        for root in roots {
            watcher
                .watch(&root.path, recursive_mode(root.depth))
                .map_err(|err| GfmError::io(&root.path, err))?;
        }

        Ok(Self {
            _watcher: watcher,
            receiver: rx,
        })
    }

    pub fn recv(&self) -> Result<FileEvent> {
        self.receiver
            .recv()
            .map_err(|err| GfmError::Format(format!("file event stream closed: {err}")))?
    }

    pub fn try_recv(&self) -> Option<Result<FileEvent>> {
        self.receiver.try_recv().ok()
    }
}

pub fn map_notify_event(event: Event) -> Vec<FileEvent> {
    if is_rescan(&event) {
        return paths_or_current(event.paths)
            .into_iter()
            .map(|path| FileEvent::new(path, FileEventKind::Rescan))
            .collect();
    }

    match event.kind {
        EventKind::Create(
            CreateKind::File | CreateKind::Folder | CreateKind::Any | CreateKind::Other,
        ) => event
            .paths
            .into_iter()
            .map(|path| FileEvent::new(path, FileEventKind::Create))
            .collect(),
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) if event.paths.len() >= 2 => {
            vec![FileEvent::new(
                event.paths[1].clone(),
                FileEventKind::Rename {
                    from: event.paths[0].clone(),
                    to: event.paths[1].clone(),
                },
            )]
        }
        EventKind::Modify(ModifyKind::Name(RenameMode::From)) => event
            .paths
            .into_iter()
            .map(|path| FileEvent::new(path, FileEventKind::Remove))
            .collect(),
        EventKind::Modify(ModifyKind::Name(RenameMode::To)) => event
            .paths
            .into_iter()
            .map(|path| FileEvent::new(path, FileEventKind::Create))
            .collect(),
        EventKind::Modify(_) => event
            .paths
            .into_iter()
            .map(|path| FileEvent::new(path, FileEventKind::Modify))
            .collect(),
        EventKind::Remove(
            RemoveKind::File | RemoveKind::Folder | RemoveKind::Any | RemoveKind::Other,
        ) => event
            .paths
            .into_iter()
            .map(|path| FileEvent::new(path, FileEventKind::Remove))
            .collect(),
        EventKind::Any | EventKind::Other => event
            .paths
            .into_iter()
            .map(|path| FileEvent::new(path, FileEventKind::Other))
            .collect(),
        EventKind::Access(_) => Vec::new(),
    }
}

fn map_notify_result(event: notify::Result<Event>) -> Vec<Result<FileEvent>> {
    match event {
        Ok(event) => map_notify_event(event).into_iter().map(Ok).collect(),
        Err(err) => vec![Err(GfmError::Format(format!("watcher event error: {err}")))],
    }
}

fn recursive_mode(depth: WatchDepth) -> RecursiveMode {
    match depth {
        WatchDepth::Directory => RecursiveMode::NonRecursive,
        WatchDepth::Tree => RecursiveMode::Recursive,
    }
}

fn is_rescan(event: &Event) -> bool {
    event.need_rescan()
}

fn paths_or_current(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    if paths.is_empty() {
        vec![Path::new(".").to_path_buf()]
    } else {
        paths
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{DataChange, ModifyKind};
    use std::fs;
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    #[test]
    fn maps_rename_pair_to_single_domain_event() {
        let event = Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
            .add_path(PathBuf::from("/tmp/from.txt"))
            .add_path(PathBuf::from("/tmp/to.txt"));

        let mapped = map_notify_event(event);

        assert_eq!(mapped.len(), 1);
        assert_eq!(
            mapped[0].kind,
            FileEventKind::Rename {
                from: PathBuf::from("/tmp/from.txt"),
                to: PathBuf::from("/tmp/to.txt")
            }
        );
        assert_eq!(mapped[0].path, PathBuf::from("/tmp/to.txt"));
    }

    #[test]
    fn maps_data_change_to_modify_events() {
        let event = Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Content)))
            .add_path(PathBuf::from("/tmp/file.txt"));

        let mapped = map_notify_event(event);

        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].kind, FileEventKind::Modify);
    }

    #[test]
    fn native_watcher_observes_real_file_mutation() {
        let root = unique_temp_dir("gfm-watch-root");
        let stream = FileEventStream::watch(&[WatchRoot::tree(&root)]).unwrap();
        let target = root.join("created.txt");
        fs::write(&target, "hello").unwrap();

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut observed = Vec::new();
        while Instant::now() < deadline {
            if let Some(event) = stream.try_recv() {
                let event = event.unwrap();
                observed.push(event.clone());
                if event.path == target || event.path == root {
                    fs::remove_dir_all(root).unwrap();
                    return;
                }
            }
            thread::sleep(Duration::from_millis(25));
        }

        fs::remove_dir_all(root).unwrap();
        panic!("watcher did not observe mutation; observed events: {observed:?}");
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "{}-{}",
            prefix,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
