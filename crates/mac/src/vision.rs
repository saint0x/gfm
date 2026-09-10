use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisionTextRecognitionReport {
    pub text: String,
    pub lines: Vec<String>,
    pub status: VisionTextRecognitionStatus,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisionTextRecognitionStatus {
    Recognized,
    Empty,
    Missing,
    Unsupported,
    Failed,
    Unavailable,
}

impl VisionTextRecognitionStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Recognized => "recognized",
            Self::Empty => "empty",
            Self::Missing => "missing",
            Self::Unsupported => "unsupported",
            Self::Failed => "failed",
            Self::Unavailable => "unavailable",
        }
    }
}

impl VisionTextRecognitionReport {
    pub fn recognize_image(path: impl AsRef<Path>) -> Self {
        let native = gfm_mac_sys::recognize_text_for_image(path.as_ref());
        let status = match native.status {
            gfm_mac_sys::NativeTextRecognitionStatus::Available => {
                VisionTextRecognitionStatus::Recognized
            }
            gfm_mac_sys::NativeTextRecognitionStatus::Empty => VisionTextRecognitionStatus::Empty,
            gfm_mac_sys::NativeTextRecognitionStatus::Missing => {
                VisionTextRecognitionStatus::Missing
            }
            gfm_mac_sys::NativeTextRecognitionStatus::Unsupported => {
                VisionTextRecognitionStatus::Unsupported
            }
            gfm_mac_sys::NativeTextRecognitionStatus::Failed => VisionTextRecognitionStatus::Failed,
            gfm_mac_sys::NativeTextRecognitionStatus::Unavailable => {
                VisionTextRecognitionStatus::Unavailable
            }
        };
        let text = native.lines.join("\n");
        Self {
            text,
            lines: native.lines,
            status,
            reason: native.reason,
        }
    }

    pub fn missing(reason: impl Into<String>) -> Self {
        Self {
            text: String::new(),
            lines: Vec::new(),
            status: VisionTextRecognitionStatus::Missing,
            reason: Some(reason.into()),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn status(&self) -> VisionTextRecognitionStatus {
        self.status
    }

    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "vision-text-recognition\tstatus={}\tlines={}\ttext-bytes={}\treason={}",
            self.status.as_str(),
            self.lines.len(),
            self.text.len(),
            self.reason
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string())
        )
    }
}

fn escape_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_tsv_escapes_reason_control_characters() {
        let report = VisionTextRecognitionReport {
            text: "recognized text".to_string(),
            lines: vec!["recognized text".to_string()],
            status: VisionTextRecognitionStatus::Recognized,
            reason: Some("native\tmessage\nwith\rcontrols".to_string()),
        };

        let tsv = report.as_tsv();

        assert_eq!(tsv.lines().count(), 1);
        assert!(tsv.contains("status=recognized"));
        assert!(tsv.contains("reason=native\\tmessage\\nwith\\rcontrols"));
    }

    #[test]
    fn missing_path_maps_to_missing_report() {
        let path = std::env::temp_dir().join(format!("gfm-vision-missing-{}", std::process::id()));

        let report = VisionTextRecognitionReport::recognize_image(path);

        assert_eq!(report.status, VisionTextRecognitionStatus::Missing);
        assert!(report.text.is_empty());
        assert!(report.lines.is_empty());
    }
}
