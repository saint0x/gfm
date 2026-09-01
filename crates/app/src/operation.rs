use crate::access::{
    preflight_access_scope_checked_with_volume_report, preflight_volume_access_scope_with_report,
    worker_admission_with_volume_report, ScopedAccessGuard,
};
use crate::permission_refresh::{refresh_permission_state, PermissionRefreshAudience};
use crate::required_path;
use crate::runtime::{
    default_journal_path, default_security_bookmarks_path, default_trash_metadata_path,
    run_volume_task_cancellable, run_volume_task_cancellable_with_runtime,
    runtime_operation_conflict_store, OperationConflictStore, RuntimeJobHandle,
    RuntimeOperationConflict,
};
use gfm_jobs::{JobProgressState, Priority};
use gfm_mac::{
    AccessIntent, MountState, SecurityDecisionAction, SecurityScopedAccessReport,
    SecurityScopedBookmarkAccess, SecurityScopedBookmarkStatus, SecurityScopedBookmarkStore,
    SecurityWorkerAction, VolumeDiscoveryReport, VolumeKind,
};
use gfm_ops::{
    read_trash_metadata, ConflictPolicy, Operation, OperationAccessDecision, OperationAccessGate,
    OperationAccessRole, OperationConflictReport, OperationContext, OperationMetadataDegradation,
    OperationMetadataDegradationKind, OperationProgress, OperationProgressEvent,
    OperationProgressPhase, OperationRecoveryPolicy, OperationThroughputClass,
    OperationVolumeClass, OperationVolumeCopyPolicy, Operator,
};
use gfm_types::{GfmError, Result, VolumeId};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub(crate) fn run(command: &str, args: &mut impl Iterator<Item = String>) -> Result<bool> {
    match command {
        "ops-recover" => {
            let (journal, policy) = parse_ops_recover_args(args)?;
            let report = recover_operations_from_journal(journal, policy)?;
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
            let pending = read_blocking_operation_conflict(&store, target.clone())?;
            let operation = operation_from_runtime_conflict(&pending)?;
            execute_operation(operation, conflict)?;
            let resolved = resolve_operation_conflict(&store, target, conflict)?;
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
            let pending = read_blocking_operation_conflicts(&store)?;
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
                    resolve_operation_conflicts(&store, completed_targets.clone(), conflict)?;
                    return Err(err);
                }
                completed_targets.push(record.target.clone());
            }
            resolve_operation_conflicts(&store, completed_targets, conflict)?;
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
        "copy-retry-probe" => {
            let from = required_path(args.next(), "copy-retry-probe requires a source path")?;
            let to = required_path(args.next(), "copy-retry-probe requires a destination path")?;
            let state = required_path(
                args.next(),
                "copy-retry-probe requires an attempt state path",
            )?;
            execute_operation_with_retry_probe(Operation::Copy { from, to }, state)?;
        }
        "operation-volume-copy-policy" => {
            let from = required_path(
                args.next(),
                "operation-volume-copy-policy requires a source path",
            )?;
            let to = required_path(
                args.next(),
                "operation-volume-copy-policy requires a destination path",
            )?;
            let operation = Operation::Copy { from, to };
            preflight_operation_volume_policy_access(&operation)?;
            println!("{}", operation_volume_copy_policy_report(&operation)?);
        }
        "operation-access-unavailable-volume-api" => {
            let from = required_path(
                args.next(),
                "operation-access-unavailable-volume-api requires a source path",
            )?;
            let to = required_path(
                args.next(),
                "operation-access-unavailable-volume-api requires a destination path",
            )?;
            let root = required_path(
                args.next(),
                "operation-access-unavailable-volume-api requires a volume root",
            )?;
            let operation = Operation::Copy { from, to };
            let report = unavailable_volume_api_report(&root)?;
            let gate = operation_access_gate_checked(&operation, &report, || Ok(()))?;
            match gate.check(&operation) {
                Ok(()) => println!(
                    "operation-access\tcopy\taction=allow\tvolume-root={}\treason=-",
                    root.display()
                ),
                Err(err) => println!(
                    "operation-access\tcopy\taction=deny\tvolume-root={}\treason={err}",
                    root.display()
                ),
            }
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

#[derive(Clone)]
struct OperationPathAccessReport {
    path: PathBuf,
    intent: AccessIntent,
    volume_report: VolumeDiscoveryReport,
}

impl OperationPathAccessReport {
    fn new_checked(
        path: PathBuf,
        intent: AccessIntent,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        check_control()?;
        let volume_report =
            VolumeDiscoveryReport::for_containing_path_checked(&path, &mut check_control)?;
        check_control()?;
        Ok(Self {
            path,
            intent,
            volume_report,
        })
    }

    fn preflight_volume(&self, worker: &str) -> Result<()> {
        preflight_volume_access_scope_with_report(
            &self.path,
            self.intent,
            worker,
            &self.volume_report,
        )
    }

    fn access_checked(
        &self,
        worker: &str,
        check_control: impl FnMut() -> Result<()>,
    ) -> Result<ScopedAccessGuard> {
        preflight_access_scope_checked_with_volume_report(
            &self.path,
            self.intent,
            worker,
            &self.volume_report,
            check_control,
        )
    }

    fn volume(&self) -> Option<VolumeId> {
        self.volume_report
            .volume_for_path(&self.path)
            .map(|volume| volume.id)
    }
}

fn recover_operations_from_journal(
    journal: PathBuf,
    policy: OperationRecoveryPolicy,
) -> Result<gfm_ops::OperationRecoveryReport> {
    const WORKER: &str = "operation journal";
    let journal_probe = write_probe_path_checked(&journal, WORKER, || Ok(()))?;
    let access_report =
        OperationPathAccessReport::new_checked(journal_probe, AccessIntent::Write, || Ok(()))?;
    access_report.preflight_volume(WORKER)?;
    let volume = access_report.volume();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _journal_access = access_report.access_checked(WORKER, || cancellation.check())?;
        cancellation.check()?;
        Operator::new(OperationContext::new(journal)).recover_with_policy(policy)
    })
}

fn read_blocking_operation_conflict(
    store: &OperationConflictStore,
    target: String,
) -> Result<RuntimeOperationConflict> {
    blocking_operation_conflict(read_operation_conflicts(store)?, &target)
}

fn read_blocking_operation_conflicts(
    store: &OperationConflictStore,
) -> Result<Vec<RuntimeOperationConflict>> {
    blocking_operation_conflicts(read_operation_conflicts(store)?)
}

fn read_operation_conflicts(
    store: &OperationConflictStore,
) -> Result<Vec<RuntimeOperationConflict>> {
    const WORKER: &str = "operation conflict store";
    let access_report = OperationPathAccessReport::new_checked(
        store.path().to_path_buf(),
        AccessIntent::Read,
        || Ok(()),
    )?;
    access_report.preflight_volume(WORKER)?;
    let volume = access_report.volume();
    let path = store.path().to_path_buf();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = access_report.access_checked(WORKER, || cancellation.check())?;
        cancellation.check()?;
        let store = OperationConflictStore::new(path);
        store.read_checked(|| cancellation.check())
    })
}

fn resolve_operation_conflict(
    store: &OperationConflictStore,
    target: String,
    conflict: ConflictPolicy,
) -> Result<RuntimeOperationConflict> {
    let resolved = resolve_operation_conflicts(store, vec![target.clone()], conflict)?;
    resolved.into_iter().next().ok_or_else(|| {
        GfmError::Format(format!(
            "operation conflict store has no blocking conflict for `{target}`"
        ))
    })
}

fn resolve_operation_conflicts(
    store: &OperationConflictStore,
    targets: Vec<String>,
    conflict: ConflictPolicy,
) -> Result<Vec<RuntimeOperationConflict>> {
    const WORKER: &str = "operation conflict store";
    let store_probe = write_probe_path_checked(store.path(), WORKER, || Ok(()))?;
    let access_report =
        OperationPathAccessReport::new_checked(store_probe, AccessIntent::Write, || Ok(()))?;
    access_report.preflight_volume(WORKER)?;
    let volume = access_report.volume();
    let path = store.path().to_path_buf();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = access_report.access_checked(WORKER, || cancellation.check())?;
        cancellation.check()?;
        let store = OperationConflictStore::new(path);
        store.resolve_targets_checked(&targets, conflict.as_str(), || cancellation.check())
    })
}

fn blocking_operation_conflict(
    conflicts: Vec<RuntimeOperationConflict>,
    target: &str,
) -> Result<RuntimeOperationConflict> {
    conflicts
        .into_iter()
        .find(|conflict| conflict.target == target && conflict.blocks_operation)
        .ok_or_else(|| {
            GfmError::Format(format!(
                "operation conflict store has no blocking conflict for `{target}`"
            ))
        })
}

