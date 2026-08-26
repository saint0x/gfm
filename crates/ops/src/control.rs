use gfm_types::{GfmError, Result};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Arc;

#[derive(Debug, Clone, Default)]
pub struct OperationCancellation(Arc<AtomicBool>);

impl OperationCancellation {
    pub fn cancel(&self) {
        self.0.store(true, AtomicOrdering::SeqCst);
    }

    pub fn check(&self) -> Result<()> {
        if self.0.load(AtomicOrdering::SeqCst) {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct OperationPause(Arc<AtomicBool>);

impl OperationPause {
    pub fn pause(&self) {
        self.0.store(true, AtomicOrdering::SeqCst);
    }

    pub fn check(&self) -> Result<()> {
        if self.0.load(AtomicOrdering::SeqCst) {
            Err(GfmError::Paused)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_control_flags_allow_progress() {
        OperationCancellation::default().check().unwrap();
        OperationPause::default().check().unwrap();
    }

    #[test]
    fn cancellation_reports_cancelled() {
        let cancellation = OperationCancellation::default();
        cancellation.cancel();

        assert!(matches!(cancellation.check(), Err(GfmError::Cancelled)));
    }

    #[test]
    fn pause_reports_paused() {
        let pause = OperationPause::default();
        pause.pause();

        assert!(matches!(pause.check(), Err(GfmError::Paused)));
    }
}
