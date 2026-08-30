use crate::PreviewRequestKey;
use gfm_jobs::{
    Cancellation, JobBatteryState, JobIoPressure, JobThermalState, JobUserActivity,
    SchedulingPressure,
};
use gfm_types::{GfmError, Result};
use std::collections::{hash_map::Entry, HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn intersects(self, other: Self) -> bool {
        let ax2 = self.x.saturating_add(self.width as i32);
        let ay2 = self.y.saturating_add(self.height as i32);
        let bx2 = other.x.saturating_add(other.width as i32);
        let by2 = other.y.saturating_add(other.height as i32);
        self.x < bx2 && ax2 > other.x && self.y < by2 && ay2 > other.y
    }

    pub fn inflate(self, margin: u32) -> Self {
        let margin = margin as i32;
        Self {
            x: self.x.saturating_sub(margin),
            y: self.y.saturating_sub(margin),
            width: self.width.saturating_add((margin * 2) as u32),
            height: self.height.saturating_add((margin * 2) as u32),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    pub visible: Rect,
    pub prefetch_margin_px: u32,
}

impl Viewport {
    pub const fn new(visible: Rect, prefetch_margin_px: u32) -> Self {
        Self {
            visible,
            prefetch_margin_px,
        }
    }

    fn priority_for(self, rect: Rect) -> PreviewPriority {
        if rect.intersects(self.visible) {
            PreviewPriority::Visible
        } else if rect.intersects(self.visible.inflate(self.prefetch_margin_px)) {
            PreviewPriority::Prefetch
        } else {
            PreviewPriority::Background
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PreviewPriority {
    Visible,
    Prefetch,
    Background,
}

impl PreviewPriority {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Visible => "visible",
            Self::Prefetch => "prefetch",
            Self::Background => "background",
        }
    }

    const fn score(self) -> u8 {
        match self {
            Self::Visible => 0,
            Self::Prefetch => 1,
            Self::Background => 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewTask {
    pub key: PreviewRequestKey,
    pub rect: Rect,
    pub generation: u64,
}

impl PreviewTask {
    pub fn new(key: PreviewRequestKey, rect: Rect) -> Self {
        Self {
            key,
            rect,
            generation: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledTask {
    pub task: PreviewTask,
    pub priority: PreviewPriority,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewTaskDecision {
    Scheduled {
        key: PreviewRequestKey,
        priority: PreviewPriority,
    },
    Coalesced {
        key: PreviewRequestKey,
        priority: PreviewPriority,
    },
    Cancelled {
        key: PreviewRequestKey,
        reason: &'static str,
    },
}

impl PreviewTaskDecision {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Scheduled { .. } => "scheduled",
            Self::Coalesced { .. } => "coalesced",
            Self::Cancelled { .. } => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewSchedulingPolicy {
    pub max_visible: usize,
    pub max_prefetch: usize,
    pub cancel_offscreen: bool,
}

impl Default for PreviewSchedulingPolicy {
    fn default() -> Self {
        Self {
            max_visible: 64,
            max_prefetch: 128,
            cancel_offscreen: true,
        }
    }
}

impl PreviewSchedulingPolicy {
    pub fn adapted_for_pressure(mut self, pressure: SchedulingPressure) -> Self {
        if matches!(pressure.io, JobIoPressure::Saturated)
            || matches!(pressure.thermal, JobThermalState::Critical)
        {
            self.max_prefetch = 0;
            self.cancel_offscreen = true;
        } else if matches!(pressure.io, JobIoPressure::Elevated)
            || matches!(pressure.thermal, JobThermalState::Serious)
            || matches!(
                pressure.battery,
                JobBatteryState::Battery | JobBatteryState::LowPower
            )
            || matches!(pressure.user_activity, JobUserActivity::Active)
        {
            self.max_prefetch = throttle_limit(self.max_prefetch);
            self.cancel_offscreen = true;
        }
        self
    }
}

pub struct PreviewScheduler {
    base_policy: PreviewSchedulingPolicy,
    policy: PreviewSchedulingPolicy,
    inflight: HashMap<PreviewRequestKey, InflightTask>,
    next_sequence: u64,
}

struct InflightTask {
    cancellation: Cancellation,
    priority: PreviewPriority,
    generation: u64,
    sequence: u64,
}

impl PreviewScheduler {
    pub fn new(policy: PreviewSchedulingPolicy) -> Result<Self> {
        if policy.max_visible == 0 {
            return Err(GfmError::Format(
                "preview scheduler max_visible must be non-zero".to_string(),
            ));
        }
        Ok(Self {
            base_policy: policy,
            policy,
            inflight: HashMap::new(),
            next_sequence: 0,
        })
    }

    pub fn schedule(
        &mut self,
        viewport: Viewport,
        tasks: impl IntoIterator<Item = PreviewTask>,
    ) -> Vec<PreviewTaskDecision> {
        self.schedule_checked(viewport, tasks, || Ok(()))
            .expect("infallible preview scheduling failed")
    }

    pub fn schedule_checked(
        &mut self,
        viewport: Viewport,
        tasks: impl IntoIterator<Item = PreviewTask>,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Vec<PreviewTaskDecision>> {
        check_control()?;
        let mut decisions = self.prune_cancelled_inflight("cancelled-token");
        check_control()?;
        let mut desired = Vec::new();
        for task in tasks {
            check_control()?;
            desired.push(ScheduledTask {
                priority: viewport.priority_for(task.rect),
                task,
            });
            check_control()?;
        }
        let mut desired = coalesce_desired_tasks_checked(desired, &mut check_control)?;
        check_control()?;
        desired.sort_by(|a, b| {
            a.priority
                .score()
                .cmp(&b.priority.score())
                .then_with(|| a.task.rect.y.cmp(&b.task.rect.y))
                .then_with(|| a.task.rect.x.cmp(&b.task.rect.x))
                .then_with(|| a.task.generation.cmp(&b.task.generation))
        });
        check_control()?;
        desired = self.apply_limits_checked(desired, &mut check_control)?;

        let mut desired_keys = HashSet::new();
        for item in &desired {
            check_control()?;
            desired_keys.insert(item.task.key.clone());
        }
        check_control()?;
        if self.policy.cancel_offscreen {
            let stale = self
                .inflight
                .keys()
                .filter(|key| !desired_keys.contains(*key))
                .cloned()
                .collect::<Vec<_>>();
            for key in stale {
                check_control()?;
                if let Some(inflight) = self.inflight.remove(&key) {
                    inflight.cancellation.cancel();
                }
                decisions.push(PreviewTaskDecision::Cancelled {
                    key,
                    reason: "offscreen-or-superseded",
                });
                check_control()?;
            }
        }

        for item in desired {
            check_control()?;
            let key = item.task.key.clone();
            if let Some(inflight) = self.inflight.get_mut(&key) {
                if inflight.generation != item.task.generation {
                    let inflight = self
                        .inflight
                        .remove(&key)
                        .expect("preview inflight generation changed during lookup");
                    inflight.cancellation.cancel();
                    decisions.push(PreviewTaskDecision::Cancelled {
                        key: key.clone(),
                        reason: "superseded-generation",
                    });

                    let sequence = self.next_sequence;
                    self.next_sequence = self.next_sequence.saturating_add(1);
                    self.inflight.insert(
                        key.clone(),
                        InflightTask {
                            cancellation: Cancellation::default(),
                            priority: item.priority,
                            generation: item.task.generation,
                            sequence,
                        },
                    );
                    decisions.push(PreviewTaskDecision::Scheduled {
                        key,
                        priority: item.priority,
                    });
                    continue;
                }
                inflight.priority = item.priority;
                decisions.push(PreviewTaskDecision::Coalesced {
                    key,
                    priority: item.priority,
                });
            } else {
                let sequence = self.next_sequence;
                self.next_sequence = self.next_sequence.saturating_add(1);
                self.inflight.insert(
                    key.clone(),
                    InflightTask {
                        cancellation: Cancellation::default(),
                        priority: item.priority,
                        generation: item.task.generation,
                        sequence,
                    },
                );
                decisions.push(PreviewTaskDecision::Scheduled {
                    key,
                    priority: item.priority,
                });
            }
            check_control()?;
        }
        decisions.extend(self.reconcile_inflight_limits_preserving_checked(
            self.policy,
            "capacity-limit",
            &desired_keys,
            &mut check_control,
        )?);
        check_control()?;
        Ok(decisions)
    }

    pub fn adapt_to_pressure(&mut self, pressure: SchedulingPressure) -> Vec<PreviewTaskDecision> {
        self.adapt_to_pressure_checked(pressure, || Ok(()))
            .expect("infallible preview pressure adaptation failed")
    }

    pub fn adapt_to_pressure_checked(
        &mut self,
        pressure: SchedulingPressure,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Vec<PreviewTaskDecision>> {
        check_control()?;
        let policy = self.base_policy.adapted_for_pressure(pressure);
        let mut decisions = self.prune_cancelled_inflight("cancelled-token");
        check_control()?;
        decisions.extend(self.reconcile_inflight_limits_checked(
            policy,
            "pressure-admission",
            &mut check_control,
        )?);
        self.policy = policy;
        check_control()?;
        Ok(decisions)
    }

    pub fn cancellation_for(&self, key: &PreviewRequestKey) -> Option<Cancellation> {
        self.inflight
            .get(key)
            .map(|inflight| inflight.cancellation.clone())
    }

    pub fn finish(&mut self, key: &PreviewRequestKey) {
        self.inflight.remove(key);
    }

    pub fn inflight_len(&self) -> usize {
        self.inflight.len()
    }

    pub fn policy(&self) -> PreviewSchedulingPolicy {
        self.policy
    }

    fn apply_limits_checked(
        &self,
        desired: Vec<ScheduledTask>,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Vec<ScheduledTask>> {
        let mut visible = 0usize;
        let mut prefetch = 0usize;
        let mut admitted = Vec::new();
        for item in desired {
            check_control()?;
            let keep = match item.priority {
                PreviewPriority::Visible => {
                    visible += 1;
                    visible <= self.policy.max_visible
                }
                PreviewPriority::Prefetch => {
                    prefetch += 1;
                    prefetch <= self.policy.max_prefetch
                }
                PreviewPriority::Background => false,
            };
            if keep {
                admitted.push(item);
            }
            check_control()?;
        }
        Ok(admitted)
    }

    fn reconcile_inflight_limits_checked(
        &mut self,
        policy: PreviewSchedulingPolicy,
        reason: &'static str,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Vec<PreviewTaskDecision>> {
        self.reconcile_inflight_limits_preserving_checked(
            policy,
            reason,
            &HashSet::new(),
            &mut check_control,
        )
    }

    fn prune_cancelled_inflight(&mut self, reason: &'static str) -> Vec<PreviewTaskDecision> {
        let cancelled = self
            .inflight
            .iter()
            .filter(|(_, task)| task.cancellation.is_cancelled())
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();

        cancelled
            .into_iter()
            .filter_map(|key| {
                self.inflight
                    .remove(&key)
                    .map(|_| PreviewTaskDecision::Cancelled { key, reason })
            })
            .collect()
    }

    fn reconcile_inflight_limits_preserving_checked(
        &mut self,
        policy: PreviewSchedulingPolicy,
        reason: &'static str,
        desired_keys: &HashSet<PreviewRequestKey>,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Vec<PreviewTaskDecision>> {
        let mut visible = 0usize;
        let mut prefetch = 0usize;
        let mut inflight = Vec::new();
        for (key, task) in &self.inflight {
            check_control()?;
            inflight.push((
                key.clone(),
                task.priority,
                task.sequence,
                desired_keys.contains(key),
            ));
        }
        check_control()?;
        inflight.sort_by(|a, b| {
            a.1.score()
                .cmp(&b.1.score())
                .then_with(|| b.3.cmp(&a.3))
                .then_with(|| a.2.cmp(&b.2))
                .then_with(|| a.0.path.cmp(&b.0.path))
        });
        check_control()?;

        let mut cancelled = Vec::new();
        for (key, priority, _, _) in inflight {
            check_control()?;
            let keep = match priority {
                PreviewPriority::Visible => {
                    visible += 1;
                    visible <= policy.max_visible
                }
                PreviewPriority::Prefetch => {
                    prefetch += 1;
                    prefetch <= policy.max_prefetch
                }
                PreviewPriority::Background => false,
            };
            if !keep {
                cancelled.push(key);
            }
        }

        let mut decisions = Vec::new();
        for key in cancelled {
            check_control()?;
            if let Some(inflight) = self.inflight.remove(&key) {
                inflight.cancellation.cancel();
                decisions.push(PreviewTaskDecision::Cancelled { key, reason });
            }
            check_control()?;
        }
        Ok(decisions)
    }
}

fn throttle_limit(limit: usize) -> usize {
    if limit == 0 {
        0
    } else {
        (limit / 2).max(1)
    }
}

fn coalesce_desired_tasks_checked(
    tasks: Vec<ScheduledTask>,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScheduledTask>> {
    let mut by_key = HashMap::new();
    for task in tasks {
        check_control()?;
        match by_key.entry(task.task.key.clone()) {
            Entry::Occupied(mut entry) => {
                if should_replace_desired(entry.get(), &task) {
                    entry.insert(task);
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(task);
            }
        }
        check_control()?;
    }
    let mut desired = Vec::with_capacity(by_key.len());
    for task in by_key.into_values() {
        check_control()?;
        desired.push(task);
    }
    Ok(desired)
}

fn should_replace_desired(current: &ScheduledTask, candidate: &ScheduledTask) -> bool {
    candidate
        .task
        .generation
        .cmp(&current.task.generation)
        .then_with(|| current.priority.score().cmp(&candidate.priority.score()))
        .is_gt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PreviewKind;
    use gfm_types::{FileId, VolumeId};
    use std::path::PathBuf;

    #[test]
    fn prioritizes_visible_before_prefetch_and_drops_background() {
        let viewport = Viewport::new(Rect::new(0, 0, 100, 100), 50);
        let mut scheduler = PreviewScheduler::new(PreviewSchedulingPolicy::default()).unwrap();
        let decisions = scheduler.schedule(
            viewport,
            vec![
                task(1, Rect::new(0, 240, 20, 20)),
                task(2, Rect::new(0, 120, 20, 20)),
                task(3, Rect::new(0, 20, 20, 20)),
            ],
        );

        assert_eq!(decisions.len(), 2);
        assert!(matches!(
            decisions[0],
            PreviewTaskDecision::Scheduled {
                priority: PreviewPriority::Visible,
                ..
            }
        ));
        assert!(matches!(
            decisions[1],
            PreviewTaskDecision::Scheduled {
                priority: PreviewPriority::Prefetch,
                ..
            }
        ));
    }

    #[test]
    fn coalesces_existing_visible_work() {
        let viewport = Viewport::new(Rect::new(0, 0, 100, 100), 0);
        let mut scheduler = PreviewScheduler::new(PreviewSchedulingPolicy::default()).unwrap();
        let first = task(1, Rect::new(0, 0, 20, 20));

        scheduler.schedule(viewport, vec![first.clone()]);
        let decisions = scheduler.schedule(viewport, vec![first]);

        assert!(matches!(
            decisions[0],
            PreviewTaskDecision::Coalesced {
                priority: PreviewPriority::Visible,
                ..
            }
        ));
    }

    #[test]
    fn cancels_work_that_moves_offscreen() {
        let mut scheduler = PreviewScheduler::new(PreviewSchedulingPolicy::default()).unwrap();
        let task = task(1, Rect::new(0, 0, 20, 20));
        scheduler.schedule(
            Viewport::new(Rect::new(0, 0, 100, 100), 0),
            vec![task.clone()],
        );
        let cancellation = scheduler
            .cancellation_for(&task.key)
            .expect("scheduled thumbnail has a cancellation token");
        assert!(!cancellation.is_cancelled());
        let decisions =
            scheduler.schedule(Viewport::new(Rect::new(0, 200, 100, 100), 0), vec![task]);

        assert!(matches!(
            decisions[0],
            PreviewTaskDecision::Cancelled {
                reason: "offscreen-or-superseded",
                ..
            }
        ));
        assert!(cancellation.is_cancelled());
        assert_eq!(scheduler.inflight_len(), 0);
    }

    #[test]
    fn coalesced_work_reuses_existing_cancellation_token() {
        let viewport = Viewport::new(Rect::new(0, 0, 100, 100), 0);
        let mut scheduler = PreviewScheduler::new(PreviewSchedulingPolicy::default()).unwrap();
        let task = task(1, Rect::new(0, 0, 20, 20));

        scheduler.schedule(viewport, vec![task.clone()]);
        let first = scheduler
            .cancellation_for(&task.key)
            .expect("first scheduled task has a cancellation token");
        scheduler.schedule(viewport, vec![task.clone()]);
        let second = scheduler
            .cancellation_for(&task.key)
            .expect("coalesced task keeps a cancellation token");

        second.cancel();
        assert!(first.is_cancelled());
    }

    #[test]
    fn externally_cancelled_work_is_pruned_before_rescheduling() {
        let viewport = Viewport::new(Rect::new(0, 0, 100, 100), 0);
        let mut scheduler = PreviewScheduler::new(PreviewSchedulingPolicy::default()).unwrap();
        let task = task(1, Rect::new(0, 0, 20, 20));

        scheduler.schedule(viewport, vec![task.clone()]);
        let cancelled = scheduler
            .cancellation_for(&task.key)
            .expect("scheduled task has a cancellation token");
        cancelled.cancel();

        let decisions = scheduler.schedule(viewport, vec![task.clone()]);
        let replacement = scheduler
            .cancellation_for(&task.key)
            .expect("rescheduled task has a fresh cancellation token");

        assert_eq!(
            decisions,
            vec![
                PreviewTaskDecision::Cancelled {
                    key: task.key.clone(),
                    reason: "cancelled-token",
                },
                PreviewTaskDecision::Scheduled {
                    key: task.key.clone(),
                    priority: PreviewPriority::Visible,
                },
            ]
        );
        assert_eq!(scheduler.inflight_len(), 1);
        assert!(!replacement.is_cancelled());
    }

    #[test]
    fn superseded_generation_cancels_old_preview_and_schedules_new_token() {
        let viewport = Viewport::new(Rect::new(0, 0, 100, 100), 0);
        let mut scheduler = PreviewScheduler::new(PreviewSchedulingPolicy::default()).unwrap();
        let mut first = task(1, Rect::new(0, 0, 20, 20));
        first.generation = 1;
        scheduler.schedule(viewport, vec![first.clone()]);
        let old_cancellation = scheduler
            .cancellation_for(&first.key)
            .expect("first preview generation has a cancellation token");

        let mut next = first.clone();
        next.generation = 2;
        let decisions = scheduler.schedule(viewport, vec![next.clone()]);
        let new_cancellation = scheduler
            .cancellation_for(&next.key)
            .expect("superseding generation has a cancellation token");

        assert_eq!(
            decisions,
            vec![
                PreviewTaskDecision::Cancelled {
                    key: first.key.clone(),
                    reason: "superseded-generation",
                },
                PreviewTaskDecision::Scheduled {
                    key: next.key.clone(),
                    priority: PreviewPriority::Visible,
                },
            ]
        );
        assert!(old_cancellation.is_cancelled());
        assert!(!new_cancellation.is_cancelled());
        assert_eq!(scheduler.inflight_len(), 1);
    }

    #[test]
    fn finished_work_removes_token_without_cancelling_completed_generation() {
        let viewport = Viewport::new(Rect::new(0, 0, 100, 100), 0);
        let mut scheduler = PreviewScheduler::new(PreviewSchedulingPolicy::default()).unwrap();
        let task = task(1, Rect::new(0, 0, 20, 20));

        scheduler.schedule(viewport, vec![task.clone()]);
        let cancellation = scheduler
            .cancellation_for(&task.key)
            .expect("scheduled task has a cancellation token");
        scheduler.finish(&task.key);

        assert!(scheduler.cancellation_for(&task.key).is_none());
        assert!(!cancellation.is_cancelled());
    }

    #[test]
    fn enforces_visible_and_prefetch_limits() {
        let policy = PreviewSchedulingPolicy {
            max_visible: 1,
            max_prefetch: 1,
            cancel_offscreen: true,
        };
        let mut scheduler = PreviewScheduler::new(policy).unwrap();
        let decisions = scheduler.schedule(
            Viewport::new(Rect::new(0, 0, 100, 100), 100),
            vec![
                task(1, Rect::new(0, 0, 20, 20)),
                task(2, Rect::new(40, 0, 20, 20)),
                task(3, Rect::new(0, 150, 20, 20)),
                task(4, Rect::new(40, 150, 20, 20)),
            ],
        );

        assert_eq!(decisions.len(), 2);
        assert_eq!(scheduler.inflight_len(), 2);
    }

    #[test]
    fn duplicate_requests_do_not_consume_multiple_visible_slots() {
        let policy = PreviewSchedulingPolicy {
            max_visible: 2,
            max_prefetch: 0,
            cancel_offscreen: true,
        };
        let mut scheduler = PreviewScheduler::new(policy).unwrap();
        let duplicate = task(1, Rect::new(0, 0, 20, 20));
        let second_visible = task(2, Rect::new(40, 0, 20, 20));

        let decisions = scheduler.schedule(
            Viewport::new(Rect::new(0, 0, 100, 100), 0),
            vec![duplicate.clone(), duplicate, second_visible.clone()],
        );

        assert_eq!(decisions.len(), 2);
        assert_eq!(scheduler.inflight_len(), 2);
        assert!(decisions.iter().any(|decision| {
            matches!(
                decision,
                PreviewTaskDecision::Scheduled { key, .. } if key == &second_visible.key
            )
        }));
    }

    #[test]
    fn duplicate_requests_keep_best_priority_for_same_generation() {
        let mut scheduler = PreviewScheduler::new(PreviewSchedulingPolicy::default()).unwrap();
        let mut prefetch = task(1, Rect::new(0, 120, 20, 20));
        prefetch.generation = 4;
        let mut visible = task(1, Rect::new(0, 0, 20, 20));
        visible.generation = 4;

        let decisions = scheduler.schedule(
            Viewport::new(Rect::new(0, 0, 100, 100), 100),
            vec![prefetch, visible],
        );

        assert_eq!(
            decisions,
            vec![PreviewTaskDecision::Scheduled {
                key: task(1, Rect::new(0, 0, 20, 20)).key,
                priority: PreviewPriority::Visible,
            }]
        );
    }

    #[test]
    fn duplicate_requests_keep_newest_generation_before_limits() {
        let policy = PreviewSchedulingPolicy {
            max_visible: 1,
            max_prefetch: 0,
            cancel_offscreen: true,
        };
        let mut scheduler = PreviewScheduler::new(policy).unwrap();
        let mut old = task(1, Rect::new(0, 0, 20, 20));
        old.generation = 1;
        let mut new = old.clone();
        new.generation = 2;

        let decisions = scheduler.schedule(
            Viewport::new(Rect::new(0, 0, 100, 100), 0),
            vec![old, new.clone()],
        );

        assert_eq!(
            decisions,
            vec![PreviewTaskDecision::Scheduled {
                key: new.key,
                priority: PreviewPriority::Visible,
            }]
        );
    }

    #[test]
    fn pressure_adaptation_preserves_visible_and_drops_prefetch_under_saturation() {
        let policy = PreviewSchedulingPolicy {
            max_visible: 4,
            max_prefetch: 4,
            cancel_offscreen: false,
        }
        .adapted_for_pressure(SchedulingPressure {
            io: JobIoPressure::Saturated,
            ..SchedulingPressure::default()
        });
        let mut scheduler = PreviewScheduler::new(policy).unwrap();
        let decisions = scheduler.schedule(
            Viewport::new(Rect::new(0, 0, 100, 100), 100),
            vec![
                task(1, Rect::new(0, 0, 20, 20)),
                task(2, Rect::new(0, 150, 20, 20)),
            ],
        );

        assert_eq!(decisions.len(), 1);
        assert!(matches!(
            decisions[0],
            PreviewTaskDecision::Scheduled {
                priority: PreviewPriority::Visible,
                ..
            }
        ));
        assert!(policy.cancel_offscreen);
    }

    #[test]
    fn pressure_adaptation_throttles_prefetch_under_active_user_load() {
        let policy = PreviewSchedulingPolicy {
            max_visible: 4,
            max_prefetch: 2,
            cancel_offscreen: false,
        }
        .adapted_for_pressure(SchedulingPressure {
            user_activity: JobUserActivity::Active,
            ..SchedulingPressure::default()
        });
        let mut scheduler = PreviewScheduler::new(policy).unwrap();
        let decisions = scheduler.schedule(
            Viewport::new(Rect::new(0, 0, 100, 100), 100),
            vec![
                task(1, Rect::new(0, 150, 20, 20)),
                task(2, Rect::new(20, 150, 20, 20)),
            ],
        );

        assert_eq!(decisions.len(), 1);
        assert!(matches!(
            decisions[0],
            PreviewTaskDecision::Scheduled {
                priority: PreviewPriority::Prefetch,
                ..
            }
        ));
        assert!(policy.cancel_offscreen);
    }

    #[test]
    fn pressure_adaptation_throttles_prefetch_on_battery_power() {
        let policy = PreviewSchedulingPolicy {
            max_visible: 4,
            max_prefetch: 4,
            cancel_offscreen: false,
        }
        .adapted_for_pressure(SchedulingPressure {
            battery: JobBatteryState::Battery,
            ..SchedulingPressure::default()
        });
        let mut scheduler = PreviewScheduler::new(policy).unwrap();
        let decisions = scheduler.schedule(
            Viewport::new(Rect::new(0, 0, 100, 100), 100),
            vec![
                task(1, Rect::new(0, 150, 20, 20)),
                task(2, Rect::new(20, 150, 20, 20)),
                task(3, Rect::new(40, 150, 20, 20)),
            ],
        );

        assert_eq!(decisions.len(), 2);
        assert!(decisions.iter().all(|decision| matches!(
            decision,
            PreviewTaskDecision::Scheduled {
                priority: PreviewPriority::Prefetch,
                ..
            }
        )));
        assert!(policy.cancel_offscreen);
    }

    #[test]
    fn live_pressure_adaptation_cancels_prefetch_over_new_budget() {
        let policy = PreviewSchedulingPolicy {
            max_visible: 4,
            max_prefetch: 4,
            cancel_offscreen: false,
        };
        let mut scheduler = PreviewScheduler::new(policy).unwrap();
        let visible = task(1, Rect::new(0, 0, 20, 20));
        let first_prefetch = task(2, Rect::new(0, 120, 20, 20));
        let second_prefetch = task(3, Rect::new(20, 120, 20, 20));
        scheduler.schedule(
            Viewport::new(Rect::new(0, 0, 100, 100), 100),
            vec![
                visible.clone(),
                first_prefetch.clone(),
                second_prefetch.clone(),
            ],
        );
        let visible_cancel = scheduler
            .cancellation_for(&visible.key)
            .expect("visible task is inflight");
        let first_prefetch_cancel = scheduler
            .cancellation_for(&first_prefetch.key)
            .expect("first prefetch task is inflight");
        let second_prefetch_cancel = scheduler
            .cancellation_for(&second_prefetch.key)
            .expect("second prefetch task is inflight");

        let decisions = scheduler.adapt_to_pressure(SchedulingPressure {
            user_activity: JobUserActivity::Active,
            ..SchedulingPressure::default()
        });

        assert_eq!(
            scheduler.policy(),
            PreviewSchedulingPolicy {
                max_visible: 4,
                max_prefetch: 2,
                cancel_offscreen: true,
            }
        );
        assert!(decisions.is_empty());
        assert_eq!(scheduler.inflight_len(), 3);
        assert!(!visible_cancel.is_cancelled());
        assert!(!first_prefetch_cancel.is_cancelled());
        assert!(!second_prefetch_cancel.is_cancelled());

        let decisions = scheduler.adapt_to_pressure(SchedulingPressure {
            io: JobIoPressure::Saturated,
            ..SchedulingPressure::default()
        });

        assert_eq!(
            decisions,
            vec![
                PreviewTaskDecision::Cancelled {
                    key: first_prefetch.key.clone(),
                    reason: "pressure-admission",
                },
                PreviewTaskDecision::Cancelled {
                    key: second_prefetch.key.clone(),
                    reason: "pressure-admission",
                },
            ]
        );
        assert_eq!(scheduler.inflight_len(), 1);
        assert!(!visible_cancel.is_cancelled());
        assert!(first_prefetch_cancel.is_cancelled());
        assert!(second_prefetch_cancel.is_cancelled());
    }

    #[test]
    fn checked_schedule_can_cancel_during_large_admission_batch() {
        let viewport = Viewport::new(Rect::new(0, 0, 1_000, 1_000), 0);
        let mut scheduler = PreviewScheduler::new(PreviewSchedulingPolicy {
            max_visible: 512,
            max_prefetch: 0,
            cancel_offscreen: true,
        })
        .unwrap();
        let mut checks = 0usize;
        let err = scheduler
            .schedule_checked(
                viewport,
                (0..256).map(|node| task(node, Rect::new(0, 0, 20, 20))),
                || {
                    checks += 1;
                    if checks > 16 {
                        Err(GfmError::Cancelled)
                    } else {
                        Ok(())
                    }
                },
            )
            .expect_err("checked scheduling should stop before finishing admission");

        assert!(matches!(err, GfmError::Cancelled));
        assert_eq!(scheduler.inflight_len(), 0);
    }

    #[test]
    fn checked_pressure_adaptation_can_cancel_during_reconcile() {
        let policy = PreviewSchedulingPolicy {
            max_visible: 4,
            max_prefetch: 4,
            cancel_offscreen: false,
        };
        let mut scheduler = PreviewScheduler::new(policy).unwrap();
        scheduler.schedule(
            Viewport::new(Rect::new(0, 0, 100, 100), 100),
            (0..64).map(|node| task(node, Rect::new(0, 120, 20, 20))),
        );
        let mut checks = 0usize;
        let err = scheduler
            .adapt_to_pressure_checked(
                SchedulingPressure {
                    io: JobIoPressure::Saturated,
                    ..SchedulingPressure::default()
                },
                || {
                    checks += 1;
                    if checks > 4 {
                        Err(GfmError::Cancelled)
                    } else {
                        Ok(())
                    }
                },
            )
            .expect_err("checked pressure admission should be cancellable");

        assert!(matches!(err, GfmError::Cancelled));
        assert_eq!(
            scheduler.policy(),
            PreviewSchedulingPolicy {
                max_visible: 4,
                max_prefetch: 4,
                cancel_offscreen: false,
            }
        );
    }

    #[test]
    fn coalesced_visible_work_updates_priority_before_pressure_reconcile() {
        let policy = PreviewSchedulingPolicy {
            max_visible: 2,
            max_prefetch: 1,
            cancel_offscreen: true,
        };
        let mut scheduler = PreviewScheduler::new(policy).unwrap();
        let task = task(1, Rect::new(0, 120, 20, 20));
        scheduler.schedule(
            Viewport::new(Rect::new(0, 0, 100, 100), 100),
            vec![task.clone()],
        );
        let cancellation = scheduler
            .cancellation_for(&task.key)
            .expect("prefetch task is inflight");

        scheduler.schedule(
            Viewport::new(Rect::new(0, 100, 100, 100), 0),
            vec![task.clone()],
        );
        let decisions = scheduler.adapt_to_pressure(SchedulingPressure {
            io: JobIoPressure::Saturated,
            ..SchedulingPressure::default()
        });

        assert!(decisions.is_empty());
        assert_eq!(scheduler.inflight_len(), 1);
        assert!(!cancellation.is_cancelled());
    }

    #[test]
    fn retained_offscreen_work_cannot_exceed_visible_capacity() {
        let policy = PreviewSchedulingPolicy {
            max_visible: 1,
            max_prefetch: 0,
            cancel_offscreen: false,
        };
        let mut scheduler = PreviewScheduler::new(policy).unwrap();
        let old_visible = task(1, Rect::new(0, 0, 20, 20));
        let new_visible = task(2, Rect::new(0, 100, 20, 20));

        scheduler.schedule(
            Viewport::new(Rect::new(0, 0, 100, 100), 0),
            vec![old_visible.clone()],
        );
        let old_cancellation = scheduler
            .cancellation_for(&old_visible.key)
            .expect("old visible preview is inflight");

        let decisions = scheduler.schedule(
            Viewport::new(Rect::new(0, 100, 100, 100), 0),
            vec![new_visible.clone()],
        );
        let new_cancellation = scheduler
            .cancellation_for(&new_visible.key)
            .expect("new viewport preview keeps capacity");

        assert_eq!(
            decisions,
            vec![
                PreviewTaskDecision::Scheduled {
                    key: new_visible.key.clone(),
                    priority: PreviewPriority::Visible,
                },
                PreviewTaskDecision::Cancelled {
                    key: old_visible.key.clone(),
                    reason: "capacity-limit",
                },
            ]
        );
        assert_eq!(scheduler.inflight_len(), 1);
        assert!(old_cancellation.is_cancelled());
        assert!(!new_cancellation.is_cancelled());
    }

    fn task(node: u64, rect: Rect) -> PreviewTask {
        PreviewTask::new(
            crate::PreviewRequestKey::new(
                FileId::new(VolumeId(1), node),
                PathBuf::from(format!("{node}.png")),
                PreviewKind::Thumbnail,
            ),
            rect,
        )
    }
}