fn blocking_operation_conflicts(
    conflicts: Vec<RuntimeOperationConflict>,
) -> Result<Vec<RuntimeOperationConflict>> {
    let conflicts = conflicts
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
    let _metadata_access = preflight_trash_metadata_read_checked(&metadata_path, || Ok(()))?;
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
    execute_operation_inner(operation, conflict, None)
}

fn execute_operation_with_retry_probe(operation: Operation, retry_probe: PathBuf) -> Result<()> {
    execute_operation_inner(operation, ConflictPolicy::Fail, Some(retry_probe))
}

fn execute_operation_inner(
    operation: Operation,
    conflict: ConflictPolicy,
    retry_probe: Option<PathBuf>,
) -> Result<()> {
    let journal = default_journal_path();
    let trash_metadata = default_trash_metadata_path();
    let label = operation_kind(&operation);
    let _ = refresh_permission_state(PermissionRefreshAudience::Operations, label)?;
    let volume_report = operation_volume_report_checked(&operation, || Ok(()))?;
    let volume_copy_policy = operation_volume_copy_policy_from_report(&operation, &volume_report);
    let volume = operation_volume(&operation, &volume_report);
    let entry = run_volume_task_cancellable_with_runtime(
        volume,
        Priority::Interactive,
        label,
        move |cancellation, runtime| {
            cancellation.check()?;
            let access_gate =
                operation_access_gate_checked(&operation, &volume_report, || cancellation.check())?;
            cancellation.check()?;
            preflight_operation_target_probe(&operation)?;
            cancellation.check()?;
            let _journal_access =
                preflight_operation_journal_write_checked(&journal, || cancellation.check())?;
            cancellation.check()?;
            let _trash_metadata_access = retain_operation_trash_metadata_access_checked(
                &operation,
                &trash_metadata,
                || cancellation.check(),
            )?;
            cancellation.check()?;
            if access_gate.check(&operation).is_ok() {
                let _security_scope =
                    operation_security_accesses_checked(&operation, &volume_report, || {
                        cancellation.check()
                    })?;
                let conflict_report = OperationConflictReport::evaluate(&operation, conflict)?;
                if conflict_report.blocks_operation {
                    if let Some(store) = runtime_operation_conflict_store() {
                        store.append_checked(&conflict_report, || cancellation.check())?;
                    }
                }
                let mut context = OperationContext::new(journal)
                    .with_conflict(conflict)
                    .with_trash_metadata_path(trash_metadata)
                    .with_access_gate(access_gate)
                    .with_volume_copy_policy(volume_copy_policy);
                if let Some(retry_probe) = retry_probe.clone() {
                    context = context.with_retry_probe_path(retry_probe);
                }
                let operator = Operator::new(context);
                return execute_with_runtime_progress_and_retry(
                    operator,
                    operation,
                    runtime,
                    &cancellation,
                );
            }
            let mut context = OperationContext::new(journal)
                .with_conflict(conflict)
                .with_trash_metadata_path(trash_metadata)
                .with_access_gate(access_gate)
                .with_volume_copy_policy(volume_copy_policy);
            if let Some(retry_probe) = retry_probe {
                context = context.with_retry_probe_path(retry_probe);
            }
            let operator = Operator::new(context);
            execute_with_runtime_progress_and_retry(operator, operation, runtime, &cancellation)
        },
    )?;
    println!("{}\t{}", entry.id, operation_status(entry.status));
    Ok(())
}

fn execute_with_runtime_progress_and_retry(
    operator: Operator,
    operation: Operation,
    runtime: RuntimeJobHandle,
    cancellation: &gfm_jobs::Cancellation,
) -> Result<gfm_ops::JournalEntry> {
    let mut progress_error = None;
    let entry = operator.execute_with_retry_policy_and_progress(
        operation,
        OperationRecoveryPolicy {
            retry_failed: true,
            max_attempts: 2,
        },
        |event| {
            emit_operation_progress_event(event.clone());
            if progress_error.is_none() {
                if let Err(err) = publish_runtime_operation_progress(&runtime, &event, cancellation)
                {
                    progress_error = Some(err);
                }
            }
        },
    )?;
    if let Some(err) = progress_error {
        return Err(err);
    }
    Ok(entry)
}

fn emit_operation_progress_event(event: OperationProgressEvent) {
    if let Some(line) = operation_progress_event_line(&event) {
        println!("{line}");
    }
}

fn publish_runtime_operation_progress(
    runtime: &RuntimeJobHandle,
    event: &OperationProgressEvent,
    cancellation: &gfm_jobs::Cancellation,
) -> Result<()> {
    let total_units = operation_progress_total_units(&event.progress);
    let completed_units = operation_progress_completed_units(&event.progress);
    let detail = operation_progress_detail(event);
    runtime.resize_checked(total_units, detail.clone(), || cancellation.check())?;
    if event.phase == OperationProgressPhase::MetadataDegraded {
        runtime.remember_completion_detail(detail.clone())?;
    }
    runtime.progress_checked(JobProgressState::Running, completed_units, detail, || {
        cancellation.check()
    })
}

fn operation_progress_total_units(progress: &OperationProgress) -> u64 {
    if progress.total_bytes > 0 {
        progress.total_bytes
    } else {
        progress.total_items.max(1)
    }
}

fn operation_progress_completed_units(progress: &OperationProgress) -> u64 {
    if progress.total_bytes > 0 {
        progress.completed_bytes
    } else {
        progress.completed_items
    }
}

fn operation_progress_detail(event: &OperationProgressEvent) -> String {
    if let Some(degradation) = event.metadata_degradation.as_ref() {
        return operation_metadata_degradation_line(degradation);
    }
    match event.phase {
        OperationProgressPhase::Planned => format!(
            "planned:items={}:bytes={}",
            event.progress.total_items, event.progress.total_bytes
        ),
        OperationProgressPhase::Advanced => {
            let throughput = event.throughput.map(|snapshot| {
                format!(
                    ":throughput={}Bps:{}",
                    snapshot.bytes_per_second,
                    operation_throughput_class_name(snapshot.class)
                )
            });
            format!(
                "advanced:items={}/{}:bytes={}/{}{}",
                event.progress.completed_items,
                event.progress.total_items,
                event.progress.completed_bytes,
                event.progress.total_bytes,
                throughput.as_deref().unwrap_or("")
            )
        }
        OperationProgressPhase::MetadataDegraded => "metadata-degraded".to_string(),
    }
}

fn operation_throughput_class_name(class: OperationThroughputClass) -> &'static str {
    match class {
        OperationThroughputClass::FullSpeed => "full-speed",
        OperationThroughputClass::Constrained => "constrained",
        OperationThroughputClass::Slow => "slow",
    }
}

fn operation_progress_event_line(event: &OperationProgressEvent) -> Option<String> {
    if event.phase != OperationProgressPhase::MetadataDegraded {
        return None;
    }
    let degradation = event.metadata_degradation.as_ref()?;
    Some(operation_metadata_degradation_line(degradation))
}

fn operation_metadata_degradation_line(degradation: &OperationMetadataDegradation) -> String {
    format!(
        "operation-metadata-degradation\tpath={}\tkind={}\tdetail={}",
        escape_operation_field(&degradation.path.display().to_string()),
        metadata_degradation_kind_name(degradation.kind),
        escape_operation_field(&degradation.detail)
    )
}

fn metadata_degradation_kind_name(kind: OperationMetadataDegradationKind) -> &'static str {
    match kind {
        OperationMetadataDegradationKind::Ownership => "ownership",
        OperationMetadataDegradationKind::CreatedTime => "created-time",
        OperationMetadataDegradationKind::ExtendedAttribute => "extended-attribute",
        OperationMetadataDegradationKind::AccessControlList => "access-control-list",
        OperationMetadataDegradationKind::FileFlags => "file-flags",
        OperationMetadataDegradationKind::SymlinkTimes => "symlink-times",
        OperationMetadataDegradationKind::HardLinkTopology => "hard-link-topology",
    }
}

fn escape_operation_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}

