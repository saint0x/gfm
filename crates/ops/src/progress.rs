use crate::{OperationCancellation, OperationPause};
use gfm_types::Result;
use std::fs;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationProgressPhase {
    Planned,
    Advanced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OperationProgress {
    pub total_items: u64,
    pub total_bytes: u64,
    pub completed_items: u64,
    pub completed_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationProgressEvent {
    pub phase: OperationProgressPhase,
    pub progress: OperationProgress,
    pub throughput: Option<OperationThroughputSnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationThroughputClass {
    FullSpeed,
    Constrained,
    Slow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationThroughputSnapshot {
    pub bytes_per_second: u64,
    pub class: OperationThroughputClass,
}

impl OperationThroughputSnapshot {
    const CONSTRAINED_BYTES_PER_SECOND: u64 = 96 * 1024 * 1024;
    const SLOW_BYTES_PER_SECOND: u64 = 16 * 1024 * 1024;

    pub fn classify(bytes: u64, elapsed_nanos: u128) -> Option<Self> {
        if bytes == 0 {
            return None;
        }
        let elapsed_nanos = elapsed_nanos.max(1);
        let bytes_per_second =
            ((bytes as u128) * 1_000_000_000 / elapsed_nanos).min(u64::MAX as u128) as u64;
        let class = if bytes_per_second < Self::SLOW_BYTES_PER_SECOND {
            OperationThroughputClass::Slow
        } else if bytes_per_second < Self::CONSTRAINED_BYTES_PER_SECOND {
            OperationThroughputClass::Constrained
        } else {
            OperationThroughputClass::FullSpeed
        };
        Some(Self {
            bytes_per_second,
            class,
        })
    }
}

pub(crate) struct ProgressTracker<'a, F: FnMut(OperationProgressEvent)> {
    progress: OperationProgress,
    cancellation: &'a OperationCancellation,
    pause: &'a OperationPause,
    on_progress: &'a mut F,
    started_at: Instant,
}

impl<'a, F: FnMut(OperationProgressEvent)> ProgressTracker<'a, F> {
    pub(crate) fn new(
        plan: OperationProgress,
        cancellation: &'a OperationCancellation,
        pause: &'a OperationPause,
        on_progress: &'a mut F,
    ) -> Self {
        let mut tracker = Self {
            progress: plan,
            cancellation,
            pause,
            on_progress,
            started_at: Instant::now(),
        };
        tracker.emit(OperationProgressPhase::Planned);
        tracker
    }

    pub(crate) fn advance(&mut self, metadata: &fs::Metadata) -> Result<()> {
        self.check_control()?;
        self.progress.completed_items += 1;
        self.progress.completed_bytes += item_bytes(metadata);
        self.emit(OperationProgressPhase::Advanced);
        self.check_control()
    }

    pub(crate) fn advance_bytes(&mut self, bytes: u64) -> Result<()> {
        self.check_control()?;
        self.progress.completed_bytes =
            (self.progress.completed_bytes + bytes).min(self.progress.total_bytes);
        self.emit(OperationProgressPhase::Advanced);
        self.check_control()
    }

    pub(crate) fn finish_current_item(&mut self) -> Result<()> {
        self.check_control()?;
        self.progress.completed_items += 1;
        self.emit(OperationProgressPhase::Advanced);
        self.check_control()
    }

    pub(crate) fn complete(&mut self) -> Result<()> {
        self.check_control()?;
        self.progress.completed_items = self.progress.total_items;
        self.progress.completed_bytes = self.progress.total_bytes;
        self.emit(OperationProgressPhase::Advanced);
        self.check_control()
    }

    fn emit(&mut self, phase: OperationProgressPhase) {
        let throughput = self.throughput_snapshot(phase);
        (self.on_progress)(OperationProgressEvent {
            phase,
            progress: self.progress,
            throughput,
        });
    }

    fn throughput_snapshot(
        &self,
        phase: OperationProgressPhase,
    ) -> Option<OperationThroughputSnapshot> {
        if phase != OperationProgressPhase::Advanced {
            return None;
        }
        OperationThroughputSnapshot::classify(
            self.progress.completed_bytes,
            self.started_at.elapsed().as_nanos(),
        )
    }

    pub(crate) fn check_cancelled(&self) -> Result<()> {
        self.check_control()
    }

    fn check_control(&self) -> Result<()> {
        self.cancellation.check().and_then(|()| self.pause.check())
    }
}

pub(crate) fn item_bytes(metadata: &fs::Metadata) -> u64 {
    if metadata.is_file() {
        metadata.len()
    } else {
        0
    }
}
