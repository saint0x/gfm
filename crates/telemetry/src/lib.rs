use std::collections::BTreeMap;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Default)]
pub struct Metrics {
    counters: BTreeMap<&'static str, u64>,
    timings: BTreeMap<&'static str, Vec<Duration>>,
}

impl Metrics {
    pub fn increment(&mut self, name: &'static str) {
        *self.counters.entry(name).or_default() += 1;
    }

    pub fn observe(&mut self, name: &'static str, duration: Duration) {
        self.timings.entry(name).or_default().push(duration);
    }

    pub fn counter(&self, name: &'static str) -> u64 {
        self.counters.get(name).copied().unwrap_or(0)
    }

    pub fn p95(&self, name: &'static str) -> Option<Duration> {
        let mut values = self.timings.get(name)?.clone();
        if values.is_empty() {
            return None;
        }
        values.sort();
        let index = ((values.len() - 1) * 95) / 100;
        values.get(index).copied()
    }
}

pub fn time<T>(metrics: &mut Metrics, name: &'static str, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let value = f();
    metrics.observe(name, start.elapsed());
    value
}