fn operation_volume_copy_policy_report(operation: &Operation) -> Result<String> {
    let report = operation_volume_report_checked(operation, || Ok(()))?;
    let policy = operation_volume_copy_policy_from_report(operation, &report);
    Ok(match operation {
        Operation::Copy { from, to } | Operation::Move { from, to } => format!(
            "operation-volume-copy-policy\tsource={}\tdestination={}\tsource-class={}\tdestination-class={}\tbuffer-bytes={}\tfile-cloning={}\thard-links={}\tsparse-files={}\tvolumes={}",
            from.display(),
            to.display(),
            operation_volume_class_name(policy.class_for_path(from)),
            operation_volume_class_name(policy.class_for_path(to)),
            policy.copy_buffer_bytes_for_paths(from, to),
            policy.file_cloning_supported_for_paths(from, to),
            policy.hard_links_supported_for_path(to),
            policy.sparse_files_supported_for_path(to),
            report.volumes.len()
        ),
        _ => "operation-volume-copy-policy\tsource=-\tdestination=-\tsource-class=-\tdestination-class=-\tbuffer-bytes=0\tfile-cloning=false\thard-links=false\tsparse-files=false\tvolumes=0".to_string(),
    })
}

fn preflight_operation_volume_policy_access(operation: &Operation) -> Result<()> {
    match operation {
        Operation::Copy { from, to } | Operation::Move { from, to } => {
            let source = OperationPathAccessReport::new_checked(
                from.clone(),
                AccessIntent::Read,
                || Ok(()),
            )?;
            source.preflight_volume("operation volume copy policy source")?;
            let destination = OperationPathAccessReport::new_checked(
                write_probe_path_checked(
                    to,
                    "operation volume copy policy destination",
                    || Ok(()),
                )?,
                AccessIntent::Write,
                || Ok(()),
            )?;
            destination.preflight_volume("operation volume copy policy destination")
        }
        _ => Ok(()),
    }
}

fn retain_operation_trash_metadata_access_checked(
    operation: &Operation,
    path: &Path,
    check_control: impl FnMut() -> Result<()>,
) -> Result<Option<ScopedAccessGuard>> {
    if !operation_uses_trash_metadata(operation) {
        return Ok(None);
    }
    preflight_trash_metadata_write_checked(path, check_control).map(Some)
}

fn operation_uses_trash_metadata(operation: &Operation) -> bool {
    matches!(
        operation,
        Operation::Delete { .. }
            | Operation::Trash { .. }
            | Operation::EmptyTrash { .. }
            | Operation::Restore { .. }
    )
}

