use crate::{
    ConflictPolicy, OperationAccessGate, OperationCancellation, OperationPause,
    OperationVolumeCopyPolicy, VerificationPolicy,
};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct OperationContext {
    pub conflict: ConflictPolicy,
    pub journal_path: PathBuf,
    pub trash_metadata_path: Option<PathBuf>,
    pub cancellation: OperationCancellation,
    pub pause: OperationPause,
    pub verification: VerificationPolicy,
    pub access_gate: OperationAccessGate,
    pub volume_copy_policy: OperationVolumeCopyPolicy,
}

impl OperationContext {
    pub fn new(journal_path: impl Into<PathBuf>) -> Self {
        Self {
            conflict: ConflictPolicy::Fail,
            journal_path: journal_path.into(),
            trash_metadata_path: None,
            cancellation: OperationCancellation::default(),
            pause: OperationPause::default(),
            verification: VerificationPolicy::Size,
            access_gate: OperationAccessGate::default(),
            volume_copy_policy: OperationVolumeCopyPolicy::default(),
        }
    }

    pub fn with_conflict(mut self, conflict: ConflictPolicy) -> Self {
        self.conflict = conflict;
        self
    }

    pub fn with_trash_metadata_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.trash_metadata_path = Some(path.into());
        self
    }

    pub fn with_cancellation(mut self, cancellation: OperationCancellation) -> Self {
        self.cancellation = cancellation;
        self
    }

    pub fn with_pause(mut self, pause: OperationPause) -> Self {
        self.pause = pause;
        self
    }

    pub fn with_verification(mut self, verification: VerificationPolicy) -> Self {
        self.verification = verification;
        self
    }

    pub fn with_access_gate(mut self, access_gate: OperationAccessGate) -> Self {
        self.access_gate = access_gate;
        self
    }

    pub fn with_volume_copy_policy(mut self, policy: OperationVolumeCopyPolicy) -> Self {
        self.volume_copy_policy = policy;
        self
    }
}
