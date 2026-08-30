use crate::{Job, JobClass, JobId};
use gfm_types::Result;
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
        self.plan_checked(jobs, || Ok(()))
            .expect("infallible job fairness planning failed")
    }

    pub fn plan_checked(
        &self,
        jobs: impl IntoIterator<Item = Job>,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<JobFairnessPlan> {
        let mut ready_by_class: [VecDeque<Job>; JOB_CLASS_ORDER.len()] =
            std::array::from_fn(|_| VecDeque::new());
        let mut blocked_with_order = Vec::new();
        let mut ready = Vec::new();

        for (order, job) in jobs.into_iter().enumerate() {
            check_control()?;
            if dependencies_satisfied(&job, &self.completed) {
                ready_by_class[class_index(job.class)].push_back(job);
            } else {
                blocked_with_order.push((order, job));
            }
            check_control()?;
        }

        loop {
            check_control()?;
            let mut progressed = false;
            for class in JOB_CLASS_ORDER {
                check_control()?;
                for _ in 0..self.policy.quota(class) {
                    check_control()?;
                    let Some(job) = ready_by_class[class_index(class)].pop_front() else {
                        break;
                    };
                    ready.push(job);
                    progressed = true;
                }
            }
            if !progressed {
                break;
            }
        }

        let mut blocked = Vec::new();
        for (_, job) in blocked_with_order {
            check_control()?;
            blocked.push(BlockedJob {
                id: job.id,
                label: job.label,
                class: job.class,
                missing_dependencies: job
                    .dependencies
                    .into_iter()
                    .filter(|dependency| !self.completed.contains(dependency))
                    .collect(),
            });
            check_control()?;
        }

        Ok(JobFairnessPlan { ready, blocked })
    }
}

const JOB_CLASS_ORDER: [JobClass; 5] = [
    JobClass::Foreground,
    JobClass::Visible,
    JobClass::Background,
    JobClass::Maintenance,
    JobClass::Repair,
];

fn class_index(class: JobClass) -> usize {
    match class {
        JobClass::Foreground => 0,
        JobClass::Visible => 1,
        JobClass::Background => 2,
        JobClass::Maintenance => 3,
        JobClass::Repair => 4,
    }
}

fn dependencies_satisfied(job: &Job, satisfied: &HashSet<JobId>) -> bool {
    job.dependencies
        .iter()
        .all(|dependency| satisfied.contains(dependency))
}
