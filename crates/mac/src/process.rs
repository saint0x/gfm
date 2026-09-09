use gfm_mac_sys::NativeProcessMemoryStatus;
use gfm_types::{GfmError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessMemoryReport {
    pub peak_resident_bytes: u64,
}

pub fn current_process_memory() -> Result<ProcessMemoryReport> {
    let native = gfm_mac_sys::copy_process_memory();
    if native.status != NativeProcessMemoryStatus::Available {
        return Err(GfmError::Format(
            "macOS process memory probe unavailable".to_string(),
        ));
    }
    Ok(ProcessMemoryReport {
        peak_resident_bytes: native.peak_resident_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_current_process_peak_resident_memory() {
        let report = current_process_memory().unwrap();

        assert!(report.peak_resident_bytes > 0);
    }
}
