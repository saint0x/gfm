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
    pub refresh_on_permission_change: bool,
}

impl OperationAccessDecision {
    pub fn allow(reason: impl Into<String>) -> Self {
        Self {
            action: OperationAccessAction::Allow,
            reason: reason.into(),
            refresh_on_permission_change: false,
        }
    }

    pub fn prompt(reason: impl Into<String>) -> Self {
        Self {
            action: OperationAccessAction::Prompt,
            reason: reason.into(),
            refresh_on_permission_change: false,
        }
    }

    pub fn deny(reason: impl Into<String>) -> Self {
        Self {
            action: OperationAccessAction::Deny,
            reason: reason.into(),
            refresh_on_permission_change: false,
        }
    }

    pub fn with_refresh_on_permission_change(mut self, refresh: bool) -> Self {
        self.refresh_on_permission_change = refresh;
        self
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

    pub fn check(&self, operation: &Operation) -> Result<()> {
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
                            "{} requires a permission prompt before mutation: {}{}",
                            requirement.role.as_str(),
                            decision.reason,
                            refresh_reason_suffix(decision.refresh_on_permission_change)
                        ),
                    });
                }
                OperationAccessAction::Deny => {
                    return Err(GfmError::Permission {
                        path: requirement.path,
                        message: format!(
                            "{} is not accessible for mutation: {}{}",
                            requirement.role.as_str(),
                            decision.reason,
                            refresh_reason_suffix(decision.refresh_on_permission_change)
                        ),
                    });
                }
            }
        }
        Ok(())
    }
}

fn refresh_reason_suffix(refresh_on_permission_change: bool) -> &'static str {
    if refresh_on_permission_change {
        "; refresh-on-permission-change=true"
    } else {
        ""
    }
}
