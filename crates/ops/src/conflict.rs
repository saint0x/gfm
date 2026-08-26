use crate::target::keep_both_path;
use crate::{Operation, OperationStatus};
use gfm_types::Result;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictPolicy {
    Fail,
    Replace,
    KeepBoth,
    Merge,
    Skip,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationConflictPlan {
    pub default: ConflictPolicy,
    target_overrides: BTreeMap<PathBuf, ConflictPolicy>,
}

impl Default for OperationConflictPlan {
    fn default() -> Self {
        Self {
            default: ConflictPolicy::Fail,
            target_overrides: BTreeMap::new(),
        }
    }
}

impl OperationConflictPlan {
    pub fn new(default: ConflictPolicy) -> Self {
        Self {
            default,
            target_overrides: BTreeMap::new(),
        }
    }

    pub fn with_target(mut self, target: impl Into<PathBuf>, conflict: ConflictPolicy) -> Self {
        self.target_overrides.insert(target.into(), conflict);
        self
    }

    pub(crate) fn conflict_for(&self, operation: &Operation) -> ConflictPolicy {
        operation
            .target_path()
            .and_then(|target| self.target_overrides.get(target))
            .copied()
            .unwrap_or(self.default)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationBatchReport {
    pub outcomes: Vec<OperationBatchOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationBatchOutcome {
    pub conflict: ConflictPolicy,
    pub status: OperationStatus,
    pub operation: Operation,
    pub message: Option<String>,
}

pub(crate) fn resolve_operation_conflicts(
    operation: Operation,
    conflict: ConflictPolicy,
) -> Result<Operation> {
    if conflict != ConflictPolicy::KeepBoth {
        return Ok(operation);
    }
    match operation {
        Operation::Copy { from, to } => Ok(Operation::Copy {
            from,
            to: keep_both_path(&to)?,
        }),
        Operation::Move { from, to } => Ok(Operation::Move {
            from,
            to: keep_both_path(&to)?,
        }),
        Operation::Rename { from, to } => Ok(Operation::Rename {
            from,
            to: keep_both_path(&to)?,
        }),
        Operation::Delete { path } => Ok(Operation::Delete { path }),
        Operation::Trash { path } => Ok(Operation::Trash { path }),
        Operation::EmptyTrash { path } => Ok(Operation::EmptyTrash { path }),
        Operation::Restore { from, to } => Ok(Operation::Restore {
            from,
            to: keep_both_path(&to)?,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_plan_uses_default_when_operation_has_no_target() {
        let plan = OperationConflictPlan::new(ConflictPolicy::Skip);

        assert_eq!(
            plan.conflict_for(&Operation::Delete {
                path: "/tmp/source.txt".into()
            }),
            ConflictPolicy::Skip
        );
    }

    #[test]
    fn conflict_plan_prefers_exact_target_override() {
        let replace_target = PathBuf::from("/tmp/replace.txt");
        let keep_target = PathBuf::from("/tmp/keep.txt");
        let plan = OperationConflictPlan::new(ConflictPolicy::Skip)
            .with_target(&replace_target, ConflictPolicy::Replace);

        assert_eq!(
            plan.conflict_for(&Operation::Copy {
                from: "/tmp/source.txt".into(),
                to: replace_target
            }),
            ConflictPolicy::Replace
        );
        assert_eq!(
            plan.conflict_for(&Operation::Copy {
                from: "/tmp/source.txt".into(),
                to: keep_target
            }),
            ConflictPolicy::Skip
        );
    }
}
