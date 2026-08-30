use crate::{LatencyMetric, Telemetry};
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiagnosticPrivacy {
    pub include_paths: bool,
    pub include_query_text: bool,
    pub include_user_identifiers: bool,
}

impl DiagnosticPrivacy {
    pub fn review(self) -> PrivacyReview {
        let mut blocked_fields = Vec::new();
        if self.include_paths {
            blocked_fields.push("paths");
        }
        if self.include_query_text {
            blocked_fields.push("query_text");
        }
        if self.include_user_identifiers {
            blocked_fields.push("user_identifiers");
        }
        PrivacyReview {
            approved: blocked_fields.is_empty(),
            blocked_fields,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivacyReview {
    pub approved: bool,
    pub blocked_fields: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticExportReceipt {
    pub path: PathBuf,
    pub bytes_written: u64,
    pub privacy: PrivacyReview,
}

#[derive(Debug)]
pub enum DiagnosticExportError {
    Cancelled,
    Privacy(PrivacyReview),
    Io { path: PathBuf, error: io::Error },
}

impl fmt::Display for DiagnosticExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => write!(f, "diagnostic export cancelled"),
            Self::Privacy(review) => write!(
                f,
                "diagnostic export rejected by privacy review: {}",
                review.blocked_fields.join(", ")
            ),
            Self::Io { path, error } => write!(f, "{}: {}", path.display(), error),
        }
    }
}

impl std::error::Error for DiagnosticExportError {}

pub fn export_diagnostics(
    path: impl AsRef<Path>,
    telemetry: &Telemetry,
    privacy: DiagnosticPrivacy,
) -> Result<DiagnosticExportReceipt, DiagnosticExportError> {
    export_diagnostics_checked(path, telemetry, privacy, || Ok(()))
}

pub fn export_diagnostics_checked(
    path: impl AsRef<Path>,
    telemetry: &Telemetry,
    privacy: DiagnosticPrivacy,
    mut check_control: impl FnMut() -> Result<(), DiagnosticExportError>,
) -> Result<DiagnosticExportReceipt, DiagnosticExportError> {
    let path = path.as_ref();
    check_control()?;
    let review = privacy.review();
    if !review.approved {
        return Err(DiagnosticExportError::Privacy(review));
    }

    check_control()?;
    let encoded = encode_report(telemetry, &review);
    check_control()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| DiagnosticExportError::Io {
            path: parent.to_path_buf(),
            error,
        })?;
    }
    check_control()?;

    let temp_path = temp_path(path);
    let result = (|| {
        let mut file = File::create(&temp_path).map_err(|error| DiagnosticExportError::Io {
            path: temp_path.clone(),
            error,
        })?;
        check_control()?;
        for chunk in encoded.as_bytes().chunks(64 * 1024) {
            check_control()?;
            file.write_all(chunk)
                .map_err(|error| DiagnosticExportError::Io {
                    path: temp_path.clone(),
                    error,
                })?;
        }
        check_control()?;
        file.sync_all().map_err(|error| DiagnosticExportError::Io {
            path: temp_path.clone(),
            error,
        })?;
        check_control()?;
        fs::rename(&temp_path, path).map_err(|error| DiagnosticExportError::Io {
            path: path.to_path_buf(),
            error,
        })?;
        sync_parent(path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result?;

    Ok(DiagnosticExportReceipt {
        path: path.to_path_buf(),
        bytes_written: encoded.len() as u64,
        privacy: review,
    })
}

fn encode_report(telemetry: &Telemetry, review: &PrivacyReview) -> String {
    let resources = telemetry.resources();
    let frames = telemetry.frame_timing();
    let mut out = String::new();
    out.push_str("{\n");
    push_u64(
        &mut out,
        1,
        "schema_version",
        u64::from(SCHEMA_VERSION),
        true,
    );
    push_object_start(&mut out, 1, "privacy", true);
    push_bool(&mut out, 2, "approved", review.approved, true);
    push_string_array(&mut out, 2, "blocked_fields", &review.blocked_fields, false);
    push_object_end(&mut out, 1, true);

    push_object_start(&mut out, 1, "latencies", true);
    for (index, metric) in LatencyMetric::ALL.into_iter().enumerate() {
        push_histogram(
            &mut out,
            2,
            metric.as_str(),
            telemetry.latency(metric),
            index + 1 != LatencyMetric::ALL.len(),
        );
    }
    push_object_end(&mut out, 1, true);

    push_object_start(&mut out, 1, "frames", true);
    push_duration(
        &mut out,
        2,
        "stall_threshold_ns",
        frames.stall_threshold,
        true,
    );
    push_u64(&mut out, 2, "stall_count", frames.stall_count, true);
    push_optional_duration(&mut out, 2, "worst_stall_ns", frames.worst_stall, true);
    push_histogram(&mut out, 2, "duration", frames.histogram, false);
    push_object_end(&mut out, 1, true);

    push_object_start(&mut out, 1, "resources", true);
    push_object_start(&mut out, 2, "io", true);
    push_u64(&mut out, 3, "read_bytes", resources.io.read_bytes, true);
    push_u64(
        &mut out,
        3,
        "written_bytes",
        resources.io.written_bytes,
        true,
    );
    push_u64(&mut out, 3, "read_ops", resources.io.read_ops, true);
    push_u64(&mut out, 3, "write_ops", resources.io.write_ops, false);
    push_object_end(&mut out, 2, true);

    push_object_start(&mut out, 2, "cpu", true);
    push_u64(&mut out, 3, "samples", resources.cpu.samples, true);
    push_optional_f64(
        &mut out,
        3,
        "mean_user_percent",
        resources.cpu.mean_user_percent,
        true,
    );
    push_optional_f64(
        &mut out,
        3,
        "mean_system_percent",
        resources.cpu.mean_system_percent,
        true,
    );
    push_optional_f64(
        &mut out,
        3,
        "peak_total_percent",
        resources.cpu.peak_total_percent,
        false,
    );
    push_object_end(&mut out, 2, true);

    push_object_start(&mut out, 2, "memory", true);
    push_u64(&mut out, 3, "samples", resources.memory.samples, true);
    push_u64(
        &mut out,
        3,
        "peak_resident_bytes",
        resources.memory.peak_resident_bytes,
        true,
    );
    push_u64(
        &mut out,
        3,
        "peak_virtual_bytes",
        resources.memory.peak_virtual_bytes,
        false,
    );
    push_object_end(&mut out, 2, true);

    push_object_start(&mut out, 2, "allocations", true);
    push_u64(
        &mut out,
        3,
        "allocated_bytes",
        resources.allocations.allocated_bytes,
        true,
    );
    push_u64(
        &mut out,
        3,
        "freed_bytes",
        resources.allocations.freed_bytes,
        true,
    );
    push_u64(
        &mut out,
        3,
        "in_use_bytes",
        resources.allocations.in_use_bytes,
        true,
    );
    push_u64(
        &mut out,
        3,
        "allocation_count",
        resources.allocations.allocation_count,
        true,
    );
    push_u64(
        &mut out,
        3,
        "free_count",
        resources.allocations.free_count,
        true,
    );
    push_u64(
        &mut out,
        3,
        "peak_in_use_bytes",
        resources.allocations.peak_in_use_bytes,
        false,
    );
    push_object_end(&mut out, 2, true);

    push_object_start(&mut out, 2, "queues", true);
    for (index, (name, queue)) in resources.queues.iter().enumerate() {
        push_object_start(&mut out, 3, name, true);
        push_u64(&mut out, 4, "samples", queue.samples, true);
        push_u64(&mut out, 4, "current_depth", queue.current_depth, true);
        push_u64(&mut out, 4, "peak_depth", queue.peak_depth, false);
        push_object_end(&mut out, 3, index + 1 != resources.queues.len());
    }
    push_object_end(&mut out, 2, true);

    push_object_start(&mut out, 2, "compaction", false);
    push_u64(&mut out, 3, "runs", resources.compaction.runs, true);
    push_u64(
        &mut out,
        3,
        "input_segments",
        resources.compaction.input_segments,
        true,
    );
    push_u64(
        &mut out,
        3,
        "output_segments",
        resources.compaction.output_segments,
        true,
    );
    push_u64(
        &mut out,
        3,
        "input_bytes",
        resources.compaction.input_bytes,
        true,
    );
    push_u64(
        &mut out,
        3,
        "output_bytes",
        resources.compaction.output_bytes,
        true,
    );
    push_u64(
        &mut out,
        3,
        "tombstones_removed",
        resources.compaction.tombstones_removed,
        true,
    );
    push_histogram(
        &mut out,
        3,
        "duration",
        resources.compaction.duration,
        false,
    );
    push_object_end(&mut out, 2, false);
    push_object_end(&mut out, 1, false);
    out.push_str("}\n");
    out
}

fn temp_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("diagnostics.json");
    path.with_file_name(format!(
        ".{file_name}.{}.{nonce}.tmp",
        std::process::id(),
        nonce = now_nanos()
    ))
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

fn sync_parent(path: &Path) -> Result<(), DiagnosticExportError> {
    if let Some(parent) = path.parent() {
        File::open(parent)
            .and_then(|file| file.sync_all())
            .map_err(|error| DiagnosticExportError::Io {
                path: parent.to_path_buf(),
                error,
            })?;
    }
    Ok(())
}

fn push_histogram(
    out: &mut String,
    indent: usize,
    name: &str,
    summary: crate::HistogramSummary,
    comma: bool,
) {
    push_object_start(out, indent, name, true);
    push_u64(out, indent + 1, "count", summary.count, true);
    push_optional_duration(out, indent + 1, "min_ns", summary.min, true);
    push_optional_duration(out, indent + 1, "max_ns", summary.max, true);
    push_optional_duration(out, indent + 1, "mean_ns", summary.mean, true);
    push_optional_duration(out, indent + 1, "p50_ns", summary.p50, true);
    push_optional_duration(out, indent + 1, "p95_ns", summary.p95, true);
    push_optional_duration(out, indent + 1, "p99_ns", summary.p99, true);
    push_u64(out, indent + 1, "overflow", summary.overflow, false);
    push_object_end(out, indent, comma);
}

fn push_object_start(out: &mut String, indent: usize, name: &str, _comma: bool) {
    push_indent(out, indent);
    out.push('"');
    out.push_str(&escape_json(name));
    out.push_str("\": {\n");
}

fn push_object_end(out: &mut String, indent: usize, comma: bool) {
    push_indent(out, indent);
    out.push('}');
    if comma {
        out.push(',');
    }
    out.push('\n');
}

fn push_u64(out: &mut String, indent: usize, name: &str, value: u64, comma: bool) {
    push_indent(out, indent);
    out.push('"');
    out.push_str(&escape_json(name));
    out.push_str("\": ");
    out.push_str(&value.to_string());
    finish_value(out, comma);
}

fn push_bool(out: &mut String, indent: usize, name: &str, value: bool, comma: bool) {
    push_indent(out, indent);
    out.push('"');
    out.push_str(&escape_json(name));
    out.push_str("\": ");
    out.push_str(if value { "true" } else { "false" });
    finish_value(out, comma);
}

fn push_duration(out: &mut String, indent: usize, name: &str, value: Duration, comma: bool) {
    push_u64(out, indent, name, duration_ns(value), comma);
}

fn push_optional_duration(
    out: &mut String,
    indent: usize,
    name: &str,
    value: Option<Duration>,
    comma: bool,
) {
    match value {
        Some(value) => push_duration(out, indent, name, value, comma),
        None => push_null(out, indent, name, comma),
    }
}

fn push_optional_f64(out: &mut String, indent: usize, name: &str, value: Option<f64>, comma: bool) {
    match value {
        Some(value) if value.is_finite() => {
            push_indent(out, indent);
            out.push('"');
            out.push_str(&escape_json(name));
            out.push_str("\": ");
            out.push_str(&format!("{value:.3}"));
            finish_value(out, comma);
        }
        _ => push_null(out, indent, name, comma),
    }
}

fn push_null(out: &mut String, indent: usize, name: &str, comma: bool) {
    push_indent(out, indent);
    out.push('"');
    out.push_str(&escape_json(name));
    out.push_str("\": null");
    finish_value(out, comma);
}

fn push_string_array(out: &mut String, indent: usize, name: &str, values: &[&str], comma: bool) {
    push_indent(out, indent);
    out.push('"');
    out.push_str(&escape_json(name));
    out.push_str("\": [");
    for (index, value) in values.iter().enumerate() {
        if index != 0 {
            out.push_str(", ");
        }
        out.push('"');
        out.push_str(&escape_json(value));
        out.push('"');
    }
    out.push(']');
    finish_value(out, comma);
}

fn push_indent(out: &mut String, indent: usize) {
    for _ in 0..indent {
        out.push_str("  ");
    }
}

fn finish_value(out: &mut String, comma: bool) {
    if comma {
        out.push(',');
    }
    out.push('\n');
}

fn duration_ns(value: Duration) -> u64 {
    value.as_nanos().min(u128::from(u64::MAX)) as u64
}

fn escape_json(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            ch if ch.is_control() => escaped.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => escaped.push(ch),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AllocationSample, CpuSample, IoSample, LatencyMetric, MemorySample};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn exports_aggregate_diagnostics_without_sensitive_fields() {
        let root = unique_temp_dir("gfm-diagnostics-export");
        let path = root.join("diagnostics.json");
        let mut telemetry = Telemetry::default();
        telemetry.observe_latency(LatencyMetric::Navigation, Duration::from_millis(2));
        telemetry.observe_io(IoSample {
            read_bytes: 512,
            written_bytes: 128,
            read_ops: 4,
            write_ops: 1,
        });
        telemetry.observe_cpu(CpuSample {
            user_percent: 25.0,
            system_percent: 5.0,
        });
        telemetry.observe_memory(MemorySample {
            resident_bytes: 1024,
            virtual_bytes: 2048,
        });
        telemetry.observe_allocation(AllocationSample {
            allocated_bytes: 300,
            freed_bytes: 100,
            allocation_count: 3,
            free_count: 1,
        });
        telemetry.observe_queue_depth("index", 7);

        let receipt = export_diagnostics(&path, &telemetry, DiagnosticPrivacy::default()).unwrap();
        let exported = fs::read_to_string(&path).unwrap();

        assert_eq!(receipt.path, path);
        assert!(receipt.privacy.approved);
        assert!(receipt.bytes_written > 0);
        assert!(exported.contains("\"schema_version\": 1"));
        assert!(exported.contains("\"navigation\""));
        assert!(exported.contains("\"read_bytes\": 512"));
        assert!(exported.contains("\"peak_resident_bytes\": 1024"));
        assert!(!exported.contains("query_text"));
        assert!(!exported.contains("user_identifiers"));
        assert_eq!(diagnostic_temp_count(&path), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_sensitive_diagnostics_before_writing() {
        let root = unique_temp_dir("gfm-diagnostics-privacy");
        let path = root.join("diagnostics.json");
        let telemetry = Telemetry::default();
        let privacy = DiagnosticPrivacy {
            include_paths: true,
            include_query_text: true,
            include_user_identifiers: false,
        };

        let err = export_diagnostics(&path, &telemetry, privacy).unwrap_err();

        let DiagnosticExportError::Privacy(review) = err else {
            unreachable!("expected privacy rejection");
        };
        assert!(!review.approved);
        assert_eq!(review.blocked_fields, vec!["paths", "query_text"]);
        assert!(!path.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn checked_export_honors_pre_cancelled_control_before_privacy_review() {
        let root = unique_temp_dir("gfm-diagnostics-pre-cancel");
        let path = root.join("diagnostics.json");
        let telemetry = Telemetry::default();

        let err =
            export_diagnostics_checked(&path, &telemetry, DiagnosticPrivacy::default(), || {
                Err(DiagnosticExportError::Cancelled)
            })
            .unwrap_err();

        assert!(matches!(err, DiagnosticExportError::Cancelled));
        assert!(!path.exists());
        assert_eq!(diagnostic_temp_count(&path), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn checked_export_preserves_existing_report_on_mid_write_cancel() {
        let root = unique_temp_dir("gfm-diagnostics-mid-write-cancel");
        let path = root.join("diagnostics.json");
        let mut telemetry = Telemetry::default();
        telemetry.observe_latency(LatencyMetric::Navigation, Duration::from_millis(1));
        export_diagnostics(&path, &telemetry, DiagnosticPrivacy::default()).unwrap();
        let before = fs::read(&path).unwrap();

        for _ in 0..8192 {
            telemetry.observe_queue_depth("index", 42);
        }
        let mut checks = 0usize;

        let err =
            export_diagnostics_checked(&path, &telemetry, DiagnosticPrivacy::default(), || {
                checks += 1;
                if checks >= 7 {
                    Err(DiagnosticExportError::Cancelled)
                } else {
                    Ok(())
                }
            })
            .unwrap_err();

        assert!(matches!(err, DiagnosticExportError::Cancelled));
        assert!(checks >= 7);
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(diagnostic_temp_count(&path), 0);
        fs::remove_dir_all(root).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn diagnostic_temp_count(path: &Path) -> usize {
        let Some(parent) = path.parent() else {
            return 0;
        };
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            return 0;
        };
        let prefix = format!(".{file_name}.{}.", std::process::id());
        fs::read_dir(parent)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
            .count()
    }
}
