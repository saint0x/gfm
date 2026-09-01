use gfm_types::{GfmError, Result};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, Weak};

const CHILD_PRUNE_FLOOR: usize = 1024;

#[derive(Debug, Clone)]
pub struct Cancellation(Arc<CancellationInner>);

impl Cancellation {
    pub fn cancel(&self) {
        let mut stack = cancellation_children_to_cancel(Arc::clone(&self.0));
        while let Some(child) = stack.pop() {
            stack.extend(cancellation_children_to_cancel(child));
        }
    }

    pub fn child(&self) -> Self {
        let child = Self(Arc::new(CancellationInner {
            cancelled: AtomicBool::new(self.is_cancelled()),
            parent: Some(Arc::downgrade(&self.0)),
            children: Mutex::new(CancellationChildren::default()),
        }));
        let mut children = self
            .0
            .children
            .lock()
            .expect("cancellation children poisoned");
        children.prune_if_needed();
        children.links.push(Arc::downgrade(&child.0));
        child
    }

    pub fn is_cancelled(&self) -> bool {
        if self.0.cancelled.load(AtomicOrdering::SeqCst) {
            return true;
        }
        let mut current = self.0.parent.as_ref().and_then(Weak::upgrade);
        let mut observed: Vec<Arc<CancellationInner>> = Vec::new();
        while let Some(parent) = current {
            if parent.cancelled.load(AtomicOrdering::SeqCst) {
                self.0.cancelled.store(true, AtomicOrdering::SeqCst);
                for token in observed {
                    token.cancelled.store(true, AtomicOrdering::SeqCst);
                }
                return true;
            }
            current = parent.parent.as_ref().and_then(Weak::upgrade);
            observed.push(parent);
        }
        false
    }

    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    }

    #[cfg(test)]
    pub(crate) fn child_link_count_for_tests(&self) -> usize {
        self.0
            .children
            .lock()
            .expect("cancellation children poisoned")
            .links
            .len()
    }
}

fn cancellation_children_to_cancel(inner: Arc<CancellationInner>) -> Vec<Arc<CancellationInner>> {
    inner.cancelled.store(true, AtomicOrdering::SeqCst);
    let mut children = inner
        .children
        .lock()
        .expect("cancellation children poisoned");
    let live = children
        .links
        .iter()
        .filter_map(Weak::upgrade)
        .collect::<Vec<_>>();
    children.links.retain(|child| child.strong_count() > 0);
    children.reset_prune_threshold();
    live
}

impl Default for Cancellation {
    fn default() -> Self {
        Self(Arc::new(CancellationInner {
            cancelled: AtomicBool::new(false),
            parent: None,
            children: Mutex::new(CancellationChildren::default()),
        }))
    }
}

#[derive(Debug)]
struct CancellationInner {
    cancelled: AtomicBool,
    parent: Option<Weak<CancellationInner>>,
    children: Mutex<CancellationChildren>,
}

#[derive(Debug)]
struct CancellationChildren {
    links: Vec<Weak<CancellationInner>>,
    next_prune_len: usize,
}

impl CancellationChildren {
    fn prune_if_needed(&mut self) {
        if self.links.len() < self.next_prune_len {
            return;
        }
        self.links.retain(|child| child.strong_count() > 0);
        self.reset_prune_threshold();
    }

    fn reset_prune_threshold(&mut self) {
        self.next_prune_len = self.links.len().saturating_add(1).saturating_mul(2);
        self.next_prune_len = self.next_prune_len.max(CHILD_PRUNE_FLOOR);
    }
}

impl Default for CancellationChildren {
    fn default() -> Self {
        Self {
            links: Vec::new(),
            next_prune_len: CHILD_PRUNE_FLOOR,
        }
    }
}
