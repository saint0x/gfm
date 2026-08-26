use gfm_types::{FileEvent, FileEventKind};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventPriority {
    Visible,
    Background,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventBackpressureReport {
    pub accepted: bool,
    pub coalesced: bool,
    pub dropped: usize,
    pub repair_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventBackpressureSnapshot {
    pub pending_visible: usize,
    pub pending_background: usize,
    pub dropped: usize,
    pub coalesced: usize,
    pub repair_required: bool,
}

impl EventBackpressureSnapshot {
    pub fn as_tsv(&self) -> String {
        format!(
            "event-backpressure\tvisible={}\tbackground={}\tdropped={}\tcoalesced={}\trepair-required={}",
            self.pending_visible,
            self.pending_background,
            self.dropped,
            self.coalesced,
            self.repair_required
        )
    }
}

#[derive(Debug, Clone)]
pub struct EventBackpressureQueue {
    max_pending: usize,
    visible_burst: usize,
    visible: VecDeque<FileEvent>,
    background: VecDeque<FileEvent>,
    dropped: usize,
    coalesced: usize,
    repair_required: bool,
}

impl EventBackpressureQueue {
    pub fn new(max_pending: usize, visible_burst: usize) -> Self {
        Self {
            max_pending: max_pending.max(1),
            visible_burst: visible_burst.max(1),
            visible: VecDeque::new(),
            background: VecDeque::new(),
            dropped: 0,
            coalesced: 0,
            repair_required: false,
        }
    }

    pub fn enqueue(
        &mut self,
        priority: EventPriority,
        event: FileEvent,
    ) -> EventBackpressureReport {
        if matches!(event.kind, FileEventKind::Rescan) {
            let dropped = self.visible.len() + self.background.len();
            self.visible.clear();
            self.background.clear();
            self.background.push_back(event);
            self.dropped += dropped;
            self.repair_required = true;
            return EventBackpressureReport {
                accepted: true,
                coalesced: false,
                dropped,
                repair_required: true,
            };
        }

        if self.coalesce(priority, &event) {
            self.coalesced += 1;
            return EventBackpressureReport {
                accepted: true,
                coalesced: true,
                dropped: 0,
                repair_required: self.repair_required,
            };
        }

        let dropped = self.make_room(priority);
        if self.pending() >= self.max_pending {
            self.dropped += 1;
            self.repair_required = true;
            return EventBackpressureReport {
                accepted: false,
                coalesced: false,
                dropped: dropped + 1,
                repair_required: true,
            };
        }

        queue_for(&mut self.visible, &mut self.background, priority).push_back(event);
        EventBackpressureReport {
            accepted: true,
            coalesced: false,
            dropped,
            repair_required: self.repair_required,
        }
    }

    pub fn drain_batch(&mut self, max: usize) -> Vec<FileEvent> {
        let mut drained = Vec::new();
        let mut visible_credit = 0usize;
        let max = max.min(self.pending());
        while drained.len() < max {
            if visible_credit < self.visible_burst {
                if let Some(event) = self.visible.pop_front() {
                    visible_credit += 1;
                    drained.push(event);
                    continue;
                }
            }

            if let Some(event) = self.background.pop_front() {
                visible_credit = 0;
                drained.push(event);
                continue;
            }

            if let Some(event) = self.visible.pop_front() {
                visible_credit += 1;
                drained.push(event);
            } else {
                break;
            }
        }
        drained
    }

    pub fn snapshot(&self) -> EventBackpressureSnapshot {
        EventBackpressureSnapshot {
            pending_visible: self.visible.len(),
            pending_background: self.background.len(),
            dropped: self.dropped,
            coalesced: self.coalesced,
            repair_required: self.repair_required,
        }
    }

    fn coalesce(&mut self, priority: EventPriority, event: &FileEvent) -> bool {
        let queue = queue_for(&mut self.visible, &mut self.background, priority);
        if let Some(existing) = queue
            .iter_mut()
            .rev()
            .find(|existing| coalesce_key(existing) == coalesce_key(event))
        {
            *existing = coalesced_event(existing, event);
            return true;
        }
        false
    }

    fn make_room(&mut self, priority: EventPriority) -> usize {
        if self.pending() < self.max_pending {
            return 0;
        }

        if self.background.pop_front().is_some() {
            self.dropped += 1;
            self.repair_required = true;
            return 1;
        }

        if priority == EventPriority::Background {
            return 0;
        }

        if self.visible.len() > self.visible_burst && self.visible.pop_front().is_some() {
            self.dropped += 1;
            self.repair_required = true;
            return 1;
        }
        0
    }

    fn pending(&self) -> usize {
        self.visible.len() + self.background.len()
    }
}

fn queue_for<'a>(
    visible: &'a mut VecDeque<FileEvent>,
    background: &'a mut VecDeque<FileEvent>,
    priority: EventPriority,
) -> &'a mut VecDeque<FileEvent> {
    match priority {
        EventPriority::Visible => visible,
        EventPriority::Background => background,
    }
}

fn coalesce_key(event: &FileEvent) -> PathBuf {
    match &event.kind {
        FileEventKind::Rename { from, to } => rename_key(from, to),
        _ => event.path.clone(),
    }
}

fn rename_key(from: &Path, to: &Path) -> PathBuf {
    let mut key = from.to_path_buf();
    key.push("->");
    key.push(to);
    key
}

fn coalesced_event(existing: &FileEvent, incoming: &FileEvent) -> FileEvent {
    let kind = match (&existing.kind, &incoming.kind) {
        (FileEventKind::Create, FileEventKind::Metadata | FileEventKind::Modify) => {
            FileEventKind::Create
        }
        (FileEventKind::Metadata, FileEventKind::Modify)
        | (FileEventKind::Modify, FileEventKind::Metadata) => FileEventKind::Modify,
        (_, FileEventKind::Remove) => FileEventKind::Remove,
        _ => incoming.kind.clone(),
    };
    FileEvent {
        path: incoming.path.clone(),
        kind,
        observed_at: incoming.observed_at,
    }
}
