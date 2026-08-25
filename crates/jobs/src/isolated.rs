use crate::{Job, RetriableTask, Task, VolumeConcurrencyPolicy};
use gfm_types::VolumeId;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex};

pub(crate) struct IsolatedTaskQueue {
    state: Mutex<IsolatedTaskQueueState>,
    wake: Condvar,
    policy: VolumeConcurrencyPolicy,
}

impl IsolatedTaskQueue {
    pub(crate) fn new(tasks: Vec<Task>, policy: VolumeConcurrencyPolicy) -> Self {
        Self {
            state: Mutex::new(IsolatedTaskQueueState {
                pending: VecDeque::from(tasks),
                active_by_volume: HashMap::new(),
            }),
            wake: Condvar::new(),
            policy,
        }
    }

    pub(crate) fn next(self: &Arc<Self>) -> Option<TaskLease> {
        let mut state = self.state.lock().expect("isolated task queue poisoned");
        loop {
            if let Some((index, volume)) = state.next_admissible(&self.policy) {
                let task = state
                    .pending
                    .remove(index)
                    .expect("admissible task vanished");
                if let Some(volume) = volume {
                    *state.active_by_volume.entry(volume).or_insert(0) += 1;
                }
                return Some(TaskLease {
                    queue: Arc::clone(self),
                    task,
                    volume,
                    finished: false,
                });
            }
            if state.pending.is_empty() {
                return None;
            }
            state = self
                .wake
                .wait(state)
                .expect("isolated task queue poisoned while waiting");
        }
    }

    fn release(&self, volume: Option<VolumeId>) {
        let mut state = self.state.lock().expect("isolated task queue poisoned");
        if let Some(volume) = volume {
            let active = state
                .active_by_volume
                .get_mut(&volume)
                .expect("volume lease released without active count");
            *active -= 1;
            if *active == 0 {
                state.active_by_volume.remove(&volume);
            }
        }
        self.wake.notify_all();
    }
}

struct IsolatedTaskQueueState {
    pending: VecDeque<Task>,
    active_by_volume: HashMap<VolumeId, usize>,
}

impl IsolatedTaskQueueState {
    fn next_admissible(
        &self,
        policy: &VolumeConcurrencyPolicy,
    ) -> Option<(usize, Option<VolumeId>)> {
        self.pending
            .iter()
            .enumerate()
            .find_map(|(index, task)| match task.job.volume {
                Some(volume)
                    if self.active_by_volume.get(&volume).copied().unwrap_or(0)
                        < policy.limit_for(volume) =>
                {
                    Some((index, Some(volume)))
                }
                Some(_) => None,
                None => Some((index, None)),
            })
    }
}

pub(crate) struct TaskLease {
    pub(crate) task: Task,
    queue: Arc<IsolatedTaskQueue>,
    volume: Option<VolumeId>,
    finished: bool,
}

impl TaskLease {
    pub(crate) fn finish(mut self) -> Job {
        self.finished = true;
        self.queue.release(self.volume);
        self.task.job.clone()
    }
}

impl Drop for TaskLease {
    fn drop(&mut self) {
        if !self.finished {
            self.queue.release(self.volume);
        }
    }
}

pub(crate) struct IsolatedRetriableTaskQueue {
    state: Mutex<IsolatedRetriableTaskQueueState>,
    wake: Condvar,
    policy: VolumeConcurrencyPolicy,
}

impl IsolatedRetriableTaskQueue {
    pub(crate) fn new(tasks: Vec<RetriableTask>, policy: VolumeConcurrencyPolicy) -> Self {
        Self {
            state: Mutex::new(IsolatedRetriableTaskQueueState {
                pending: VecDeque::from(tasks),
                active_by_volume: HashMap::new(),
            }),
            wake: Condvar::new(),
            policy,
        }
    }

    pub(crate) fn next(self: &Arc<Self>) -> Option<RetriableTaskLease> {
        let mut state = self
            .state
            .lock()
            .expect("isolated retriable task queue poisoned");
        loop {
            if let Some((index, volume)) = state.next_admissible(&self.policy) {
                let task = state
                    .pending
                    .remove(index)
                    .expect("admissible retriable task vanished");
                if let Some(volume) = volume {
                    *state.active_by_volume.entry(volume).or_insert(0) += 1;
                }
                return Some(RetriableTaskLease {
                    queue: Arc::clone(self),
                    task,
                    volume,
                    finished: false,
                });
            }
            if state.pending.is_empty() {
                return None;
            }
            state = self
                .wake
                .wait(state)
                .expect("isolated retriable task queue poisoned while waiting");
        }
    }

    fn release(&self, volume: Option<VolumeId>) {
        let mut state = self
            .state
            .lock()
            .expect("isolated retriable task queue poisoned");
        if let Some(volume) = volume {
            let active = state
                .active_by_volume
                .get_mut(&volume)
                .expect("volume lease released without active count");
            *active -= 1;
            if *active == 0 {
                state.active_by_volume.remove(&volume);
            }
        }
        self.wake.notify_all();
    }
}

struct IsolatedRetriableTaskQueueState {
    pending: VecDeque<RetriableTask>,
    active_by_volume: HashMap<VolumeId, usize>,
}

impl IsolatedRetriableTaskQueueState {
    fn next_admissible(
        &self,
        policy: &VolumeConcurrencyPolicy,
    ) -> Option<(usize, Option<VolumeId>)> {
        self.pending
            .iter()
            .enumerate()
            .find_map(|(index, task)| match task.job.volume {
                Some(volume)
                    if self.active_by_volume.get(&volume).copied().unwrap_or(0)
                        < policy.limit_for(volume) =>
                {
                    Some((index, Some(volume)))
                }
                Some(_) => None,
                None => Some((index, None)),
            })
    }
}

pub(crate) struct RetriableTaskLease {
    pub(crate) task: RetriableTask,
    queue: Arc<IsolatedRetriableTaskQueue>,
    volume: Option<VolumeId>,
    finished: bool,
}

impl RetriableTaskLease {
    pub(crate) fn finish(mut self) -> Job {
        self.finished = true;
        self.queue.release(self.volume);
        self.task.job.clone()
    }
}

impl Drop for RetriableTaskLease {
    fn drop(&mut self) {
        if !self.finished {
            self.queue.release(self.volume);
        }
    }
}
