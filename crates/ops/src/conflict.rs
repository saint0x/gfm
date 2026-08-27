use crate::target::{keep_both_path, path_exists_or_symlink};
use crate::{Operation, OperationStatus};
use gfm_types::Result;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictPolicy {
    Fail,
    Replace,
    KeepBoth,
    Merge,
    Skip,
}

impl ConflictPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fail => "fail",
            Self::Replace => "replace",
            Self::KeepBoth => "keep-both",
            Self::Merge => "merge",
            Self::Skip => "skip",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationConflictKind {
    None,
    File,
    Directory,
    Symlink,
    Other,
}

impl OperationConflictKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::File => "file",
            Self::Directory => "directory",
            Self::Symlink => "symlink",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationConflictReport {
    pub operation: &'static str,
    pub target: Option<PathBuf>,
    pub target_exists: bool,
    pub target_kind: OperationConflictKind,
    pub selected_policy: ConflictPolicy,
    pub available_policies: Vec<ConflictPolicy>,
    pub blocks_operation: bool,
    pub reason: String,
}

impl OperationConflictReport {
    pub fn evaluate(operation: &Operation, selected_policy: ConflictPolicy) -> Self {
        let target = operation.target_path().map(Path::to_path_buf);
        let Some(target_path) = &target else {
            return Self {
                operation: operation_kind(operation),
                target,
                target_exists: false,
                target_kind: OperationConflictKind::None,
                selected_policy,
                available_policies: Vec::new(),
                blocks_operation: false,
                reason: "operation-has-no-conflict-target".to_string(),
            };
        };
        let target_exists = path_exists_or_symlink(target_path);
        let target_kind = conflict_kind(target_path);
        let available_policies = if target_exists {
            available_conflict_policies(target_kind)
        } else {
            Vec::new()
        };
        let selected_policy_available = selected_policy == ConflictPolicy::Fail
            || available_policies.contains(&selected_policy);
        let blocks_operation = target_exists
            && (selected_policy == ConflictPolicy::Fail || !selected_policy_available);
        let reason = if !target_exists {
            "target-available".to_string()
        } else if !selected_policy_available {
            format!(
                "destination-conflict-policy-unavailable-for-{}",
                target_kind.as_str()
            )
        } else if blocks_operation {
            "destination-conflict-requires-user-resolution".to_string()
        } else {
            format!(
                "destination-conflict-resolved-by-{}",
                selected_policy.as_str()
            )
        };

        Self {
            operation: operation_kind(operation),
            target,
            target_exists,
            target_kind,
            selected_policy,
            available_policies,
            blocks_operation,
            reason,
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "operation-conflict\toperation={}\ttarget={}\texists={}\tkind={}\tpolicy={}\tavailable={}\tblocks-operation={}\treason={}",
            self.operation,
            self.target
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.target_exists,
            self.target_kind.as_str(),
            self.selected_policy.as_str(),
            self.available_policies
                .iter()
                .map(|policy| policy.as_str())
                .collect::<Vec<_>>()
                .join(","),
            self.blocks_operation,
            self.reason
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationConflictPlan {
    pub default: ConflictPolicy,
    target_overrides: BTreeMap<PathBuf, ConflictPolicy>,
}

fn operation_kind(operation: &Operation) -> &'static str {
    match operation {
        Operation::Copy { .. } => "copy",
        Operation::Move { .. } => "move",
        Operation::Rename { .. } => "rename",
        Operation::Delete { .. } => "delete",
        Operation::Trash { .. } => "trash",
        Operation::EmptyTrash { .. } => "empty-trash",
        Operation::Restore { .. } => "restore",
    }
}

fn conflict_kind(path: &Path) -> OperationConflictKind {
    fs::symlink_metadata(path)
        .map(|metadata| {
            if metadata.file_type().is_symlink() {
                OperationConflictKind::Symlink
            } else if metadata.is_dir() {
                OperationConflictKind::Directory
            } else if metadata.is_file() {
                OperationConflictKind::File
            } else {
                OperationConflictKind::Other
            }
        })
        .unwrap_or(OperationConflictKind::None)
}

fn available_conflict_policies(kind: OperationConflictKind) -> Vec<ConflictPolicy> {
    let mut policies = vec![
        ConflictPolicy::Replace,
        ConflictPolicy::KeepBoth,
        ConflictPolicy::Skip,
    ];
    if kind == OperationConflictKind::Directory {
        policies.insert(2, ConflictPolicy::Merge);
    }
    policies
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

    #[test]
    fn conflict_report_requires_user_resolution_for_existing_destination() {
        let root = std::env::temp_dir().join(format!("gfm-conflict-report-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("source.txt");
        let target = root.join("target.txt");
        std::fs::write(&source, "new").unwrap();
        std::fs::write(&target, "old").unwrap();

        let report = OperationConflictReport::evaluate(
            &Operation::Copy {
                from: source,
                to: target.clone(),
            },
            ConflictPolicy::Fail,
        );

        assert_eq!(report.operation, "copy");
        assert_eq!(report.target, Some(target));
        assert_eq!(report.target_kind, OperationConflictKind::File);
        assert!(report.blocks_operation);
        assert_eq!(
            report.reason,
            "destination-conflict-requires-user-resolution"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn directory_conflict_report_includes_merge_policy() {
        let root = std::env::temp_dir().join(format!(
            "gfm-directory-conflict-report-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let source = root.join("source");
        let target = root.join("target");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&target).unwrap();

        let report = OperationConflictReport::evaluate(
            &Operation::Move {
                from: source,
                to: target,
            },
            ConflictPolicy::Fail,
        );

        assert_eq!(report.target_kind, OperationConflictKind::Directory);
        assert!(report.available_policies.contains(&ConflictPolicy::Merge));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn conflict_report_blocks_unavailable_policy_for_file_destination() {
        let root = std::env::temp_dir().join(format!(
            "gfm-unavailable-conflict-policy-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("source.txt");
        let target = root.join("target.txt");
        std::fs::write(&source, "new").unwrap();
        std::fs::write(&target, "old").unwrap();

        let report = OperationConflictReport::evaluate(
            &Operation::Copy {
                from: source,
                to: target,
            },
            ConflictPolicy::Merge,
        );

        assert_eq!(report.target_kind, OperationConflictKind::File);
        assert!(!report.available_policies.contains(&ConflictPolicy::Merge));
        assert!(report.blocks_operation);
        assert_eq!(
            report.reason,
            "destination-conflict-policy-unavailable-for-file"
        );

        let _ = std::fs::remove_dir_all(root);
    }
}
