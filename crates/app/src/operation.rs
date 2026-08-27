use crate::permission_refresh::{refresh_permission_state, PermissionRefreshAudience};
use crate::runtime::{
    default_journal_path, default_security_bookmarks_path, default_trash_metadata_path,
    run_volume_task, runtime_operation_conflict_store, OperationConflictStore,
    RuntimeOperationConflict,
};
use crate::{detect_volume_id, parent_volume, required_path};
use gfm_jobs::Priority;
use gfm_mac::{
    AccessIntent, SecurityDecisionAction, SecurityScopedAccessReport, SecurityScopedBookmarkAccess,
    SecurityScopedBookmarkStatus, SecurityScopedBookmarkStore, SecurityWorkerAction,
    VolumeDiscoveryReport, VolumeKind,
};
use gfm_ops::{
    read_trash_metadata, ConflictPolicy, Operation, OperationAccessDecision, OperationAccessGate,
    OperationAccessRole, OperationConflictReport, OperationContext, OperationRecoveryPolicy,
    OperationVolumeClass, OperationVolumeCopyPolicy, Operator,
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
        "operation-conflict-apply" => {
            let store_path = required_path(
                args.next(),
                "operation-conflict-apply requires an operation conflict store path",
            )?;
            let target = crate::required_string(
                args.next(),
                "operation-conflict-apply requires a target path",
            )?;
            let conflict = parse_required_conflict_policy(
                args.next(),
                "operation-conflict-apply requires replace, keep-both, merge, or skip",
            )?;
            let store = OperationConflictStore::new(store_path);
            let pending = blocking_operation_conflict(&store, &target)?;
            let operation = operation_from_runtime_conflict(&pending)?;
            execute_operation(operation, conflict)?;
            let resolved = store.resolve(&target, conflict.as_str())?;
            println!(
                "operation-conflict-control\tapply\ttarget={}\tpolicy={}\tblocks-operation={}\treason={}",
                resolved.target,
                resolved.selected_policy,
                resolved.blocks_operation,
                resolved.reason
            );
        }
        "operation-conflict-apply-all" => {
            let store_path = required_path(
                args.next(),
                "operation-conflict-apply-all requires an operation conflict store path",
            )?;
            let conflict = parse_required_conflict_policy(
                args.next(),
                "operation-conflict-apply-all requires replace, keep-both, merge, or skip",
            )?;
            let store = OperationConflictStore::new(store_path);
            let pending = blocking_operation_conflicts(&store)?;
            let operations = pending
                .iter()
                .map(|record| {
                    ensure_conflict_policy_available(record, conflict)?;
                    operation_from_runtime_conflict(record)
                })
                .collect::<Result<Vec<_>>>()?;
            let total = pending.len();
            let mut completed_targets = Vec::with_capacity(total);
            for (record, operation) in pending.iter().zip(operations) {
                if let Err(err) = execute_operation(operation, conflict) {
                    store.resolve_targets(&completed_targets, conflict.as_str())?;
                    return Err(err);
                }
                completed_targets.push(record.target.clone());
            }
            store.resolve_targets(&completed_targets, conflict.as_str())?;
            println!(
                "operation-conflict-control\tapply-all\tpolicy={}\tresolved={total}\tblocks-operation=false",
                conflict.as_str()
            );
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

fn parse_required_conflict_policy(value: Option<String>, message: &str) -> Result<ConflictPolicy> {
    match value.as_deref() {
        Some("replace") => Ok(ConflictPolicy::Replace),
        Some("keep-both") => Ok(ConflictPolicy::KeepBoth),
        Some("merge") => Ok(ConflictPolicy::Merge),
        Some("skip") => Ok(ConflictPolicy::Skip),
        Some(other) => Err(GfmError::Format(format!(
            "{message}; got unsupported policy `{other}`"
        ))),
        None => Err(GfmError::Format(message.to_string())),
    }
}

fn blocking_operation_conflict(
    store: &OperationConflictStore,
    target: &str,
) -> Result<RuntimeOperationConflict> {
    store
        .read()?
        .into_iter()
        .find(|conflict| conflict.target == target && conflict.blocks_operation)
        .ok_or_else(|| {
            GfmError::Format(format!(
                "operation conflict store has no blocking conflict for `{target}`"
            ))
        })
}

fn blocking_operation_conflicts(
    store: &OperationConflictStore,
) -> Result<Vec<RuntimeOperationConflict>> {
    let conflicts = store
        .read()?
        .into_iter()
        .filter(|conflict| conflict.blocks_operation)
        .collect::<Vec<_>>();
    if conflicts.is_empty() {
        return Err(GfmError::Format(
            "operation conflict store has no blocking conflicts".to_string(),
        ));
    }
    Ok(conflicts)
}

fn ensure_conflict_policy_available(
    conflict: &RuntimeOperationConflict,
    selected_policy: ConflictPolicy,
) -> Result<()> {
    if conflict
        .available_policies
        .iter()
        .any(|policy| policy == selected_policy.as_str())
    {
        return Ok(());
    }
    Err(GfmError::Format(format!(
        "operation conflict for `{}` cannot resolve with `{}`; available={}",
        conflict.target,
        selected_policy.as_str(),
        conflict.available_policies.join(",")
    )))
}

fn operation_from_runtime_conflict(conflict: &RuntimeOperationConflict) -> Result<Operation> {
    let source = conflict_path(&conflict.source, "source")?;
    let target = conflict_path(&conflict.target, "target")?;
    match conflict.operation.as_str() {
        "copy" => Ok(Operation::Copy {
            from: source,
            to: target,
        }),
        "move" => Ok(Operation::Move {
            from: source,
            to: target,
        }),
        "rename" => Ok(Operation::Rename {
            from: source,
            to: target,
        }),
        "restore" => Ok(Operation::Restore {
            from: source,
            to: target,
        }),
        other => Err(GfmError::Format(format!(
            "operation conflict apply does not support `{other}` records"
        ))),
    }
}

fn conflict_path(value: &str, field: &str) -> Result<PathBuf> {
    if value.is_empty() || value == "-" {
        return Err(GfmError::Format(format!(
            "operation conflict record is missing `{field}` path"
        )));
    }
    Ok(PathBuf::from(value))
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
    let volume_report = VolumeDiscoveryReport::discover();
    let label = operation_kind(&operation);
    let _ = refresh_permission_state(PermissionRefreshAudience::Operations, label)?;
    let conflict_report = OperationConflictReport::evaluate(&operation, conflict);
    if conflict_report.blocks_operation {
        if let Some(store) = runtime_operation_conflict_store() {
            store.append(&conflict_report)?;
        }
    }
    let access_gate = operation_access_gate(&operation, &volume_report);
    let volume_copy_policy = operation_volume_copy_policy_from_report(&operation, &volume_report);
    let volume = operation_volume(&operation);
    let entry = run_volume_task(volume, Priority::Interactive, label, move || {
        let _security_scope = operation_security_accesses(&operation)?;
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

fn operation_access_gate(
    operation: &Operation,
    volume_report: &VolumeDiscoveryReport,
) -> OperationAccessGate {
    let mut gate = OperationAccessGate::new();
    let bookmark_store = SecurityScopedBookmarkStore::new(default_security_bookmarks_path());
    for requirement in operation.access_requirements() {
        let probe_path = operation_access_probe_path(&requirement.path, requirement.role);
        let report = SecurityScopedAccessReport::evaluate(&probe_path, AccessIntent::Operate);
        let admission = report.worker_admission(format!(
            "{} {}",
            operation_kind(operation),
            requirement.role.as_str()
        ));
        eprintln!("{}", admission.as_tsv());
        if matches!(report.action, SecurityDecisionAction::Deny)
            && matches!(report.probe, gfm_mac::AccessProbeState::Missing)
            && !matches!(requirement.role, OperationAccessRole::DestinationParent)
        {
            continue;
        }
        let reason = format!(
            "{}; scope={}; mode={}; worker-action={}; role={}; probe={}",
            report.reason,
            report.scope.as_str(),
            report.mode.as_str(),
            admission.worker_action.as_str(),
            requirement.role.as_str(),
            probe_path.display()
        );
        let decision = if admission.needs_bookmark_access {
            stored_bookmark_decision(&bookmark_store, &probe_path, &reason)
        } else {
            None
        }
        .unwrap_or_else(|| match admission.worker_action {
            SecurityWorkerAction::Start => OperationAccessDecision::allow(reason),
            SecurityWorkerAction::Prompt => OperationAccessDecision::prompt(reason),
            SecurityWorkerAction::MetadataOnly | SecurityWorkerAction::Deny => {
                OperationAccessDecision::deny(reason)
            }
        });
        gate = gate.with_decision(requirement.path, decision);
    }
    for requirement in operation.access_requirements() {
        if !requirement_mutates_volume(operation, requirement.role) {
            continue;
        }
        let probe_path = operation_access_probe_path(&requirement.path, requirement.role);
        if mutation_allowed_for_role(&probe_path, requirement.role) {
            continue;
        }
        let Some(volume) = read_only_volume_for_path(volume_report, &probe_path) else {
            continue;
        };
        let reason = format!(
            "read-only volume {}; label={}; root={}; stable-id={}; role={}",
            volume.kind.as_str(),
            volume.label,
            volume.path.display(),
            volume.stable_identity,
            requirement.role.as_str()
        );
        gate = gate.with_decision(requirement.path, OperationAccessDecision::deny(reason));
    }
    gate
}

fn requirement_mutates_volume(operation: &Operation, role: OperationAccessRole) -> bool {
    match operation {
        Operation::Copy { .. } => role == OperationAccessRole::DestinationParent,
        Operation::Move { .. } | Operation::Rename { .. } | Operation::Restore { .. } => true,
        Operation::Delete { .. } | Operation::Trash { .. } | Operation::EmptyTrash { .. } => true,
    }
}

fn read_only_volume_for_path<'a>(
    report: &'a VolumeDiscoveryReport,
    path: &Path,
) -> Option<&'a gfm_mac::VolumeDescriptor> {
    report
        .volumes
        .iter()
        .filter(|volume| volume.read_only && path.starts_with(&volume.path))
        .max_by_key(|volume| volume.path.components().count())
}

fn mutation_allowed_for_role(path: &Path, role: OperationAccessRole) -> bool {
    let probe_path = match role {
        OperationAccessRole::DestinationParent => path.to_path_buf(),
        OperationAccessRole::Source | OperationAccessRole::Target => path
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| path.to_path_buf()),
    };
    matches!(
        SecurityScopedAccessReport::evaluate(probe_path, AccessIntent::Operate).action,
        SecurityDecisionAction::Allow
    )
}

fn operation_security_accesses(operation: &Operation) -> Result<Vec<SecurityScopedBookmarkAccess>> {
    let store = SecurityScopedBookmarkStore::new(default_security_bookmarks_path());
    let mut accesses = Vec::new();
    for requirement in operation.access_requirements() {
        let probe_path = operation_access_probe_path(&requirement.path, requirement.role);
        let report = SecurityScopedAccessReport::evaluate(&probe_path, AccessIntent::Operate);
        let admission = report.worker_admission(format!(
            "{} {}",
            operation_kind(operation),
            requirement.role.as_str()
        ));
        if !admission.needs_bookmark_access {
            continue;
        }
        if matches!(report.action, SecurityDecisionAction::Deny)
            && matches!(report.probe, gfm_mac::AccessProbeState::Missing)
            && !matches!(requirement.role, OperationAccessRole::DestinationParent)
        {
            continue;
        }
        let lookup = store.start_access_for_path(&probe_path, false, true)?;
        let Some(access) = lookup.access else {
            return Err(GfmError::Permission {
                path: probe_path,
                message: format!(
                    "{} requires stored security-scoped access before mutation",
                    requirement.role.as_str()
                ),
            });
        };
        accesses.push(access);
    }
    Ok(accesses)
}

fn stored_bookmark_decision(
    store: &SecurityScopedBookmarkStore,
    path: &Path,
    reason: &str,
) -> Option<OperationAccessDecision> {
    let lookup = store.resolve_for_path(path, false, true, true).ok()?;
    let resolution = lookup.resolution?;
    (resolution.report.status == SecurityScopedBookmarkStatus::Resolved).then(|| {
        OperationAccessDecision::allow(format!(
            "{reason}; bookmark=resolved; stale={}; repaired={}; resolved={}",
            resolution.report.stale,
            resolution.repaired,
            resolution
                .report
                .resolved_path
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "-".to_string())
        ))
    })
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
    use gfm_mac::VolumeDescriptor;
    use gfm_ops::{read_journal, OperationStatus};
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

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

    #[test]
    #[cfg(unix)]
    fn operation_access_gate_refuses_write_into_read_only_volume() {
        let root = unique_temp_dir("gfm-app-op-readonly-volume");
        let source_root = root.join("Source");
        let volume = root.join("ReadOnlyDrive");
        fs::create_dir_all(&source_root).unwrap();
        fs::create_dir_all(&volume).unwrap();
        fs::write(volume.join(".gfm-volume-kind"), "external-removable\n").unwrap();
        fs::set_permissions(&volume, fs::Permissions::from_mode(0o555)).unwrap();
        let source = source_root.join("source.txt");
        let destination = volume.join("copy.txt");
        fs::write(&source, "content").unwrap();
        let report = VolumeDiscoveryReport::from_paths(vec![volume.clone()]);
        let operation = Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        };

        let gate = operation_access_gate(&operation, &report);
        let journal = root.join("journal.tsv");
        let err = Operator::new(OperationContext::new(&journal).with_access_gate(gate))
            .execute(operation)
            .unwrap_err();

        assert!(matches!(err, GfmError::Permission { .. }));
        assert!(err.to_string().contains("read-only volume external"));
        assert!(!destination.exists());
        let journal_entries = read_journal(&journal).unwrap();
        assert_eq!(journal_entries.len(), 2);
        assert_eq!(journal_entries[0].status, OperationStatus::Started);
        assert_eq!(journal_entries[1].status, OperationStatus::Failed);
        assert!(journal_entries[1]
            .message
            .as_deref()
            .unwrap_or_default()
            .contains("read-only volume external"));

        fs::set_permissions(&volume, fs::Permissions::from_mode(0o755)).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operation_access_gate_ignores_broad_read_only_root_for_writable_data_path() {
        let root = unique_temp_dir("gfm-app-op-writable-data-volume");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "content").unwrap();
        let mut system_root = VolumeDescriptor::for_path("/").unwrap();
        system_root.read_only = true;
        system_root.writable = false;
        system_root.kind = VolumeKind::System;
        let report = VolumeDiscoveryReport {
            volumes: vec![system_root],
        };
        let operation = Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        };

        let gate = operation_access_gate(&operation, &report);
        let journal = root.join("journal.tsv");
        let entry = Operator::new(OperationContext::new(&journal).with_access_gate(gate))
            .execute(operation)
            .unwrap();

        assert_eq!(entry.status, OperationStatus::Completed);
        assert_eq!(fs::read_to_string(&destination).unwrap(), "content");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stored_bookmark_allows_protected_operation_preflight() {
        let root = unique_temp_dir("gfm-app-op-bookmark");
        let store_path = root.join("bookmarks.tsv");
        let store = SecurityScopedBookmarkStore::new(&store_path);
        let protected = root.join("Documents").join("Plan.md");
        fs::create_dir_all(protected.parent().unwrap()).unwrap();
        fs::write(&protected, "plan").unwrap();
        let bookmark = gfm_mac::SecurityScopedBookmark::create(&protected, false).unwrap();
        store.upsert(bookmark).unwrap();

        let decision = stored_bookmark_decision(&store, &protected, "needs scoped access")
            .expect("stored bookmark should resolve");

        assert_eq!(decision.action, gfm_ops::OperationAccessAction::Allow);
        assert!(decision.reason.contains("bookmark=resolved"));

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
