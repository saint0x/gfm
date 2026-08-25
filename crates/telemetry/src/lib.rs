use std::collections::BTreeMap;
use std::time::{Duration, Instant};

const DEFAULT_STALL_THRESHOLD: Duration = Duration::from_millis(50);
const LATENCY_BUCKETS: [Duration; 23] = [
    Duration::from_micros(100),
    Duration::from_micros(250),
    Duration::from_micros(500),
    Duration::from_millis(1),
    Duration::from_millis(2),
    Duration::from_millis(4),
    Duration::from_millis(8),
    Duration::from_millis(12),
    Duration::from_millis(16),
    Duration::from_millis(25),
    Duration::from_millis(33),
    Duration::from_millis(50),
    Duration::from_millis(75),
    Duration::from_millis(100),
    Duration::from_millis(150),
    Duration::from_millis(250),
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(16),
    Duration::from_secs(32),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LatencyMetric {
    Navigation,
    Selection,
    Rename,
    SearchKeystroke,
    ResultStreaming,
    ThumbnailDisplay,
    PreviewOpen,
    CopyStart,
    Cancel,
    WindowRender,
}

impl LatencyMetric {
    pub const ALL: [Self; 10] = [
        Self::Navigation,
        Self::Selection,
        Self::Rename,
        Self::SearchKeystroke,
        Self::ResultStreaming,
        Self::ThumbnailDisplay,
        Self::PreviewOpen,
        Self::CopyStart,
        Self::Cancel,
        Self::WindowRender,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Navigation => "navigation",
            Self::Selection => "selection",
            Self::Rename => "rename",
            Self::SearchKeystroke => "search_keystroke",
            Self::ResultStreaming => "result_streaming",
            Self::ThumbnailDisplay => "thumbnail_display",
            Self::PreviewOpen => "preview_open",
            Self::CopyStart => "copy_start",
            Self::Cancel => "cancel",
            Self::WindowRender => "window_render",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Histogram {
    buckets: Vec<Bucket>,
    overflow: u64,
    count: u64,
    sum: Duration,
    min: Option<Duration>,
    max: Option<Duration>,
}

impl Histogram {
    pub fn latency() -> Self {
        Self::new(&LATENCY_BUCKETS)
    }

    pub fn new(bounds: &[Duration]) -> Self {
        let mut last = None;
        let buckets = bounds
            .iter()
            .copied()
            .map(|upper_bound| {
                if let Some(last) = last {
                    assert!(upper_bound > last, "histogram buckets must be increasing");
                }
                last = Some(upper_bound);
                Bucket {
                    upper_bound,
                    count: 0,
                }
            })
            .collect();
        Self {
            buckets,
            overflow: 0,
            count: 0,
            sum: Duration::ZERO,
            min: None,
            max: None,
        }
    }

    pub fn observe(&mut self, value: Duration) {
        self.count += 1;
        self.sum = self.sum.saturating_add(value);
        self.min = Some(self.min.map(|min| min.min(value)).unwrap_or(value));
        self.max = Some(self.max.map(|max| max.max(value)).unwrap_or(value));

        if let Some(bucket) = self
            .buckets
            .iter_mut()
            .find(|bucket| value <= bucket.upper_bound)
        {
            bucket.count += 1;
        } else {
            self.overflow += 1;
        }
    }

    pub fn summary(&self) -> HistogramSummary {
        HistogramSummary {
            count: self.count,
            min: self.min,
            max: self.max,
            mean: self.mean(),
            p50: self.percentile(50),
            p95: self.percentile(95),
            p99: self.percentile(99),
            overflow: self.overflow,
        }
    }

    pub fn count(&self) -> u64 {
        self.count
    }

    pub fn percentile(&self, percentile: u8) -> Option<Duration> {
        if self.count == 0 {
            return None;
        }
        let percentile = percentile.min(100) as u64;
        let target = ((self.count * percentile).saturating_add(99) / 100).max(1);
        let mut seen = 0;
        for bucket in &self.buckets {
            seen += bucket.count;
            if seen >= target {
                return Some(bucket.upper_bound);
            }
        }
        self.max
    }

    fn mean(&self) -> Option<Duration> {
        (self.count != 0).then(|| duration_div(self.sum, self.count))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bucket {
    pub upper_bound: Duration,
    pub count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistogramSummary {
    pub count: u64,
    pub min: Option<Duration>,
    pub max: Option<Duration>,
    pub mean: Option<Duration>,
    pub p50: Option<Duration>,
    pub p95: Option<Duration>,
    pub p99: Option<Duration>,
    pub overflow: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Telemetry {
    latencies: BTreeMap<LatencyMetric, Histogram>,
    frames: FrameTiming,
    counters: BTreeMap<&'static str, u64>,
}

impl Default for Telemetry {
    fn default() -> Self {
        Self {
            latencies: LatencyMetric::ALL
                .into_iter()
                .map(|metric| (metric, Histogram::latency()))
                .collect(),
            frames: FrameTiming::default(),
            counters: BTreeMap::new(),
        }
    }
}

impl Telemetry {
    pub fn increment(&mut self, name: &'static str) {
        *self.counters.entry(name).or_default() += 1;
    }

    pub fn counter(&self, name: &'static str) -> u64 {
        self.counters.get(name).copied().unwrap_or(0)
    }

    pub fn observe_latency(&mut self, metric: LatencyMetric, duration: Duration) {
        self.latencies
            .entry(metric)
            .or_insert_with(Histogram::latency)
            .observe(duration);
    }

    pub fn time_latency<T>(&mut self, metric: LatencyMetric, f: impl FnOnce() -> T) -> T {
        let timer = Timer::start();
        let value = f();
        self.observe_latency(metric, timer.elapsed());
        value
    }

    pub fn latency(&self, metric: LatencyMetric) -> HistogramSummary {
        self.latencies
            .get(&metric)
            .map(Histogram::summary)
            .unwrap_or_else(|| Histogram::latency().summary())
    }

    pub fn observe_frame(&mut self, duration: Duration) -> Option<FrameStall> {
        self.frames.observe(duration)
    }

    pub fn frame_timing(&self) -> FrameTimingSummary {
        self.frames.summary()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameTiming {
    histogram: Histogram,
    stall_threshold: Duration,
    stalls: Vec<FrameStall>,
}

impl Default for FrameTiming {
    fn default() -> Self {
        Self {
            histogram: Histogram::latency(),
            stall_threshold: DEFAULT_STALL_THRESHOLD,
            stalls: Vec::new(),
        }
    }
}

impl FrameTiming {
    pub fn with_stall_threshold(stall_threshold: Duration) -> Self {
        Self {
            stall_threshold,
            ..Self::default()
        }
    }

    pub fn observe(&mut self, duration: Duration) -> Option<FrameStall> {
        self.histogram.observe(duration);
        if duration <= self.stall_threshold {
            return None;
        }
        let stall = FrameStall {
            frame_index: self.histogram.count(),
            duration,
            threshold: self.stall_threshold,
        };
        self.stalls.push(stall);
        Some(stall)
    }

    pub fn summary(&self) -> FrameTimingSummary {
        FrameTimingSummary {
            histogram: self.histogram.summary(),
            stall_threshold: self.stall_threshold,
            stall_count: self.stalls.len() as u64,
            worst_stall: self.stalls.iter().map(|stall| stall.duration).max(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameStall {
    pub frame_index: u64,
    pub duration: Duration,
    pub threshold: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameTimingSummary {
    pub histogram: HistogramSummary,
    pub stall_threshold: Duration,
    pub stall_count: u64,
    pub worst_stall: Option<Duration>,
}

#[derive(Debug, Clone, Copy)]
pub struct Timer {
    started_at: Instant,
}

impl Timer {
    pub fn start() -> Self {
        Self {
            started_at: Instant::now(),
        }
    }

    pub fn elapsed(self) -> Duration {
        self.started_at.elapsed()
    }
}

pub type Metrics = Telemetry;

pub fn time<T>(metrics: &mut Metrics, name: &'static str, f: impl FnOnce() -> T) -> T {
    metrics.increment(name);
    let timer = Timer::start();
    let value = f();
    metrics.observe_latency(LatencyMetric::ResultStreaming, timer.elapsed());
    value
}

fn duration_div(duration: Duration, divisor: u64) -> Duration {
    if divisor == 0 {
        return Duration::ZERO;
    }
    let nanos = duration.as_nanos() / u128::from(divisor);
    let bounded = nanos.min(u128::from(u64::MAX));
    Duration::from_nanos(bounded as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_all_required_latency_metrics() {
        let mut telemetry = Telemetry::default();
        for (index, metric) in LatencyMetric::ALL.into_iter().enumerate() {
            telemetry.observe_latency(metric, Duration::from_millis((index + 1) as u64));
        }

        for metric in LatencyMetric::ALL {
            assert_eq!(telemetry.latency(metric).count, 1, "{}", metric.as_str());
        }
    }

    #[test]
    fn histogram_reports_bounded_percentiles_without_raw_samples() {
        let mut histogram = Histogram::new(&[
            Duration::from_millis(1),
            Duration::from_millis(10),
            Duration::from_millis(100),
        ]);
        histogram.observe(Duration::from_micros(500));
        histogram.observe(Duration::from_millis(2));
        histogram.observe(Duration::from_millis(90));
        histogram.observe(Duration::from_millis(250));

        let summary = histogram.summary();

        assert_eq!(summary.count, 4);
        assert_eq!(summary.min, Some(Duration::from_micros(500)));
        assert_eq!(summary.max, Some(Duration::from_millis(250)));
        assert_eq!(summary.p50, Some(Duration::from_millis(10)));
        assert_eq!(summary.p95, Some(Duration::from_millis(250)));
        assert_eq!(summary.overflow, 1);
    }

    #[test]
    fn frame_timing_detects_ui_thread_stalls() {
        let mut frames = FrameTiming::with_stall_threshold(Duration::from_millis(16));

        assert_eq!(frames.observe(Duration::from_millis(12)), None);
        let stall = frames.observe(Duration::from_millis(45)).unwrap();
        frames.observe(Duration::from_millis(17)).unwrap();

        let summary = frames.summary();
        assert_eq!(stall.frame_index, 2);
        assert_eq!(summary.histogram.count, 3);
        assert_eq!(summary.stall_count, 2);
        assert_eq!(summary.worst_stall, Some(Duration::from_millis(45)));
    }

    #[test]
    fn telemetry_tracks_counters_and_timed_operations() {
        let mut telemetry = Telemetry::default();
        let value = telemetry.time_latency(LatencyMetric::SearchKeystroke, || 42);
        telemetry.increment("searches");

        assert_eq!(value, 42);
        assert_eq!(telemetry.counter("searches"), 1);
        assert_eq!(telemetry.latency(LatencyMetric::SearchKeystroke).count, 1);
    }
}
