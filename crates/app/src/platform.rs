use crate::access::{
    preflight_access_scope_checked, preflight_volume_access_scope,
    worker_admission_with_volume_gate, worker_admissions_with_shared_volume_report,
    ScopedAccessGuard, WorkerAdmissionRequest,
};
use crate::volume::{resolve_volume_event_path, volume_event_invalidation_for_descriptor};
use crate::{
    detect_volume_id, index_volume_descriptor, parent_volume, parse_required_scheduling_pressure,
    run_preview_contract_adaptive_with_volume_and_payload_path,
    run_preview_contract_cancellable_with_payload_path,
    runtime::{
        default_job_journal_path, preflight_runtime_job_state, run_volume_task_cancellable,
        RuntimeJobHandle,
    },
};
use gfm_fs::record_for_path_checked;
use gfm_index::{
    parse_volume_indexing_policy, IndexMountState, IndexVolumeClass, IndexVolumeDescriptor,
    IndexVolumeEventKind, ProviderMetadataInvalidationReport, VolumeEventIndexInvalidationReport,
    VolumeIndexPolicy, VolumeInvalidationReport,
};
use gfm_jobs::{
    Cancellation, JobClass, JobIoPressure, JobJournal, JobPayloadKind, JobProgressState, Priority,
    RetriableTask, RetryPolicy, Scheduler, SchedulingAction, SchedulingPressure, TaskStatus,
    VolumeConcurrencyPolicy, WorkerPool,
};
use gfm_mac::{
    current_host_profile, parse_spotlight_fixture, AccessIntent, CloudStorageState,
    CloudTransferDirection, FileProviderConflictReport, FileProviderDomainEnumerationReport,
    FileProviderDomainReport, FileProviderInvalidationReport, FileProviderObservedInvalidation,
    FileProviderOperation, FileProviderOperationReport, FileProviderProgressReport,
    FileProviderStateInvalidationReport, FileProviderStateObserver, FileProviderStateReport,
    FileProviderStateSnapshot, MacBridgeContract, NativeIconBridgeContract, NativeIconDescriptor,
    NativeIconInvalidationReport, SecurityScopedAccessReport, SecurityScopedBookmarkStatus,
    SecurityScopedBookmarkStore, SecurityWorkerAction, SecurityWorkerAdmissionReport,
    SpotlightMetadataReader, SpotlightReconciliationReport, VolumeDescriptor,
    VolumeDiscoveryReport, VolumeEventInvalidationReport, VolumeEventKind, VolumeEventState,
    VolumeEventStream, VolumeMountIdentityReport, VolumeOperation, VolumeOperationReport,
    VolumeTopologyChangeKind, VolumeTopologyDiff, WatchRoot,
};
use gfm_preview::{
    decide_invalidation, decide_preview_security, preview_invalidation_for_fileprovider,
    security_input_for_path, IconPreviewContract, IconPreviewInput, PreviewCache,
    PreviewCacheConfig, PreviewInvalidationEvent, PreviewKind, PreviewRequestKey, PreviewScheduler,
    PreviewSchedulingPolicy, PreviewSecurityPolicy, PreviewTask, QuickLookSessionContract,
    QuickLookSessionInput, Rect, ThumbnailGenerationContract, ThumbnailGenerationInput, Viewport,
};
use gfm_types::{FileEvent, FileEventKind, FileId, FileRecord, GfmError, Result, VolumeId};
use gfm_ui::{
    SidebarVolumeEventKind, SidebarVolumeInvalidation, SidebarVolumeKind, SidebarVolumeMountState,
    SidebarVolumeSpec,
};
use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

