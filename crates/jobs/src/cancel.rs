use gfm_types::{GfmError, Result};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, Weak};

#[derive(Debug, Clone)]
pub struct Cancellation(Arc<CancellationInner>);

impl Cancellation {
    pub fn cancel(&self) {
        self.0.cancelled.store(true, AtomicOrdering::SeqCst);
        let children = {
            let mut children = self
                .0
                .children
                .lock()
                .expect("cancellation children poisoned");
            let live = children
                .iter()
                .filter_map(Weak::upgrade)
                .collect::<Vec<_>>();
            children.retain(|child| child.strong_count() > 0);
            live
        };
        for child in children {
            Self(child).cancel();
        }
    }

    pub fn child(&self) -> Self {
        let child = Self(Arc::new(CancellationInner {
            cancelled: AtomicBool::new(false),
            parent: Some(self.clone()),
            children: Mutex::new(Vec::new()),
        }));
        self.0
            .children
            .lock()
            .expect("cancellation children poisoned")
            .push(Arc::downgrade(&child.0));
        child
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(AtomicOrdering::SeqCst)
            || self
                .0
                .parent
                .as_ref()
                .is_some_and(Cancellation::is_cancelled)
    }

    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    }
}

impl Default for Cancellation {
    fn default() -> Self {
        Self(Arc::new(CancellationInner {
            cancelled: AtomicBool::new(false),
            parent: None,
            children: Mutex::new(Vec::new()),
        }))
    }
}

#[derive(Debug)]
struct CancellationInner {
    cancelled: AtomicBool,
    parent: Option<Cancellation>,
    children: Mutex<Vec<Weak<CancellationInner>>>,
}
