use crate::copy::copy_path;
use crate::journal::{append_journal, now_nanos, operation_status_from_error};
use crate::plan::plan_operation_checked;
use crate::progress::ProgressTracker;
use crate::recovery::recoverable_operations;
use crate::relocate::{move_path, restore_path};
use crate::removal::{delete_path, empty_trash_path, trash_path};
use crate::target::path_exists_or_symlink;
use crate::{
    ConflictPolicy, JournalEntry, Operation, OperationBatchOutcome, OperationBatchReport,
    OperationConflictPlan, OperationContext, OperationProgressEvent, OperationRecoveryOutcome,
    OperationRecoveryPolicy, OperationRecoveryReport, OperationStatus,
};
use gfm_types::{GfmError, Result};

pub struct Operator {
    context: OperationContext,
}

impl Operator {
    pub fn new(context: OperationContext) -> Self {
        Self { context }
    }

    pub fn execute(&self, operation: Operation) -> Result<JournalEntry> {
        self.execute_with_progress(operation, |_| {})
    }

    pub fn execute_with_progress(
        &self,
        operation: Operation,
        mut on_progress: impl FnMut(OperationProgressEvent),
    ) -> Result<JournalEntry> {
        if let Err(err) = self.context.access_gate.check(&operation) {
            let id = now_nanos();
            self.append(JournalEntry::started(id, operation.clone()))?;
            let entry = JournalEntry::from_error(id, operation, &err);
            let _ = self.append(entry);
            return Err(err);
        }
        let operation =
            crate::conflict::resolve_operation_conflicts(operation, self.context.conflict)?;
        let id = now_nanos();
        self.append(JournalEntry::started(id, operation.clone()))?;
        self.execute_started(id, operation, &mut on_progress)
    }

    pub fn execute_batch_with_conflicts(
        &self,
        operations: impl IntoIterator<Item = Operation>,
        plan: OperationConflictPlan,
    ) -> Result<OperationBatchReport> {
        let mut outcomes = Vec::new();
        for operation in operations {
            let conflict = plan.conflict_for(&operation);
            let operator = Operator::new(self.context.clone().with_conflict(conflict));
            match operator.execute(operation.clone()) {
                Ok(entry) => outcomes.push(OperationBatchOutcome {
                    conflict,
                    status: entry.status,
                    operation: entry.operation,
                    message: entry.message,
                }),
                Err(err) => {
                    let status = operation_status_from_error(&err);
                    outcomes.push(OperationBatchOutcome {
                        conflict,
                        status,
                        operation,
                        message: (!matches!(err, GfmError::Cancelled | GfmError::Paused))
                            .then(|| err.to_string()),
                    });
                    if matches!(status, OperationStatus::Cancelled | OperationStatus::Paused) {
                        break;
                    }
                }
            }
        }
        Ok(OperationBatchReport { outcomes })
    }

    pub fn recover_interrupted(&self) -> Result<OperationRecoveryReport> {
        self.recover_with_policy(OperationRecoveryPolicy::default())
    }

    pub fn recover_with_policy(
        &self,
        policy: OperationRecoveryPolicy,
    ) -> Result<OperationRecoveryReport> {
        let recoverable = recoverable_operations(self.journal()?, policy);
        let mut outcomes = Vec::with_capacity(recoverable.len());
        for plan in recoverable {
            let entry = plan.entry;
            let operation = entry.operation;
            if plan.append_started {
                self.append(JournalEntry::started(entry.id, operation.clone()))?;
            }
            match self.execute_started_resuming(entry.id, operation.clone(), &mut |_| {}) {
                Ok(completed) => outcomes.push(OperationRecoveryOutcome {
                    id: completed.id,
                    status: completed.status,
                    operation: completed.operation,
                    message: completed.message,
                }),
                Err(err) => outcomes.push(OperationRecoveryOutcome {
                    id: entry.id,
                    status: operation_status_from_error(&err),
                    operation,
                    message: (!matches!(err, GfmError::Cancelled | GfmError::Paused))
                        .then(|| err.to_string()),
                }),
            }
        }
        Ok(OperationRecoveryReport { outcomes })
    }

    fn execute_started(
        &self,
        id: u128,
        operation: Operation,
        on_progress: &mut impl FnMut(OperationProgressEvent),
    ) -> Result<JournalEntry> {
        self.execute_started_inner(id, operation, false, on_progress)
    }

    fn execute_started_resuming(
        &self,
        id: u128,
        operation: Operation,
        on_progress: &mut impl FnMut(OperationProgressEvent),
    ) -> Result<JournalEntry> {
        self.execute_started_inner(id, operation, true, on_progress)
    }

    fn execute_started_inner(
        &self,
        id: u128,
        operation: Operation,
        resuming: bool,
        on_progress: &mut impl FnMut(OperationProgressEvent),
    ) -> Result<JournalEntry> {
        if let Err(err) = self.context.access_gate.check(&operation) {
            let entry = JournalEntry::from_error(id, operation, &err);
            let _ = self.append(entry);
            return Err(err);
        }
        if should_skip_operation(&operation, self.context.conflict)? {
            let entry = JournalEntry::skipped(id, operation);
            self.append(entry.clone())?;
            return Ok(entry);
        }
        let plan = match plan_operation_checked(&operation, &self.context.cancellation) {
            Ok(plan) => plan,
            Err(err) => {
                let entry = JournalEntry::from_error(id, operation, &err);
                let _ = self.append(entry);
                return Err(err);
            }
        };
        let mut progress = ProgressTracker::new(
            plan,
            &self.context.cancellation,
            &self.context.pause,
            on_progress,
        );
        match self.apply(&operation, resuming, &mut progress) {
            Ok(()) => {
                let entry = JournalEntry::completed(id, operation);
                self.append(entry.clone())?;
                Ok(entry)
            }
            Err(err) => {
                let entry = JournalEntry::from_error(id, operation, &err);
                let _ = self.append(entry);
                Err(err)
            }
        }
    }

    pub fn journal(&self) -> Result<Vec<JournalEntry>> {
        crate::read_journal(&self.context.journal_path)
    }

    fn apply(
        &self,
        operation: &Operation,
        resuming: bool,
        progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
    ) -> Result<()> {
        match operation {
            Operation::Copy { from, to } => copy_path(
                from,
                to,
                self.context.conflict,
                self.context.verification,
                &self.context.volume_copy_policy,
                resuming,
                progress,
            ),
            Operation::Move { from, to } | Operation::Rename { from, to } => move_path(
                from,
                to,
                self.context.conflict,
                self.context.verification,
                &self.context.volume_copy_policy,
                resuming,
                progress,
            ),
            Operation::Delete { path } => {
                delete_path(path, self.context.trash_metadata_path.as_deref(), progress)
            }
            Operation::Trash { path } => {
                trash_path(path, self.context.trash_metadata_path.as_deref(), progress)
            }
            Operation::EmptyTrash { path } => {
                empty_trash_path(path, self.context.trash_metadata_path.as_deref(), progress)
            }
            Operation::Restore { from, to } => restore_path(
                from,
                to,
                self.context.conflict,
                &self.context.volume_copy_policy,
                self.context.trash_metadata_path.as_deref(),
                progress,
            ),
        }
    }

    fn append(&self, entry: JournalEntry) -> Result<()> {
        append_journal(&self.context.journal_path, &entry)
    }
}

fn should_skip_operation(operation: &Operation, conflict: ConflictPolicy) -> Result<bool> {
    if conflict != ConflictPolicy::Skip {
        return Ok(false);
    }
    operation
        .target_path()
        .map(path_exists_or_symlink)
        .unwrap_or(Ok(false))
}