pub(crate) fn run(command: &str, args: &mut impl Iterator<Item = String>) -> Result<bool> {
    match command {
        "security-scope" => {
            let path = required_path(args.next(), "security-scope requires a path")?;
            let intent = args
                .next()
                .map(|value| AccessIntent::parse(&value))
                .transpose()?
                .unwrap_or(AccessIntent::Read);
            println!(
                "{}",
                SecurityScopedAccessReport::evaluate(path, intent).as_tsv()
            );
        }
        "security-worker-admission" => {
            let worker = required_string(
                args.next(),
                "security-worker-admission requires a worker label",
            )?;
            let path = required_path(args.next(), "security-worker-admission requires a path")?;
            let intent = args
                .next()
                .map(|value| AccessIntent::parse(&value))
                .transpose()?
                .unwrap_or(AccessIntent::Read);
            println!(
                "{}",
                worker_admission_with_volume_gate(&path, intent, worker).as_tsv()
            );
        }
        "security-worker-admission-fanout" => {
            let path = required_path(
                args.next(),
                "security-worker-admission-fanout requires a path",
            )?;
            let requests = parse_worker_admission_requests(args)?;
            let admissions = worker_admissions_with_shared_volume_report(&path, &requests);
            println!("{}", worker_admission_fanout_summary(&admissions));
            for admission in admissions {
                println!("{}", admission.as_tsv());
            }
        }
        "security-worker-admission-unavailable-volume-api" => {
            let worker = required_string(
                args.next(),
                "security-worker-admission-unavailable-volume-api requires a worker label",
            )?;
            let path = required_path(
                args.next(),
                "security-worker-admission-unavailable-volume-api requires a path",
            )?;
            let root = required_path(
                args.next(),
                "security-worker-admission-unavailable-volume-api requires a volume root",
            )?;
            let intent = args
                .next()
                .map(|value| AccessIntent::parse(&value))
                .transpose()?
                .unwrap_or(AccessIntent::Read);
            let mut volume = VolumeDescriptor::for_path(&root)?;
            volume.kind = gfm_mac::VolumeKind::Network;
            volume.mount_state = gfm_mac::MountState::Mounted;
            volume.reachable = Some(true);
            volume.native_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
            volume.resource_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
            volume.mount_table_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
            let report = VolumeDiscoveryReport {
                volumes: vec![volume],
            };
            println!(
                "{}",
                crate::access::worker_admission_with_volume_report(&path, intent, worker, &report,)
                    .as_tsv()
            );
        }
        "security-bookmark-create" => {
            let path = required_path(args.next(), "security-bookmark-create requires a path")?;
            let intent = args
                .next()
                .map(|value| AccessIntent::parse(&value))
                .transpose()?
                .unwrap_or(AccessIntent::Read);
            for line in run_security_bookmark_create(path, intent)? {
                println!("{line}");
            }
        }
        "security-bookmark-reconcile" => {
            println!("{}", run_security_bookmark_reconcile()?.as_tsv());
        }
        "mac-bridges" => {
            println!("{}", MacBridgeContract::finder_required().as_tsv());
        }
        "native-icon" => {
            let path = required_path(args.next(), "native-icon requires a path")?;
            println!("{}", run_native_icon(path)?.as_tsv());
        }
        "native-icon-bridge" => {
            let path = required_path(args.next(), "native-icon-bridge requires a path")?;
            println!("{}", run_native_icon_bridge(path)?.as_tsv());
        }
        "native-icon-fileprovider-invalidation" => {
            let previous = CloudStorageState::parse(&required_string(
                args.next(),
                "native-icon-fileprovider-invalidation requires a previous state",
            )?)?;
            let path = required_path(
                args.next(),
                "native-icon-fileprovider-invalidation requires a path",
            )?;
            let report = run_fileprovider_read(path, "native icon fileprovider", move |path| {
                FileProviderInvalidationReport::evaluate(path, previous)
            })?;
            println!(
                "{}",
                NativeIconInvalidationReport::from_fileprovider(&report).as_tsv()
            );
        }
        "native-icon-fileprovider-observer-probe" => {
            let state_path = required_path(
                args.next(),
                "native-icon-fileprovider-observer-probe requires a state path",
            )?;
            let root = required_path(
                args.next(),
                "native-icon-fileprovider-observer-probe requires a root",
            )?;
            let target = required_path(
                args.next(),
                "native-icon-fileprovider-observer-probe requires a FileProvider target path",
            )?;
            let observed = run_fileprovider_observer_probe(
                &state_path,
                &root,
                &target,
                "native icon fileprovider observer",
            )?;
            println!("{}", observed_native_icon_invalidation_tsv(&observed));
        }
        "fileprovider-state" => {
            let path = required_path(args.next(), "fileprovider-state requires a path")?;
            let report = run_fileprovider_read(
                path,
                "fileprovider state",
                FileProviderStateReport::read_path,
            )?;
            println!("{}", report.as_tsv());
        }
        "fileprovider-state-with-identity" => {
            let path = required_path(
                args.next(),
                "fileprovider-state-with-identity requires a path",
            )?;
            let report = run_fileprovider_read(path, "fileprovider identity state", |path| {
                Ok(FileProviderStateReport::from_path_with_native_identity(
                    path,
                ))
            });
            println!("{}", report?.as_tsv());
        }
        "fileprovider-domain" => {
            let path = required_path(args.next(), "fileprovider-domain requires a path")?;
            let report = run_fileprovider_read(
                path,
                "fileprovider domain",
                FileProviderDomainReport::read_path,
            )?;
            println!("{}", report.as_tsv());
        }
        "fileprovider-domains" => {
            println!(
                "{}",
                FileProviderDomainEnumerationReport::discover().as_tsv()
            );
        }
        "fileprovider-progress" => {
            let path = required_path(args.next(), "fileprovider-progress requires a path")?;
            let report = run_fileprovider_read(
                path,
                "fileprovider progress",
                FileProviderProgressReport::read_path,
            )?;
            println!("{}", report.as_tsv());
        }
        "fileprovider-conflict" => {
            let path = required_path(args.next(), "fileprovider-conflict requires a path")?;
            let report = run_fileprovider_read(
                path,
                "fileprovider conflict",
                FileProviderConflictReport::read_path,
            )?;
            println!("{}", report.as_tsv());
        }
        "fileprovider-progress-job" | "fileprovider-progress-job-cancel-after-access" => {
            let cancel_after_access = command == "fileprovider-progress-job-cancel-after-access";
            let path = required_path(args.next(), "fileprovider-progress-job requires a path")?;
            let _runtime_access = preflight_runtime_job_state("fileprovider progress job")?;
            let journal = default_job_journal_path();
            let journal_probe = write_probe_path(&journal)?.to_path_buf();
            let _journal_access = preflight_access_scope_checked(
                &journal_probe,
                AccessIntent::Write,
                "fileprovider progress job",
                || Ok(()),
            )?;
            let report = match run_fileprovider_progress_job(path, cancel_after_access) {
                Ok(report) => report,
                Err(GfmError::Cancelled) if cancel_after_access => {
                    println!(
                        "fileprovider-progress\tstatus=cancelled\treason=cancelled-after-access"
                    );
                    return Ok(true);
                }
                Err(err) => return Err(err),
            };
            println!("{}", report.as_tsv());
        }
        "fileprovider-operation" => {
            let operation = FileProviderOperation::parse(&required_string(
                args.next(),
                "fileprovider-operation requires an operation",
            )?)?;
            let path = required_path(args.next(), "fileprovider-operation requires a path")?;
            let report = run_fileprovider_operation(path, operation)?;
            println!("{}", report.as_tsv());
        }
        "fileprovider-invalidation" => {
            let previous = CloudStorageState::parse(&required_string(
                args.next(),
                "fileprovider-invalidation requires a previous state",
            )?)?;
            let path = required_path(args.next(), "fileprovider-invalidation requires a path")?;
            let report = run_fileprovider_read(path, "fileprovider invalidation", move |path| {
                FileProviderInvalidationReport::evaluate(path, previous)
            });
            println!("{}", report?.as_tsv());
        }
        "fileprovider-metadata-invalidation" => {
            let previous = CloudStorageState::parse(&required_string(
                args.next(),
                "fileprovider-metadata-invalidation requires a previous state",
            )?)?;
            let path = required_path(
                args.next(),
                "fileprovider-metadata-invalidation requires a path",
            )?;
            let report =
                run_fileprovider_read(path, "fileprovider metadata invalidation", move |path| {
                    FileProviderInvalidationReport::evaluate(path, previous)
                })?;
            println!(
                "{}",
                ProviderMetadataInvalidationReport::from_provider_transition(
                    report.path,
                    report.previous.as_str(),
                    report.current.storage_state.as_str(),
                    report.reindex_metadata,
                    report.state_changed,
                    report.reason,
                )
                .as_tsv()
            );
        }
        "preview-cache-fileprovider-invalidation" => {
            let cache_root = required_path(
                args.next(),
                "preview-cache-fileprovider-invalidation requires a cache root",
            )?;
            let previous = CloudStorageState::parse(&required_string(
                args.next(),
                "preview-cache-fileprovider-invalidation requires a previous state",
            )?)?;
            let path = required_path(
                args.next(),
                "preview-cache-fileprovider-invalidation requires a path",
            )?;
            let kind = parse_preview_kind(args.next())?;
            println!(
                "{}",
                run_preview_cache_fileprovider_invalidation(cache_root, previous, path, kind)?
            );
        }
        "preview-cache-fileprovider-observed-invalidation" => {
            let cache_root = required_path(
                args.next(),
                "preview-cache-fileprovider-observed-invalidation requires a cache root",
            )?;
            let state_path = required_path(
                args.next(),
                "preview-cache-fileprovider-observed-invalidation requires a state path",
            )?;
            let kind = parse_preview_kind(args.next())?;
            let event_kind = required_string(
                args.next(),
                "preview-cache-fileprovider-observed-invalidation requires an event kind",
            )?;
            let path = required_path(
                args.next(),
                "preview-cache-fileprovider-observed-invalidation requires a path",
            )?;
            let event =
                parse_fileprovider_event(&event_kind, path, args.next().map(PathBuf::from))?;
            println!(
                "{}",
                run_preview_cache_fileprovider_observed_invalidation(
                    cache_root, state_path, kind, event,
                )?
            );
        }
        "preview-cache-fileprovider-observer-probe" => {
            let cache_root = required_path(
                args.next(),
                "preview-cache-fileprovider-observer-probe requires a cache root",
            )?;
            let state_path = required_path(
                args.next(),
                "preview-cache-fileprovider-observer-probe requires a state path",
            )?;
            let kind = parse_preview_kind(args.next())?;
            let root = required_path(
                args.next(),
                "preview-cache-fileprovider-observer-probe requires a root",
            )?;
            let target = required_path(
                args.next(),
                "preview-cache-fileprovider-observer-probe requires a FileProvider target path",
            )?;
            let observed = run_fileprovider_observer_probe(
                &state_path,
                &root,
                &target,
                "preview cache fileprovider observer",
            )?;
            let cache_probe = write_probe_path(&cache_root)?.to_path_buf();
            preflight_volume_access_scope(
                &cache_probe,
                AccessIntent::Write,
                "preview cache fileprovider observer cache",
            )?;
            let volume = detect_volume_id(&cache_probe)
                .ok()
                .or_else(|| parent_volume(&cache_probe));
            println!(
                "{}",
                run_volume_task_cancellable(
                    volume,
                    Priority::Visible,
                    "preview cache fileprovider observer cache",
                    move |cancellation| {
                        cancellation.check()?;
                        let _cache_access = preflight_access_scope_checked(
                            &cache_probe,
                            AccessIntent::Write,
                            "preview cache fileprovider observer cache",
                            || cancellation.check(),
                        )?;
                        cancellation.check()?;
                        observed_preview_cache_invalidation_tsv(
                            &observed,
                            &cache_root,
                            kind,
                            &cancellation,
                        )
                    },
                )?
            );
        }
        "fileprovider-invalidation-scan" => {
            let state_path = required_path(
                args.next(),
                "fileprovider-invalidation-scan requires a state path",
            )?;
            let paths = args.map(PathBuf::from).collect::<Vec<_>>();
            if paths.is_empty() {
                return Err(GfmError::Format(
                    "fileprovider-invalidation-scan requires at least one path".to_string(),
                ));
            }
            let report = run_fileprovider_invalidation_scan(state_path, paths)?;
            println!("{}", report.as_tsv());
        }
        "fileprovider-invalidation-event" => {
            let state_path = required_path(
                args.next(),
                "fileprovider-invalidation-event requires a state path",
            )?;
            let event_kind = required_string(
                args.next(),
                "fileprovider-invalidation-event requires an event kind",
            )?;
            let path = required_path(
                args.next(),
                "fileprovider-invalidation-event requires a path",
            )?;
            let event =
                parse_fileprovider_event(&event_kind, path, args.next().map(PathBuf::from))?;
            let observed = run_fileprovider_observed_invalidation(
                state_path,
                event,
                "fileprovider invalidation event",
            )?;
            println!("{}", observed.as_tsv());
        }
        "fileprovider-observed-metadata-invalidation" => {
            let state_path = required_path(
                args.next(),
                "fileprovider-observed-metadata-invalidation requires a state path",
            )?;
            let event_kind = required_string(
                args.next(),
                "fileprovider-observed-metadata-invalidation requires an event kind",
            )?;
            let path = required_path(
                args.next(),
                "fileprovider-observed-metadata-invalidation requires a path",
            )?;
            let event =
                parse_fileprovider_event(&event_kind, path, args.next().map(PathBuf::from))?;
            let observed = run_fileprovider_observed_invalidation(
                state_path,
                event,
                "fileprovider observed metadata invalidation",
            )?;
            println!("{}", observed_metadata_invalidation_tsv(&observed));
        }
        "fileprovider-observer-probe" => {
            let state_path = required_path(
                args.next(),
                "fileprovider-observer-probe requires a state path",
            )?;
            let root = required_path(args.next(), "fileprovider-observer-probe requires a root")?;
            let target = required_path(
                args.next(),
                "fileprovider-observer-probe requires a FileProvider target path",
            )?;
            let observed = run_fileprovider_observer_probe(
                &state_path,
                &root,
                &target,
                "fileprovider observer",
            )?;
            println!("{}", observed.as_tsv());
        }
        "fileprovider-observer-metadata-probe" => {
            let state_path = required_path(
                args.next(),
                "fileprovider-observer-metadata-probe requires a state path",
            )?;
            let root = required_path(
                args.next(),
                "fileprovider-observer-metadata-probe requires a root",
            )?;
            let target = required_path(
                args.next(),
                "fileprovider-observer-metadata-probe requires a FileProvider target path",
            )?;
            let observed = run_fileprovider_observer_probe(
                &state_path,
                &root,
                &target,
                "fileprovider observer metadata",
            )?;
            println!("{}", observed_metadata_invalidation_tsv(&observed));
        }
        "volume-discovery" => {
            let paths: Vec<PathBuf> = args.map(PathBuf::from).collect();
            let report = volume_discovery_report(paths)?;
            println!("{}", report.as_tsv());
        }
        "volume-events-probe" => {
            let stream = VolumeEventStream::start();
            println!(
                "volume-events\tattached={}\tpending={}",
                stream.is_attached(),
                stream.try_recv().is_some()
            );
        }
        "volume-events-shutdown-probe" => {
            let stream = VolumeEventStream::start();
            let pending = stream.try_recv().is_some();
            let shutdown = stream.shutdown();
            println!("{}\tpending-before={}", shutdown.as_tsv(), pending);
        }
        "volume-event-invalidation" => {
            let kind = parse_volume_event_kind(&required_string(
                args.next(),
                "volume-event-invalidation requires an event kind",
            )?)?;
            let resolution = resolve_volume_event_path(kind, args.next().map(PathBuf::from))?;
            println!("{}", resolution.invalidation_report(kind).as_tsv());
        }
        "volume-event-transition-invalidation" => {
            let kind = parse_volume_event_kind(&required_string(
                args.next(),
                "volume-event-transition-invalidation requires an event kind",
            )?)?;
            let path = PathBuf::from(required_string(
                args.next(),
                "volume-event-transition-invalidation requires a volume path",
            )?);
            let previous_label = required_string(
                args.next(),
                "volume-event-transition-invalidation requires a previous label",
            )?;
            let current_label = required_string(
                args.next(),
                "volume-event-transition-invalidation requires a current label",
            )?;
            let mut previous = VolumeDescriptor::for_path(&path)?;
            previous.label = previous_label;
            let mut current = previous.clone();
            current.label = current_label;
            println!(
                "{}",
                VolumeEventInvalidationReport::from_transition(
                    kind,
                    gfm_mac::NativeVolumeStatus::Available,
                    Some(&previous),
                    Some(&current),
                    None,
                )
                .as_tsv()
            );
        }
        "volume-event-transition-case-sensitivity" => {
            let previous_case_sensitive = parse_platform_bool(
                &required_string(
                    args.next(),
                    "volume-event-transition-case-sensitivity requires a previous case-sensitive flag",
                )?,
                "previous case-sensitive",
            )?;
            let current_case_sensitive = parse_platform_bool(
                &required_string(
                    args.next(),
                    "volume-event-transition-case-sensitivity requires a current case-sensitive flag",
                )?,
                "current case-sensitive",
            )?;
            println!(
                "{}",
                volume_event_transition_case_sensitivity(
                    previous_case_sensitive,
                    current_case_sensitive
                )?
                .as_tsv()
            );
        }
        "volume-event-transition-removable-media" => {
            println!("{}", volume_event_transition_removable_media()?.as_tsv());
        }
        "volume-event-transition-api-status" => {
            println!("{}", volume_event_transition_api_status()?.as_tsv());
        }
        "volume-event-description-api-status" => {
            println!("{}", volume_event_description_api_status()?.as_tsv());
        }
        "volume-event-index-invalidation" => {
            println!(
                "{}",
                volume_event_index_invalidation_from_args(args)?.as_tsv()
            );
        }
        "volume-event-state-index-invalidation" => {
            println!(
                "{}",
                volume_event_state_index_invalidation_from_args(args)?.as_tsv()
            );
        }
        "volume-case-sensitivity-invalidation" => {
            let previous_case_sensitive = parse_platform_bool(
                &required_string(
                    args.next(),
                    "volume-case-sensitivity-invalidation requires a previous case-sensitive flag",
                )?,
                "previous case-sensitive",
            )?;
            let current_case_sensitive = parse_platform_bool(
                &required_string(
                    args.next(),
                    "volume-case-sensitivity-invalidation requires a current case-sensitive flag",
                )?,
                "current case-sensitive",
            )?;
            println!(
                "{}",
                volume_case_sensitivity_invalidation(
                    previous_case_sensitive,
                    current_case_sensitive
                )
                .as_tsv()
            );
        }
        "volume-removable-media-invalidation" => {
            println!("{}", volume_removable_media_invalidation().as_tsv());
        }
        "volume-root-filesystem-invalidation" => {
            println!("{}", volume_root_filesystem_invalidation().as_tsv());
        }
        "volume-bsd-identity-invalidation" => {
            println!("{}", volume_bsd_identity_invalidation().as_tsv());
        }
        "volume-event-runtime-invalidation" => {
            let report = volume_event_index_invalidation_from_args(args)?;
            println!("{}", report.as_tsv());
            if let Some(cancellation) = runtime_volume_cancellation(&report) {
                println!("{}", cancellation.as_tsv());
            } else {
                println!(
                    "volume-job-cancellation\tvolume=-\tclass=background\tcancelled=0\treason={}",
                    if report.cancel_index_jobs {
                        "missing-volume-id"
                    } else {
                        "index-jobs-still-valid"
                    }
                );
            }
        }
        "volume-event-runtime-fanout" => {
            for line in volume_event_runtime_fanout_from_args(args)? {
                println!("{line}");
            }
        }
        "volume-operation" => {
            let operation = VolumeOperation::parse(&required_string(
                args.next(),
                "volume-operation requires an operation",
            )?)?;
            let path = required_path(args.next(), "volume-operation requires a path")?;
            let _access = match path.try_exists() {
                Ok(true) => Some(preflight_access_scope_checked(
                    &path,
                    AccessIntent::Operate,
                    "volume operation",
                    || Ok(()),
                )?),
                Ok(false) | Err(_) => None,
            };
            println!(
                "{}",
                VolumeOperationReport::execute(path, operation)?.as_tsv()
            );
        }
        "volume-mount-bsd" => {
            let bsd_name = required_string(
                args.next(),
                "volume-mount-bsd requires a BSD disk name such as disk4s1",
            )?;
            println!("{}", VolumeMountIdentityReport::execute(bsd_name).as_tsv());
        }
        "permission-invalidation-unavailable-volume-api" => {
            let state = required_path(
                args.next(),
                "permission-invalidation-unavailable-volume-api requires a permission state path",
            )?;
            let root = required_path(
                args.next(),
                "permission-invalidation-unavailable-volume-api requires a volume root",
            )?;
            let mut volume = VolumeDescriptor::for_path(&root)?;
            volume.kind = gfm_mac::VolumeKind::Network;
            volume.mount_state = gfm_mac::MountState::Mounted;
            volume.reachable = Some(true);
            volume.native_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
            volume.resource_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
            volume.mount_table_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
            let report = VolumeDiscoveryReport {
                volumes: vec![volume],
            };
            println!(
                "{}",
                crate::permission_refresh::refresh_permission_state_at_path_with_report(
                    &state, &root, &report,
                )?
                .as_tsv()
            );
        }
        "volume-index-policy" => {
            let external = parse_volume_indexing_policy(&required_string(
                args.next(),
                "volume-index-policy requires an external policy",
            )?)?;
            let network = parse_volume_indexing_policy(&required_string(
                args.next(),
                "volume-index-policy requires a network policy",
            )?)?;
            let mut opted_in = Vec::new();
            let mut paths = Vec::new();
            for arg in args {
                if let Some(path) = arg.strip_prefix("opt-in:") {
                    opted_in.push(PathBuf::from(path));
                } else {
                    paths.push(PathBuf::from(arg));
                }
            }
            let volumes = volume_discovery_report(paths)?
                .volumes
                .iter()
                .map(index_volume_descriptor)
                .collect::<Vec<_>>();
            let policy = VolumeIndexPolicy::new(external, network).with_opted_in_roots(opted_in);
            println!("{}", policy.plan(volumes).as_tsv());
        }
        "volume-invalidation" => {
            let previous_class = IndexVolumeClass::parse(&required_string(
                args.next(),
                "volume-invalidation requires a previous class",
            )?)?;
            let previous_mount = IndexMountState::parse(&required_string(
                args.next(),
                "volume-invalidation requires a previous mount state",
            )?)?;
            let previous_path = required_path(args.next(), "volume-invalidation requires a path")?;
            let current = current_index_volume_descriptor(&previous_path)?;
            let previous = previous_index_volume_descriptor_from_args(
                IndexVolumeDescriptor::new(
                    previous_path
                        .file_name()
                        .and_then(|label| label.to_str())
                        .unwrap_or("Volume"),
                    previous_path.clone(),
                    previous_class,
                    previous_mount,
                ),
                args,
            )?;
            println!(
                "{}",
                VolumeInvalidationReport::evaluate(Some(&previous), current.as_ref()).as_tsv()
            );
        }
        "volume-known-facts-lost-invalidation" => {
            for line in volume_known_facts_lost_invalidation() {
                println!("{line}");
            }
        }
        "volume-topology-diff" => {
            let (previous_paths, current_paths) = split_topology_paths(args)?;
            let previous = VolumeDiscoveryReport::from_paths_checked(previous_paths)?;
            let current = VolumeDiscoveryReport::from_paths_checked(current_paths)?;
            println!(
                "{}",
                VolumeTopologyDiff::evaluate(&previous, &current).as_tsv()
            );
        }
        "volume-topology-index-invalidation" => {
            let (previous_paths, current_paths) = split_topology_paths(args)?;
            let previous = VolumeDiscoveryReport::from_paths_checked(previous_paths)?;
            let current = VolumeDiscoveryReport::from_paths_checked(current_paths)?;
            println!(
                "{}",
                volume_topology_index_invalidation_tsv(&previous, &current)
            );
        }
        "volume-topology-case-sensitivity" => {
            let previous_case_sensitive = parse_platform_bool(
                &required_string(
                    args.next(),
                    "volume-topology-case-sensitivity requires a previous case-sensitive flag",
                )?,
                "previous case-sensitive",
            )?;
            let current_case_sensitive = parse_platform_bool(
                &required_string(
                    args.next(),
                    "volume-topology-case-sensitivity requires a current case-sensitive flag",
                )?,
                "current case-sensitive",
            )?;
            println!(
                "{}",
                topology_case_sensitivity_diff(previous_case_sensitive, current_case_sensitive)?
                    .as_tsv()
            );
        }
        "volume-topology-removable-media" => {
            println!("{}", topology_removable_media_diff()?.as_tsv());
        }
        "volume-topology-api-status" => {
            println!("{}", topology_api_status_diff()?.as_tsv());
        }
        "spotlight-reconcile" => {
            let path = required_path(args.next(), "spotlight-reconcile requires a path")?;
            let fixture_path = args.next().map(PathBuf::from);
            println!("{}", run_spotlight_reconcile(path, fixture_path)?.as_tsv());
        }
        "preview-check" => {
            let path = required_path(args.next(), "preview-check requires a path")?;
            let kind = parse_preview_kind(args.next())?;
            let input = security_input_for_path(&path, kind);
            let decision = decide_preview_security(&PreviewSecurityPolicy::default(), &input);
            let invalidation = decide_invalidation(PreviewInvalidationEvent {
                content_changed: true,
                ..PreviewInvalidationEvent::default()
            });
            println!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                kind.as_str(),
                input.trust.as_str(),
                input.is_executable,
                input.is_remote,
                decision.as_str(),
                invalidation.invalidate_disk,
                path.display()
            );
        }
        "icon-preview" => {
            let path = required_path(args.next(), "icon-preview requires a path")?;
            println!("{}", run_icon_preview(path)?.as_tsv());
        }
        "icon-preview-retry-probe" => {
            let path = required_path(args.next(), "icon-preview-retry-probe requires a path")?;
            let attempt_state = required_path(
                args.next(),
                "icon-preview-retry-probe requires an attempt state path",
            )?;
            println!(
                "{}",
                run_icon_preview_retry_probe(path, attempt_state)?.as_tsv()
            );
        }
        "quicklook-session" => {
            let path = required_path(args.next(), "quicklook-session requires a path")?;
            println!("{}", run_quicklook_session(path)?.as_tsv());
        }
        "quicklook-session-retry-probe" => {
            let path = required_path(args.next(), "quicklook-session-retry-probe requires a path")?;
            let attempt_state = required_path(
                args.next(),
                "quicklook-session-retry-probe requires an attempt state path",
            )?;
            println!(
                "{}",
                run_quicklook_session_retry_probe(path, attempt_state)?.as_tsv()
            );
        }
        "quicklook-session-adaptive" | "quicklook-session-adaptive-cancel-after-access" => {
            let cancel_after_access = command == "quicklook-session-adaptive-cancel-after-access";
            let path = required_path(args.next(), &format!("{command} requires a path"))?;
            let pressure = parse_required_scheduling_pressure(args, "quicklook preview")?;
            let outcome = match run_adaptive_quicklook_session(path, pressure, cancel_after_access)
            {
                Ok(outcome) => outcome,
                Err(GfmError::Cancelled) if cancel_after_access => {
                    println!("quicklook-session\tstatus=cancelled\treason=cancelled-after-access");
                    return Ok(true);
                }
                Err(err) => return Err(err),
            };
            match outcome.result {
                Some(contract) => println!(
                    "{}\taction={}\tdeferred={}",
                    contract.as_tsv(),
                    outcome.scheduling_action.as_str(),
                    outcome.deferred
                ),
                None if outcome.scheduling_action == SchedulingAction::Defer => println!(
                    "quicklook-session\tstatus=deferred\taction={}\tdeferred=true",
                    outcome.scheduling_action.as_str()
                ),
                None => {
                    return Err(GfmError::Format(
                        "quicklook preview adaptive job completed without a contract".to_string(),
                    ))
                }
            }
        }
        "quicklook-session-cancel" => {
            let path = required_path(args.next(), "quicklook-session-cancel requires a path")?;
            let cancellation = Cancellation::default();
            cancellation.cancel();
            if let Err(GfmError::Cancelled) = cancellation.check() {
                println!("quicklook-session\tstatus=cancelled\treason=cancelled-before-plan");
                return Ok(true);
            }
            let record = record_for_path_checked(&path, None, false, || cancellation.check())?;
            let input = QuickLookSessionInput::new(
                PreviewRequestKey::new(record.id, path.clone(), PreviewKind::QuickLook),
                Rect::new(0, 0, 640, 480),
                Viewport::new(Rect::new(0, 0, 1024, 768), 256),
            );
            match QuickLookSessionContract::from_input_checked(
                &PreviewSecurityPolicy::default(),
                input,
                || cancellation.check(),
            ) {
                Err(GfmError::Cancelled) => {
                    println!("quicklook-session\tstatus=cancelled\treason=cancelled-before-plan")
                }
                Err(err) => return Err(err),
                Ok(_) => {
                    return Err(GfmError::Format(
                        "pre-cancelled quicklook session unexpectedly completed".to_string(),
                    ))
                }
            }
        }
        "thumbnail-generation" => {
            let path = required_path(args.next(), "thumbnail-generation requires a path")?;
            println!("{}", run_thumbnail_generation(path)?.as_tsv());
        }
        "thumbnail-generation-retry-probe" => {
            let path = required_path(
                args.next(),
                "thumbnail-generation-retry-probe requires a path",
            )?;
            let attempt_state = required_path(
                args.next(),
                "thumbnail-generation-retry-probe requires an attempt state path",
            )?;
            println!(
                "{}",
                run_thumbnail_generation_retry_probe(path, attempt_state)?.as_tsv()
            );
        }
        "thumbnail-generation-adaptive" | "thumbnail-generation-adaptive-cancel-after-access" => {
            let cancel_after_access =
                command == "thumbnail-generation-adaptive-cancel-after-access";
            let path = required_path(args.next(), &format!("{command} requires a path"))?;
            let pressure = parse_required_scheduling_pressure(args, "thumbnail generation")?;
            let outcome =
                match run_adaptive_thumbnail_generation(path, pressure, cancel_after_access) {
                    Ok(outcome) => outcome,
                    Err(GfmError::Cancelled) if cancel_after_access => {
                        println!(
                            "thumbnail-generation\tstatus=cancelled\treason=cancelled-after-access"
                        );
                        return Ok(true);
                    }
                    Err(err) => return Err(err),
                };
            match outcome.result {
                Some(contract) => println!(
                    "{}\taction={}\tdeferred={}",
                    contract.as_tsv(),
                    outcome.scheduling_action.as_str(),
                    outcome.deferred
                ),
                None if outcome.scheduling_action == SchedulingAction::Defer => println!(
                    "thumbnail-generation\tstatus=deferred\taction={}\tdeferred=true",
                    outcome.scheduling_action.as_str()
                ),
                None => {
                    return Err(GfmError::Format(
                        "thumbnail generation adaptive job completed without a contract"
                            .to_string(),
                    ))
                }
            }
        }
        "thumbnail-generation-cancel" => {
            let path = required_path(args.next(), "thumbnail-generation-cancel requires a path")?;
            let cancellation = Cancellation::default();
            cancellation.cancel();
            if let Err(GfmError::Cancelled) = cancellation.check() {
                println!("thumbnail-generation\tstatus=cancelled\treason=cancelled-before-plan");
                return Ok(true);
            }
            let record = record_for_path_checked(&path, None, false, || cancellation.check())?;
            let input = ThumbnailGenerationInput::new(
                PreviewRequestKey::new(record.id, path.clone(), PreviewKind::Thumbnail),
                Rect::new(0, 0, 160, 160),
                Viewport::new(Rect::new(0, 0, 1024, 768), 256),
            )
            .with_size(512, 2_000);
            match ThumbnailGenerationContract::from_input_checked(
                &PreviewSecurityPolicy::default(),
                input,
                || cancellation.check(),
            ) {
                Err(GfmError::Cancelled) => {
                    println!("thumbnail-generation\tstatus=cancelled\treason=cancelled-before-plan")
                }
                Err(err) => return Err(err),
                Ok(_) => {
                    return Err(GfmError::Format(
                        "pre-cancelled thumbnail generation unexpectedly completed".to_string(),
                    ))
                }
            }
        }
        "preview-schedule" => {
            let mut scheduler = PreviewScheduler::new(PreviewSchedulingPolicy {
                max_visible: 8,
                max_prefetch: 8,
                cancel_offscreen: true,
            })?;
            let viewport = Viewport::new(Rect::new(0, 0, 100, 100), 64);
            for decision in scheduler.schedule(
                viewport,
                vec![
                    preview_task(1, 0, 0),
                    preview_task(2, 0, 130),
                    preview_task(3, 0, 260),
                ],
            ) {
                println!(
                    "{}\t{}",
                    decision.as_str(),
                    preview_decision_priority(&decision)
                );
            }
            let mut superseding = preview_task(1, 0, 0);
            superseding.generation = 1;
            for decision in scheduler.schedule(viewport, vec![superseding, preview_task(2, 0, 130)])
            {
                println!(
                    "{}\t{}",
                    decision.as_str(),
                    preview_decision_priority(&decision)
                );
            }
            for decision in scheduler.adapt_to_pressure(SchedulingPressure {
                io: JobIoPressure::Saturated,
                ..SchedulingPressure::default()
            }) {
                println!(
                    "{}\t{}",
                    decision.as_str(),
                    preview_decision_priority(&decision)
                );
            }
        }
        "preview-schedule-retained-capacity" => {
            let mut scheduler = PreviewScheduler::new(PreviewSchedulingPolicy {
                max_visible: 1,
                max_prefetch: 0,
                cancel_offscreen: false,
            })?;
            let first_viewport = Viewport::new(Rect::new(0, 0, 100, 100), 0);
            let second_viewport = Viewport::new(Rect::new(0, 100, 100, 100), 0);
            let old_visible = preview_task(1, 0, 0);
            let new_visible = preview_task(2, 0, 100);
            for decision in scheduler.schedule(first_viewport, vec![old_visible]) {
                println!(
                    "{}\t{}",
                    decision.as_str(),
                    preview_decision_priority(&decision)
                );
            }
            for decision in scheduler.schedule(second_viewport, vec![new_visible]) {
                println!(
                    "{}\t{}",
                    decision.as_str(),
                    preview_decision_priority(&decision)
                );
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn split_topology_paths(
    args: &mut impl Iterator<Item = String>,
) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let mut previous = Vec::new();
    let mut current = Vec::new();
    let mut seen_separator = false;
    for arg in args {
        if arg == "--" {
            seen_separator = true;
        } else if seen_separator {
            current.push(PathBuf::from(arg));
        } else {
            previous.push(PathBuf::from(arg));
        }
    }
    if !seen_separator {
        return Err(GfmError::Format(
            "volume-topology-diff requires `--` between previous and current paths".to_string(),
        ));
    }
    Ok((previous, current))
}

fn topology_case_sensitivity_diff(
    previous_case_sensitive: bool,
    current_case_sensitive: bool,
) -> Result<VolumeTopologyDiff> {
    let mut previous = VolumeDescriptor::for_path("/")?;
    previous.stable_identity = "diskarbitration:uuid:CASE-TOPOLOGY".to_string();
    previous.label = "Case Topology".to_string();
    previous.path = PathBuf::from("/Volumes/Case Topology");
    previous.kind = gfm_mac::VolumeKind::External;
    previous.case_sensitive = Some(previous_case_sensitive);
    let mut current = previous.clone();
    current.case_sensitive = Some(current_case_sensitive);
    Ok(VolumeTopologyDiff::evaluate(
        &VolumeDiscoveryReport {
            volumes: vec![previous],
        },
        &VolumeDiscoveryReport {
            volumes: vec![current],
        },
    ))
}

fn topology_removable_media_diff() -> Result<VolumeTopologyDiff> {
    let mut previous = VolumeDescriptor::for_path("/")?;
    previous.stable_identity = "diskarbitration:uuid:REMOVABLE-TOPOLOGY".to_string();
    previous.label = "Removable Topology".to_string();
    previous.path = PathBuf::from("/Volumes/Removable Topology");
    previous.kind = gfm_mac::VolumeKind::External;
    previous.removable = false;
    previous.ejectable = true;
    let mut current = previous.clone();
    current.removable = true;
    Ok(VolumeTopologyDiff::evaluate(
        &VolumeDiscoveryReport {
            volumes: vec![previous],
        },
        &VolumeDiscoveryReport {
            volumes: vec![current],
        },
    ))
}

fn topology_api_status_diff() -> Result<VolumeTopologyDiff> {
    let mut previous = VolumeDescriptor::for_path("/")?;
    previous.stable_identity = "diskarbitration:uuid:API-STATUS".to_string();
    previous.label = "API Status".to_string();
    previous.path = PathBuf::from("/Volumes/API Status");
    previous.kind = gfm_mac::VolumeKind::External;
    previous.native_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
    previous.resource_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
    previous.mount_table_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
    let mut current = previous.clone();
    current.native_status = Some(gfm_mac::NativeVolumeStatus::Available);
    current.resource_status = Some(gfm_mac::NativeVolumeStatus::Available);
    current.mount_table_status = Some(gfm_mac::NativeVolumeStatus::Available);
    Ok(VolumeTopologyDiff::evaluate(
        &VolumeDiscoveryReport {
            volumes: vec![previous],
        },
        &VolumeDiscoveryReport {
            volumes: vec![current],
        },
    ))
}

fn volume_topology_index_invalidation_tsv(
    previous: &VolumeDiscoveryReport,
    current: &VolumeDiscoveryReport,
) -> String {
    let diff = VolumeTopologyDiff::evaluate(previous, current);
    let mut lines = vec![diff.as_tsv()];
    for change in &diff.changes {
        let previous = previous
            .volumes
            .iter()
            .find(|volume| volume.stable_identity == change.stable_identity)
            .map(index_volume_descriptor);
        let current = current
            .volumes
            .iter()
            .find(|volume| volume.stable_identity == change.stable_identity)
            .map(index_volume_descriptor);
        let kind = match change.kind {
            VolumeTopologyChangeKind::Connected => IndexVolumeEventKind::Appeared,
            VolumeTopologyChangeKind::Disconnected => IndexVolumeEventKind::Disappeared,
            VolumeTopologyChangeKind::Changed => IndexVolumeEventKind::DescriptionChanged,
        };
        lines.push(
            VolumeEventIndexInvalidationReport::from_event(
                kind,
                Some(change.path.clone()),
                previous.as_ref(),
                current.as_ref(),
                change.invalidate_index_admission,
                change.rescan_index,
            )
            .as_tsv(),
        );
    }
    lines.join("\n")
}

fn volume_known_facts_lost_invalidation() -> Vec<String> {
    let previous = IndexVolumeDescriptor::new(
        "Known Facts Lost",
        "/Volumes/Known Facts Lost",
        IndexVolumeClass::External,
        IndexMountState::Mounted,
    )
    .with_volume_id(VolumeId(78))
    .with_stable_identity("diskarbitration:uuid:KNOWN-FACTS")
    .with_read_only(Some(false))
    .with_writable(Some(true))
    .with_ejectable(Some(true))
    .with_mountable(Some(false))
    .with_case_sensitive(Some(false))
    .with_filesystem_signature("fs=apfs|case-sensitive=0|writable=1|ejectable=1|mountable=0");
    let current = IndexVolumeDescriptor::new(
        "Known Facts Lost",
        "/Volumes/Known Facts Lost",
        IndexVolumeClass::External,
        IndexMountState::Mounted,
    )
    .with_volume_id(VolumeId(78));
    let path = Some(PathBuf::from("/Volumes/Known Facts Lost"));
    vec![
        VolumeInvalidationReport::evaluate(Some(&previous), Some(&current)).as_tsv(),
        VolumeEventIndexInvalidationReport::from_event(
            IndexVolumeEventKind::DescriptionChanged,
            path,
            Some(&previous),
            Some(&current),
            false,
            false,
        )
        .as_tsv(),
    ]
}

fn volume_event_transition_case_sensitivity(
    previous_case_sensitive: bool,
    current_case_sensitive: bool,
) -> Result<VolumeEventInvalidationReport> {
    let mut previous = VolumeDescriptor::for_path("/")?;
    previous.stable_identity = "diskarbitration:uuid:CASE-EVENT".to_string();
    previous.label = "Case Event".to_string();
    previous.path = PathBuf::from("/Volumes/Case Event");
    previous.kind = gfm_mac::VolumeKind::External;
    previous.case_sensitive = Some(previous_case_sensitive);
    previous.native_status = Some(gfm_mac::NativeVolumeStatus::Available);
    previous.resource_status = Some(gfm_mac::NativeVolumeStatus::Available);
    previous.mount_table_status = Some(gfm_mac::NativeVolumeStatus::Available);
    let mut current = previous.clone();
    current.case_sensitive = Some(current_case_sensitive);
    Ok(VolumeEventInvalidationReport::from_transition(
        VolumeEventKind::DescriptionChanged,
        gfm_mac::NativeVolumeStatus::Available,
        Some(&previous),
        Some(&current),
        None,
    ))
}

fn volume_event_transition_removable_media() -> Result<VolumeEventInvalidationReport> {
    let mut previous = VolumeDescriptor::for_path("/")?;
    previous.stable_identity = "diskarbitration:uuid:REMOVABLE-EVENT".to_string();
    previous.label = "Removable Event".to_string();
    previous.path = PathBuf::from("/Volumes/Removable Event");
    previous.kind = gfm_mac::VolumeKind::External;
    previous.removable = false;
    previous.ejectable = true;
    previous.native_status = Some(gfm_mac::NativeVolumeStatus::Available);
    previous.resource_status = Some(gfm_mac::NativeVolumeStatus::Available);
    previous.mount_table_status = Some(gfm_mac::NativeVolumeStatus::Available);
    let mut current = previous.clone();
    current.removable = true;
    Ok(VolumeEventInvalidationReport::from_transition(
        VolumeEventKind::DescriptionChanged,
        gfm_mac::NativeVolumeStatus::Available,
        Some(&previous),
        Some(&current),
        None,
    ))
}

fn volume_event_transition_api_status() -> Result<VolumeEventInvalidationReport> {
    let mut previous = VolumeDescriptor::for_path("/")?;
    previous.stable_identity = "diskarbitration:uuid:API-EVENT".to_string();
    previous.label = "API Event".to_string();
    previous.path = PathBuf::from("/Volumes/API Event");
    previous.kind = gfm_mac::VolumeKind::External;
    previous.native_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
    previous.resource_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
    previous.mount_table_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
    let mut current = previous.clone();
    current.native_status = Some(gfm_mac::NativeVolumeStatus::Available);
    current.resource_status = Some(gfm_mac::NativeVolumeStatus::Available);
    current.mount_table_status = Some(gfm_mac::NativeVolumeStatus::Available);
    Ok(VolumeEventInvalidationReport::from_transition(
        VolumeEventKind::DescriptionChanged,
        gfm_mac::NativeVolumeStatus::Available,
        Some(&previous),
        Some(&current),
        None,
    ))
}

fn volume_event_description_api_status() -> Result<VolumeEventInvalidationReport> {
    let mut descriptor = VolumeDescriptor::for_path("/")?;
    descriptor.stable_identity = "diskarbitration:uuid:API-DESCRIPTION".to_string();
    descriptor.label = "API Description".to_string();
    descriptor.path = PathBuf::from("/Volumes/API Description");
    descriptor.kind = gfm_mac::VolumeKind::External;
    descriptor.native_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
    descriptor.resource_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
    descriptor.mount_table_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
    Ok(volume_event_invalidation_for_descriptor(
        VolumeEventKind::DescriptionChanged,
        descriptor.path.clone(),
        &descriptor,
    ))
}

fn publish_fileprovider_progress_job(
    path: PathBuf,
    cancellation: &Cancellation,
) -> Result<FileProviderProgressReport> {
    maybe_fail_fileprovider_progress_retry_probe(cancellation)?;
    let report = FileProviderProgressReport::read_path_checked(&path, || cancellation.check())?;
    let mut scheduler = Scheduler::new();
    let label = fileprovider_progress_label(report.state.progress.direction);
    let volume = detect_volume_id(&path).ok();
    let job = if let Some(volume) = volume {
        scheduler.schedule_on_volume_in_class(Priority::Visible, JobClass::Visible, label, volume)
    } else {
        scheduler.schedule_in_class(Priority::Visible, JobClass::Visible, label)
    };
    let detail = fileprovider_progress_detail(&report);
    let runtime = RuntimeJobHandle::begin_with_payload_path(
        &job,
        JobPayloadKind::Operation,
        label,
        path.clone(),
        fileprovider_progress_total_units(&report),
        detail.clone(),
    )?;
    runtime.progress_checked(
        fileprovider_progress_job_state(&report),
        u64::from(report.state.progress.percent_milli.unwrap_or(0)),
        detail,
        || cancellation.check(),
    )?;
    Ok(report)
}

fn maybe_fail_fileprovider_progress_retry_probe(cancellation: &Cancellation) -> Result<()> {
    let Some(path) = std::env::var_os("GFM_FILEPROVIDER_PROGRESS_RETRY_PROBE").map(PathBuf::from)
    else {
        return Ok(());
    };
    cancellation.check()?;
    let attempt = read_retry_probe_attempt_checked(&path, || cancellation.check())? + 1;
    cancellation.check()?;
    write_retry_probe_attempt_checked(&path, attempt, || cancellation.check())?;
    cancellation.check()?;
    if attempt == 1 {
        Err(GfmError::Format(
            "temporary fileprovider progress busy".to_string(),
        ))
    } else {
        Ok(())
    }
}

fn read_retry_probe_attempt_checked(
    path: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<usize> {
    check_control()?;
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return Ok(0),
    };
    check_control()?;
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        check_control()?;
        let read = match file.read(&mut buffer) {
            Ok(read) => read,
            Err(_) => return Ok(0),
        };
        check_control()?;
        if read == 0 {
            break;
        }
        if bytes.len().saturating_add(read) > 4096 {
            return Ok(0);
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    let Ok(value) = std::str::from_utf8(&bytes) else {
        return Ok(0);
    };
    Ok(value.trim().parse::<usize>().unwrap_or(0))
}

fn write_retry_probe_attempt_checked(
    path: &Path,
    attempt: usize,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    check_control()?;
    let encoded = attempt.to_string();
    gfm_store::atomic_write_checked(path, &mut check_control, |writer, check_control| {
        for chunk in encoded.as_bytes().chunks(4096) {
            check_control()?;
            writer
                .write_all(chunk)
                .map_err(|err| GfmError::io(path, err))?;
            check_control()?;
        }
        Ok(())
    })?;
    check_control()?;
    Ok(())
}

fn fileprovider_progress_label(direction: CloudTransferDirection) -> &'static str {
    match direction {
        CloudTransferDirection::Idle => "fileprovider transfer",
        CloudTransferDirection::Download => "fileprovider download",
        CloudTransferDirection::Upload => "fileprovider upload",
        CloudTransferDirection::Materialize => "fileprovider materialize",
    }
}

fn volume_event_index_invalidation_from_args(
    args: &mut impl Iterator<Item = String>,
) -> Result<VolumeEventIndexInvalidationReport> {
    let kind = parse_volume_event_kind(&required_string(
        args.next(),
        "volume event index invalidation requires an event kind",
    )?)?;
    let path = args.next().map(PathBuf::from);
    let resolution = resolve_volume_event_path(kind, path)?;
    let descriptor = resolution.descriptor.as_ref();
    let event_report = resolution.invalidation_report(kind);
    let previous = (kind == VolumeEventKind::Disappeared)
        .then(|| descriptor.map(index_volume_descriptor))
        .flatten();
    let current = (kind != VolumeEventKind::Disappeared)
        .then(|| descriptor.map(index_volume_descriptor))
        .flatten();
    Ok(VolumeEventIndexInvalidationReport::from_event(
        index_volume_event_kind(kind),
        resolution.path,
        previous.as_ref(),
        current.as_ref(),
        event_report.invalidate_index_admission,
        event_report.rescan_index,
    ))
}

fn volume_event_state_index_invalidation_from_args(
    args: &mut impl Iterator<Item = String>,
) -> Result<VolumeEventIndexInvalidationReport> {
    let mut previous_paths = Vec::new();
    loop {
        let arg = required_string(
            args.next(),
            "volume event state index invalidation requires previous paths, `--`, event kind, and optional event path",
        )?;
        if arg == "--" {
            break;
        }
        previous_paths.push(PathBuf::from(arg));
    }

    let kind = parse_volume_event_kind(&required_string(
        args.next(),
        "volume event state index invalidation requires an event kind after `--`",
    )?)?;
    let resolution = resolve_volume_event_path(kind, args.next().map(PathBuf::from))?;
    let mut state = VolumeEventState::new(volume_discovery_report(previous_paths)?);
    let current = (kind != VolumeEventKind::Disappeared)
        .then_some(resolution.descriptor)
        .flatten();
    let transition = state.apply_parts_transition(
        kind,
        resolution.native_status,
        resolution.path.clone(),
        current,
        resolution.native_reason,
    );
    let previous = transition.previous.as_ref().map(index_volume_descriptor);
    let current = transition.current.as_ref().map(index_volume_descriptor);
    Ok(VolumeEventIndexInvalidationReport::from_event(
        index_volume_event_kind(kind),
        transition.invalidation.path,
        previous.as_ref(),
        current.as_ref(),
        transition.invalidation.invalidate_index_admission,
        transition.invalidation.rescan_index,
    ))
}

fn volume_event_runtime_fanout_from_args(
    args: &mut impl Iterator<Item = String>,
) -> Result<Vec<String>> {
    let mut previous_paths = Vec::new();
    loop {
        let arg = required_string(
            args.next(),
            "volume event runtime fanout requires previous paths, `--`, event kind, and optional event path",
        )?;
        if arg == "--" {
            break;
        }
        previous_paths.push(PathBuf::from(arg));
    }

    let kind = parse_volume_event_kind(&required_string(
        args.next(),
        "volume event runtime fanout requires an event kind after `--`",
    )?)?;
    let resolution = resolve_volume_event_path(kind, args.next().map(PathBuf::from))?;
    let mut state = VolumeEventState::new(volume_discovery_report(previous_paths)?);
    let current = (kind != VolumeEventKind::Disappeared)
        .then_some(resolution.descriptor)
        .flatten();
    let transition = state.apply_parts_transition(
        kind,
        resolution.native_status,
        resolution.path.clone(),
        current,
        resolution.native_reason,
    );
    let previous_index = transition.previous.as_ref().map(index_volume_descriptor);
    let current_index = transition.current.as_ref().map(index_volume_descriptor);
    let index = VolumeEventIndexInvalidationReport::from_event(
        index_volume_event_kind(kind),
        transition.invalidation.path.clone(),
        previous_index.as_ref(),
        current_index.as_ref(),
        transition.invalidation.invalidate_index_admission,
        transition.invalidation.rescan_index,
    );
    let previous_sidebar = transition.previous.as_ref().map(sidebar_volume_spec);
    let current_sidebar = transition.current.as_ref().map(sidebar_volume_spec);
    let sidebar = SidebarVolumeInvalidation::from_event(
        sidebar_volume_event_kind(kind),
        transition.invalidation.path.clone(),
        previous_sidebar.as_ref(),
        current_sidebar.as_ref(),
        transition.invalidation.invalidate_sidebar,
        transition.invalidation.reason.clone(),
    )
    .with_platform_statuses(
        volume_status_string(transition.invalidation.previous_native_status),
        volume_status_string(transition.invalidation.previous_resource_status),
        volume_status_string(transition.invalidation.previous_mount_table_status),
        volume_status_string(transition.invalidation.current_native_status),
        volume_status_string(transition.invalidation.current_resource_status),
        volume_status_string(transition.invalidation.current_mount_table_status),
    );

    let mut lines = vec![
        volume_event_runtime_fanout_summary(&transition.invalidation, &index, &sidebar),
        index.as_tsv(),
        sidebar.as_tsv(),
        volume_event_operation_policy_invalidation_tsv(
            &transition.invalidation,
            transition.previous.as_ref(),
            transition.current.as_ref(),
        ),
    ];
    if let Some(cancellation) = runtime_volume_cancellation(&index) {
        lines.push(cancellation.as_tsv());
    } else {
        lines.push(format!(
            "volume-job-cancellation\tvolume=-\tclass=background\tcancelled=0\treason={}",
            if index.cancel_index_jobs {
                "missing-volume-id"
            } else {
                "index-jobs-still-valid"
            }
        ));
    }
    Ok(lines)
}

fn volume_event_runtime_fanout_summary(
    platform: &VolumeEventInvalidationReport,
    index: &VolumeEventIndexInvalidationReport,
    sidebar: &SidebarVolumeInvalidation,
) -> String {
    format!(
        "volume-event-runtime-fanout\tkind={}\tpath={}\tsidebar={}\toperation-policy={}\tindex-admission={}\trescan-index={}\tcancel-index-jobs={}\tclear-fsevents-cursor={}\tsidebar-row={}\tsidebar-section={}\treason={}",
        platform.kind.as_str(),
        platform
            .path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "-".to_string()),
        platform.invalidate_sidebar,
        platform.invalidate_operation_policy,
        index.invalidate_index_admission,
        index.rescan_index,
        index.cancel_index_jobs,
        index.clear_fsevents_cursor,
        sidebar.invalidate_row,
        sidebar.invalidate_section,
        platform.reason
    )
}

fn volume_event_operation_policy_invalidation_tsv(
    platform: &VolumeEventInvalidationReport,
    previous: Option<&VolumeDescriptor>,
    current: Option<&VolumeDescriptor>,
) -> String {
    format!(
        "volume-event-operation-policy-invalidation\tkind={}\tpath={}\tprevious-class={}\tprevious-mount={}\tprevious-read-only={}\tcurrent-class={}\tcurrent-mount={}\tcurrent-read-only={}\tinvalidate-policy={}\treason={}",
        platform.kind.as_str(),
        platform
            .path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "-".to_string()),
        previous.map(|volume| volume.kind.as_str()).unwrap_or("-"),
        previous
            .map(|volume| volume.mount_state.as_str())
            .unwrap_or("-"),
        previous
            .map(|volume| volume.read_only)
            .map(|read_only| read_only.to_string())
            .unwrap_or_else(|| "-".to_string()),
        current.map(|volume| volume.kind.as_str()).unwrap_or("-"),
        current
            .map(|volume| volume.mount_state.as_str())
            .unwrap_or("-"),
        current
            .map(|volume| volume.read_only)
            .map(|read_only| read_only.to_string())
            .unwrap_or_else(|| "-".to_string()),
        platform.invalidate_operation_policy,
        platform.reason
    )
}

