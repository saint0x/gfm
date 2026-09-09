use crate::{
    Job, JobClass, JobFairnessPolicy, RetriableTask, Task, TaskOutcome, TaskStatus,
    VolumeConcurrencyPolicy,
};
use gfm_types::VolumeId;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex};

pub(crate) struct IsolatedTaskQueue {
    state: Mutex<IsolatedTaskQueueState>,
    wake: Condvar,
    volume_policy: VolumeConcurrencyPolicy,
    admission_order: Vec<JobClass>,
}

impl IsolatedTaskQueue {
    pub(crate) fn new(tasks: Vec<Task>, policy: VolumeConcurrencyPolicy) -> Self {
        Self {
            state: Mutex::new(IsolatedTaskQueueState {
                pending: VecDeque::from(tasks),
                active_by_volume: HashMap::new(),
                active: HashSet::new(),
                completed: HashSet::new(),
                failed: HashSet::new(),
                next_admission_class: 0,
            }),
            wake: Condvar::new(),
            volume_policy: policy,
            admission_order: JobFairnessPolicy::default().admission_order(),
        }
    }

    pub(crate) fn next(self: &Arc<Self>) -> Option<TaskLeaseResult> {
        let mut state = self.state.lock().expect("isolated task queue poisoned");
        loop {
            if let Some((index, volume)) =
                state.next_admissible(&self.admission_order, &self.volume_policy)
            {
                let task = state
                    .pending
                    .remove(index)
                    .expect("admissible task vanished");
                state.active.insert(task.job.id);
                if let Some(volume) = volume {
                    *state.active_by_volume.entry(volume).or_insert(0) += 1;
                }
                return Some(TaskLeaseResult::Run(TaskLease {
                    queue: Arc::clone(self),
                    task,
                    volume,
                    finished: false,
                }));
            }
            if let Some(outcome) = state.remove_dependency_blocked() {
                return Some(TaskLeaseResult::Blocked(outcome));
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

    fn release(&self, job: Job, volume: Option<VolumeId>, status: &TaskStatus) {
        let mut state = self.state.lock().expect("isolated task queue poisoned");
        state.active.remove(&job.id);
        match status {
            TaskStatus::Completed => {
                state.failed.remove(&job.id);
                state.completed.insert(job.id);
            }
            TaskStatus::Started => {}
            TaskStatus::Cancelled | TaskStatus::Failed(_) => {
                state.completed.remove(&job.id);
                state.failed.insert(job.id);
            }
        }
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
    active: HashSet<crate::JobId>,
    completed: HashSet<crate::JobId>,
    failed: HashSet<crate::JobId>,
    next_admission_class: usize,
}

impl IsolatedTaskQueueState {
    fn next_admissible(
        &mut self,
        admission_order: &[JobClass],
        volume_policy: &VolumeConcurrencyPolicy,
    ) -> Option<(usize, Option<VolumeId>)> {
        if admission_order.is_empty() {
            return None;
        }
        for offset in 0..admission_order.len() {
            let cursor = (self.next_admission_class + offset) % admission_order.len();
            let class = admission_order[cursor];
            if let Some(admission) = self.pending.iter().enumerate().find_map(|(index, task)| {
                if task.job.class != class || !self.dependencies_completed(&task.job) {
                    return None;
                }
                self.admissible_volume(&task.job, volume_policy)
                    .map(|volume| (index, volume))
            }) {
                self.next_admission_class = (cursor + 1) % admission_order.len();
                return Some(admission);
            }
        }
        None
    }

    fn remove_dependency_blocked(&mut self) -> Option<TaskOutcome> {
        let known_pending = self
            .pending
            .iter()
            .map(|task| task.job.id)
            .chain(self.active.iter().copied())
            .chain(self.completed.iter().copied())
            .chain(self.failed.iter().copied())
            .collect::<HashSet<_>>();
        let index = self.pending.iter().position(|task| {
            task.job.dependencies.iter().any(|dependency| {
                self.failed.contains(dependency) || !known_pending.contains(dependency)
            })
        });
        let index = match index {
            Some(index) => index,
            None if self.active.is_empty() => self.pending.iter().position(|task| {
                task.job
                    .dependencies
                    .iter()
                    .any(|dependency| !self.completed.contains(dependency))
            })?,
            None => return None,
        };
        let task = self
            .pending
            .remove(index)
            .expect("dependency-blocked task vanished");
        self.failed.insert(task.job.id);
        let (missing, failed) = dependency_block_detail(&task.job, &self.completed, &self.failed);
        Some(TaskOutcome {
            id: task.job.id,
            label: task.job.label,
            status: TaskStatus::Failed(format!(
                "job dependencies were not satisfied missing={} failed={}",
                format_dependency_ids(&missing),
                format_dependency_ids(&failed)
            )),
        })
    }

    fn dependencies_completed(&self, job: &Job) -> bool {
        job.dependencies
            .iter()
            .all(|dependency| self.completed.contains(dependency))
    }

    fn admissible_volume(
        &self,
        job: &Job,
        policy: &VolumeConcurrencyPolicy,
    ) -> Option<Option<VolumeId>> {
        match job.volume {
            Some(volume)
                if self.active_by_volume.get(&volume).copied().unwrap_or(0)
                    < policy.limit_for(volume) =>
            {
                Some(Some(volume))
            }
            Some(_) => None,
            None => Some(None),
        }
    }
}

pub(crate) enum TaskLeaseResult {
    Run(TaskLease),
    Blocked(TaskOutcome),
}

pub(crate) struct TaskLease {
    pub(crate) task: Task,
    queue: Arc<IsolatedTaskQueue>,
    volume: Option<VolumeId>,
    finished: bool,
}

impl TaskLease {
    pub(crate) fn finish(mut self, status: &TaskStatus) -> Job {
        self.finished = true;
        let job = self.task.job.clone();
        self.queue.release(job.clone(), self.volume, status);
        job
    }
}

impl Drop for TaskLease {
    fn drop(&mut self) {
        if !self.finished {
            self.queue.release(
                self.task.job.clone(),
                self.volume,
                &TaskStatus::Failed("worker lease dropped before finish".to_string()),
            );
        }
    }
}

pub(crate) struct IsolatedRetriableTaskQueue {
    state: Mutex<IsolatedRetriableTaskQueueState>,
    wake: Condvar,
    volume_policy: VolumeConcurrencyPolicy,
    admission_order: Vec<JobClass>,
}

impl IsolatedRetriableTaskQueue {
    pub(crate) fn new(tasks: Vec<RetriableTask>, policy: VolumeConcurrencyPolicy) -> Self {
        Self {
            state: Mutex::new(IsolatedRetriableTaskQueueState {
                pending: VecDeque::from(tasks),
                active_by_volume: HashMap::new(),
                active: HashSet::new(),
                completed: HashSet::new(),
                failed: HashSet::new(),
                next_admission_class: 0,
            }),
            wake: Condvar::new(),
            volume_policy: policy,
            admission_order: JobFairnessPolicy::default().admission_order(),
        }
    }

    pub(crate) fn next(self: &Arc<Self>) -> Option<RetriableTaskLeaseResult> {
        let mut state = self
            .state
            .lock()
            .expect("isolated retriable task queue poisoned");
        loop {
            if let Some((index, volume)) =
                state.next_admissible(&self.admission_order, &self.volume_policy)
            {
                let task = state
                    .pending
                    .remove(index)
                    .expect("admissible retriable task vanished");
                state.active.insert(task.job.id);
                if let Some(volume) = volume {
                    *state.active_by_volume.entry(volume).or_insert(0) += 1;
                }
                return Some(RetriableTaskLeaseResult::Run(RetriableTaskLease {
                    queue: Arc::clone(self),
                    task,
                    volume,
                    finished: false,
                }));
            }
            if let Some(outcome) = state.remove_dependency_blocked() {
                return Some(RetriableTaskLeaseResult::Blocked(outcome));
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

    fn release(&self, job: Job, volume: Option<VolumeId>, status: &TaskStatus) {
        let mut state = self
            .state
            .lock()
            .expect("isolated retriable task queue poisoned");
        state.active.remove(&job.id);
        match status {
            TaskStatus::Completed => {
                state.failed.remove(&job.id);
                state.completed.insert(job.id);
            }
            TaskStatus::Started => {}
            TaskStatus::Cancelled | TaskStatus::Failed(_) => {
                state.completed.remove(&job.id);
                state.failed.insert(job.id);
            }
        }
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
    active: HashSet<crate::JobId>,
    completed: HashSet<crate::JobId>,
    failed: HashSet<crate::JobId>,
    next_admission_class: usize,
}

impl IsolatedRetriableTaskQueueState {
    fn next_admissible(
        &mut self,
        admission_order: &[JobClass],
        volume_policy: &VolumeConcurrencyPolicy,
    ) -> Option<(usize, Option<VolumeId>)> {
        if admission_order.is_empty() {
            return None;
        }
        for offset in 0..admission_order.len() {
            let cursor = (self.next_admission_class + offset) % admission_order.len();
            let class = admission_order[cursor];
            if let Some(admission) = self.pending.iter().enumerate().find_map(|(index, task)| {
                if task.job.class != class || !self.dependencies_completed(&task.job) {
                    return None;
                }
                self.admissible_volume(&task.job, volume_policy)
                    .map(|volume| (index, volume))
            }) {
                self.next_admission_class = (cursor + 1) % admission_order.len();
                return Some(admission);
            }
        }
        None
    }

    fn remove_dependency_blocked(&mut self) -> Option<TaskOutcome> {
        let known_pending = self
            .pending
            .iter()
            .map(|task| task.job.id)
            .chain(self.active.iter().copied())
            .chain(self.completed.iter().copied())
            .chain(self.failed.iter().copied())
            .collect::<HashSet<_>>();
        let index = self.pending.iter().position(|task| {
            task.job.dependencies.iter().any(|dependency| {
                self.failed.contains(dependency) || !known_pending.contains(dependency)
            })
        });
        let index = match index {
            Some(index) => index,
            None if self.active.is_empty() => self.pending.iter().position(|task| {
                task.job
                    .dependencies
                    .iter()
                    .any(|dependency| !self.completed.contains(dependency))
            })?,
            None => return None,
        };
        let task = self
            .pending
            .remove(index)
            .expect("dependency-blocked retriable task vanished");
        self.failed.insert(task.job.id);
        let (missing, failed) = dependency_block_detail(&task.job, &self.completed, &self.failed);
        Some(TaskOutcome {
            id: task.job.id,
            label: task.job.label,
            status: TaskStatus::Failed(format!(
                "job dependencies were not satisfied missing={} failed={}",
                format_dependency_ids(&missing),
                format_dependency_ids(&failed)
            )),
        })
    }

    fn dependencies_completed(&self, job: &Job) -> bool {
        job.dependencies
            .iter()
            .all(|dependency| self.completed.contains(dependency))
    }

    fn admissible_volume(
        &self,
        job: &Job,
        policy: &VolumeConcurrencyPolicy,
    ) -> Option<Option<VolumeId>> {
        match job.volume {
            Some(volume)
                if self.active_by_volume.get(&volume).copied().unwrap_or(0)
                    < policy.limit_for(volume) =>
            {
                Some(Some(volume))
            }
            Some(_) => None,
            None => Some(None),
        }
    }
}

pub(crate) enum RetriableTaskLeaseResult {
    Run(RetriableTaskLease),
    Blocked(TaskOutcome),
}

pub(crate) struct RetriableTaskLease {
    pub(crate) task: RetriableTask,
    queue: Arc<IsolatedRetriableTaskQueue>,
    volume: Option<VolumeId>,
    finished: bool,
}

impl RetriableTaskLease {
    pub(crate) fn finish(mut self, status: &TaskStatus) -> Job {
        self.finished = true;
        let job = self.task.job.clone();
        self.queue.release(job.clone(), self.volume, status);
        job
    }
}

impl Drop for RetriableTaskLease {
    fn drop(&mut self) {
        if !self.finished {
            self.queue.release(
                self.task.job.clone(),
                self.volume,
                &TaskStatus::Failed("worker lease dropped before finish".to_string()),
            );
        }
    }
}

fn dependency_block_detail(
    job: &Job,
    completed: &HashSet<crate::JobId>,
    failed: &HashSet<crate::JobId>,
) -> (Vec<crate::JobId>, Vec<crate::JobId>) {
    let mut missing = Vec::new();
    let mut failed_ids = Vec::new();
    for dependency in &job.dependencies {
        if completed.contains(dependency) {
            continue;
        }
        if failed.contains(dependency) {
            failed_ids.push(*dependency);
        } else {
            missing.push(*dependency);
        }
    }
    missing.sort_by_key(|id| id.value());
    failed_ids.sort_by_key(|id| id.value());
    (missing, failed_ids)
}

fn format_dependency_ids(ids: &[crate::JobId]) -> String {
    if ids.is_empty() {
        return "-".to_string();
    }
    ids.iter()
        .map(|id| id.value().to_string())
        .collect::<Vec<_>>()
        .join(",")
}