fn preflight_operation_journal_write_checked(
    path: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<ScopedAccessGuard> {
    check_control()?;
    let probe = write_probe_path_checked(path, "operation journal", &mut check_control)?;
    check_control()?;
    OperationPathAccessReport::new_checked(probe, AccessIntent::Write, &mut check_control)?
        .access_checked("operation journal", check_control)
}

fn preflight_trash_metadata_read_checked(
    path: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<ScopedAccessGuard> {
    OperationPathAccessReport::new_checked(
        path.to_path_buf(),
        AccessIntent::Read,
        &mut check_control,
    )?
    .access_checked("trash metadata", check_control)
}

fn preflight_trash_metadata_write_checked(
    path: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<ScopedAccessGuard> {
    check_control()?;
    let probe = write_probe_path_checked(path, "trash metadata", &mut check_control)?;
    check_control()?;
    OperationPathAccessReport::new_checked(probe, AccessIntent::Write, &mut check_control)?
        .access_checked("trash metadata", check_control)
}

fn preflight_operation_target_probe(operation: &Operation) -> Result<()> {
    for requirement in operation.access_requirements() {
        if requirement.role != OperationAccessRole::DestinationParent {
            continue;
        }
        let probe_path = operation_access_probe_path(&requirement.path, requirement.role);
        match probe_path.try_exists() {
            Ok(_) => {}
            Err(err) => {
                return Err(GfmError::io(
                    &probe_path,
                    format!("operation target path existence unavailable: {err}"),
                ));
            }
        }
    }
    Ok(())
}

fn write_probe_path(path: &Path) -> Result<&Path> {
    match fs::metadata(path) {
        Ok(_) => Ok(path),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(crate::parent_or_cwd(path)),
        Err(err) => Err(GfmError::io(
            path,
            format!("operation write path metadata unavailable: {err}"),
        )),
    }
}

fn write_probe_path_checked(
    path: &Path,
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<PathBuf> {
    check_control()?;
    preflight_write_target_volume_checked(path, worker, &mut check_control)?;
    check_control()?;
    let probe = write_probe_path(path)?.to_path_buf();
    check_control()?;
    Ok(probe)
}

fn preflight_write_target_volume_checked(
    path: &Path,
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    check_control()?;
    let volume_path = crate::parent_or_cwd(path);
    let volume_report =
        VolumeDiscoveryReport::for_containing_path_checked(volume_path, &mut check_control)?;
    check_control()?;
    preflight_volume_access_scope_with_report(
        volume_path,
        AccessIntent::Write,
        worker,
        &volume_report,
    )
}

fn operation_access_gate_checked(
    operation: &Operation,
    volume_report: &VolumeDiscoveryReport,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<OperationAccessGate> {
    let bookmark_store = SecurityScopedBookmarkStore::new(default_security_bookmarks_path());
    operation_access_gate_with_bookmark_store_checked(
        operation,
        volume_report,
        &bookmark_store,
        &mut check_control,
    )
}

fn operation_access_gate_with_bookmark_store_checked(
    operation: &Operation,
    volume_report: &VolumeDiscoveryReport,
    bookmark_store: &SecurityScopedBookmarkStore,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<OperationAccessGate> {
    let mut gate = OperationAccessGate::new();
    check_control()?;
    for requirement in operation.access_requirements() {
        check_control()?;
        let probe_path = operation_access_probe_path(&requirement.path, requirement.role);
        check_control()?;
        let admission = worker_admission_with_volume_report(
            &probe_path,
            AccessIntent::Operate,
            format!(
                "{} {}",
                operation_kind(operation),
                requirement.role.as_str()
            ),
            volume_report,
        );
        check_control()?;
        let report = &admission.access;
        eprintln!("{}", admission.as_tsv());
        if matches!(admission.worker_action, SecurityWorkerAction::Deny)
            && !admission.can_touch_filesystem
            && !matches!(
                (report.action, report.probe),
                (
                    SecurityDecisionAction::Deny,
                    gfm_mac::AccessProbeState::Missing
                )
            )
        {
            let reason = format!(
                "{}; scope={}; mode={}; worker-action={}; role={}; probe={}; probe-path={}",
                admission.reason,
                report.scope.as_str(),
                report.mode.as_str(),
                admission.worker_action.as_str(),
                requirement.role.as_str(),
                report.probe.as_str(),
                probe_path.display()
            );
            gate = gate.with_decision(
                requirement.path,
                OperationAccessDecision::deny(reason)
                    .with_refresh_on_permission_change(admission.refresh_on_permission_change),
            );
            continue;
        }
        if matches!(report.action, SecurityDecisionAction::Deny)
            && matches!(report.probe, gfm_mac::AccessProbeState::Missing)
            && !matches!(requirement.role, OperationAccessRole::DestinationParent)
        {
            continue;
        }
        let reason = format!(
            "{}; scope={}; mode={}; worker-action={}; role={}; probe={}; probe-path={}",
            report.reason,
            report.scope.as_str(),
            report.mode.as_str(),
            admission.worker_action.as_str(),
            requirement.role.as_str(),
            report.probe.as_str(),
            probe_path.display()
        );
        let decision = if admission.needs_bookmark_access {
            check_control()?;
            Some(stored_bookmark_decision_with_refresh_checked(
                bookmark_store,
                &probe_path,
                &reason,
                admission.refresh_on_permission_change || admission.needs_bookmark_access,
                &mut check_control,
            )?)
        } else {
            None
        };
        check_control()?;
        let decision = decision.unwrap_or_else(|| match admission.worker_action {
            SecurityWorkerAction::Start => OperationAccessDecision::allow(reason)
                .with_refresh_on_permission_change(admission.refresh_on_permission_change),
            SecurityWorkerAction::Prompt => OperationAccessDecision::prompt(reason)
                .with_refresh_on_permission_change(admission.refresh_on_permission_change),
            SecurityWorkerAction::MetadataOnly | SecurityWorkerAction::Deny => {
                OperationAccessDecision::deny(reason)
                    .with_refresh_on_permission_change(admission.refresh_on_permission_change)
            }
        });
        gate = gate.with_decision(requirement.path, decision);
        check_control()?;
    }
    for requirement in operation.access_requirements() {
        check_control()?;
        let probe_path = operation_access_probe_path(&requirement.path, requirement.role);
        let Some(volume) = unavailable_mount_volume_for_path(volume_report, &probe_path) else {
            continue;
        };
        let reason = format!(
            "unmounted volume {}; label={}; root={}; stable-id={}; mount={}; reachable={}; role={}",
            volume.kind.as_str(),
            volume.label,
            volume.path.display(),
            volume.stable_identity,
            volume.mount_state.as_str(),
            volume
                .reachable
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            requirement.role.as_str()
        );
        gate = gate.with_decision(
            requirement.path,
            OperationAccessDecision::deny(reason).with_refresh_on_permission_change(true),
        );
        check_control()?;
    }
    for requirement in operation.access_requirements() {
        check_control()?;
        let probe_path = operation_access_probe_path(&requirement.path, requirement.role);
        let Some(volume) = unreachable_volume_for_path(volume_report, &probe_path) else {
            continue;
        };
        let reason = format!(
            "unreachable volume {}; label={}; root={}; stable-id={}; mount={}; reachable={}; role={}",
            volume.kind.as_str(),
            volume.label,
            volume.path.display(),
            volume.stable_identity,
            volume.mount_state.as_str(),
            volume
                .reachable
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            requirement.role.as_str()
        );
        gate = gate.with_decision(
            requirement.path,
            OperationAccessDecision::deny(reason).with_refresh_on_permission_change(true),
        );
        check_control()?;
    }
    for requirement in operation.access_requirements() {
        check_control()?;
        if !requirement_mutates_volume(operation, requirement.role) {
            continue;
        }
        let probe_path = operation_access_probe_path(&requirement.path, requirement.role);
        let Some(volume) = read_only_volume_for_path(volume_report, &probe_path) else {
            continue;
        };
        if broad_read_only_root_allows_path(volume, &probe_path, requirement.role) {
            continue;
        }
        let reason = format!(
            "read-only volume {}; label={}; root={}; stable-id={}; role={}",
            volume.kind.as_str(),
            volume.label,
            volume.path.display(),
            volume.stable_identity,
            requirement.role.as_str()
        );
        gate = gate.with_decision(
            requirement.path,
            OperationAccessDecision::deny(reason).with_refresh_on_permission_change(true),
        );
        check_control()?;
    }
    check_control()?;
    Ok(gate)
}

fn unavailable_volume_api_report(root: &Path) -> Result<VolumeDiscoveryReport> {
    let mut volume = gfm_mac::VolumeDescriptor::for_path(root)?;
    volume.kind = VolumeKind::Network;
    volume.mount_state = MountState::Mounted;
    volume.reachable = Some(true);
    volume.native_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
    volume.native_reason = Some("DiskArbitration unavailable for operation volume".to_string());
    volume.resource_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
    volume.resource_reason =
        Some("URL resource values unavailable for operation volume".to_string());
    volume.mount_table_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
    volume.mount_table_reason = Some("mount table unavailable for operation volume".to_string());
    Ok(VolumeDiscoveryReport {
        volumes: vec![volume],
    })
}

fn broad_read_only_root_allows_path(
    volume: &gfm_mac::VolumeDescriptor,
    path: &Path,
    role: OperationAccessRole,
) -> bool {
    volume.path == Path::new("/") && mutation_allowed_for_role(path, role)
}

fn unavailable_mount_volume_for_path<'a>(
    report: &'a VolumeDiscoveryReport,
    path: &Path,
) -> Option<&'a gfm_mac::VolumeDescriptor> {
    report
        .volumes
        .iter()
        .filter(|volume| {
            !matches!(volume.mount_state, MountState::Mounted) && path.starts_with(&volume.path)
        })
        .max_by_key(|volume| volume.path.components().count())
}

fn unreachable_volume_for_path<'a>(
    report: &'a VolumeDiscoveryReport,
    path: &Path,
) -> Option<&'a gfm_mac::VolumeDescriptor> {
    report
        .volumes
        .iter()
        .filter(|volume| {
            matches!(volume.mount_state, MountState::Mounted)
                && volume.reachable == Some(false)
                && path.starts_with(&volume.path)
        })
        .max_by_key(|volume| volume.path.components().count())
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

fn operation_security_accesses_checked(
    operation: &Operation,
    volume_report: &VolumeDiscoveryReport,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<SecurityScopedBookmarkAccess>> {
    let store = SecurityScopedBookmarkStore::new(default_security_bookmarks_path());
    operation_security_accesses_from_store_checked(
        operation,
        volume_report,
        &store,
        &mut check_control,
    )
}

fn operation_security_accesses_from_store_checked(
    operation: &Operation,
    volume_report: &VolumeDiscoveryReport,
    store: &SecurityScopedBookmarkStore,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<SecurityScopedBookmarkAccess>> {
    let mut accesses = Vec::new();
    check_control()?;
    for requirement in operation.access_requirements() {
        check_control()?;
        let probe_path = operation_access_probe_path(&requirement.path, requirement.role);
        check_control()?;
        let admission = worker_admission_with_volume_report(
            &probe_path,
            AccessIntent::Operate,
            format!(
                "{} {}",
                operation_kind(operation),
                requirement.role.as_str()
            ),
            volume_report,
        );
        check_control()?;
        let report = &admission.access;
        if !admission.needs_bookmark_access {
            continue;
        }
        if matches!(report.action, SecurityDecisionAction::Deny)
            && matches!(report.probe, gfm_mac::AccessProbeState::Missing)
            && !matches!(requirement.role, OperationAccessRole::DestinationParent)
        {
            continue;
        }
        check_control()?;
        let lookup =
            store.start_access_for_path_checked(&probe_path, false, true, &mut check_control)?;
        check_control()?;
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
        check_control()?;
    }
    Ok(accesses)
}

fn stored_bookmark_decision_with_refresh_checked(
    store: &SecurityScopedBookmarkStore,
    path: &Path,
    reason: &str,
    refresh_on_permission_change: bool,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<OperationAccessDecision> {
    check_control()?;
    let Ok(lookup) = store.resolve_for_path_checked(path, false, true, true, &mut check_control)
    else {
        return Ok(OperationAccessDecision::prompt(format!(
            "{reason}; bookmark=unavailable; status=unavailable"
        ))
        .with_refresh_on_permission_change(refresh_on_permission_change));
    };
    check_control()?;
    let Some(resolution) = lookup.resolution else {
        return Ok(OperationAccessDecision::prompt(format!(
            "{reason}; bookmark=missing; status=missing"
        ))
        .with_refresh_on_permission_change(refresh_on_permission_change));
    };
    check_control()?;
    if resolution.report.status == SecurityScopedBookmarkStatus::Resolved {
        Ok(OperationAccessDecision::allow(format!(
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
        .with_refresh_on_permission_change(refresh_on_permission_change))
    } else {
        Ok(OperationAccessDecision::prompt(format!(
            "{reason}; bookmark=unavailable; status={}; stale={}; repaired={}",
            resolution.report.status.as_str(),
            resolution.report.stale,
            resolution.repaired
        ))
        .with_refresh_on_permission_change(refresh_on_permission_change))
    }
}

fn operation_access_probe_path(path: &Path, role: OperationAccessRole) -> PathBuf {
    if !matches!(role, OperationAccessRole::DestinationParent) {
        return path.to_path_buf();
    }
    let mut candidate = path.to_path_buf();
    loop {
        match candidate.try_exists() {
            Ok(true) | Err(_) => break,
            Ok(false) => {
                let Some(parent) = candidate.parent() else {
                    break;
                };
                if parent == candidate {
                    break;
                }
                candidate = parent.to_path_buf();
            }
        }
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
            policy = policy
                .with_root(
                    volume.path.clone(),
                    operation_volume_class_for_descriptor(volume),
                )
                .with_root_volume_identity(volume.path.clone(), volume.stable_identity.clone());
            if let Some(supported) = volume.resource_supports_file_cloning {
                policy = policy.with_root_file_cloning_support(volume.path.clone(), supported);
            }
            if let Some(supported) = volume.resource_supports_hard_links {
                policy = policy.with_root_hard_link_support(volume.path.clone(), supported);
            }
            if let Some(supported) = volume.resource_supports_sparse_files {
                policy = policy.with_root_sparse_file_support(volume.path.clone(), supported);
            }
        }
    }
    policy
}

fn operation_volume_report_checked(
    operation: &Operation,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<VolumeDiscoveryReport> {
    let mut report = VolumeDiscoveryReport {
        volumes: Vec::new(),
    };
    for path in operation_paths(operation) {
        check_control()?;
        let containing =
            VolumeDiscoveryReport::for_containing_path_checked(path, &mut check_control)?;
        if let Some(volume) = containing.volume_for_path(path) {
            report.volumes.push(volume.clone());
        }
        check_control()?;
    }
    report.volumes.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.label.cmp(&right.label))
            .then(left.id.cmp(&right.id))
    });
    report
        .volumes
        .dedup_by(|left, right| left.id == right.id && left.path == right.path);
    check_control()?;
    Ok(report)
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
        VolumeKind::System | VolumeKind::Internal => OperationVolumeClass::Local,
        VolumeKind::External | VolumeKind::Removable | VolumeKind::DiskImage => {
            OperationVolumeClass::External
        }
        VolumeKind::Network => OperationVolumeClass::Network,
        VolumeKind::Unknown => OperationVolumeClass::Network,
    }
}

fn operation_volume_class_for_descriptor(
    volume: &gfm_mac::VolumeDescriptor,
) -> OperationVolumeClass {
    match operation_volume_class_for_kind(volume.kind) {
        OperationVolumeClass::External if slow_operation_volume(volume) => {
            OperationVolumeClass::Slow
        }
        class => class,
    }
}

fn slow_operation_volume(volume: &gfm_mac::VolumeDescriptor) -> bool {
    if volume.network || volume.reachable == Some(false) {
        return false;
    }
    if volume.kind == VolumeKind::DiskImage {
        return true;
    }
    if !matches!(volume.kind, VolumeKind::External | VolumeKind::Removable) {
        return false;
    }
    let protocol = volume
        .device_protocol
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let media_kind = volume
        .media_kind
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let media_type = volume
        .media_type
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    volume.resource_automounted == Some(true)
        || volume.removable
            && (protocol.contains("usb")
                || protocol.contains("firewire")
                || media_kind.contains("removable")
                || media_type.contains("removable"))
}

fn operation_volume(operation: &Operation, report: &VolumeDiscoveryReport) -> Option<VolumeId> {
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
        .and_then(|path| report.volume_for_path(path).map(|volume| volume.id))
        .or_else(|| {
            operation
                .target_path()
                .and_then(|path| report.volume_for_path(path).map(|volume| volume.id))
        })
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

fn operation_volume_class_name(class: OperationVolumeClass) -> &'static str {
    match class {
        OperationVolumeClass::Local => "local",
        OperationVolumeClass::External => "external",
        OperationVolumeClass::Network => "network",
        OperationVolumeClass::Slow => "slow",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfm_mac::VolumeDescriptor;
    use gfm_ops::{read_journal, OperationProgress, OperationStatus};
    use std::ffi::{OsStr, OsString};
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn operation_progress_event_reports_metadata_degradation() {
        let event = OperationProgressEvent {
            phase: OperationProgressPhase::MetadataDegraded,
            progress: OperationProgress {
                total_items: 2,
                total_bytes: 0,
                completed_items: 1,
                completed_bytes: 0,
            },
            throughput: None,
            metadata_degradation: Some(OperationMetadataDegradation {
                path: PathBuf::from("/Volumes/Backup/dir\talias.txt"),
                kind: OperationMetadataDegradationKind::HardLinkTopology,
                detail: "hard-link topology was not preserved\nvolume lacks links".to_string(),
            }),
        };

        assert_eq!(
            operation_progress_event_line(&event).as_deref(),
            Some(
                "operation-metadata-degradation\tpath=/Volumes/Backup/dir\\talias.txt\tkind=hard-link-topology\tdetail=hard-link topology was not preserved\\nvolume lacks links"
            )
        );
    }

    #[test]
    fn operation_progress_event_ignores_non_degradation_progress() {
        let event = OperationProgressEvent {
            phase: OperationProgressPhase::Advanced,
            progress: OperationProgress {
                total_items: 1,
                total_bytes: 4,
                completed_items: 1,
                completed_bytes: 4,
            },
            throughput: None,
            metadata_degradation: None,
        };

        assert!(operation_progress_event_line(&event).is_none());
    }

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
    fn operation_volume_copy_policy_report_uses_discovered_descriptors() {
        let root = unique_temp_dir("gfm-app-op-volume-policy-report");
        let network = root.join("TeamShare");
        let external = root.join("Backup");
        fs::create_dir_all(&network).unwrap();
        fs::create_dir_all(&external).unwrap();
        fs::write(network.join(".gfm-volume-kind"), "network-smb\n").unwrap();
        fs::write(external.join(".gfm-volume-kind"), "external-removable\n").unwrap();
        let source = network.join("source.bin");
        let destination = external.join("destination.bin");
        let operation = Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        };

        let report = operation_volume_copy_policy_report(&operation).unwrap();

        assert!(report.starts_with("operation-volume-copy-policy\t"));
        assert!(report.contains("\tsource-class=network\t"));
        assert!(report.contains("\tdestination-class=external\t"));
        assert!(report.contains("\tbuffer-bytes=65536\t"));
        assert!(report.contains("\tfile-cloning=false\t"));
        assert!(report.contains("\thard-links=true\t"));
        assert!(report.contains("\tsparse-files=true\t"));
        assert!(report.contains("\tvolumes="));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operation_volume_policy_uses_descriptor_file_cloning_support() {
        let root = unique_temp_dir("gfm-app-op-volume-policy-clone-support");
        let source_root = root.join("Source");
        let destination_root = root.join("LegacyBackup");
        fs::create_dir_all(&source_root).unwrap();
        fs::create_dir_all(&destination_root).unwrap();
        fs::write(
            destination_root.join(".gfm-volume-kind"),
            "external-removable\n",
        )
        .unwrap();
        let source = source_root.join("source.bin");
        let destination = destination_root.join("destination.bin");
        let mut report = VolumeDiscoveryReport::from_paths(vec![destination_root.clone()]);
        report.volumes[0].resource_supports_file_cloning = Some(false);
        report.volumes[0].resource_supports_hard_links = Some(false);
        report.volumes[0].resource_supports_sparse_files = Some(false);
        let operation = Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        };

        let policy = operation_volume_copy_policy_from_report(&operation, &report);

        assert!(!policy.file_cloning_supported_for_paths(&source, &destination));
        assert!(!policy.hard_links_supported_for_path(&destination));
        assert!(!policy.sparse_files_supported_for_path(&destination));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operation_volume_policy_requires_explicit_non_local_capabilities() {
        let root = unique_temp_dir("gfm-app-op-volume-policy-unknown-capabilities");
        let source_root = root.join("Source");
        let destination_root = root.join("Backup");
        fs::create_dir_all(&source_root).unwrap();
        fs::create_dir_all(&destination_root).unwrap();
        let source = source_root.join("source.bin");
        let destination = destination_root.join("destination.bin");
        let mut destination_volume = VolumeDescriptor::for_path(&destination_root).unwrap();
        destination_volume.kind = VolumeKind::External;
        destination_volume.resource_supports_file_cloning = None;
        destination_volume.resource_supports_hard_links = None;
        destination_volume.resource_supports_sparse_files = None;
        let report = VolumeDiscoveryReport {
            volumes: vec![destination_volume],
        };
        let operation = Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        };

        let policy = operation_volume_copy_policy_from_report(&operation, &report);

        assert!(!policy.file_cloning_supported_for_paths(&source, &destination));
        assert!(!policy.hard_links_supported_for_path(&destination));
        assert!(!policy.sparse_files_supported_for_path(&destination));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operation_volume_policy_requires_identity_for_explicit_non_local_cloning() {
        let root = unique_temp_dir("gfm-app-op-volume-policy-clone-identity");
        let source_root = root.join("Source");
        let destination_root = root.join("Backup");
        fs::create_dir_all(&source_root).unwrap();
        fs::create_dir_all(&destination_root).unwrap();
        let source = source_root.join("source.bin");
        let destination = destination_root.join("destination.bin");
        let mut destination_volume = VolumeDescriptor::for_path(&destination_root).unwrap();
        destination_volume.kind = VolumeKind::External;
        destination_volume.resource_supports_file_cloning = Some(true);
        destination_volume.stable_identity.clear();
        let report = VolumeDiscoveryReport {
            volumes: vec![destination_volume],
        };
        let operation = Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        };

        let policy = operation_volume_copy_policy_from_report(&operation, &report);

        assert!(!policy.file_cloning_supported_for_paths(&source, &destination));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operation_volume_policy_skips_file_cloning_for_distinct_known_volumes() {
        let root = unique_temp_dir("gfm-app-op-volume-policy-distinct-clone");
        let source_root = root.join("Source");
        let destination_root = root.join("Destination");
        fs::create_dir_all(&source_root).unwrap();
        fs::create_dir_all(&destination_root).unwrap();
        fs::write(source_root.join(".gfm-volume-kind"), "external-removable\n").unwrap();
        fs::write(
            destination_root.join(".gfm-volume-kind"),
            "external-removable\n",
        )
        .unwrap();
        let source = source_root.join("source.bin");
        let destination = destination_root.join("destination.bin");
        let mut report =
            VolumeDiscoveryReport::from_paths(vec![source_root.clone(), destination_root.clone()]);
        for volume in &mut report.volumes {
            volume.resource_supports_file_cloning = Some(true);
        }
        let operation = Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        };

        let policy = operation_volume_copy_policy_from_report(&operation, &report);

        assert!(!policy.file_cloning_supported_for_paths(&source, &destination));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operation_volume_policy_uses_slow_class_for_removable_usb_descriptor() {
        let root = unique_temp_dir("gfm-app-op-volume-policy-slow-usb");
        let removable = root.join("CameraCard");
        let local = root.join("LocalWork");
        fs::create_dir_all(&removable).unwrap();
        fs::create_dir_all(&local).unwrap();
        fs::write(removable.join(".gfm-volume-kind"), "external-removable\n").unwrap();
        let source = removable.join("source.bin");
        let destination = local.join("destination.bin");
        let mut report = VolumeDiscoveryReport::from_paths(vec![removable.clone()]);
        report.volumes[0].device_protocol = Some("USB".to_string());
        report.volumes[0].media_kind = Some("Removable Media".to_string());
        report.volumes[0].media_type = Some("Generic".to_string());
        report.volumes[0].removable = true;
        let operation = Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        };

        let policy = operation_volume_copy_policy_from_report(&operation, &report);

        assert_eq!(policy.class_for_path(&source), OperationVolumeClass::Slow);
        assert_eq!(
            policy.class_for_path(&destination),
            OperationVolumeClass::Local
        );
        assert_eq!(
            policy.copy_buffer_bytes_for_paths(&source, &destination),
            64 * 1024
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operation_volume_policy_uses_slow_class_for_disk_image_descriptor() {
        let root = unique_temp_dir("gfm-app-op-volume-policy-disk-image");
        let image = root.join("Installer");
        let local = root.join("LocalWork");
        fs::create_dir_all(&image).unwrap();
        fs::create_dir_all(&local).unwrap();
        fs::write(image.join(".gfm-volume-kind"), "disk-image\n").unwrap();
        let source = image.join("source.bin");
        let destination = local.join("destination.bin");
        let report = VolumeDiscoveryReport::from_paths(vec![image.clone()]);
        let operation = Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        };

        let policy = operation_volume_copy_policy_from_report(&operation, &report);

        assert_eq!(policy.class_for_path(&source), OperationVolumeClass::Slow);
        assert_eq!(
            policy.copy_buffer_bytes_for_paths(&source, &destination),
            64 * 1024
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operation_volume_policy_uses_slow_class_for_automounted_external_descriptor() {
        let root = unique_temp_dir("gfm-app-op-volume-policy-automounted");
        let external = root.join("AutoMounted");
        let local = root.join("LocalWork");
        fs::create_dir_all(&external).unwrap();
        fs::create_dir_all(&local).unwrap();
        fs::write(external.join(".gfm-volume-kind"), "external-removable\n").unwrap();
        let source = external.join("source.bin");
        let destination = local.join("destination.bin");
        let mut report = VolumeDiscoveryReport::from_paths(vec![external.clone()]);
        report.volumes[0].resource_automounted = Some(true);
        report.volumes[0].removable = false;
        report.volumes[0].device_protocol = Some("PCI-Express".to_string());
        let operation = Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        };

        let policy = operation_volume_copy_policy_from_report(&operation, &report);

        assert_eq!(policy.class_for_path(&source), OperationVolumeClass::Slow);
        assert_eq!(
            policy.copy_buffer_bytes_for_paths(&source, &destination),
            64 * 1024
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operation_volume_policy_keeps_network_descriptor_network_not_slow() {
        let root = unique_temp_dir("gfm-app-op-volume-policy-network");
        let network = root.join("TeamShare");
        let local = root.join("LocalWork");
        fs::create_dir_all(&network).unwrap();
        fs::create_dir_all(&local).unwrap();
        fs::write(network.join(".gfm-volume-kind"), "network-smb\n").unwrap();
        let source = network.join("source.bin");
        let destination = local.join("destination.bin");
        let mut report = VolumeDiscoveryReport::from_paths(vec![network.clone()]);
        report.volumes[0].device_protocol = Some("USB".to_string());
        report.volumes[0].removable = true;
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
            policy.copy_buffer_bytes_for_paths(&source, &destination),
            64 * 1024
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operation_volume_policy_constrains_unknown_descriptor_as_network() {
        let root = unique_temp_dir("gfm-app-op-volume-policy-unknown");
        let source_root = root.join("Unclassified");
        let destination_root = root.join("LocalWork");
        fs::create_dir_all(&source_root).unwrap();
        fs::create_dir_all(&destination_root).unwrap();
        let source = source_root.join("source.bin");
        let destination = destination_root.join("destination.bin");
        let mut report = VolumeDiscoveryReport::from_paths(vec![source_root.clone()]);
        report.volumes[0].kind = VolumeKind::Unknown;
        report.volumes[0].network = false;
        report.volumes[0].local = None;
        report.volumes[0].resource_supports_file_cloning = None;
        report.volumes[0].resource_supports_hard_links = None;
        report.volumes[0].resource_supports_sparse_files = None;
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
            policy.copy_buffer_bytes_for_paths(&source, &destination),
            64 * 1024
        );
        assert!(!policy.file_cloning_supported_for_paths(&source, &destination));
        assert!(!policy.hard_links_supported_for_path(&source));
        assert!(!policy.sparse_files_supported_for_path(&source));

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
    fn operation_access_gate_refuses_write_into_read_only_volume() {
        let root = unique_temp_dir("gfm-app-op-readonly-volume");
        let source_root = root.join("Source");
        let volume = root.join("ReadOnlyDrive");
        fs::create_dir_all(&source_root).unwrap();
        fs::create_dir_all(&volume).unwrap();
        fs::write(volume.join(".gfm-volume-kind"), "external-removable\n").unwrap();
        let source = source_root.join("source.txt");
        let destination = volume.join("copy.txt");
        fs::write(&source, "content").unwrap();
        let mut report = VolumeDiscoveryReport::from_paths(vec![volume.clone()]);
        report.volumes[0].read_only = true;
        report.volumes[0].writable = false;
        let operation = Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        };

        let gate = operation_access_gate_checked(&operation, &report, || Ok(())).unwrap();
        let journal = root.join("journal.tsv");
        let err = Operator::new(OperationContext::new(&journal).with_access_gate(gate))
            .execute(operation)
            .unwrap_err();

        assert!(matches!(err, GfmError::Permission { .. }));
        assert!(err.to_string().contains("read-only volume external"));
        assert!(err
            .to_string()
            .contains("refresh-on-permission-change=true"));
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

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operation_access_gate_refuses_unreachable_network_volume_before_copying() {
        let root = unique_temp_dir("gfm-app-op-unreachable-volume");
        let source_root = root.join("Source");
        let volume = root.join("TeamShare");
        fs::create_dir_all(&source_root).unwrap();
        fs::create_dir_all(&volume).unwrap();
        fs::write(volume.join(".gfm-volume-kind"), "network-smb\n").unwrap();
        let source = source_root.join("source.txt");
        let destination = volume.join("copy.txt");
        fs::write(&source, "content").unwrap();
        let mut report = VolumeDiscoveryReport::from_paths(vec![volume.clone()]);
        report.volumes[0].reachable = Some(false);
        let operation = Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        };

        let gate = operation_access_gate_checked(&operation, &report, || Ok(())).unwrap();
        let journal = root.join("journal.tsv");
        let err = Operator::new(OperationContext::new(&journal).with_access_gate(gate))
            .execute(operation)
            .unwrap_err();

        assert!(matches!(err, GfmError::Permission { .. }));
        assert!(err.to_string().contains("unreachable volume network"));
        assert!(err.to_string().contains("role=destination-parent"));
        assert!(!destination.exists());
        let journal_entries = read_journal(&journal).unwrap();
        assert_eq!(journal_entries.len(), 2);
        assert_eq!(journal_entries[0].status, OperationStatus::Started);
        assert_eq!(journal_entries[1].status, OperationStatus::Failed);
        assert!(journal_entries[1]
            .message
            .as_deref()
            .unwrap_or_default()
            .contains("unreachable volume network"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operation_access_gate_refuses_unavailable_volume_api_state_before_copying() {
        let root = unique_temp_dir("gfm-app-op-unavailable-volume-api");
        let source_root = root.join("Source");
        let volume = root.join("TeamShare");
        fs::create_dir_all(&source_root).unwrap();
        fs::create_dir_all(&volume).unwrap();
        let source = source_root.join("source.txt");
        let destination = volume.join("copy.txt");
        fs::write(&source, "content").unwrap();
        let operation = Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        };
        let report = unavailable_volume_api_report(&volume).unwrap();

        let gate = operation_access_gate_checked(&operation, &report, || Ok(())).unwrap();
        let journal = root.join("journal.tsv");
        let err = Operator::new(OperationContext::new(&journal).with_access_gate(gate))
            .execute(operation)
            .unwrap_err();

        assert!(matches!(err, GfmError::Permission { .. }));
        assert!(err.to_string().contains("unavailable volume network"));
        assert!(err.to_string().contains("native-status=unavailable"));
        assert!(err
            .to_string()
            .contains("native-reason=DiskArbitration unavailable for operation volume"));
        assert!(err
            .to_string()
            .contains("resource-reason=URL resource values unavailable for operation volume"));
        assert!(err
            .to_string()
            .contains("mount-reason=mount table unavailable for operation volume"));
        assert!(err.to_string().contains("role=destination-parent"));
        assert!(err
            .to_string()
            .contains("refresh-on-permission-change=true"));
        assert!(!destination.exists());
        let journal_entries = read_journal(&journal).unwrap();
        assert_eq!(journal_entries.len(), 2);
        assert_eq!(journal_entries[0].status, OperationStatus::Started);
        assert_eq!(journal_entries[1].status, OperationStatus::Failed);
        assert!(journal_entries[1]
            .message
            .as_deref()
            .unwrap_or_default()
            .contains("unavailable volume network"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operation_access_gate_refuses_unreachable_network_source_before_planning() {
        let root = unique_temp_dir("gfm-app-op-unreachable-source");
        let volume = root.join("TeamShare");
        let destination_root = root.join("Destination");
        fs::create_dir_all(&volume).unwrap();
        fs::create_dir_all(&destination_root).unwrap();
        fs::write(volume.join(".gfm-volume-kind"), "network-smb\n").unwrap();
        let source = volume.join("source.txt");
        let destination = destination_root.join("copy.txt");
        let mut report = VolumeDiscoveryReport::from_paths(vec![volume.clone()]);
        report.volumes[0].reachable = Some(false);
        let operation = Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        };

        let gate = operation_access_gate_checked(&operation, &report, || Ok(())).unwrap();
        let journal = root.join("journal.tsv");
        let err = Operator::new(OperationContext::new(&journal).with_access_gate(gate))
            .execute(operation)
            .unwrap_err();

        assert!(matches!(err, GfmError::Permission { .. }));
        assert!(err.to_string().contains("unreachable volume network"));
        assert!(err.to_string().contains("role=source"));
        assert!(!destination.exists());
        let journal_entries = read_journal(&journal).unwrap();
        assert_eq!(journal_entries.len(), 2);
        assert_eq!(journal_entries[0].status, OperationStatus::Started);
        assert_eq!(journal_entries[1].status, OperationStatus::Failed);
        assert!(journal_entries[1]
            .message
            .as_deref()
            .unwrap_or_default()
            .contains("unreachable volume network"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn destination_parent_probe_stops_at_unavailable_path() {
        let root = unique_temp_dir("gfm-app-op-destination-parent-unavailable");
        let missing_child = root.join("missing").join("leaf.txt");
        let unavailable_child = root.join("destination-parent-unavailable".repeat(16));

        assert_eq!(
            operation_access_probe_path(&missing_child, OperationAccessRole::DestinationParent),
            root
        );
        assert_eq!(
            operation_access_probe_path(&unavailable_child, OperationAccessRole::DestinationParent),
            unavailable_child
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operation_access_gate_refuses_stale_volume_source_before_planning() {
        let root = unique_temp_dir("gfm-app-op-stale-source");
        let volume = root.join("TeamShare");
        let destination_root = root.join("Destination");
        fs::create_dir_all(&volume).unwrap();
        fs::create_dir_all(&destination_root).unwrap();
        fs::write(volume.join(".gfm-volume-kind"), "network-smb\n").unwrap();
        let source = volume.join("source.txt");
        let destination = destination_root.join("copy.txt");
        let mut report = VolumeDiscoveryReport::from_paths(vec![volume.clone()]);
        report.volumes[0].mount_state = MountState::Stale;
        report.volumes[0].reachable = Some(true);
        let operation = Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        };

        let gate = operation_access_gate_checked(&operation, &report, || Ok(())).unwrap();
        let journal = root.join("journal.tsv");
        let err = Operator::new(OperationContext::new(&journal).with_access_gate(gate))
            .execute(operation)
            .unwrap_err();

        assert!(matches!(err, GfmError::Permission { .. }));
        assert!(err.to_string().contains("unmounted volume network"));
        assert!(err.to_string().contains("mount=stale"));
        assert!(err.to_string().contains("role=source"));
        assert!(err
            .to_string()
            .contains("refresh-on-permission-change=true"));
        assert!(!destination.exists());
        let journal_entries = read_journal(&journal).unwrap();
        assert_eq!(journal_entries.len(), 2);
        assert_eq!(journal_entries[0].status, OperationStatus::Started);
        assert_eq!(journal_entries[1].status, OperationStatus::Failed);
        assert!(journal_entries[1]
            .message
            .as_deref()
            .unwrap_or_default()
            .contains("mount=stale"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operation_access_gate_refuses_unmounted_destination_before_copying() {
        let root = unique_temp_dir("gfm-app-op-unmounted-destination");
        let source_root = root.join("Source");
        let volume = root.join("Archive");
        fs::create_dir_all(&source_root).unwrap();
        fs::create_dir_all(&volume).unwrap();
        fs::write(volume.join(".gfm-volume-kind"), "external-removable\n").unwrap();
        let source = source_root.join("source.txt");
        let destination = volume.join("copy.txt");
        fs::write(&source, "content").unwrap();
        let mut report = VolumeDiscoveryReport::from_paths(vec![volume.clone()]);
        report.volumes[0].mount_state = MountState::Unmounted;
        report.volumes[0].reachable = Some(true);
        let operation = Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        };

        let gate = operation_access_gate_checked(&operation, &report, || Ok(())).unwrap();
        let journal = root.join("journal.tsv");
        let err = Operator::new(OperationContext::new(&journal).with_access_gate(gate))
            .execute(operation)
            .unwrap_err();

        assert!(matches!(err, GfmError::Permission { .. }));
        assert!(err.to_string().contains("unmounted volume external"));
        assert!(err.to_string().contains("mount=unmounted"));
        assert!(err.to_string().contains("role=destination-parent"));
        assert!(err
            .to_string()
            .contains("refresh-on-permission-change=true"));
        assert!(!destination.exists());
        let journal_entries = read_journal(&journal).unwrap();
        assert_eq!(journal_entries.len(), 2);
        assert_eq!(journal_entries[0].status, OperationStatus::Started);
        assert_eq!(journal_entries[1].status, OperationStatus::Failed);
        assert!(journal_entries[1]
            .message
            .as_deref()
            .unwrap_or_default()
            .contains("mount=unmounted"));

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

        let gate = operation_access_gate_checked(&operation, &report, || Ok(())).unwrap();
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

        let decision = stored_bookmark_decision_with_refresh_checked(
            &store,
            &protected,
            "needs scoped access",
            true,
            || Ok(()),
        )
        .unwrap();

        assert_eq!(decision.action, gfm_ops::OperationAccessAction::Allow);
        assert!(decision.reason.contains("bookmark=resolved"));
        assert!(decision.refresh_on_permission_change);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_bookmark_prompts_operation_preflight_before_mutation() {
        let root = unique_temp_dir("gfm-app-op-missing-bookmark");
        let store = SecurityScopedBookmarkStore::new(root.join("bookmarks.tsv"));
        let protected = root.join("Documents").join("Plan.md");
        fs::create_dir_all(protected.parent().unwrap()).unwrap();
        fs::write(&protected, "plan").unwrap();

        let decision = stored_bookmark_decision_with_refresh_checked(
            &store,
            &protected,
            "needs scoped access",
            true,
            || Ok(()),
        )
        .unwrap();

        assert_eq!(decision.action, gfm_ops::OperationAccessAction::Prompt);
        assert!(decision.reason.contains("bookmark=missing"));
        assert!(decision.refresh_on_permission_change);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operation_access_gate_checked_honors_pre_cancelled_control() {
        let root = unique_temp_dir("gfm-app-op-access-gate-pre-cancel");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "content").unwrap();
        let operation = Operation::Copy {
            from: source,
            to: destination,
        };
        let report = VolumeDiscoveryReport::from_paths(vec![root.clone()]);

        let result =
            operation_access_gate_checked(&operation, &report, || Err(GfmError::Cancelled));

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operation_access_gate_checked_can_cancel_before_bookmark_store_lookup() {
        let _env = ENV_LOCK.lock().unwrap();
        let root = unique_temp_dir("gfm-app-op-access-gate-bookmark-cancel");
        let home = root.join("home");
        let documents = home.join("Documents");
        let protected = documents.join("Plan.md");
        let destination = root.join("destination.txt");
        let store = SecurityScopedBookmarkStore::new(root.join("bookmarks.tsv"));
        fs::create_dir_all(&documents).unwrap();
        fs::write(&protected, "content").unwrap();
        let _home = EnvVarGuard::set("HOME", &home);
        let operation = Operation::Copy {
            from: protected,
            to: destination,
        };
        let report = VolumeDiscoveryReport::from_paths(vec![root.clone()]);
        let mut checks = 0usize;

        let result =
            operation_access_gate_with_bookmark_store_checked(&operation, &report, &store, || {
                checks += 1;
                if checks >= 5 {
                    Err(GfmError::Cancelled)
                } else {
                    Ok(())
                }
            });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(checks >= 5);
        assert!(
            !store.path().exists(),
            "cancelled operation access gate must stop before touching bookmark store"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operation_security_accesses_checked_honors_pre_cancelled_control() {
        let root = unique_temp_dir("gfm-app-op-security-access-pre-cancel");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "content").unwrap();
        let operation = Operation::Copy {
            from: source,
            to: destination,
        };
        let report = VolumeDiscoveryReport::from_paths(vec![root.clone()]);

        let result =
            operation_security_accesses_checked(&operation, &report, || Err(GfmError::Cancelled));

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operation_security_accesses_checked_can_cancel_before_bookmark_store_lookup() {
        let _env = ENV_LOCK.lock().unwrap();
        let root = unique_temp_dir("gfm-app-op-security-access-bookmark-cancel");
        let home = root.join("home");
        let documents = home.join("Documents");
        let protected = documents.join("Plan.md");
        let destination = root.join("destination.txt");
        let store = SecurityScopedBookmarkStore::new(root.join("bookmarks.tsv"));
        fs::create_dir_all(&documents).unwrap();
        fs::write(&protected, "content").unwrap();
        let _home = EnvVarGuard::set("HOME", &home);
        let operation = Operation::Copy {
            from: protected,
            to: destination,
        };
        let report = VolumeDiscoveryReport::from_paths(vec![root.clone()]);
        let mut checks = 0usize;

        let result =
            operation_security_accesses_from_store_checked(&operation, &report, &store, || {
                checks += 1;
                if checks >= 5 {
                    Err(GfmError::Cancelled)
                } else {
                    Ok(())
                }
            });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(checks >= 5);
        assert!(
            !store.path().exists(),
            "cancelled operation preflight must stop before touching bookmark store"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operation_journal_write_checked_honors_pre_cancelled_control() {
        let root = unique_temp_dir("gfm-app-op-journal-pre-cancel");
        let journal = root.join("ops.journal");

        let result =
            preflight_operation_journal_write_checked(&journal, || Err(GfmError::Cancelled));

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!journal.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operation_journal_write_refuses_unreachable_volume_before_write_probe() {
        let root = unique_temp_dir("gfm-app-op-journal-unreachable-before-probe");
        fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
        let journal = root.join(format!(
            "{}.journal",
            "operation-journal-unavailable".repeat(16)
        ));

        let err = match preflight_operation_journal_write_checked(&journal, || Ok(())) {
            Ok(_) => panic!("unreachable operation journal was admitted before volume preflight"),
            Err(err) => err,
        };

        assert!(
            err.to_string()
                .contains("operation journal volume access blocked: unreachable volume network"),
            "{err}"
        );
        assert!(
            !err.to_string()
                .contains("operation write path metadata unavailable"),
            "{err}"
        );
        assert!(!journal.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operation_conflict_store_write_refuses_unreachable_volume_before_write_probe() {
        let root = unique_temp_dir("gfm-app-op-conflict-store-unreachable-before-probe");
        fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
        let store = OperationConflictStore::new(root.join(format!(
            "{}.tsv",
            "operation-conflicts-unavailable".repeat(16)
        )));

        let err = match resolve_operation_conflicts(
            &store,
            vec!["target.txt".into()],
            ConflictPolicy::Skip,
        ) {
            Ok(_) => {
                panic!("unreachable operation conflict store was admitted before volume preflight")
            }
            Err(err) => err,
        };

        assert!(
            err.to_string().contains(
                "operation conflict store volume access blocked: unreachable volume network"
            ),
            "{err}"
        );
        assert!(
            !err.to_string()
                .contains("operation write path metadata unavailable"),
            "{err}"
        );
        assert!(!store.path().exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operation_volume_policy_write_refuses_unreachable_destination_before_write_probe() {
        let root = unique_temp_dir("gfm-app-op-copy-policy-unreachable-before-probe");
        let source_root = root.join("Source");
        let offline = root.join("Offline");
        fs::create_dir_all(&source_root).unwrap();
        fs::create_dir_all(&offline).unwrap();
        fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
        let source = source_root.join("source.bin");
        fs::write(&source, "policy only").unwrap();
        let destination = offline.join(format!(
            "{}.bin",
            "operation-copy-policy-destination-unavailable".repeat(8)
        ));
        let operation = Operation::Copy {
            from: source,
            to: destination.clone(),
        };

        let err = match preflight_operation_volume_policy_access(&operation) {
            Ok(_) => {
                panic!(
                    "unreachable operation policy destination was admitted before volume preflight"
                )
            }
            Err(err) => err,
        };

        assert!(
            err.to_string().contains(
                "operation volume copy policy destination volume access blocked: unreachable volume network"
            ),
            "{err}"
        );
        assert!(
            !err.to_string()
                .contains("operation write path metadata unavailable"),
            "{err}"
        );
        assert!(!destination.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operation_path_access_report_checked_honors_pre_cancelled_control_before_volume_discovery() {
        let path = std::env::temp_dir()
            .join(format!(
                "gfm-app-op-path-report-pre-cancel-{}",
                std::process::id()
            ))
            .join("source.txt");

        let result =
            OperationPathAccessReport::new_checked(path.clone(), AccessIntent::Read, || {
                Err(GfmError::Cancelled)
            });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!path.exists());
    }

    #[test]
    fn operation_volume_report_checked_can_cancel_between_paths() {
        let root = unique_temp_dir("gfm-app-op-volume-report-cancel");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "content").unwrap();
        let operation = Operation::Copy {
            from: source,
            to: destination,
        };
        let mut checks = 0usize;

        let result = operation_volume_report_checked(&operation, || {
            checks += 1;
            if checks > 3 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(checks > 3);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operation_trash_metadata_access_checked_can_cancel_before_write_probe() {
        let root = unique_temp_dir("gfm-app-op-trash-metadata-cancel");
        let metadata = root.join("trash.tsv");
        let operation = Operation::Trash {
            path: root.join("target.txt"),
        };
        let mut checks = 0usize;

        let result = retain_operation_trash_metadata_access_checked(&operation, &metadata, || {
            checks += 1;
            if checks >= 2 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(checks >= 2);
        assert!(!metadata.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operation_trash_metadata_write_refuses_unreachable_volume_before_write_probe() {
        let root = unique_temp_dir("gfm-app-trash-metadata-unreachable-before-probe");
        fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
        let metadata = root.join(format!("{}.tsv", "trash-metadata-unavailable".repeat(16)));
        let operation = Operation::Trash {
            path: root.join("File.txt"),
        };

        let err = match retain_operation_trash_metadata_access_checked(
            &operation,
            &metadata,
            || Ok(()),
        ) {
            Ok(_) => panic!("unreachable trash metadata was admitted before volume preflight"),
            Err(err) => err,
        };

        assert!(
            err.to_string()
                .contains("trash metadata volume access blocked: unreachable volume network"),
            "{err}"
        );
        assert!(
            !err.to_string()
                .contains("operation write path metadata unavailable"),
            "{err}"
        );
        assert!(!metadata.exists());
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

    struct EnvVarGuard {
        key: &'static str,
        original: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
            let original = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(original) = &self.original {
                std::env::set_var(self.key, original);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }
}
