use crate::{
    default_journal_path, default_trash_metadata_path, detect_volume_id, parent_volume,
    required_path, run_volume_task,
};
use gfm_jobs::Priority;
use gfm_mac::{
    AccessIntent, SecurityDecisionAction, SecurityScopedAccessReport, VolumeDiscoveryReport,
    VolumeKind,
};
use gfm_ops::{
    read_trash_metadata, ConflictPolicy, Operation, OperationAccessDecision, OperationAccessGate,
    OperationAccessRole, OperationContext, OperationRecoveryPolicy, OperationVolumeClass,
    OperationVolumeCopyPolicy, Operator,
};
use gfm_types::{GfmError, Result, VolumeId};
use std::path::{Path, PathBuf};

pub(crate) fn run(command: &str, args: &mut impl Iterator<Item = String>) -> Result<bool> {
    match command {
        "ops-recover" => {
            let (journal, policy) = parse_ops_recover_args(args)?;
            let report =
                Operator::new(OperationContext::new(journal)).recover_with_policy(policy)?;
            for outcome in report.outcomes {
                println!(
                    "{}\t{}\t{}\t{}",
                    outcome.id,
                    operation_status(outcome.status),
                    operation_kind(&outcome.operation),
                    outcome.message.unwrap_or_default()
                );
            }
        }
        "copy" => {
            let from = required_path(args.next(), "copy requires a source path")?;
            let to = required_path(args.next(), "copy requires a destination path")?;
            let conflict = parse_operation_conflict_args(args, "copy")?;
            execute_operation(Operation::Copy { from, to }, conflict)?;
        }
        "move" => {
            let from = required_path(args.next(), "move requires a source path")?;
            let to = required_path(args.next(), "move requires a destination path")?;
            let conflict = parse_operation_conflict_args(args, "move")?;
            execute_operation(Operation::Move { from, to }, conflict)?;
        }
        "rename" => {
            let from = required_path(args.next(), "rename requires a source path")?;
            let to = required_path(args.next(), "rename requires a destination path")?;
            let conflict = parse_operation_conflict_args(args, "rename")?;
            execute_operation(Operation::Rename { from, to }, conflict)?;
        }
        "delete" => {
            let path = required_path(args.next(), "delete requires a path")?;
            execute_operation(Operation::Delete { path }, ConflictPolicy::Fail)?;
        }
        "trash" => {
            let path = required_path(args.next(), "trash requires a path")?;
            execute_operation(Operation::Trash { path }, ConflictPolicy::Fail)?;
        }
        "empty-trash" => {
            let path = required_path(args.next(), "empty-trash requires a trash directory path")?;
            execute_operation(Operation::EmptyTrash { path }, ConflictPolicy::Fail)?;
        }
        "restore" => {
            let from = required_path(args.next(), "restore requires a trash entry path")?;
            let mut restore_args = args.collect::<Vec<_>>();
            let to = if restore_args
                .first()
                .is_some_and(|value| !value.starts_with("--"))
            {
                PathBuf::from(restore_args.remove(0))
            } else {
                restore_destination_from_metadata(&from)?
            };
            let conflict = parse_operation_conflict_args(&mut restore_args.into_iter(), "restore")?;
            execute_operation(Operation::Restore { from, to }, conflict)?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn parse_ops_recover_args(
    args: &mut impl Iterator<Item = String>,
) -> Result<(PathBuf, OperationRecoveryPolicy)> {
    let mut journal = None;
    let mut retry_failed = false;
    let mut max_attempts = 1;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--retry-failed" => retry_failed = true,
            "--max-attempts" => {
                let value = args.next().ok_or_else(|| {
                    GfmError::Format("ops-recover --max-attempts requires a value".to_string())
                })?;
                max_attempts = value.parse().map_err(|err| {
                    GfmError::Format(format!("invalid ops-recover max attempts `{value}`: {err}"))
                })?;
            }
            other if other.starts_with("--") => {
                return Err(GfmError::Format(format!(
                    "unknown ops-recover option `{other}`"
                )));
            }
            path if journal.is_none() => journal = Some(PathBuf::from(path)),
            path => {
                return Err(GfmError::Format(format!(
                    "unexpected ops-recover argument `{path}`"
                )));
            }
        }
    }
    Ok((
        journal.unwrap_or_else(default_journal_path),
        OperationRecoveryPolicy {
            retry_failed,
            max_attempts,
        },
    ))
}

fn parse_operation_conflict_args(
    args: &mut impl Iterator<Item = String>,
    command: &str,
) -> Result<ConflictPolicy> {
    let mut conflict = ConflictPolicy::Fail;
    for arg in args {
        match arg.as_str() {
            "--replace" => conflict = ConflictPolicy::Replace,
            "--keep-both" => conflict = ConflictPolicy::KeepBoth,
            "--merge" => conflict = ConflictPolicy::Merge,
            "--skip" => conflict = ConflictPolicy::Skip,
            other => {
                return Err(GfmError::Format(format!(
                    "unknown {command} conflict option `{other}`"
                )));
            }
        }
    }
    Ok(conflict)
}

fn restore_destination_from_metadata(trashed_path: &Path) -> Result<PathBuf> {
    let name = trashed_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            GfmError::Format(format!(
                "could not derive trash entry name for {}",
                trashed_path.display()
            ))
        })?;
    let metadata_path = default_trash_metadata_path();
    let metadata = read_trash_metadata(&metadata_path)?;
    metadata
        .get(&name)
        .filter(|entry| entry.can_restore)
        .map(|entry| entry.original_path.clone())
        .ok_or_else(|| {
            GfmError::Format(format!(
                "no restorable trash metadata for `{name}` in {}",
                metadata_path.display()
            ))
        })
}

