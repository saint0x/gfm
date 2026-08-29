mod access;
mod conflict;
mod context;
mod control;
mod copy;
mod journal;
mod locked;
mod operation;
mod operator;
mod plan;
mod preserve;
mod progress;
mod recovery;
mod relocate;
mod removal;
mod target;
mod transfer;
mod trashmeta;
mod verify;
mod volume;

pub use access::{
    OperationAccessAction, OperationAccessDecision, OperationAccessGate,
    OperationAccessRequirement, OperationAccessRole,
};
pub use conflict::{
    ConflictPolicy, OperationBatchOutcome, OperationBatchReport, OperationConflictKind,
    OperationConflictPlan, OperationConflictReport,
};
pub use context::OperationContext;
pub use control::{OperationCancellation, OperationPause};
pub use copy::CopyMethod;
pub use journal::{read_journal, JournalEntry, OperationStatus};
pub use operation::Operation;
pub use operator::Operator;
pub use plan::plan_operation;
pub use progress::{
    OperationMetadataDegradation, OperationMetadataDegradationKind, OperationProgress,
    OperationProgressEvent, OperationProgressPhase, OperationThroughputClass,
    OperationThroughputSnapshot,
};
pub use recovery::{OperationRecoveryOutcome, OperationRecoveryPolicy, OperationRecoveryReport};
pub use trashmeta::{read_trash_metadata, TrashRestoreMetadata};
pub use verify::VerificationPolicy;
pub use volume::{OperationVolumeClass, OperationVolumeCopyPolicy};

#[cfg(test)]
mod tests;
