use crate::PreviewRequestKey;
use gfm_jobs::{
    Cancellation, JobBatteryState, JobIoPressure, JobThermalState, JobUserActivity,
    SchedulingPressure,
};
use gfm_types::{GfmError, Result};
use std::collections::{HashMap, HashSet};

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
            || matches!(pressure.battery, JobBatteryState::LowPower)
            || matches!(pressure.user_activity, JobUserActivity::Active)
        {
            self.max_prefetch = throttle_limit(self.max_prefetch);
            self.cancel_offscreen = true;
        }
        self
    }
}

pub struct PreviewScheduler {
    policy: PreviewSchedulingPolicy,
    inflight: HashMap<PreviewRequestKey, InflightTask>,
}

struct InflightTask {
    cancellation: Cancellation,
}

impl PreviewScheduler {
    pub fn new(policy: PreviewSchedulingPolicy) -> Result<Self> {
        if policy.max_visible == 0 {
            return Err(GfmError::Format(
                "preview scheduler max_visible must be non-zero".to_string(),
            ));
        }
        Ok(Self {
            policy,
            inflight: HashMap::new(),
        })
    }

    pub fn schedule(
        &mut self,
        viewport: Viewport,
        tasks: impl IntoIterator<Item = PreviewTask>,
    ) -> Vec<PreviewTaskDecision> {
        let mut desired = tasks
            .into_iter()
            .map(|task| ScheduledTask {
                priority: viewport.priority_for(task.rect),
                task,
            })
            .collect::<Vec<_>>();
        desired.sort_by(|a, b| {
            a.priority
                .score()
                .cmp(&b.priority.score())
                .then_with(|| a.task.rect.y.cmp(&b.task.rect.y))
                .then_with(|| a.task.rect.x.cmp(&b.task.rect.x))
                .then_with(|| a.task.generation.cmp(&b.task.generation))
        });
        desired = self.apply_limits(desired);

        let desired_keys = desired
            .iter()
            .map(|item| item.task.key.clone())
            .collect::<HashSet<_>>();
        let mut decisions = Vec::new();
        if self.policy.cancel_offscreen {
            let stale = self
                .inflight
                .keys()
                .filter(|key| !desired_keys.contains(*key))
                .cloned()
                .collect::<Vec<_>>();
            for key in stale {
                if let Some(inflight) = self.inflight.remove(&key) {
                    inflight.cancellation.cancel();
                }
                decisions.push(PreviewTaskDecision::Cancelled {
                    key,
                    reason: "offscreen-or-superseded",
                });
            }
        }

        for item in desired {
            let key = item.task.key.clone();
            if self.inflight.contains_key(&key) {
                decisions.push(PreviewTaskDecision::Coalesced {
                    key,
                    priority: item.priority,
                });
            } else {
                self.inflight.insert(
                    key.clone(),
                    InflightTask {
                        cancellation: Cancellation::default(),
                    },
                );
                decisions.push(PreviewTaskDecision::Scheduled {
                    key,
                    priority: item.priority,
                });
            }
        }
        decisions
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

    fn apply_limits(&self, desired: Vec<ScheduledTask>) -> Vec<ScheduledTask> {
        let mut visible = 0usize;
        let mut prefetch = 0usize;
        desired
            .into_iter()
            .filter(|item| match item.priority {
                PreviewPriority::Visible => {
                    visible += 1;
                    visible <= self.policy.max_visible
                }
                PreviewPriority::Prefetch => {
                    prefetch += 1;
                    prefetch <= self.policy.max_prefetch
                }
                PreviewPriority::Background => false,
            })
            .collect()
    }
}

fn throttle_limit(limit: usize) -> usize {
    if limit == 0 {
        0
    } else {
        (limit / 2).max(1)
    }
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