fn execute_operation(operation: Operation, conflict: ConflictPolicy) -> Result<()> {
    let journal = default_journal_path();
    let trash_metadata = default_trash_metadata_path();
    let access_gate = operation_access_gate(&operation);
    let volume_copy_policy = operation_volume_copy_policy(&operation);
    let label = operation_kind(&operation);
    let volume = operation_volume(&operation);
    let entry = run_volume_task(volume, Priority::Interactive, label, move || {
        let operator = Operator::new(
            OperationContext::new(journal)
                .with_conflict(conflict)
                .with_trash_metadata_path(trash_metadata)
                .with_access_gate(access_gate)
                .with_volume_copy_policy(volume_copy_policy),
        );
        operator.execute(operation)
    })?;
    println!("{}\t{}", entry.id, operation_status(entry.status));
    Ok(())
}

fn operation_access_gate(operation: &Operation) -> OperationAccessGate {
    let mut gate = OperationAccessGate::new();
    for requirement in operation.access_requirements() {
        let probe_path = operation_access_probe_path(&requirement.path, requirement.role);
        let report = SecurityScopedAccessReport::evaluate(&probe_path, AccessIntent::Operate);
        if matches!(report.action, SecurityDecisionAction::Deny)
            && matches!(report.probe, gfm_mac::AccessProbeState::Missing)
            && !matches!(requirement.role, OperationAccessRole::DestinationParent)
        {
            continue;
        }
        let reason = format!(
            "{}; scope={}; mode={}; role={}; probe={}",
            report.reason,
            report.scope.as_str(),
            report.mode.as_str(),
            requirement.role.as_str(),
            probe_path.display()
        );
        let decision = match report.action {
            SecurityDecisionAction::Allow => OperationAccessDecision::allow(reason),
            SecurityDecisionAction::Prompt => OperationAccessDecision::prompt(reason),
            SecurityDecisionAction::Degrade | SecurityDecisionAction::Deny => {
                OperationAccessDecision::deny(reason)
            }
        };
        gate = gate.with_decision(requirement.path, decision);
    }
    gate
}

fn operation_access_probe_path(path: &Path, role: OperationAccessRole) -> PathBuf {
    if !matches!(role, OperationAccessRole::DestinationParent) {
        return path.to_path_buf();
    }
    let mut candidate = path.to_path_buf();
    while !candidate.exists() {
        let Some(parent) = candidate.parent() else {
            break;
        };
        if parent == candidate {
            break;
        }
        candidate = parent.to_path_buf();
    }
    candidate
}

fn operation_volume_copy_policy(operation: &Operation) -> OperationVolumeCopyPolicy {
    operation_volume_copy_policy_from_report(operation, &VolumeDiscoveryReport::discover())
}

fn operation_volume_copy_policy_from_report(
    operation: &Operation,
    report: &VolumeDiscoveryReport,
) -> OperationVolumeCopyPolicy {
    let mut policy = OperationVolumeCopyPolicy::default();
    for volume in &report.volumes {
        if operation_touches_volume(operation, &volume.path) {
            policy = policy.with_root(
                volume.path.clone(),
                operation_volume_class_for_kind(volume.kind),
            );
        }
    }
    policy
}

