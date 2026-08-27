use crate::Priority;
use gfm_types::VolumeId;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulingPressure {
    pub io: JobIoPressure,
    pub thermal: JobThermalState,
    pub battery: JobBatteryState,
    pub user_activity: JobUserActivity,
}

impl Default for SchedulingPressure {
    fn default() -> Self {
        Self {
            io: JobIoPressure::Nominal,
            thermal: JobThermalState::Nominal,
            battery: JobBatteryState::AcPower,
            user_activity: JobUserActivity::Idle,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobIoPressure {
    Nominal,
    Elevated,
    Saturated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobThermalState {
    Nominal,
    Fair,
    Serious,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobBatteryState {
    AcPower,
    Battery,
    LowPower,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobUserActivity {
    Idle,
    Active,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulingAction {
    Run,
    Throttle,
    Defer,
}

impl SchedulingAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Run => "Run",
            Self::Throttle => "Throttle",
            Self::Defer => "Defer",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulingDecision {
    pub action: SchedulingAction,
    pub worker_threads: usize,
    pub volume_policy: VolumeConcurrencyPolicy,
}

impl SchedulingPressure {
    pub fn decide(
        self,
        priority: Priority,
        base_threads: usize,
        base_volume_limit: usize,
    ) -> SchedulingDecision {
        let base_threads = base_threads.max(1);
        let base_volume_limit = base_volume_limit.max(1);
        let action = self.action_for(priority);
        let (worker_threads, volume_limit) = match action {
            SchedulingAction::Run => (base_threads, base_volume_limit),
            SchedulingAction::Throttle => (
                throttle_limit(base_threads),
                throttle_limit(base_volume_limit),
            ),
            SchedulingAction::Defer => (0, 1),
        };
        SchedulingDecision {
            action,
            worker_threads,
            volume_policy: VolumeConcurrencyPolicy::new(volume_limit),
        }
    }

    fn action_for(self, priority: Priority) -> SchedulingAction {
        if matches!(priority, Priority::Visible | Priority::Interactive) {
            return SchedulingAction::Run;
        }
        if matches!(self.io, JobIoPressure::Saturated)
            || matches!(self.thermal, JobThermalState::Critical)
        {
            return SchedulingAction::Defer;
        }
        if matches!(self.io, JobIoPressure::Elevated)
            || matches!(self.thermal, JobThermalState::Serious)
            || matches!(
                self.battery,
                JobBatteryState::Battery | JobBatteryState::LowPower
            )
            || matches!(self.user_activity, JobUserActivity::Active)
        {
            return SchedulingAction::Throttle;
        }
        SchedulingAction::Run
    }
}

fn throttle_limit(limit: usize) -> usize {
    (limit / 2).max(1)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeConcurrencyPolicy {
    default_limit: usize,
    overrides: HashMap<VolumeId, usize>,
}

impl VolumeConcurrencyPolicy {
    pub fn new(default_limit: usize) -> Self {
        Self {
            default_limit: default_limit.max(1),
            overrides: HashMap::new(),
        }
    }

    pub fn with_volume_limit(mut self, volume: VolumeId, limit: usize) -> Self {
        self.overrides.insert(volume, limit.max(1));
        self
    }

    pub(crate) fn limit_for(&self, volume: VolumeId) -> usize {
        self.overrides
            .get(&volume)
            .copied()
            .unwrap_or(self.default_limit)
    }

    pub fn default_limit(&self) -> usize {
        self.default_limit
    }
}

impl Default for VolumeConcurrencyPolicy {
    fn default() -> Self {
        Self::new(1)
    }
}
