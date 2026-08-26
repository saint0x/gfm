use crate::Operation;
use gfm_types::{GfmError, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(crate) fn destination_probe_path(path: &Path) -> PathBuf {
    path.parent().unwrap_or(path).to_path_buf()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationAccessRole {
    Source,
    DestinationParent,
    Target,
}

impl OperationAccessRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::DestinationParent => "destination-parent",
            Self::Target => "target",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationAccessRequirement {
    pub path: PathBuf,
    pub role: OperationAccessRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationAccessAction {
    Allow,
    Prompt,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationAccessDecision {
    pub action: OperationAccessAction,
    pub reason: String,
}

impl OperationAccessDecision {
    pub fn allow(reason: impl Into<String>) -> Self {
        Self {
            action: OperationAccessAction::Allow,
            reason: reason.into(),
        }
    }

    pub fn prompt(reason: impl Into<String>) -> Self {
        Self {
            action: OperationAccessAction::Prompt,
            reason: reason.into(),
        }
    }

    pub fn deny(reason: impl Into<String>) -> Self {
        Self {
            action: OperationAccessAction::Deny,
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OperationAccessGate {
    decisions: BTreeMap<PathBuf, OperationAccessDecision>,
}

impl OperationAccessGate {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_decision(
        mut self,
        path: impl Into<PathBuf>,
        decision: OperationAccessDecision,
    ) -> Self {
        self.decisions.insert(path.into(), decision);
        self
    }

    pub(crate) fn check(&self, operation: &Operation) -> Result<()> {
        for requirement in operation.access_requirements() {
            let Some(decision) = self.decisions.get(&requirement.path) else {
                continue;
            };
            match decision.action {
                OperationAccessAction::Allow => {}
                OperationAccessAction::Prompt => {
                    return Err(GfmError::Permission {
                        path: requirement.path,
                        message: format!(
                            "{} requires a permission prompt before mutation: {}",
                            requirement.role.as_str(),
                            decision.reason
                        ),
                    });
                }
                OperationAccessAction::Deny => {
                    return Err(GfmError::Permission {
                        path: requirement.path,
                        message: format!(
                            "{} is not accessible for mutation: {}",
                            requirement.role.as_str(),
                            decision.reason
                        ),
                    });
                }
            }
        }
        Ok(())
    }
}