fn operation_touches_volume(operation: &Operation, root: &Path) -> bool {
    operation_paths(operation)
        .into_iter()
        .any(|path| path.starts_with(root))
}

fn operation_paths(operation: &Operation) -> Vec<&Path> {
    let mut paths = Vec::new();
    match operation {
        Operation::Copy { from, to }
        | Operation::Move { from, to }
        | Operation::Rename { from, to }
        | Operation::Restore { from, to } => {
            paths.push(from.as_path());
            paths.push(to.as_path());
        }
        Operation::Delete { path } | Operation::Trash { path } | Operation::EmptyTrash { path } => {
            paths.push(path.as_path());
        }
    }
    paths
}

fn operation_volume_class_for_kind(kind: VolumeKind) -> OperationVolumeClass {
    match kind {
        VolumeKind::System | VolumeKind::Internal | VolumeKind::Unknown => {
            OperationVolumeClass::Local
        }
        VolumeKind::External | VolumeKind::Removable | VolumeKind::DiskImage => {
            OperationVolumeClass::External
        }
        VolumeKind::Network => OperationVolumeClass::Network,
    }
}

fn operation_volume(operation: &Operation) -> Option<VolumeId> {
    let primary = match operation {
        Operation::Copy { from, .. }
        | Operation::Move { from, .. }
        | Operation::Rename { from, .. }
        | Operation::Restore { from, .. } => Some(from.as_path()),
        Operation::Delete { path } | Operation::Trash { path } | Operation::EmptyTrash { path } => {
            Some(path.as_path())
        }
    };
    primary
        .and_then(|path| detect_volume_id(path).ok())
        .or_else(|| operation.target_path().and_then(parent_volume))
}

fn operation_status(status: gfm_ops::OperationStatus) -> &'static str {
    match status {
        gfm_ops::OperationStatus::Started => "started",
        gfm_ops::OperationStatus::Completed => "completed",
        gfm_ops::OperationStatus::Skipped => "skipped",
        gfm_ops::OperationStatus::Paused => "paused",
        gfm_ops::OperationStatus::Cancelled => "cancelled",
        gfm_ops::OperationStatus::Failed => "failed",
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn operation_volume_policy_maps_discovered_network_and_external_roots() {
        let root = unique_temp_dir("gfm-app-op-volume-policy");
        let network = root.join("TeamShare");
        let external = root.join("Backup");
        fs::create_dir_all(&network).unwrap();
        fs::create_dir_all(&external).unwrap();
        fs::write(network.join(".gfm-volume-kind"), "network-smb\n").unwrap();
        fs::write(external.join(".gfm-volume-kind"), "external-removable\n").unwrap();
        let source = network.join("source.bin");
        let destination = external.join("destination.bin");
        let report = VolumeDiscoveryReport::from_paths(vec![network.clone(), external.clone()]);
        let operation = Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        };

        let policy = operation_volume_copy_policy_from_report(&operation, &report);

        assert_eq!(
            policy.class_for_path(&source),
            OperationVolumeClass::Network
        );
        assert_eq!(
            policy.class_for_path(&destination),
            OperationVolumeClass::External
        );
        assert!(policy.copy_buffer_bytes_for_paths(&source, &destination) < 256 * 1024);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operation_volume_policy_ignores_unrelated_discovered_roots() {
        let root = unique_temp_dir("gfm-app-op-volume-unrelated");
        let unrelated = root.join("UnrelatedShare");
        let local = root.join("LocalWork");
        fs::create_dir_all(&unrelated).unwrap();
        fs::create_dir_all(&local).unwrap();
        fs::write(unrelated.join(".gfm-volume-kind"), "network-smb\n").unwrap();
        let source = local.join("source.bin");
        let destination = local.join("destination.bin");
        let report = VolumeDiscoveryReport::from_paths(vec![unrelated.clone()]);
        let operation = Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        };

        let policy = operation_volume_copy_policy_from_report(&operation, &report);

        assert_eq!(policy.class_for_path(&source), OperationVolumeClass::Local);
        assert_eq!(
            policy.copy_buffer_bytes_for_paths(&source, &destination),
            256 * 1024
        );

        fs::remove_dir_all(root).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "{}-{}-{}",
            prefix,
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
