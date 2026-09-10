use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfPageRasterizationReport {
    pub pages: Vec<PathBuf>,
    pub status: PdfPageRasterizationStatus,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdfPageRasterizationStatus {
    Available,
    Empty,
    Missing,
    Unsupported,
    Failed,
    Unavailable,
}

impl PdfPageRasterizationStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Empty => "empty",
            Self::Missing => "missing",
            Self::Unsupported => "unsupported",
            Self::Failed => "failed",
            Self::Unavailable => "unavailable",
        }
    }
}

impl PdfPageRasterizationReport {
    pub fn rasterize_for_ocr(
        pdf: impl AsRef<Path>,
        output_dir: impl AsRef<Path>,
        max_pages: usize,
        max_dimension_px: u32,
    ) -> Self {
        let native = gfm_mac_sys::rasterize_pdf_pages_for_ocr(
            pdf.as_ref(),
            output_dir.as_ref(),
            max_pages,
            max_dimension_px,
        );
        let status = match native.status {
            gfm_mac_sys::NativePdfPageRasterizationStatus::Available => {
                PdfPageRasterizationStatus::Available
            }
            gfm_mac_sys::NativePdfPageRasterizationStatus::Empty => {
                PdfPageRasterizationStatus::Empty
            }
            gfm_mac_sys::NativePdfPageRasterizationStatus::Missing => {
                PdfPageRasterizationStatus::Missing
            }
            gfm_mac_sys::NativePdfPageRasterizationStatus::Unsupported => {
                PdfPageRasterizationStatus::Unsupported
            }
            gfm_mac_sys::NativePdfPageRasterizationStatus::Failed => {
                PdfPageRasterizationStatus::Failed
            }
            gfm_mac_sys::NativePdfPageRasterizationStatus::Unavailable => {
                PdfPageRasterizationStatus::Unavailable
            }
        };
        Self {
            pages: native.pages,
            status,
            reason: native.reason,
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "pdf-ocr-rasterization\tstatus={}\tpages={}\treason={}",
            self.status.as_str(),
            self.pages.len(),
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
    fn rasterization_tsv_escapes_reason_control_characters() {
        let report = PdfPageRasterizationReport {
            pages: vec![PathBuf::from("/tmp/page.png")],
            status: PdfPageRasterizationStatus::Failed,
            reason: Some("native\tmessage\nwith\rcontrols".to_string()),
        };

        let tsv = report.as_tsv();

        assert_eq!(tsv.lines().count(), 1);
        assert!(tsv.contains("status=failed"));
        assert!(tsv.contains("pages=1"));
        assert!(tsv.contains("reason=native\\tmessage\\nwith\\rcontrols"));
    }

    #[test]
    fn missing_pdf_maps_to_missing_report() {
        let pdf = std::env::temp_dir().join(format!("gfm-pdf-missing-{}", std::process::id()));
        let output =
            std::env::temp_dir().join(format!("gfm-pdf-missing-output-{}", std::process::id()));

        let report = PdfPageRasterizationReport::rasterize_for_ocr(pdf, output, 1, 512);

        assert_eq!(report.status, PdfPageRasterizationStatus::Missing);
        assert!(report.pages.is_empty());
    }
}