fn volume_case_sensitivity_invalidation(
    previous_case_sensitive: bool,
    current_case_sensitive: bool,
) -> VolumeEventIndexInvalidationReport {
    let previous = IndexVolumeDescriptor::new(
        "Case Test",
        "/Volumes/Case Test",
        IndexVolumeClass::External,
        IndexMountState::Mounted,
    )
    .with_volume_id(VolumeId(42))
    .with_stable_identity("diskarbitration:uuid:CASE-TEST")
    .with_case_sensitive(Some(previous_case_sensitive))
    .with_filesystem_signature(format!(
        "fs=apfs|case-sensitive={}",
        bool_signature_value(previous_case_sensitive)
    ));
    let current = IndexVolumeDescriptor::new(
        "Case Test",
        "/Volumes/Case Test",
        IndexVolumeClass::External,
        IndexMountState::Mounted,
    )
    .with_volume_id(VolumeId(42))
    .with_stable_identity("diskarbitration:uuid:CASE-TEST")
    .with_case_sensitive(Some(current_case_sensitive))
    .with_filesystem_signature(format!(
        "fs=apfs|case-sensitive={}",
        bool_signature_value(current_case_sensitive)
    ));

    VolumeEventIndexInvalidationReport::from_event(
        IndexVolumeEventKind::DescriptionChanged,
        Some(PathBuf::from("/Volumes/Case Test")),
        Some(&previous),
        Some(&current),
        false,
        false,
    )
}

