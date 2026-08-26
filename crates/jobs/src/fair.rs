use crate::{Job, JobClass, JobId};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobFairnessPolicy {
    quotas: HashMap<JobClass, usize>,
}

impl JobFairnessPolicy {
    pub fn new() -> Self {
        Self {
            quotas: [
                (JobClass::Foreground, 3),
                (JobClass::Visible, 3),
                (JobClass::Background, 1),
                (JobClass::Maintenance, 1),
                (JobClass::Repair, 1),
            ]
            .into_iter()
            .collect(),
        }
    }

    pub fn with_quota(mut self, class: JobClass, quota: usize) -> Self {
        self.quotas.insert(class, quota.max(1));
        self
    }

    fn quota(&self, class: JobClass) -> usize {
        self.quotas.get(&class).copied().unwrap_or(1).max(1)
    }
}

impl Default for JobFairnessPolicy {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockedJob {
    pub id: JobId,
    pub label: String,
    pub class: JobClass,
    pub missing_dependencies: Vec<JobId>,
}

#[derive(Debug, Clone)]
pub struct JobFairnessPlan {
    pub ready: Vec<Job>,
    pub blocked: Vec<BlockedJob>,
}

impl JobFairnessPlan {
    pub fn labels(&self) -> Vec<&str> {
        self.ready.iter().map(|job| job.label.as_str()).collect()
    }
}

#[derive(Debug, Clone)]
pub struct JobFairnessPlanner {
    policy: JobFairnessPolicy,
    completed: HashSet<JobId>,
}

impl JobFairnessPlanner {
    pub fn new(policy: JobFairnessPolicy) -> Self {
        Self {
            policy,
            completed: HashSet::new(),
        }
    }

    pub fn with_completed(mut self, completed: impl IntoIterator<Item = JobId>) -> Self {
        self.completed.extend(completed);
        self
    }

    pub fn plan(&self, jobs: impl IntoIterator<Item = Job>) -> JobFairnessPlan {
        let mut pending: VecDeque<Job> = jobs.into_iter().collect();
        let mut satisfied = self.completed.clone();
        let mut ready = Vec::new();

        loop {
            let mut progressed = false;
            for class in JOB_CLASS_ORDER {
                for _ in 0..self.policy.quota(class) {
                    let Some(index) = pending.iter().position(|job| {
                        job.class == class && dependencies_satisfied(job, &satisfied)
                    }) else {
                        break;
                    };
                    let job = pending.remove(index).expect("fairness job vanished");
                    satisfied.insert(job.id);
                    ready.push(job);
                    progressed = true;
                }
            }
            if !progressed {
                break;
            }
        }

        let blocked = pending
            .into_iter()
            .map(|job| BlockedJob {
                id: job.id,
                label: job.label,
                class: job.class,
                missing_dependencies: job
                    .dependencies
                    .into_iter()
                    .filter(|dependency| !satisfied.contains(dependency))
                    .collect(),
            })
            .collect();

        JobFairnessPlan { ready, blocked }
    }
}

const JOB_CLASS_ORDER: [JobClass; 5] = [
    JobClass::Foreground,
    JobClass::Visible,
    JobClass::Background,
    JobClass::Maintenance,
    JobClass::Repair,
];

fn dependencies_satisfied(job: &Job, satisfied: &HashSet<JobId>) -> bool {
    job.dependencies
        .iter()
        .all(|dependency| satisfied.contains(dependency))
}
