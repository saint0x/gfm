mod budgets;
mod diagnostics;

pub use budgets::{
    BudgetEvaluation, BudgetViolation, LatencyBudget, Percentile, PerformanceBudgets,
    ScenarioBudget, ScenarioMetric,
};
pub use diagnostics::{
    export_diagnostics, DiagnosticExportError, DiagnosticExportReceipt, DiagnosticPrivacy,
    PrivacyReview,
};

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

#[derive(Debug, Clone, PartialEq)]
pub struct Telemetry {
    latencies: BTreeMap<LatencyMetric, Histogram>,
    frames: FrameTiming,
    resources: ResourceTelemetry,
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
            resources: ResourceTelemetry::default(),
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

    pub fn observe_io(&mut self, sample: IoSample) {
        self.resources.observe_io(sample);
    }

    pub fn observe_cpu(&mut self, sample: CpuSample) {
        self.resources.observe_cpu(sample);
    }

    pub fn observe_memory(&mut self, sample: MemorySample) {
        self.resources.observe_memory(sample);
    }

    pub fn observe_allocation(&mut self, sample: AllocationSample) {
        self.resources.observe_allocation(sample);
    }

    pub fn observe_queue_depth(&mut self, queue: &'static str, depth: u64) {
        self.resources.observe_queue_depth(queue, depth);
    }

    pub fn observe_compaction(&mut self, sample: CompactionSample) {
        self.resources.observe_compaction(sample);
    }