fn volume_removable_media_invalidation() -> VolumeEventIndexInvalidationReport {
    let previous = IndexVolumeDescriptor::new(
        "Removable Test",
        "/Volumes/Removable Test",
        IndexVolumeClass::External,
        IndexMountState::Mounted,
    )
    .with_volume_id(VolumeId(43))
    .with_stable_identity("diskarbitration:uuid:REMOVABLE-TEST")
    .with_ejectable(Some(true))
    .with_filesystem_signature("fs=apfs|ejectable=1|removable=0");
    let current = IndexVolumeDescriptor::new(
        "Removable Test",
        "/Volumes/Removable Test",
        IndexVolumeClass::External,
        IndexMountState::Mounted,
    )
    .with_volume_id(VolumeId(43))
    .with_stable_identity("diskarbitration:uuid:REMOVABLE-TEST")
    .with_ejectable(Some(true))
    .with_filesystem_signature("fs=apfs|ejectable=1|removable=1");

    VolumeEventIndexInvalidationReport::from_event(
        IndexVolumeEventKind::DescriptionChanged,
        Some(PathBuf::from("/Volumes/Removable Test")),
        Some(&previous),
        Some(&current),
        false,
        false,
    )
}

fn volume_root_filesystem_invalidation() -> VolumeEventIndexInvalidationReport {
    let previous = IndexVolumeDescriptor::new(
        "Root Filesystem Test",
        "/Volumes/Root Filesystem Test",
        IndexVolumeClass::System,
        IndexMountState::Mounted,
    )
    .with_volume_id(VolumeId(44))
    .with_stable_identity("diskarbitration:uuid:ROOT-FILESYSTEM-TEST")
    .with_filesystem_signature("fs=apfs|root-filesystem=0");
    let current = IndexVolumeDescriptor::new(
        "Root Filesystem Test",
        "/Volumes/Root Filesystem Test",
        IndexVolumeClass::System,
        IndexMountState::Mounted,
    )
    .with_volume_id(VolumeId(44))
    .with_stable_identity("diskarbitration:uuid:ROOT-FILESYSTEM-TEST")
    .with_filesystem_signature("fs=apfs|root-filesystem=1");

    VolumeEventIndexInvalidationReport::from_event(
        IndexVolumeEventKind::DescriptionChanged,
        Some(PathBuf::from("/Volumes/Root Filesystem Test")),
        Some(&previous),
        Some(&current),
        false,
        false,
    )
}

fn volume_bsd_identity_invalidation() -> VolumeEventIndexInvalidationReport {
    let previous = IndexVolumeDescriptor::new(
        "BSD Identity Test",
        "/Volumes/BSD Identity Test",
        IndexVolumeClass::External,
        IndexMountState::Mounted,
    )
    .with_volume_id(VolumeId(45))
    .with_stable_identity("diskarbitration:uuid:BSD-IDENTITY-TEST")
    .with_filesystem_signature("fs=apfs|bsd=disk4s1|bsd-major=1|bsd-minor=2|bsd-unit=4");
    let current = IndexVolumeDescriptor::new(
        "BSD Identity Test",
        "/Volumes/BSD Identity Test",
        IndexVolumeClass::External,
        IndexMountState::Mounted,
    )
    .with_volume_id(VolumeId(45))
    .with_stable_identity("diskarbitration:uuid:BSD-IDENTITY-TEST")
    .with_filesystem_signature("fs=apfs|bsd=disk4s1|bsd-major=8|bsd-minor=9|bsd-unit=10");

    VolumeEventIndexInvalidationReport::from_event(
        IndexVolumeEventKind::DescriptionChanged,
        Some(PathBuf::from("/Volumes/BSD Identity Test")),
        Some(&previous),
        Some(&current),
        false,
        false,
    )
}

