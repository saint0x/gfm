use crate::{JournalEntry, Operation, OperationStatus};
use gfm_jobs::RetryPolicy;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationRecoveryReport {
    pub outcomes: Vec<OperationRecoveryOutcome>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationRecoveryPolicy {
    pub retry_failed: bool,
    pub max_attempts: usize,
}

impl Default for OperationRecoveryPolicy {
    fn default() -> Self {
        Self {
            retry_failed: false,
            max_attempts: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationRecoveryOutcome {
    pub id: u128,
    pub status: OperationStatus,
    pub operation: Operation,
    pub message: Option<String>,
}

#[derive(Debug, Clone)]
struct OperationRecoveryState {
    id: u128,
    operation: Operation,
    last_status: OperationStatus,
    started_count: usize,
    message: Option<String>,
    timestamp_nanos: u128,
}

#[derive(Debug, Clone)]
pub(crate) struct OperationRecoveryPlan {
    pub(crate) entry: JournalEntry,
    pub(crate) append_started: bool,
}

pub(crate) fn recoverable_operations(
    entries: Vec<JournalEntry>,
    policy: OperationRecoveryPolicy,
) -> Vec<OperationRecoveryPlan> {
    let mut states: Vec<OperationRecoveryState> = Vec::new();
    for entry in entries {
        if let Some(state) = states.iter_mut().find(|state| state.id == entry.id) {
            let status = entry.status;
            state.operation = entry.operation;
            state.last_status = status;
            state.message = entry.message;
            if status == OperationStatus::Started {
                state.started_count += 1;
            }
            state.timestamp_nanos = entry.timestamp_nanos;
        } else {
            let started_count = usize::from(entry.status == OperationStatus::Started);
            states.push(OperationRecoveryState {
                id: entry.id,
                operation: entry.operation,
                last_status: entry.status,
                started_count,
                message: entry.message,
                timestamp_nanos: entry.timestamp_nanos,
            });
        }
    }
    states.sort_by_key(|state| (state.timestamp_nanos, state.id));
    states
        .into_iter()
        .filter_map(|state| {
            let failed_retryable = policy.retry_failed
                && state.last_status == OperationStatus::Failed
                && state.started_count < policy.max_attempts.max(1)
                && retryable_failure_message(
                    state.message.as_deref(),
                    policy.max_attempts,
                    state.started_count,
                );
            let append_started = if state.last_status == OperationStatus::Started {
                false
            } else if state.last_status == OperationStatus::Paused || failed_retryable {
                true
            } else {
                return None;
            };
            Some(OperationRecoveryPlan {
                entry: JournalEntry {
                    id: state.id,
                    status: state.last_status,
                    operation: state.operation,
                    message: state.message,
                    timestamp_nanos: state.timestamp_nanos,
                },
                append_started,
            })
        })
        .collect()
}

fn retryable_failure_message(message: Option<&str>, max_attempts: usize, attempts: usize) -> bool {
    let Some(message) = message else {
        return false;
    };
    RetryPolicy { max_attempts }
        .retry_decision(attempts, message)
        .retryable
}

#[cfg(test)]
mod tests {
    use super::*;

    fn copy_operation(name: &str) -> Operation {
        Operation::Copy {
            from: format!("/tmp/{name}").into(),
            to: format!("/tmp/{name}.copy").into(),
        }
    }

    fn entry(
        id: u128,
        status: OperationStatus,
        operation: Operation,
        message: Option<&str>,
        timestamp_nanos: u128,
    ) -> JournalEntry {
        JournalEntry {
            id,
            status,
            operation,
            message: message.map(str::to_string),
            timestamp_nanos,
        }
    }

    #[test]
    fn paused_operations_are_recovered_with_a_new_started_entry() {
        let operation = copy_operation("paused");
        let plans = recoverable_operations(
            vec![
                entry(7, OperationStatus::Started, operation.clone(), None, 10),
                entry(7, OperationStatus::Paused, operation.clone(), None, 11),
            ],
            OperationRecoveryPolicy::default(),
        );

        assert_eq!(plans.len(), 1);
        assert!(plans[0].append_started);
        assert_eq!(plans[0].entry.operation, operation);
        assert_eq!(plans[0].entry.status, OperationStatus::Paused);
    }

    #[test]
    fn exhausted_failed_operations_are_not_recovered() {
        let operation = copy_operation("network");
        let plans = recoverable_operations(
            vec![
                entry(9, OperationStatus::Started, operation.clone(), None, 20),
                entry(
                    9,
                    OperationStatus::Failed,
                    operation.clone(),
                    Some("network is unreachable"),
                    21,
                ),
            ],
            OperationRecoveryPolicy {
                retry_failed: true,
                max_attempts: 1,
            },
        );

        assert!(plans.is_empty());
    }

    #[test]
    fn retryable_failed_operations_are_recovered_when_attempts_remain() {
        let operation = copy_operation("retry");
        let plans = recoverable_operations(
            vec![
                entry(11, OperationStatus::Started, operation.clone(), None, 30),
                entry(
                    11,
                    OperationStatus::Failed,
                    operation,
                    Some("operation timed out"),
                    31,
                ),
            ],
            OperationRecoveryPolicy {
                retry_failed: true,
                max_attempts: 2,
            },
        );

        assert_eq!(plans.len(), 1);
        assert!(plans[0].append_started);
        assert_eq!(plans[0].entry.status, OperationStatus::Failed);
    }
}