    pub fn resources(&self) -> ResourceSummary {
        self.resources.summary()
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

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResourceTelemetry {
    io: IoTelemetry,
    cpu: CpuTelemetry,
    memory: MemoryTelemetry,
    allocations: AllocationTelemetry,
    queues: BTreeMap<&'static str, QueueTelemetry>,
    compaction: CompactionTelemetry,
}

impl ResourceTelemetry {
    pub fn observe_io(&mut self, sample: IoSample) {
        self.io.observe(sample);
    }

    pub fn observe_cpu(&mut self, sample: CpuSample) {
        self.cpu.observe(sample);
    }

    pub fn observe_memory(&mut self, sample: MemorySample) {
        self.memory.observe(sample);
    }

    pub fn observe_allocation(&mut self, sample: AllocationSample) {
        self.allocations.observe(sample);
    }

    pub fn observe_queue_depth(&mut self, queue: &'static str, depth: u64) {
        self.queues.entry(queue).or_default().observe(depth);
    }

    pub fn observe_compaction(&mut self, sample: CompactionSample) {
        self.compaction.observe(sample);
    }

    pub fn summary(&self) -> ResourceSummary {
        ResourceSummary {
            io: self.io.summary(),
            cpu: self.cpu.summary(),
            memory: self.memory.summary(),
            allocations: self.allocations.summary(),
            queues: self
                .queues
                .iter()
                .map(|(name, queue)| (*name, queue.summary()))
                .collect(),
            compaction: self.compaction.summary(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IoSample {
    pub read_bytes: u64,
    pub written_bytes: u64,
    pub read_ops: u64,
    pub write_ops: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IoTelemetry {
    read_bytes: u64,
    written_bytes: u64,
    read_ops: u64,
    write_ops: u64,
}

impl IoTelemetry {
    fn observe(&mut self, sample: IoSample) {
        self.read_bytes = self.read_bytes.saturating_add(sample.read_bytes);
        self.written_bytes = self.written_bytes.saturating_add(sample.written_bytes);
        self.read_ops = self.read_ops.saturating_add(sample.read_ops);
        self.write_ops = self.write_ops.saturating_add(sample.write_ops);
    }

    fn summary(&self) -> IoSummary {
        IoSummary {
            read_bytes: self.read_bytes,
            written_bytes: self.written_bytes,
            read_ops: self.read_ops,
            write_ops: self.write_ops,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IoSummary {
    pub read_bytes: u64,
    pub written_bytes: u64,
    pub read_ops: u64,
    pub write_ops: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CpuSample {
    pub user_percent: f64,
    pub system_percent: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CpuTelemetry {
    samples: u64,
    user_percent_sum: f64,
    system_percent_sum: f64,
    peak_total_percent: f64,
}

impl CpuTelemetry {
    fn observe(&mut self, sample: CpuSample) {
        let user = sample.user_percent.max(0.0);
        let system = sample.system_percent.max(0.0);
        self.samples += 1;
        self.user_percent_sum += user;
        self.system_percent_sum += system;
        self.peak_total_percent = self.peak_total_percent.max(user + system);
    }

    fn summary(&self) -> CpuSummary {
        CpuSummary {
            samples: self.samples,
            mean_user_percent: mean_f64(self.user_percent_sum, self.samples),
            mean_system_percent: mean_f64(self.system_percent_sum, self.samples),
            peak_total_percent: (self.samples != 0).then_some(self.peak_total_percent),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CpuSummary {
    pub samples: u64,
    pub mean_user_percent: Option<f64>,
    pub mean_system_percent: Option<f64>,
    pub peak_total_percent: Option<f64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemorySample {
    pub resident_bytes: u64,
    pub virtual_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryTelemetry {
    samples: u64,
    peak_resident_bytes: u64,
    peak_virtual_bytes: u64,
}

impl MemoryTelemetry {
    fn observe(&mut self, sample: MemorySample) {
        self.samples += 1;
        self.peak_resident_bytes = self.peak_resident_bytes.max(sample.resident_bytes);
        self.peak_virtual_bytes = self.peak_virtual_bytes.max(sample.virtual_bytes);
    }

    fn summary(&self) -> MemorySummary {
        MemorySummary {
            samples: self.samples,
            peak_resident_bytes: self.peak_resident_bytes,
            peak_virtual_bytes: self.peak_virtual_bytes,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemorySummary {
    pub samples: u64,
    pub peak_resident_bytes: u64,
    pub peak_virtual_bytes: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AllocationSample {
    pub allocated_bytes: u64,
    pub freed_bytes: u64,
    pub allocation_count: u64,
    pub free_count: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AllocationTelemetry {
    allocated_bytes: u64,
    freed_bytes: u64,
    allocation_count: u64,
    free_count: u64,
    peak_in_use_bytes: u64,
}

impl AllocationTelemetry {
    fn observe(&mut self, sample: AllocationSample) {
        self.allocated_bytes = self.allocated_bytes.saturating_add(sample.allocated_bytes);
        self.freed_bytes = self.freed_bytes.saturating_add(sample.freed_bytes);
        self.allocation_count = self
            .allocation_count
            .saturating_add(sample.allocation_count);
        self.free_count = self.free_count.saturating_add(sample.free_count);
        self.peak_in_use_bytes = self.peak_in_use_bytes.max(self.in_use_bytes());
    }

    fn in_use_bytes(&self) -> u64 {
        self.allocated_bytes.saturating_sub(self.freed_bytes)
    }

    fn summary(&self) -> AllocationSummary {
        AllocationSummary {
            allocated_bytes: self.allocated_bytes,
            freed_bytes: self.freed_bytes,
            in_use_bytes: self.in_use_bytes(),
            allocation_count: self.allocation_count,
            free_count: self.free_count,
            peak_in_use_bytes: self.peak_in_use_bytes,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AllocationSummary {
    pub allocated_bytes: u64,
    pub freed_bytes: u64,
    pub in_use_bytes: u64,
    pub allocation_count: u64,
    pub free_count: u64,
    pub peak_in_use_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueueTelemetry {
    samples: u64,
    current_depth: u64,
    peak_depth: u64,
}

impl QueueTelemetry {
    fn observe(&mut self, depth: u64) {
        self.samples += 1;
        self.current_depth = depth;
        self.peak_depth = self.peak_depth.max(depth);
    }

    fn summary(&self) -> QueueSummary {
        QueueSummary {
            samples: self.samples,
            current_depth: self.current_depth,
            peak_depth: self.peak_depth,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueueSummary {
    pub samples: u64,
    pub current_depth: u64,
    pub peak_depth: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CompactionSample {
    pub input_segments: u64,
    pub output_segments: u64,
    pub input_bytes: u64,
    pub output_bytes: u64,
    pub tombstones_removed: u64,
    pub duration: Duration,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompactionTelemetry {
    runs: u64,
    input_segments: u64,
    output_segments: u64,
    input_bytes: u64,
    output_bytes: u64,
    tombstones_removed: u64,
    durations: Histogram,
}

impl CompactionTelemetry {
    fn observe(&mut self, sample: CompactionSample) {
        self.runs += 1;
        self.input_segments = self.input_segments.saturating_add(sample.input_segments);
        self.output_segments = self.output_segments.saturating_add(sample.output_segments);
        self.input_bytes = self.input_bytes.saturating_add(sample.input_bytes);
        self.output_bytes = self.output_bytes.saturating_add(sample.output_bytes);
        self.tombstones_removed = self
            .tombstones_removed
            .saturating_add(sample.tombstones_removed);
        self.durations.observe(sample.duration);
    }

    fn summary(&self) -> CompactionSummary {
        CompactionSummary {
            runs: self.runs,
            input_segments: self.input_segments,
            output_segments: self.output_segments,
            input_bytes: self.input_bytes,
            output_bytes: self.output_bytes,
            tombstones_removed: self.tombstones_removed,
            duration: self.durations.summary(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionSummary {
    pub runs: u64,
    pub input_segments: u64,
    pub output_segments: u64,
    pub input_bytes: u64,
    pub output_bytes: u64,
    pub tombstones_removed: u64,
    pub duration: HistogramSummary,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResourceSummary {
    pub io: IoSummary,
    pub cpu: CpuSummary,
    pub memory: MemorySummary,
    pub allocations: AllocationSummary,
    pub queues: BTreeMap<&'static str, QueueSummary>,
    pub compaction: CompactionSummary,
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

fn mean_f64(sum: f64, count: u64) -> Option<f64> {
    (count != 0).then_some(sum / count as f64)
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

    #[test]
    fn telemetry_tracks_resource_summaries() {
        let mut telemetry = Telemetry::default();
        telemetry.observe_io(IoSample {
            read_bytes: 10,
            written_bytes: 20,
            read_ops: 1,
            write_ops: 2,
        });
        telemetry.observe_io(IoSample {
            read_bytes: 30,
            written_bytes: 40,
            read_ops: 3,
            write_ops: 4,
        });
        telemetry.observe_cpu(CpuSample {
            user_percent: 25.0,
            system_percent: 10.0,
        });
        telemetry.observe_cpu(CpuSample {
            user_percent: 75.0,
            system_percent: 5.0,
        });
        telemetry.observe_memory(MemorySample {
            resident_bytes: 100,
            virtual_bytes: 300,
        });
        telemetry.observe_memory(MemorySample {
            resident_bytes: 200,
            virtual_bytes: 250,
        });
        telemetry.observe_allocation(AllocationSample {
            allocated_bytes: 1_000,
            freed_bytes: 250,
            allocation_count: 10,
            free_count: 4,
        });
        telemetry.observe_allocation(AllocationSample {
            allocated_bytes: 500,
            freed_bytes: 1_000,
            allocation_count: 5,
            free_count: 6,
        });
        telemetry.observe_queue_depth("index", 4);
        telemetry.observe_queue_depth("index", 9);
        telemetry.observe_queue_depth("index", 2);
        telemetry.observe_compaction(CompactionSample {
            input_segments: 4,
            output_segments: 1,
            input_bytes: 1_000,
            output_bytes: 250,
            tombstones_removed: 12,
            duration: Duration::from_millis(12),
        });

        let summary = telemetry.resources();

        assert_eq!(summary.io.read_bytes, 40);
        assert_eq!(summary.io.written_bytes, 60);
        assert_eq!(summary.cpu.samples, 2);
        assert_eq!(summary.cpu.mean_user_percent, Some(50.0));
        assert_eq!(summary.cpu.peak_total_percent, Some(80.0));
        assert_eq!(summary.memory.peak_resident_bytes, 200);
        assert_eq!(summary.allocations.in_use_bytes, 250);
        assert_eq!(summary.allocations.peak_in_use_bytes, 750);
        assert_eq!(summary.queues["index"].current_depth, 2);
        assert_eq!(summary.queues["index"].peak_depth, 9);
        assert_eq!(summary.compaction.runs, 1);
        assert_eq!(summary.compaction.output_bytes, 250);
        assert_eq!(summary.compaction.duration.count, 1);
    }
}