fn bool_signature_value(value: bool) -> &'static str {
    if value {
        "1"
    } else {
        "0"
    }
}

fn runtime_volume_cancellation(
    report: &VolumeEventIndexInvalidationReport,
) -> Option<gfm_jobs::VolumeCancellationReport> {
    if !report.cancel_index_jobs {
        return None;
    }
    let volume = report.current_volume_id.or(report.previous_volume_id)?;
    let mut scheduler = Scheduler::new();
    scheduler.schedule_on_volume_in_class(
        Priority::Background,
        JobClass::Background,
        "index invalidated volume",
        volume,
    );
    scheduler.schedule_on_volume_in_class(
        Priority::Visible,
        JobClass::Visible,
        "render visible volume previews",
        volume,
    );
    scheduler.schedule_on_volume_in_class(
        Priority::Background,
        JobClass::Background,
        "index unrelated volume",
        VolumeId(volume.0 + 1),
    );
    Some(scheduler.cancel_volume_jobs(volume, Some(JobClass::Background)))
}

fn sidebar_volume_spec(volume: &VolumeDescriptor) -> SidebarVolumeSpec {
    SidebarVolumeSpec::from_native_seed(
        &volume.stable_identity,
        volume.label.clone(),
        volume.path.clone(),
        volume.ejectable,
    )
    .with_volume_state(
        sidebar_volume_kind(volume.kind),
        sidebar_volume_mount_state(volume.mount_state),
        volume.read_only,
        volume.network,
        volume.reachable,
    )
}

fn sidebar_volume_kind(kind: gfm_mac::VolumeKind) -> SidebarVolumeKind {
    match kind {
        gfm_mac::VolumeKind::System | gfm_mac::VolumeKind::Internal => SidebarVolumeKind::Internal,
        gfm_mac::VolumeKind::External => SidebarVolumeKind::External,
        gfm_mac::VolumeKind::Removable => SidebarVolumeKind::Removable,
        gfm_mac::VolumeKind::Network => SidebarVolumeKind::Network,
        gfm_mac::VolumeKind::DiskImage => SidebarVolumeKind::DiskImage,
        gfm_mac::VolumeKind::Unknown => SidebarVolumeKind::Unknown,
    }
}

fn sidebar_volume_mount_state(state: gfm_mac::MountState) -> SidebarVolumeMountState {
    match state {
        gfm_mac::MountState::Mounted => SidebarVolumeMountState::Mounted,
        gfm_mac::MountState::Unmounted => SidebarVolumeMountState::Unmounted,
        gfm_mac::MountState::Stale => SidebarVolumeMountState::Stale,
    }
}

fn sidebar_volume_event_kind(kind: VolumeEventKind) -> SidebarVolumeEventKind {
    match kind {
        VolumeEventKind::Appeared => SidebarVolumeEventKind::Appeared,
        VolumeEventKind::DescriptionChanged => SidebarVolumeEventKind::DescriptionChanged,
        VolumeEventKind::Disappeared => SidebarVolumeEventKind::Disappeared,
        VolumeEventKind::Unavailable => SidebarVolumeEventKind::Unavailable,
    }
}

fn volume_status_string(status: Option<gfm_mac::NativeVolumeStatus>) -> Option<String> {
    status.map(|status| status.as_str().to_string())
}

fn fileprovider_progress_total_units(report: &FileProviderProgressReport) -> u64 {
    if report.state.progress.indeterminate {
        1
    } else {
        100_000
    }
}

fn fileprovider_progress_job_state(report: &FileProviderProgressReport) -> JobProgressState {
    if report.state.progress.complete {
        JobProgressState::Completed
    } else if report.state.progress.indeterminate {
        JobProgressState::Running
    } else {
        match report.state.storage_state {
            CloudStorageState::Downloading
            | CloudStorageState::Uploading
            | CloudStorageState::Waiting => JobProgressState::Running,
            CloudStorageState::Downloaded => JobProgressState::Completed,
            CloudStorageState::LocalOnly
            | CloudStorageState::Evicted
            | CloudStorageState::Conflict
            | CloudStorageState::Offline
            | CloudStorageState::Unknown
            | CloudStorageState::Removed => JobProgressState::Paused,
        }
    }
}

fn fileprovider_progress_detail(report: &FileProviderProgressReport) -> String {
    format!(
        "fileprovider:{}:{}:{}:{}",
        report.state.domain.as_str(),
        report.state.storage_state.as_str(),
        report.state.progress.direction.as_str(),
        report
            .state
            .progress
            .reason
            .as_deref()
            .unwrap_or("native-progress")
    )
}

fn volume_discovery_report(paths: Vec<PathBuf>) -> Result<VolumeDiscoveryReport> {
    if paths.is_empty() {
        Ok(VolumeDiscoveryReport::discover())
    } else {
        VolumeDiscoveryReport::from_paths_checked(paths)
    }
}

fn current_index_volume_descriptor(path: &Path) -> Result<Option<IndexVolumeDescriptor>> {
    match path.try_exists() {
        Ok(true) => Ok(Some(index_volume_descriptor(&VolumeDescriptor::for_path(
            path,
        )?))),
        Ok(false) => Ok(None),
        Err(err) => Err(GfmError::io(
            path,
            format!("volume invalidation current path existence unavailable: {err}"),
        )),
    }
}

fn previous_index_volume_descriptor_from_args(
    mut previous: IndexVolumeDescriptor,
    args: &mut impl Iterator<Item = String>,
) -> Result<IndexVolumeDescriptor> {
    if let Some(read_only) = optional_platform_bool(args.next(), "previous read-only")? {
        previous = previous.with_read_only(Some(read_only));
    }
    if let Some(writable) = optional_platform_bool(args.next(), "previous writable")? {
        previous = previous.with_writable(Some(writable));
    }
    if let Some(ejectable) = optional_platform_bool(args.next(), "previous ejectable")? {
        previous = previous.with_ejectable(Some(ejectable));
    }
    if let Some(mountable) = optional_platform_bool(args.next(), "previous mountable")? {
        previous = previous.with_mountable(Some(mountable));
    }
    if let Some(case_sensitive) = optional_platform_bool(args.next(), "previous case-sensitive")? {
        previous = previous.with_case_sensitive(Some(case_sensitive));
    }
    if let Some(stable_identity) = optional_platform_string(args.next()) {
        previous = previous.with_stable_identity(stable_identity);
    }
    if let Some(filesystem_signature) = optional_platform_string(args.next()) {
        previous = previous.with_filesystem_signature(filesystem_signature);
    }
    Ok(previous)
}

fn parse_fileprovider_event(kind: &str, path: PathBuf, to: Option<PathBuf>) -> Result<FileEvent> {
    let event_kind = match kind {
        "create" => FileEventKind::Create,
        "metadata" => FileEventKind::Metadata,
        "modify" => FileEventKind::Modify,
        "remove" => FileEventKind::Remove,
        "rescan" => FileEventKind::Rescan,
        "other" => FileEventKind::Other,
        "rename" => FileEventKind::Rename {
            from: path.clone(),
            to: to.ok_or_else(|| {
                GfmError::Format(
                    "fileprovider-invalidation-event rename requires a destination path"
                        .to_string(),
                )
            })?,
        },
        other => {
            return Err(GfmError::Format(format!(
                "unsupported FileProvider event kind `{other}`"
            )))
        }
    };
    Ok(FileEvent::new(path, event_kind))
}

fn observed_metadata_invalidation_tsv(observed: &FileProviderObservedInvalidation) -> String {
    let mut lines = vec![observed.as_tsv()];
    lines.extend(observed.report.changes.iter().map(|report| {
        ProviderMetadataInvalidationReport::from_provider_transition(
            report.path.clone(),
            report.previous.as_str(),
            report.current.storage_state.as_str(),
            report.reindex_metadata,
            report.state_changed,
            report.reason,
        )
        .as_tsv()
    }));
    lines.join("\n")
}

fn observed_native_icon_invalidation_tsv(observed: &FileProviderObservedInvalidation) -> String {
    let mut lines = vec![observed.as_tsv()];
    lines.extend(
        observed
            .report
            .changes
            .iter()
            .map(NativeIconInvalidationReport::from_fileprovider)
            .map(|report| report.as_tsv()),
    );
    lines.join("\n")
}

fn run_fileprovider_read<T>(
    path: PathBuf,
    worker: &'static str,
    read: impl FnOnce(PathBuf) -> Result<T> + Send + 'static,
) -> Result<T>
where
    T: Send + 'static,
{
    preflight_volume_access_scope(&path, AccessIntent::Read, worker)?;
    let volume = detect_volume_id(&path).ok();
    run_volume_task_cancellable(volume, Priority::Visible, worker, move |cancellation| {
        cancellation.check()?;
        let _access = preflight_access_scope_checked(&path, AccessIntent::Read, worker, || {
            cancellation.check()
        })?;
        cancellation.check()?;
        read(path)
    })
}

fn run_fileprovider_progress_job(
    path: PathBuf,
    cancel_after_access: bool,
) -> Result<FileProviderProgressReport> {
    const WORKER: &str = "fileprovider progress job";
    preflight_volume_access_scope(&path, AccessIntent::Read, WORKER)?;
    let volume = detect_volume_id(&path).ok();
    run_fileprovider_worker_without_runtime_progress(volume, WORKER, move |cancellation| {
        let path = path.clone();
        cancellation.check()?;
        let _access = preflight_access_scope_checked(&path, AccessIntent::Read, WORKER, || {
            cancellation.check()
        })?;
        cancellation.check()?;
        if cancel_after_access {
            cancellation.cancel();
        }
        publish_fileprovider_progress_job(path, &cancellation)
    })
}

fn run_fileprovider_operation(
    path: PathBuf,
    operation: FileProviderOperation,
) -> Result<FileProviderOperationReport> {
    const WORKER: &str = "fileprovider operation";
    preflight_volume_access_scope(&path, AccessIntent::Operate, WORKER)?;
    let volume = detect_volume_id(&path).ok();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = preflight_access_scope_checked(&path, AccessIntent::Operate, WORKER, || {
            cancellation.check()
        })?;
        cancellation.check()?;
        FileProviderOperationReport::execute_checked(path, operation, || cancellation.check())
    })
}

fn run_native_icon(path: PathBuf) -> Result<NativeIconDescriptor> {
    const WORKER: &str = "native icon";
    preflight_volume_access_scope(&path, AccessIntent::Preview, WORKER)?;
    let volume = detect_volume_id(&path)
        .ok()
        .or_else(|| parent_volume(&path));
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let record =
            record_for_path_with_access(&path, AccessIntent::Preview, WORKER, &cancellation)?;
        cancellation.check()?;
        Ok(NativeIconDescriptor::for_record(&record))
    })
}

fn run_native_icon_bridge(path: PathBuf) -> Result<NativeIconBridgeContract> {
    const WORKER: &str = "native icon bridge";
    preflight_volume_access_scope(&path, AccessIntent::Preview, WORKER)?;
    let volume = detect_volume_id(&path)
        .ok()
        .or_else(|| parent_volume(&path));
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let record =
            record_for_path_with_access(&path, AccessIntent::Preview, WORKER, &cancellation)?;
        cancellation.check()?;
        let host = current_host_profile()?;
        cancellation.check()?;
        Ok(NativeIconBridgeContract::for_record_on_host(&record, &host))
    })
}

fn run_icon_preview(path: PathBuf) -> Result<IconPreviewContract> {
    const WORKER: &str = "icon preview";
    preflight_volume_access_scope(&path, AccessIntent::Preview, WORKER)?;
    let volume = detect_volume_id(&path)
        .ok()
        .or_else(|| parent_volume(&path));
    run_preview_contract_cancellable_with_payload_path(
        volume,
        WORKER,
        path.clone(),
        move |cancellation| build_icon_preview_contract(&path, WORKER, &cancellation),
    )
}

fn run_icon_preview_retry_probe(
    path: PathBuf,
    attempt_state: PathBuf,
) -> Result<IconPreviewContract> {
    const WORKER: &str = "icon preview";
    preflight_volume_access_scope(&path, AccessIntent::Preview, WORKER)?;
    preflight_volume_access_scope(
        write_probe_path(&attempt_state)?,
        AccessIntent::Write,
        WORKER,
    )?;
    let volume = detect_volume_id(&path)
        .ok()
        .or_else(|| parent_volume(&path));
    run_preview_contract_cancellable_with_payload_path(
        volume,
        WORKER,
        path.clone(),
        move |cancellation| {
            fail_first_retry_probe_attempt(&attempt_state, WORKER, &cancellation)?;
            build_icon_preview_contract(&path, WORKER, &cancellation)
        },
    )
}

fn run_quicklook_session(path: PathBuf) -> Result<QuickLookSessionContract> {
    const WORKER: &str = "quicklook preview";
    preflight_volume_access_scope(&path, AccessIntent::Preview, WORKER)?;
    let volume = detect_volume_id(&path)
        .ok()
        .or_else(|| parent_volume(&path));
    run_preview_contract_cancellable_with_payload_path(
        volume,
        WORKER,
        path.clone(),
        move |cancellation| build_quicklook_session_contract(&path, WORKER, &cancellation),
    )
}

fn run_quicklook_session_retry_probe(
    path: PathBuf,
    attempt_state: PathBuf,
) -> Result<QuickLookSessionContract> {
    const WORKER: &str = "quicklook preview";
    preflight_volume_access_scope(&path, AccessIntent::Preview, WORKER)?;
    preflight_volume_access_scope(
        write_probe_path(&attempt_state)?,
        AccessIntent::Write,
        WORKER,
    )?;
    let volume = detect_volume_id(&path)
        .ok()
        .or_else(|| parent_volume(&path));
    run_preview_contract_cancellable_with_payload_path(
        volume,
        WORKER,
        path.clone(),
        move |cancellation| {
            fail_first_retry_probe_attempt(&attempt_state, WORKER, &cancellation)?;
            build_quicklook_session_contract(&path, WORKER, &cancellation)
        },
    )
}

fn run_thumbnail_generation(path: PathBuf) -> Result<ThumbnailGenerationContract> {
    const WORKER: &str = "thumbnail generation";
    preflight_volume_access_scope(&path, AccessIntent::Preview, WORKER)?;
    let volume = detect_volume_id(&path)
        .ok()
        .or_else(|| parent_volume(&path));
    run_preview_contract_cancellable_with_payload_path(
        volume,
        WORKER,
        path.clone(),
        move |cancellation| build_thumbnail_generation_contract(&path, WORKER, &cancellation),
    )
}

fn run_thumbnail_generation_retry_probe(
    path: PathBuf,
    attempt_state: PathBuf,
) -> Result<ThumbnailGenerationContract> {
    const WORKER: &str = "thumbnail generation";
    preflight_volume_access_scope(&path, AccessIntent::Preview, WORKER)?;
    preflight_volume_access_scope(
        write_probe_path(&attempt_state)?,
        AccessIntent::Write,
        WORKER,
    )?;
    let volume = detect_volume_id(&path)
        .ok()
        .or_else(|| parent_volume(&path));
    run_preview_contract_cancellable_with_payload_path(
        volume,
        WORKER,
        path.clone(),
        move |cancellation| {
            fail_first_retry_probe_attempt(&attempt_state, WORKER, &cancellation)?;
            build_thumbnail_generation_contract(&path, WORKER, &cancellation)
        },
    )
}

