use crate::Operation;
use gfm_types::GfmError;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationStatus {
    Started,
    Completed,
    Skipped,
    Paused,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    pub id: u128,
    pub status: OperationStatus,
    pub operation: Operation,
    pub message: Option<String>,
    pub timestamp_nanos: u128,
}

impl JournalEntry {
    pub(crate) fn started(id: u128, operation: Operation) -> Self {
        Self {
            id,
            status: OperationStatus::Started,
            operation,
            message: None,
            timestamp_nanos: now_nanos(),
        }
    }

    pub(crate) fn completed(id: u128, operation: Operation) -> Self {
        Self {
            id,
            status: OperationStatus::Completed,
            operation,
            message: None,
            timestamp_nanos: now_nanos(),
        }
    }

    pub(crate) fn skipped(id: u128, operation: Operation) -> Self {
        Self {
            id,
            status: OperationStatus::Skipped,
            operation,
            message: Some("operation skipped by conflict policy".to_string()),
            timestamp_nanos: now_nanos(),
        }
    }

    pub(crate) fn paused(id: u128, operation: Operation) -> Self {
        Self {
            id,
            status: OperationStatus::Paused,
            operation,
            message: None,
            timestamp_nanos: now_nanos(),
        }
    }

    pub(crate) fn cancelled(id: u128, operation: Operation) -> Self {
        Self {
            id,
            status: OperationStatus::Cancelled,
            operation,
            message: None,
            timestamp_nanos: now_nanos(),
        }
    }

    pub(crate) fn failed(id: u128, operation: Operation, message: String) -> Self {
        Self {
            id,
            status: OperationStatus::Failed,
            operation,
            message: Some(message),
            timestamp_nanos: now_nanos(),
        }
    }

    pub(crate) fn from_error(id: u128, operation: Operation, err: &GfmError) -> Self {
        if matches!(err, GfmError::Paused) {
            Self::paused(id, operation)
        } else if matches!(err, GfmError::Cancelled) {
            Self::cancelled(id, operation)
        } else {
            Self::failed(id, operation, err.to_string())
        }
    }
}

pub(crate) fn operation_status_from_error(err: &GfmError) -> OperationStatus {
    if matches!(err, GfmError::Paused) {
        OperationStatus::Paused
    } else if matches!(err, GfmError::Cancelled) {
        OperationStatus::Cancelled
    } else {
        OperationStatus::Failed
    }
}

pub(crate) fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn operation() -> Operation {
        Operation::Delete {
            path: "/tmp/gfm-journal-entry".into(),
        }
    }

    #[test]
    fn skipped_entries_carry_the_conflict_policy_message() {
        let entry = JournalEntry::skipped(5, operation());

        assert_eq!(entry.status, OperationStatus::Skipped);
        assert_eq!(
            entry.message.as_deref(),
            Some("operation skipped by conflict policy")
        );
    }

    #[test]
    fn status_mapping_preserves_control_errors() {
        assert_eq!(
            operation_status_from_error(&GfmError::Cancelled),
            OperationStatus::Cancelled
        );
        assert_eq!(
            operation_status_from_error(&GfmError::Paused),
            OperationStatus::Paused
        );
        assert_eq!(
            operation_status_from_error(&GfmError::Format("bad journal".to_string())),
            OperationStatus::Failed
        );
    }

    #[test]
    fn error_entries_preserve_failure_messages() {
        let entry =
            JournalEntry::from_error(7, operation(), &GfmError::Format("bad entry".to_string()));

        assert_eq!(entry.status, OperationStatus::Failed);
        assert_eq!(entry.message.as_deref(), Some("bad entry"));
    }
}
