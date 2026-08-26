use gfm_mac::{AccessIntent, SecurityDecisionAction, SecurityScopedAccessReport};
use gfm_types::{GfmError, Result};
use std::path::Path;

pub(crate) fn preflight_access(path: &Path, intent: AccessIntent, worker: &str) -> Result<()> {
    let report = SecurityScopedAccessReport::evaluate(path, intent);
    eprintln!("{}", report.as_tsv());
    match report.action {
        SecurityDecisionAction::Allow | SecurityDecisionAction::Degrade => Ok(()),
        SecurityDecisionAction::Prompt | SecurityDecisionAction::Deny => {
            Err(GfmError::Permission {
                path: path.to_path_buf(),
                message: format!("{worker} access blocked: {}", report.reason),
            })
        }
    }
}