fn build_icon_preview_contract(
    path: &Path,
    worker: &str,
    cancellation: &Cancellation,
) -> Result<IconPreviewContract> {
    cancellation.check()?;
    let record = record_for_path_with_access(path, AccessIntent::Preview, worker, cancellation)?;
    cancellation.check()?;
    let input = IconPreviewInput::new(
        PreviewRequestKey::new(record.id, path.to_path_buf(), PreviewKind::Icon),
        record,
    )
    .with_invalidation(PreviewInvalidationEvent {
        tags_changed: true,
        ..PreviewInvalidationEvent::default()
    });
    cancellation.check()?;
    Ok(IconPreviewContract::from_input(input))
}

fn build_quicklook_session_contract(
    path: &Path,
    worker: &str,
    cancellation: &Cancellation,
) -> Result<QuickLookSessionContract> {
    cancellation.check()?;
    let record = record_for_path_with_access(path, AccessIntent::Preview, worker, cancellation)?;
    cancellation.check()?;
    let cloud = fileprovider_materialization_for_preview(path, cancellation)?;
    cancellation.check()?;
    let input = QuickLookSessionInput::new(
        PreviewRequestKey::new(record.id, path.to_path_buf(), PreviewKind::QuickLook),
        Rect::new(0, 0, 640, 480),
        Viewport::new(Rect::new(0, 0, 1024, 768), 256),
    )
    .with_cloud_materialization(cloud)
    .with_invalidation(PreviewInvalidationEvent {
        content_changed: true,
        ..PreviewInvalidationEvent::default()
    });
    QuickLookSessionContract::from_input_checked(&PreviewSecurityPolicy::default(), input, || {
        cancellation.check()
    })
}

fn build_thumbnail_generation_contract(
    path: &Path,
    worker: &str,
    cancellation: &Cancellation,
) -> Result<ThumbnailGenerationContract> {
    cancellation.check()?;
    let record = record_for_path_with_access(path, AccessIntent::Preview, worker, cancellation)?;
    cancellation.check()?;
    let cloud = fileprovider_materialization_for_preview(path, cancellation)?;
    cancellation.check()?;
    let input = ThumbnailGenerationInput::new(
        PreviewRequestKey::new(record.id, path.to_path_buf(), PreviewKind::Thumbnail),
        Rect::new(0, 0, 160, 160),
        Viewport::new(Rect::new(0, 0, 1024, 768), 256),
    )
    .with_cloud_materialization(cloud)
    .with_size(512, 2_000)
    .with_invalidation(PreviewInvalidationEvent {
        metadata_changed: true,
        ..PreviewInvalidationEvent::default()
    });
    ThumbnailGenerationContract::from_input_checked(
        &PreviewSecurityPolicy::default(),
        input,
        || cancellation.check(),
    )
}

fn fail_first_retry_probe_attempt(
    attempt_state: &Path,
    worker: &str,
    cancellation: &Cancellation,
) -> Result<()> {
    cancellation.check()?;
    let probe = write_probe_path(attempt_state)?.to_path_buf();
    let _access = preflight_access_scope_checked(&probe, AccessIntent::Write, worker, || {
        cancellation.check()
    })?;
    cancellation.check()?;
    let attempts = read_retry_probe_attempt_checked(attempt_state, || cancellation.check())?;
    cancellation.check()?;
    write_retry_probe_attempt_checked(attempt_state, attempts + 1, || cancellation.check())?;
    cancellation.check()?;
    if attempts == 0 {
        return Err(GfmError::Format(format!(
            "temporary {worker} retry probe busy"
        )));
    }
    Ok(())
}

fn run_adaptive_quicklook_session(
    path: PathBuf,
    pressure: SchedulingPressure,
    cancel_after_access: bool,
) -> Result<crate::runtime::ScheduledTaskOutcome<QuickLookSessionContract>> {
    const WORKER: &str = "adaptive quicklook preview";
    let volume_path = path.clone();
    run_preview_contract_adaptive_with_volume_and_payload_path(
        Priority::Visible,
        "quicklook preview",
        pressure,
        move || {
            preflight_volume_access_scope(&volume_path, AccessIntent::Preview, WORKER)?;
            Ok(detect_volume_id(&volume_path)
                .ok()
                .or_else(|| parent_volume(&volume_path)))
        },
        path.clone(),
        move |cancellation| {
            cancellation.check()?;
            let _access =
                preflight_access_scope_checked(&path, AccessIntent::Preview, WORKER, || {
                    cancellation.check()
                })?;
            if cancel_after_access {
                cancellation.cancel();
            }
            cancellation.check()?;
            let record = record_for_path_checked(&path, None, false, || cancellation.check())?;
            cancellation.check()?;
            let cloud = fileprovider_materialization_for_preview(&path, &cancellation)?;
            cancellation.check()?;
            let input = QuickLookSessionInput::new(
                PreviewRequestKey::new(record.id, path.clone(), PreviewKind::QuickLook),
                Rect::new(0, 0, 640, 480),
                Viewport::new(Rect::new(0, 0, 1024, 768), 256),
            )
            .with_cloud_materialization(cloud)
            .with_scheduling_policy(
                PreviewSchedulingPolicy {
                    max_visible: 1,
                    max_prefetch: 1,
                    cancel_offscreen: true,
                }
                .adapted_for_pressure(pressure),
            )
            .with_invalidation(PreviewInvalidationEvent {
                content_changed: true,
                ..PreviewInvalidationEvent::default()
            });
            QuickLookSessionContract::from_input_checked(
                &PreviewSecurityPolicy::default(),
                input,
                || cancellation.check(),
            )
        },
    )
}

fn run_adaptive_thumbnail_generation(
    path: PathBuf,
    pressure: SchedulingPressure,
    cancel_after_access: bool,
) -> Result<crate::runtime::ScheduledTaskOutcome<ThumbnailGenerationContract>> {
    const VOLUME_WORKER: &str = "adaptive thumbnail generation volume";
    const WORKER: &str = "adaptive thumbnail generation";
    let volume_path = path.clone();
    run_preview_contract_adaptive_with_volume_and_payload_path(
        Priority::Background,
        "thumbnail generation",
        pressure,
        move || {
            preflight_volume_access_scope(&volume_path, AccessIntent::Preview, VOLUME_WORKER)?;
            Ok(detect_volume_id(&volume_path)
                .ok()
                .or_else(|| parent_volume(&volume_path)))
        },
        path.clone(),
        move |cancellation| {
            cancellation.check()?;
            let _access =
                preflight_access_scope_checked(&path, AccessIntent::Preview, WORKER, || {
                    cancellation.check()
                })?;
            if cancel_after_access {
                cancellation.cancel();
            }
            cancellation.check()?;
            let record = record_for_path_checked(&path, None, false, || cancellation.check())?;
            cancellation.check()?;
            let cloud = fileprovider_materialization_for_preview(&path, &cancellation)?;
            cancellation.check()?;
            let input = ThumbnailGenerationInput::new(
                PreviewRequestKey::new(record.id, path.clone(), PreviewKind::Thumbnail),
                Rect::new(0, 0, 160, 160),
                Viewport::new(Rect::new(0, 0, 1024, 768), 256),
            )
            .with_cloud_materialization(cloud)
            .with_scheduling_policy(
                PreviewSchedulingPolicy::default().adapted_for_pressure(pressure),
            )
            .with_size(512, 2_000)
            .with_invalidation(PreviewInvalidationEvent {
                metadata_changed: true,
                ..PreviewInvalidationEvent::default()
            });
            ThumbnailGenerationContract::from_input_checked(
                &PreviewSecurityPolicy::default(),
                input,
                || cancellation.check(),
            )
        },
    )
}

fn fileprovider_materialization_for_preview(
    path: &Path,
    cancellation: &Cancellation,
) -> Result<gfm_mac::CloudMaterialization> {
    cancellation.check()?;
    let report = FileProviderStateReport::read_path_checked(path, || cancellation.check())?;
    cancellation.check()?;
    Ok(report.materialization)
}

fn run_security_bookmark_create(path: PathBuf, intent: AccessIntent) -> Result<Vec<String>> {
    const STORE_WORKER: &str = "security bookmark store";
    const WORKER: &str = "security bookmark create";
    preflight_volume_access_scope(&path, intent, WORKER)?;
    let report = SecurityScopedAccessReport::evaluate(&path, intent).create_bookmark();
    if report.status != SecurityScopedBookmarkStatus::Created {
        return Ok(vec![report.as_tsv()]);
    }
    let store = SecurityScopedBookmarkStore::new(crate::runtime::default_security_bookmarks_path());
    let store_probe = write_probe_path(store.path())?.to_path_buf();
    preflight_volume_access_scope(&store_probe, AccessIntent::Write, STORE_WORKER)?;
    let volume = detect_volume_id(&store_probe)
        .ok()
        .or_else(|| parent_volume(&store_probe))
        .or_else(|| detect_volume_id(&path).ok())
        .or_else(|| parent_volume(&path));
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _store_access = preflight_access_scope_checked(
            &store_probe,
            AccessIntent::Write,
            STORE_WORKER,
            || cancellation.check(),
        )?;
        cancellation.check()?;
        let bookmark = gfm_mac::SecurityScopedBookmark::create(&path, report.read_only).map_err(
            |failure| GfmError::Permission {
                path: path.clone(),
                message: failure
                    .reason
                    .unwrap_or_else(|| "security-scoped bookmark creation failed".to_string()),
            },
        )?;
        cancellation.check()?;
        let store_report = store.upsert_checked(bookmark, || cancellation.check())?;
        Ok(vec![report.as_tsv(), store_report.as_tsv()])
    })
}

fn run_security_bookmark_reconcile() -> Result<gfm_mac::SecurityScopedBookmarkStoreReport> {
    const WORKER: &str = "security bookmark reconcile";
    let store = SecurityScopedBookmarkStore::new(crate::runtime::default_security_bookmarks_path());
    let store_probe = write_probe_path(store.path())?.to_path_buf();
    preflight_volume_access_scope(&store_probe, AccessIntent::Write, WORKER)?;
    run_volume_task_cancellable(parent_volume(&store_probe), Priority::Visible, WORKER, {
        let store_probe = store_probe.clone();
        move |cancellation| {
            cancellation.check()?;
            let _store_access =
                preflight_access_scope_checked(&store_probe, AccessIntent::Write, WORKER, || {
                    cancellation.check()
                })?;
            cancellation.check()?;
            store.reconcile_checked(|| cancellation.check())
        }
    })
}

fn run_spotlight_reconcile(
    path: PathBuf,
    fixture_path: Option<PathBuf>,
) -> Result<SpotlightReconciliationReport> {
    const WORKER: &str = "spotlight reconcile";
    const FIXTURE_WORKER: &str = "spotlight fixture";
    preflight_volume_access_scope(&path, AccessIntent::Index, WORKER)?;
    if let Some(fixture_path) = fixture_path.as_ref() {
        preflight_volume_access_scope(fixture_path, AccessIntent::Read, FIXTURE_WORKER)?;
    }
    let volume = detect_volume_id(&path)
        .ok()
        .or_else(|| parent_volume(&path))
        .or_else(|| {
            fixture_path
                .as_ref()
                .and_then(|path| detect_volume_id(path).ok().or_else(|| parent_volume(path)))
        });
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let record =
            record_for_path_with_access(&path, AccessIntent::Index, WORKER, &cancellation)?;
        cancellation.check()?;
        let snapshot = match fixture_path {
            Some(fixture_path) => {
                let _fixture_access = preflight_access_scope_checked(
                    &fixture_path,
                    AccessIntent::Read,
                    FIXTURE_WORKER,
                    || cancellation.check(),
                )?;
                cancellation.check()?;
                let text =
                    read_spotlight_fixture_text_checked(&fixture_path, || cancellation.check())?;
                cancellation.check()?;
                parse_spotlight_fixture(&path, &text)?
            }
            None => SpotlightMetadataReader.read_path(&path)?,
        };
        cancellation.check()?;
        Ok(SpotlightReconciliationReport::reconcile(record, snapshot))
    })
}

fn read_spotlight_fixture_text_checked(
    path: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<String> {
    const CHUNK_BYTES: usize = 64 * 1024;

    check_control()?;
    let mut file = std::fs::File::open(path).map_err(|err| GfmError::io(path, err))?;
    check_control()?;
    let mut bytes = Vec::new();
    let mut buffer = [0; CHUNK_BYTES];
    loop {
        check_control()?;
        let len = file
            .read(&mut buffer)
            .map_err(|err| GfmError::io(path, err))?;
        if len == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..len]);
        check_control()?;
    }
    check_control()?;
    String::from_utf8(bytes)
        .map_err(|err| GfmError::io(path, format!("spotlight fixture is not utf-8: {err}")))
}

fn run_preview_cache_fileprovider_invalidation(
    cache_root: PathBuf,
    previous: CloudStorageState,
    path: PathBuf,
    kind: PreviewKind,
) -> Result<String> {
    const CACHE_WORKER: &str = "preview cache root";
    const WORKER: &str = "preview cache";
    let cache_probe = write_probe_path(&cache_root)?.to_path_buf();
    preflight_volume_access_scope(&cache_probe, AccessIntent::Write, CACHE_WORKER)?;
    preflight_volume_access_scope(&path, AccessIntent::Preview, WORKER)?;
    let volume = detect_volume_id(&cache_probe)
        .ok()
        .or_else(|| parent_volume(&cache_probe))
        .or_else(|| detect_volume_id(&path).ok())
        .or_else(|| parent_volume(&path));
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _cache_access = preflight_access_scope_checked(
            &cache_probe,
            AccessIntent::Write,
            CACHE_WORKER,
            || cancellation.check(),
        )?;
        cancellation.check()?;
        let record =
            record_for_path_with_access(&path, AccessIntent::Preview, WORKER, &cancellation)?;
        cancellation.check()?;
        let report = FileProviderInvalidationReport::evaluate(path.clone(), previous)?;
        let key = PreviewRequestKey::new(record.id, path, kind);
        cancellation.check()?;
        let mut cache =
            PreviewCache::new_cancellable(PreviewCacheConfig::new(cache_root), &cancellation)?;
        let invalidation_key = cache
            .disk_key_for_path_kind(&key.path, key.kind)
            .unwrap_or_else(|| key.clone());
        cancellation.check()?;
        Ok(cache
            .apply_invalidation_cancellable(
                &invalidation_key,
                preview_invalidation_for_fileprovider(&report),
                &cancellation,
            )?
            .as_tsv())
    })
}

fn run_preview_cache_fileprovider_observed_invalidation(
    cache_root: PathBuf,
    state_path: PathBuf,
    kind: PreviewKind,
    event: FileEvent,
) -> Result<String> {
    const WORKER: &str = "preview cache fileprovider observed invalidation";
    let cache_probe = write_probe_path(&cache_root)?.to_path_buf();
    preflight_volume_access_scope(&cache_probe, AccessIntent::Write, "preview cache root")?;
    preflight_fileprovider_observed_event_volumes(&state_path, &event, WORKER)?;
    let state_probe = write_probe_existing_ancestor(&state_path, WORKER)?;
    let volume = detect_volume_id(&cache_probe)
        .ok()
        .or_else(|| parent_volume(&cache_probe))
        .or_else(|| {
            fileprovider_raw_event_paths(&event)
                .iter()
                .find_map(|path| parent_volume(path))
        })
        .or_else(|| parent_volume(&state_probe));
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _cache_access = preflight_access_scope_checked(
            &cache_probe,
            AccessIntent::Write,
            "preview cache root",
            || cancellation.check(),
        )?;
        let observed =
            evaluate_fileprovider_observed_invalidation(&state_path, event, WORKER, &cancellation)?;
        cancellation.check()?;
        observed_preview_cache_invalidation_tsv(&observed, &cache_root, kind, &cancellation)
    })
}

fn run_fileprovider_invalidation_scan(
    state_path: PathBuf,
    paths: Vec<PathBuf>,
) -> Result<FileProviderStateInvalidationReport> {
    const WORKER: &str = "fileprovider invalidation scan";
    preflight_fileprovider_snapshot_volumes(&state_path, &paths, WORKER)?;
    let state_probe = write_probe_existing_ancestor(&state_path, WORKER)?;
    let volume =
        parent_volume(&state_probe).or_else(|| paths.iter().find_map(|path| parent_volume(path)));
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access =
            retain_fileprovider_snapshot_access_checked(&state_path, &paths, WORKER, || {
                cancellation.check()
            })?;
        cancellation.check()?;
        let previous = if fileprovider_state_file_exists(&state_path, WORKER)? {
            Some(FileProviderStateSnapshot::read_checked(
                &state_path,
                || cancellation.check(),
            )?)
        } else {
            None
        };
        cancellation.check()?;
        let (report, snapshot) =
            FileProviderStateInvalidationReport::evaluate(previous.as_ref(), paths)?;
        cancellation.check()?;
        snapshot.write_checked(&state_path, || cancellation.check())?;
        Ok(report)
    })
}

fn run_fileprovider_observed_invalidation(
    state_path: PathBuf,
    event: FileEvent,
    worker: &'static str,
) -> Result<FileProviderObservedInvalidation> {
    preflight_fileprovider_observed_event_volumes(&state_path, &event, worker)?;
    let state_probe = write_probe_existing_ancestor(&state_path, worker)?;
    let volume = parent_volume(&state_probe).or_else(|| {
        fileprovider_raw_event_paths(&event)
            .iter()
            .find_map(|path| parent_volume(path))
    });
    run_volume_task_cancellable(volume, Priority::Visible, worker, move |cancellation| {
        evaluate_fileprovider_observed_invalidation(&state_path, event, worker, &cancellation)
    })
}

fn evaluate_fileprovider_observed_invalidation(
    state_path: &Path,
    event: FileEvent,
    worker: &str,
    cancellation: &Cancellation,
) -> Result<FileProviderObservedInvalidation> {
    evaluate_fileprovider_observed_invalidation_checked(state_path, event, worker, || {
        cancellation.check()
    })
}

fn evaluate_fileprovider_observed_invalidation_checked(
    state_path: &Path,
    event: FileEvent,
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<FileProviderObservedInvalidation> {
    check_control()?;
    let state_probe = write_probe_existing_ancestor(state_path, worker)?;
    check_control()?;
    let mut access = vec![preflight_access_scope_checked(
        &state_probe,
        AccessIntent::Write,
        worker,
        &mut check_control,
    )?];
    check_control()?;
    let previous = if fileprovider_state_file_exists(state_path, worker)? {
        Some(FileProviderStateSnapshot::read_checked(
            state_path,
            &mut check_control,
        )?)
    } else {
        None
    };
    check_control()?;
    access.extend(retain_fileprovider_event_access_checked(
        &event,
        previous.as_ref(),
        worker,
        &mut check_control,
    )?);
    check_control()?;
    let (observed, snapshot) =
        FileProviderObservedInvalidation::evaluate(previous.as_ref(), [event])?;
    check_control()?;
    snapshot.write_checked(state_path, &mut check_control)?;
    Ok(observed)
}

fn preflight_fileprovider_snapshot_volumes(
    state_path: &Path,
    paths: &[PathBuf],
    worker: &str,
) -> Result<()> {
    let state_probe = write_probe_existing_ancestor(state_path, worker)?;
    preflight_volume_access_scope(&state_probe, AccessIntent::Write, worker)?;
    for path in unique_fileprovider_paths(paths.iter().map(PathBuf::as_path)) {
        preflight_volume_access_scope(path, AccessIntent::Read, worker)?;
    }
    Ok(())
}

fn preflight_fileprovider_observed_event_volumes(
    state_path: &Path,
    event: &FileEvent,
    worker: &str,
) -> Result<()> {
    let state_probe = write_probe_existing_ancestor(state_path, worker)?;
    preflight_volume_access_scope(&state_probe, AccessIntent::Write, worker)?;
    let paths = fileprovider_raw_event_paths(event);
    for path in unique_fileprovider_paths(paths.iter().map(PathBuf::as_path)) {
        preflight_volume_access_scope(path, AccessIntent::Read, worker)?;
    }
    Ok(())
}

fn fileprovider_raw_event_paths(event: &FileEvent) -> Vec<PathBuf> {
    match &event.kind {
        FileEventKind::Rename { from, to } => vec![from.clone(), to.clone()],
        FileEventKind::Remove
        | FileEventKind::Create
        | FileEventKind::Metadata
        | FileEventKind::Modify
        | FileEventKind::Rescan
        | FileEventKind::Other => vec![event.path.clone()],
    }
}

fn run_fileprovider_worker_without_runtime_progress<T>(
    volume: Option<VolumeId>,
    worker: &'static str,
    work: impl Fn(Cancellation) -> Result<T> + Send + Sync + 'static,
) -> Result<T>
where
    T: Send + 'static,
{
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
    let mut scheduler = Scheduler::new();
    let job = if let Some(volume) = volume {
        scheduler.schedule_on_volume_in_class(Priority::Visible, JobClass::Visible, worker, volume)
    } else {
        scheduler.schedule_in_class(Priority::Visible, JobClass::Visible, worker)
    };
    let task = RetriableTask::new(job.clone(), move |cancellation| {
        let result = work(cancellation)?;
        result_tx
            .send(result)
            .map_err(|_| GfmError::Format(format!("{worker} result receiver dropped")))?;
        Ok(())
    });
    let journal = JobJournal::new(default_job_journal_path());
    let report = WorkerPool::new(1).run_retriable_isolated(
        vec![task],
        &journal,
        RetryPolicy { max_attempts: 2 },
        VolumeConcurrencyPolicy::new(1),
    );
    let outcome = report
        .outcomes
        .iter()
        .find(|outcome| outcome.id == job.id)
        .ok_or_else(|| GfmError::Format(format!("{worker} job did not run")))?;
    match &outcome.status {
        TaskStatus::Completed => {}
        TaskStatus::Started => {
            return Err(GfmError::Format(format!("{worker} job is still running")))
        }
        TaskStatus::Cancelled => return Err(GfmError::Cancelled),
        TaskStatus::Failed(message) => {
            return Err(GfmError::Format(format!("{worker} job failed: {message}")))
        }
    }
    result_rx
        .try_recv()
        .map_err(|_| GfmError::Format(format!("{worker} job completed without a result")))
}

pub(crate) fn run_fileprovider_observer_probe(
    state_path: &Path,
    root: &Path,
    target: &Path,
    worker: &str,
) -> Result<FileProviderObservedInvalidation> {
    let root_worker = format!("{worker} root");
    let target_worker = format!("{worker} target");
    let state_worker = format!("{worker} state");
    let target_probe = write_probe_existing_ancestor(target, &target_worker)?;
    preflight_volume_access_scope(root, AccessIntent::Index, &root_worker)?;
    preflight_volume_access_scope(&target_probe, AccessIntent::Write, &target_worker)?;
    preflight_fileprovider_snapshot_volumes(state_path, &[target.to_path_buf()], &state_worker)?;
    let state_probe = write_probe_existing_ancestor(state_path, &state_worker)?;
    let state_path = state_path.to_path_buf();
    let root = root.to_path_buf();
    let target = target.to_path_buf();
    let worker_name = worker.to_string();
    let volume = parent_volume(&root)
        .or_else(|| parent_volume(&target_probe))
        .or_else(|| parent_volume(&state_probe));
    run_volume_task_cancellable(
        volume,
        Priority::Visible,
        "fileprovider observer probe",
        move |cancellation| {
            cancellation.check()?;
            let root_worker = format!("{worker_name} root");
            let target_worker = format!("{worker_name} target");
            let state_worker = format!("{worker_name} state");
            cancellation.check()?;
            let target_probe = write_probe_existing_ancestor(&target, &target_worker)?;
            cancellation.check()?;
            let _root_access =
                preflight_access_scope_checked(&root, AccessIntent::Index, &root_worker, || {
                    cancellation.check()
                })?;
            cancellation.check()?;
            let _target_access = preflight_access_scope_checked(
                &target_probe,
                AccessIntent::Write,
                &target_worker,
                || cancellation.check(),
            )?;
            cancellation.check()?;
            let _state_access = retain_fileprovider_snapshot_access_checked(
                &state_path,
                std::slice::from_ref(&target),
                &state_worker,
                || cancellation.check(),
            )?;
            cancellation.check()?;
            let previous = if fileprovider_state_file_exists(&state_path, &state_worker)? {
                Some(FileProviderStateSnapshot::read_checked(
                    &state_path,
                    || cancellation.check(),
                )?)
            } else {
                None
            };
            cancellation.check()?;
            let mut observer =
                FileProviderStateObserver::watch(&[WatchRoot::tree(&root)], previous)?;
            cancellation.check()?;
            std::fs::write(&target, b"observer-probe").map_err(|err| GfmError::io(&target, err))?;
            cancellation.check()?;
            let observed = drain_fileprovider_observer_probe(&mut observer, &cancellation)?;
            cancellation.check()?;
            observer
                .snapshot()
                .write_checked(&state_path, || cancellation.check())?;
            Ok(observed)
        },
    )
}

fn observed_preview_cache_invalidation_tsv(
    observed: &FileProviderObservedInvalidation,
    cache_root: &Path,
    kind: PreviewKind,
    cancellation: &Cancellation,
) -> Result<String> {
    cancellation.check()?;
    let mut cache =
        PreviewCache::new_cancellable(PreviewCacheConfig::new(cache_root), cancellation)?;
    let mut lines = vec![observed.as_tsv()];
    for report in &observed.report.changes {
        cancellation.check()?;
        let key = preview_cache_key_for_path_kind(&cache, &report.path, kind, cancellation)?;
        let invalidation_key = cache
            .disk_key_for_path_kind(&key.path, key.kind)
            .unwrap_or_else(|| key.clone());
        cancellation.check()?;
        lines.push(
            cache
                .apply_invalidation_cancellable(
                    &invalidation_key,
                    preview_invalidation_for_fileprovider(report),
                    cancellation,
                )?
                .as_tsv(),
        );
    }
    Ok(lines.join("\n"))
}

fn preview_cache_key_for_path_kind(
    cache: &PreviewCache,
    path: &Path,
    kind: PreviewKind,
    cancellation: &Cancellation,
) -> Result<PreviewRequestKey> {
    cancellation.check()?;
    if let Some(key) = cache.disk_key_for_path_kind(path, kind) {
        return Ok(key);
    }
    cancellation.check()?;
    let file_id = match path.try_exists() {
        Ok(true) => record_for_path_checked(path, None, false, || cancellation.check())?.id,
        Ok(false) => FileId::new(VolumeId(0), 0),
        Err(err) => {
            return Err(GfmError::io(
                path,
                format!("preview cache key path existence unavailable: {err}"),
            ))
        }
    };
    cancellation.check()?;
    Ok(PreviewRequestKey::new(file_id, path.to_path_buf(), kind))
}

fn drain_fileprovider_observer_probe(
    observer: &mut FileProviderStateObserver,
    cancellation: &Cancellation,
) -> Result<FileProviderObservedInvalidation> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        cancellation.check()?;
        if let Some(observed) = observer.drain_available(64)? {
            if !observed.paths.is_empty() {
                return Ok(observed);
            }
        }
        fileprovider_observer_poll_pause(Duration::from_millis(25), cancellation)?;
    }
    Err(GfmError::Format(
        "fileprovider observer probe timed out waiting for a provider event".to_string(),
    ))
}

fn fileprovider_observer_poll_pause(delay: Duration, cancellation: &Cancellation) -> Result<()> {
    const CANCEL_GRANULARITY: Duration = Duration::from_millis(1);
    let deadline = Instant::now() + delay;
    loop {
        cancellation.check()?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        thread::sleep(remaining.min(CANCEL_GRANULARITY));
    }
}

fn parse_volume_event_kind(kind: &str) -> Result<VolumeEventKind> {
    match kind {
        "appeared" => Ok(VolumeEventKind::Appeared),
        "description-changed" => Ok(VolumeEventKind::DescriptionChanged),
        "disappeared" => Ok(VolumeEventKind::Disappeared),
        "unavailable" => Ok(VolumeEventKind::Unavailable),
        other => Err(GfmError::Format(format!(
            "unsupported volume event kind `{other}`"
        ))),
    }
}

fn parse_worker_admission_requests(
    args: &mut impl Iterator<Item = String>,
) -> Result<Vec<WorkerAdmissionRequest>> {
    let mut requests = Vec::new();
    while let Some(worker) = args.next() {
        let intent = AccessIntent::parse(&required_string(
            args.next(),
            "security-worker-admission-fanout requires an intent after every worker label",
        )?)?;
        requests.push(WorkerAdmissionRequest { worker, intent });
    }
    if requests.is_empty() {
        return Err(GfmError::Format(
            "security-worker-admission-fanout requires at least one worker label and intent"
                .to_string(),
        ));
    }
    Ok(requests)
}

fn worker_admission_fanout_summary(admissions: &[SecurityWorkerAdmissionReport]) -> String {
    let starts = admissions
        .iter()
        .filter(|admission| admission.worker_action == SecurityWorkerAction::Start)
        .count();
    let prompts = admissions
        .iter()
        .filter(|admission| admission.worker_action == SecurityWorkerAction::Prompt)
        .count();
    let metadata_only = admissions
        .iter()
        .filter(|admission| admission.worker_action == SecurityWorkerAction::MetadataOnly)
        .count();
    let denied = admissions
        .iter()
        .filter(|admission| admission.worker_action == SecurityWorkerAction::Deny)
        .count();
    let can_touch_filesystem = admissions
        .iter()
        .filter(|admission| admission.can_touch_filesystem)
        .count();
    let bookmark_access = admissions
        .iter()
        .filter(|admission| admission.needs_bookmark_access)
        .count();
    let refresh_on_permission_change = admissions
        .iter()
        .filter(|admission| admission.refresh_on_permission_change)
        .count();
    format!(
        "security-worker-admission-fanout\tworkers={}\tstart={}\tprompt={}\tmetadata-only={}\tdeny={}\tcan-touch-filesystem={}\tbookmark-access={}\trefresh-on-permission-change={}",
        admissions.len(),
        starts,
        prompts,
        metadata_only,
        denied,
        can_touch_filesystem,
        bookmark_access,
        refresh_on_permission_change
    )
}

fn index_volume_event_kind(kind: VolumeEventKind) -> IndexVolumeEventKind {
    match kind {
        VolumeEventKind::Appeared => IndexVolumeEventKind::Appeared,
        VolumeEventKind::DescriptionChanged => IndexVolumeEventKind::DescriptionChanged,
        VolumeEventKind::Disappeared => IndexVolumeEventKind::Disappeared,
        VolumeEventKind::Unavailable => IndexVolumeEventKind::Unavailable,
    }
}

fn retain_fileprovider_snapshot_access_checked(
    state_path: &Path,
    paths: &[PathBuf],
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    check_control()?;
    let mut guards = Vec::with_capacity(paths.len() + 1);
    let state_probe = write_probe_existing_ancestor(state_path, worker)?;
    check_control()?;
    guards.push(preflight_access_scope_checked(
        &state_probe,
        AccessIntent::Write,
        worker,
        &mut check_control,
    )?);
    for path in unique_fileprovider_paths(paths.iter().map(PathBuf::as_path)) {
        check_control()?;
        guards.push(preflight_access_scope_checked(
            path,
            AccessIntent::Read,
            worker,
            &mut check_control,
        )?);
    }
    check_control()?;
    Ok(guards)
}

fn retain_fileprovider_event_access_checked(
    event: &FileEvent,
    previous: Option<&FileProviderStateSnapshot>,
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    check_control()?;
    let mut guards = Vec::new();
    let paths = fileprovider_event_access_paths(event, previous, worker)?;
    for path in unique_fileprovider_paths(paths.iter().map(PathBuf::as_path)) {
        check_control()?;
        guards.push(preflight_access_scope_checked(
            path,
            AccessIntent::Read,
            worker,
            &mut check_control,
        )?);
    }
    check_control()?;
    Ok(guards)
}

fn unique_fileprovider_paths<'a>(paths: impl IntoIterator<Item = &'a Path>) -> Vec<&'a Path> {
    let mut seen = BTreeSet::new();
    paths
        .into_iter()
        .filter(|path| seen.insert((*path).to_path_buf()))
        .collect()
}

fn fileprovider_event_access_paths(
    event: &FileEvent,
    previous: Option<&FileProviderStateSnapshot>,
    worker: &str,
) -> Result<Vec<PathBuf>> {
    match &event.kind {
        FileEventKind::Rename { from, to } => Ok([from, to]
            .into_iter()
            .map(|path| fileprovider_event_access_path(path, previous, worker))
            .collect::<Result<Vec<_>>>()?),
        FileEventKind::Remove => Ok(vec![fileprovider_event_access_path(
            &event.path,
            previous,
            worker,
        )?]),
        FileEventKind::Create
        | FileEventKind::Metadata
        | FileEventKind::Modify
        | FileEventKind::Rescan
        | FileEventKind::Other => Ok(vec![event.path.clone()]),
    }
}

fn fileprovider_event_access_path(
    path: &Path,
    previous: Option<&FileProviderStateSnapshot>,
    worker: &str,
) -> Result<PathBuf> {
    if snapshot_tracks_path_or_descendant(previous, path) {
        return write_probe_existing_ancestor(path, worker);
    }
    Ok(path.to_path_buf())
}

fn snapshot_tracks_path_or_descendant(
    previous: Option<&FileProviderStateSnapshot>,
    path: &Path,
) -> bool {
    previous.is_some_and(|snapshot| {
        snapshot
            .entries
            .iter()
            .any(|entry| path_matches_or_contains(path, &entry.path))
    })
}

fn path_matches_or_contains(root: &Path, candidate: &Path) -> bool {
    if candidate == root || candidate.starts_with(root) {
        return true;
    }
    let Some(root) = normalized_existing_ancestor_path(root) else {
        return false;
    };
    normalized_existing_ancestor_path(candidate)
        .as_deref()
        .is_some_and(|candidate| candidate == root || candidate.starts_with(root))
}

fn normalized_existing_ancestor_path(path: &Path) -> Option<PathBuf> {
    if let Ok(canonical) = path.canonicalize() {
        return Some(canonical);
    }

    let mut candidate = path;
    let mut missing = Vec::new();
    loop {
        match candidate.try_exists() {
            Ok(true) => {
                let mut normalized = candidate.canonicalize().ok()?;
                for component in missing.iter().rev() {
                    normalized.push(component);
                }
                return Some(normalized);
            }
            Ok(false) => {}
            Err(_) => return None,
        }
        missing.push(candidate.file_name()?.to_os_string());
        candidate = candidate.parent()?;
    }
}

fn write_probe_path(path: &Path) -> Result<&Path> {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(path),
        Ok(_) => Ok(crate::parent_or_cwd(path)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(crate::parent_or_cwd(path)),
        Err(err) => Err(GfmError::io(
            path,
            format!("platform write path metadata unavailable: {err}"),
        )),
    }
}

fn write_probe_existing_ancestor(path: &Path, worker: &str) -> Result<PathBuf> {
    let mut candidate = write_probe_path(path)?.to_path_buf();
    loop {
        match candidate.try_exists() {
            Ok(true) => return Ok(candidate),
            Ok(false) => {
                let Some(parent) = candidate.parent() else {
                    return Ok(candidate);
                };
                if parent == candidate {
                    return Ok(candidate);
                }
                candidate = parent.to_path_buf();
            }
            Err(err) => {
                return Err(GfmError::io(
                    &candidate,
                    format!("{worker} write path ancestor unavailable: {err}"),
                ))
            }
        }
    }
}

fn fileprovider_state_file_exists(path: &Path, worker: &str) -> Result<bool> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(metadata.is_file()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(GfmError::io(
            path,
            format!("{worker} state metadata unavailable: {err}"),
        )),
    }
}

fn record_for_path_with_access(
    path: &Path,
    intent: AccessIntent,
    worker: &str,
    cancellation: &Cancellation,
) -> Result<FileRecord> {
    let _access = preflight_access_scope_checked(path, intent, worker, || cancellation.check())?;
    cancellation.check()?;
    record_for_path_checked(path, None, false, || cancellation.check())
}

fn parse_preview_kind(value: Option<String>) -> Result<PreviewKind> {
    match value.as_deref() {
        Some("icon") | None => Ok(PreviewKind::Icon),
        Some("thumbnail") => Ok(PreviewKind::Thumbnail),
        Some("quick-look") => Ok(PreviewKind::QuickLook),
        Some("text") => Ok(PreviewKind::Text),
        Some(other) => Err(GfmError::Format(format!(
            "preview kind must be icon, thumbnail, quick-look, or text; got `{other}`"
        ))),
    }
}

fn preview_task(node: u64, x: i32, y: i32) -> PreviewTask {
    PreviewTask::new(
        PreviewRequestKey::new(
            FileId::new(VolumeId(1), node),
            PathBuf::from(format!("{node}.preview")),
            PreviewKind::Thumbnail,
        ),
        Rect::new(x, y, 32, 32),
    )
}

fn preview_decision_priority(decision: &gfm_preview::PreviewTaskDecision) -> &'static str {
    match decision {
        gfm_preview::PreviewTaskDecision::Scheduled { priority, .. }
        | gfm_preview::PreviewTaskDecision::Coalesced { priority, .. } => priority.as_str(),
        gfm_preview::PreviewTaskDecision::Cancelled { reason, .. } => reason,
    }
}

fn required_path(value: Option<String>, message: &str) -> Result<PathBuf> {
    value
        .map(PathBuf::from)
        .ok_or_else(|| GfmError::Format(message.to_string()))
}

fn required_string(value: Option<String>, message: &str) -> Result<String> {
    value.ok_or_else(|| GfmError::Format(message.to_string()))
}

fn parse_platform_bool(value: &str, name: &str) -> Result<bool> {
    match value {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => Err(GfmError::Format(format!(
            "{name} must be true or false; got `{value}`"
        ))),
    }
}

fn optional_platform_bool(value: Option<String>, name: &str) -> Result<Option<bool>> {
    value
        .filter(|value| value != "-")
        .map(|value| parse_platform_bool(&value, name))
        .transpose()
}

fn optional_platform_string(value: Option<String>) -> Option<String> {
    value.filter(|value| value != "-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfm_mac::FileProviderStateSnapshotEntry;

    #[test]
    fn fileprovider_observer_poll_pause_returns_promptly_after_cancellation() {
        let cancellation = Cancellation::default();
        let canceller = cancellation.clone();
        let started = Instant::now();
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(5));
            canceller.cancel();
        });

        let err = fileprovider_observer_poll_pause(Duration::from_millis(250), &cancellation)
            .expect_err("observer poll pause should observe cancellation");

        handle.join().unwrap();
        assert_eq!(err, GfmError::Cancelled);
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "cancelled observer poll pause waited {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn observed_preview_cache_invalidation_honors_pre_cancelled_token_before_disk_touch() {
        let root = std::env::temp_dir().join(format!(
            "gfm-platform-cancelled-preview-cache-observed-{}",
            std::process::id()
        ));
        let cancellation = Cancellation::default();
        cancellation.cancel();
        let observed = FileProviderObservedInvalidation {
            events: 1,
            event_kinds: Vec::new(),
            paths: Vec::new(),
            report: FileProviderStateInvalidationReport {
                initialized: false,
                changes: Vec::new(),
                invalidate_icon: false,
                invalidate_preview_memory: false,
                invalidate_preview_disk: false,
                invalidate_sidebar: false,
                reindex_metadata: false,
            },
        };

        let err = observed_preview_cache_invalidation_tsv(
            &observed,
            &root,
            PreviewKind::Thumbnail,
            &cancellation,
        )
        .expect_err("pre-cancelled observed invalidation should stop before cache open");

        assert_eq!(err, GfmError::Cancelled);
        assert!(!root.exists());
    }

    #[test]
    fn preview_cache_key_resolution_honors_pre_cancelled_token_before_path_probe() {
        let cache = PreviewCache::new(PreviewCacheConfig {
            memory_budget_bytes: 16,
            max_entry_bytes: 16,
            disk_root: std::env::temp_dir().join(format!(
                "gfm-platform-preview-cache-key-cancelled-{}",
                std::process::id()
            )),
            disk_enabled: false,
        })
        .unwrap();
        let cancellation = Cancellation::default();
        cancellation.cancel();
        let unprobeable = std::env::temp_dir().join("gfm-preview-cache-key".repeat(64));

        let err = preview_cache_key_for_path_kind(
            &cache,
            &unprobeable,
            PreviewKind::Thumbnail,
            &cancellation,
        )
        .expect_err("pre-cancelled preview cache key resolution should not touch the path");

        assert_eq!(err, GfmError::Cancelled);
    }

    #[test]
    fn preview_fileprovider_materialization_honors_pre_cancelled_token_before_state_read() {
        let cancellation = Cancellation::default();
        cancellation.cancel();
        let path = std::env::temp_dir().join("gfm-preview-materialization-cancelled");

        let err = fileprovider_materialization_for_preview(&path, &cancellation)
            .expect_err("pre-cancelled preview materialization should not read FileProvider state");

        assert_eq!(err, GfmError::Cancelled);
    }

    #[test]
    fn spotlight_fixture_reader_honors_pre_cancelled_token_before_open() {
        let path = std::env::temp_dir().join(format!(
            "gfm-platform-spotlight-fixture-pre-cancelled-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);

        let err =
            read_spotlight_fixture_text_checked(&path, || Err(GfmError::Cancelled)).unwrap_err();

        assert_eq!(err, GfmError::Cancelled);
        assert!(!path.exists());
    }

    #[test]
    fn spotlight_fixture_reader_checks_control_between_chunks() {
        let path = std::env::temp_dir().join(format!(
            "gfm-platform-spotlight-fixture-chunked-{}",
            std::process::id()
        ));
        let text = "kMDItemDisplayName\tPrimary.md\n".repeat(5000);
        std::fs::write(&path, &text).unwrap();
        let mut checks = 0usize;

        let read = read_spotlight_fixture_text_checked(&path, || {
            checks += 1;
            Ok(())
        })
        .unwrap();

        assert_eq!(read, text);
        assert!(checks > 4, "expected repeated chunk checks, saw {checks}");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn spotlight_fixture_reader_returns_cancelled_between_chunks() {
        let path = std::env::temp_dir().join(format!(
            "gfm-platform-spotlight-fixture-cancelled-{}",
            std::process::id()
        ));
        let text = "kMDItemDisplayName\tPrimary.md\n".repeat(5000);
        std::fs::write(&path, &text).unwrap();
        let before = std::fs::read(&path).unwrap();
        let mut checks = 0usize;

        let err = read_spotlight_fixture_text_checked(&path, || {
            checks += 1;
            if checks >= 5 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        })
        .unwrap_err();

        assert_eq!(err, GfmError::Cancelled);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn fileprovider_observed_invalidation_preserves_state_when_cancelled_during_publish() {
        let root = std::env::temp_dir().join(format!(
            "gfm-platform-fileprovider-publish-cancel-{}",
            std::process::id()
        ));
        let state_path = root.join("state.tsv");
        let tracked = root.join("Remote.icloud-placeholder").join("Gone.md");
        std::fs::create_dir_all(tracked.parent().unwrap()).unwrap();
        let previous = FileProviderStateSnapshot {
            entries: vec![FileProviderStateSnapshotEntry {
                path: tracked.clone(),
                state: CloudStorageState::Evicted,
            }],
        };
        previous.write(&state_path).unwrap();
        let before = std::fs::read(&state_path).unwrap();
        std::fs::remove_dir_all(tracked.parent().unwrap()).unwrap();
        let event = FileEvent::new(tracked, FileEventKind::Remove);
        let mut checks = 0usize;

        let err = evaluate_fileprovider_observed_invalidation_checked(
            &state_path,
            event,
            "fileprovider observed invalidation",
            || {
                checks += 1;
                if checks >= 10 {
                    Err(GfmError::Cancelled)
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();

        assert_eq!(err, GfmError::Cancelled);
        assert_eq!(std::fs::read(&state_path).unwrap(), before);
        let leftovers = std::fs::read_dir(&root)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".state.tsv")
            })
            .count();
        assert_eq!(leftovers, 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fileprovider_snapshot_access_checked_honors_pre_cancelled_control() {
        let state_path = std::env::temp_dir()
            .join(format!(
                "gfm-platform-snapshot-access-pre-cancelled-{}",
                std::process::id()
            ))
            .join("state.tsv");
        let tracked = std::env::temp_dir().join("gfm-platform-snapshot-access-tracked");

        let result = retain_fileprovider_snapshot_access_checked(
            &state_path,
            &[tracked],
            "fileprovider snapshot",
            || Err(GfmError::Cancelled),
        );

        let err = match result {
            Ok(_) => panic!("pre-cancelled snapshot access should not retain guards"),
            Err(err) => err,
        };
        assert_eq!(err, GfmError::Cancelled);
        assert!(!state_path.exists());
    }

    #[test]
    fn fileprovider_event_access_checked_honors_pre_cancelled_control() {
        let event = FileEvent::new(
            std::env::temp_dir().join("gfm-platform-event-access-pre-cancelled"),
            FileEventKind::Modify,
        );

        let result =
            retain_fileprovider_event_access_checked(&event, None, "fileprovider event", || {
                Err(GfmError::Cancelled)
            });

        let err = match result {
            Ok(_) => panic!("pre-cancelled event access should not retain guards"),
            Err(err) => err,
        };
        assert_eq!(err, GfmError::Cancelled);
    }

    #[test]
    fn tracked_removed_fileprovider_event_uses_existing_ancestor_without_path_probe() {
        let root = std::env::temp_dir().join(format!(
            "gfm-platform-fileprovider-event-access-{}",
            std::process::id()
        ));
        let tracked = root.join("Remote.icloud").join("Gone.md");
        std::fs::create_dir_all(tracked.parent().unwrap()).unwrap();
        let previous = FileProviderStateSnapshot {
            entries: vec![FileProviderStateSnapshotEntry {
                path: tracked.clone(),
                state: CloudStorageState::Evicted,
            }],
        };
        std::fs::remove_dir_all(tracked.parent().unwrap()).unwrap();

        let access_path =
            fileprovider_event_access_path(&tracked, Some(&previous), "fileprovider event")
                .unwrap();

        assert_eq!(access_path, root);
        assert!(!tracked.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unique_fileprovider_paths_preserves_first_occurrence_order() {
        let first = PathBuf::from("/tmp/gfm-fileprovider/first.icloud-placeholder");
        let second = PathBuf::from("/tmp/gfm-fileprovider/second.icloud-placeholder");

        let unique =
            unique_fileprovider_paths([first.as_path(), second.as_path(), first.as_path()])
                .into_iter()
                .map(Path::to_path_buf)
                .collect::<Vec<_>>();

        assert_eq!(unique, vec![first, second]);
    }

    #[test]
    fn normalized_existing_ancestor_path_returns_unknown_for_unprobeable_component() {
        let root = std::env::temp_dir().join(format!(
            "gfm-platform-normalized-unprobeable-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let unprobeable = root.join("platform-path-unavailable".repeat(16));

        assert_eq!(normalized_existing_ancestor_path(&unprobeable), None);

        std::fs::remove_dir_all(root).unwrap();
    }
}
