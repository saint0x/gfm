use crate::access::{preflight_access_scope, ScopedAccessGuard};
use crate::{
    detect_volume_id, index_volume_descriptor, parse_required_scheduling_pressure,
    run_preview_contract_adaptive, run_preview_contract_cancellable, runtime::RuntimeJobHandle,
};
use gfm_fs::record_for_path;
use gfm_index::{
    parse_volume_indexing_policy, IndexMountState, IndexVolumeClass, IndexVolumeDescriptor,
    IndexVolumeEventKind, ProviderMetadataInvalidationReport, VolumeEventIndexInvalidationReport,
    VolumeIndexPolicy, VolumeInvalidationReport,
};
use gfm_jobs::{
    Cancellation, JobClass, JobPayloadKind, JobProgressState, Priority, Scheduler, SchedulingAction,
};
use gfm_mac::{
    current_host_profile, parse_spotlight_fixture, AccessIntent, CloudStorageState,
    CloudTransferDirection, FileProviderConflictReport, FileProviderDomainEnumerationReport,
    FileProviderDomainReport, FileProviderInvalidationReport, FileProviderObservedInvalidation,
    FileProviderOperation, FileProviderOperationReport, FileProviderProgressReport,
    FileProviderStateInvalidationReport, FileProviderStateObserver, FileProviderStateReport,
    FileProviderStateSnapshot, MacBridgeContract, NativeIconBridgeContract, NativeIconDescriptor,
    NativeIconInvalidationReport, SecurityScopedAccessReport, SecurityScopedBookmarkStatus,
    SecurityScopedBookmarkStore, SecurityWorkerAdmissionReport, SpotlightMetadataReader,
    SpotlightReconciliationReport, VolumeDescriptor, VolumeDiscoveryReport,
    VolumeEventInvalidationReport, VolumeEventKind, VolumeEventStream, VolumeMountIdentityReport,
    VolumeOperation, VolumeOperationReport, VolumeTopologyDiff, WatchRoot,
};
use gfm_preview::{
    decide_invalidation, decide_preview_security, preview_invalidation_for_fileprovider,
    security_input_for_path, IconPreviewContract, IconPreviewInput, PreviewCache,
    PreviewCacheConfig, PreviewInvalidationEvent, PreviewKind, PreviewRequestKey, PreviewScheduler,
    PreviewSchedulingPolicy, PreviewSecurityPolicy, PreviewTask, QuickLookSessionContract,
    QuickLookSessionInput, Rect, ThumbnailGenerationContract, ThumbnailGenerationInput, Viewport,
};
use gfm_types::{FileEvent, FileEventKind, FileId, FileRecord, GfmError, Result, VolumeId};
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
                SecurityWorkerAdmissionReport::evaluate(path, intent, worker).as_tsv()
            );
        }
        "security-bookmark-create" => {
            let path = required_path(args.next(), "security-bookmark-create requires a path")?;
            let intent = args
                .next()
                .map(|value| AccessIntent::parse(&value))
                .transpose()?
                .unwrap_or(AccessIntent::Read);
            let report = SecurityScopedAccessReport::evaluate(&path, intent).create_bookmark();
            if report.status == SecurityScopedBookmarkStatus::Created {
                let store = SecurityScopedBookmarkStore::new(
                    crate::runtime::default_security_bookmarks_path(),
                );
                let _store_access = preflight_access_scope(
                    write_probe_path(store.path()),
                    AccessIntent::Write,
                    "security bookmark store",
                )?;
                let bookmark = gfm_mac::SecurityScopedBookmark::create(&path, report.read_only)
                    .map_err(|failure| GfmError::Permission {
                        path: path.clone(),
                        message: failure.reason.unwrap_or_else(|| {
                            "security-scoped bookmark creation failed".to_string()
                        }),
                    })?;
                println!("{}", report.as_tsv());
                println!("{}", store.upsert(bookmark)?.as_tsv());
            } else {
                println!("{}", report.as_tsv());
            }
        }
        "mac-bridges" => {
            println!("{}", MacBridgeContract::finder_required().as_tsv());
        }
        "native-icon" => {
            let path = required_path(args.next(), "native-icon requires a path")?;
            let record = record_for_path_with_access(&path, AccessIntent::Preview, "native icon")?;
            println!("{}", NativeIconDescriptor::for_record(&record).as_tsv());
        }
        "native-icon-bridge" => {
            let path = required_path(args.next(), "native-icon-bridge requires a path")?;
            let record =
                record_for_path_with_access(&path, AccessIntent::Preview, "native icon bridge")?;
            let host = current_host_profile()?;
            println!(
                "{}",
                NativeIconBridgeContract::for_record_on_host(&record, &host).as_tsv()
            );
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
            let _access =
                preflight_access_scope(&path, AccessIntent::Read, "native icon fileprovider")?;
            let report = FileProviderInvalidationReport::evaluate(path, previous)?;
            println!(
                "{}",
                NativeIconInvalidationReport::from_fileprovider(&report).as_tsv()
            );
        }
        "fileprovider-state" => {
            let path = required_path(args.next(), "fileprovider-state requires a path")?;
            let _access = preflight_access_scope(&path, AccessIntent::Read, "fileprovider state")?;
            println!("{}", FileProviderStateReport::read_path(path)?.as_tsv());
        }
        "fileprovider-state-with-identity" => {
            let path = required_path(
                args.next(),
                "fileprovider-state-with-identity requires a path",
            )?;
            let _access =
                preflight_access_scope(&path, AccessIntent::Read, "fileprovider identity state")?;
            println!(
                "{}",
                FileProviderStateReport::from_path_with_native_identity(path).as_tsv()
            );
        }
        "fileprovider-domain" => {
            let path = required_path(args.next(), "fileprovider-domain requires a path")?;
            let _access = preflight_access_scope(&path, AccessIntent::Read, "fileprovider domain")?;
            println!("{}", FileProviderDomainReport::read_path(path)?.as_tsv());
        }
        "fileprovider-domains" => {
            println!(
                "{}",
                FileProviderDomainEnumerationReport::discover().as_tsv()
            );
        }
        "fileprovider-progress" => {
            let path = required_path(args.next(), "fileprovider-progress requires a path")?;
            let _access =
                preflight_access_scope(&path, AccessIntent::Read, "fileprovider progress")?;
            println!("{}", FileProviderProgressReport::read_path(path)?.as_tsv());
        }
        "fileprovider-conflict" => {
            let path = required_path(args.next(), "fileprovider-conflict requires a path")?;
            let _access =
                preflight_access_scope(&path, AccessIntent::Read, "fileprovider conflict")?;
            println!("{}", FileProviderConflictReport::read_path(path)?.as_tsv());
        }
        "fileprovider-progress-job" => {
            let path = required_path(args.next(), "fileprovider-progress-job requires a path")?;
            let _access =
                preflight_access_scope(&path, AccessIntent::Read, "fileprovider progress job")?;
            println!("{}", publish_fileprovider_progress_job(path)?.as_tsv());
        }
        "fileprovider-operation" => {
            let operation = FileProviderOperation::parse(&required_string(
                args.next(),
                "fileprovider-operation requires an operation",
            )?)?;
            let path = required_path(args.next(), "fileprovider-operation requires a path")?;
            let _access =
                preflight_access_scope(&path, AccessIntent::Operate, "fileprovider operation")?;
            println!(
                "{}",
                FileProviderOperationReport::execute(path, operation)?.as_tsv()
            );
        }
        "fileprovider-invalidation" => {
            let previous = CloudStorageState::parse(&required_string(
                args.next(),
                "fileprovider-invalidation requires a previous state",
            )?)?;
            let path = required_path(args.next(), "fileprovider-invalidation requires a path")?;
            let _access =
                preflight_access_scope(&path, AccessIntent::Read, "fileprovider invalidation")?;
            println!(
                "{}",
                FileProviderInvalidationReport::evaluate(path, previous)?.as_tsv()
            );
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
            let _access = preflight_access_scope(
                &path,
                AccessIntent::Read,
                "fileprovider metadata invalidation",
            )?;
            let report = FileProviderInvalidationReport::evaluate(path, previous)?;
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
            let _cache_access = preflight_access_scope(
                write_probe_path(&cache_root),
                AccessIntent::Write,
                "preview cache root",
            )?;
            let record =
                record_for_path_with_access(&path, AccessIntent::Preview, "preview cache")?;
            let report = FileProviderInvalidationReport::evaluate(path.clone(), previous)?;
            let key = PreviewRequestKey::new(record.id, path, kind);
            let mut cache = PreviewCache::new(PreviewCacheConfig::new(cache_root))?;
            println!(
                "{}",
                cache
                    .apply_invalidation(&key, preview_invalidation_for_fileprovider(&report))?
                    .as_tsv()
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
            let _access = retain_fileprovider_snapshot_access(
                &state_path,
                &paths,
                "fileprovider invalidation scan",
            )?;
            let previous = if state_path.is_file() {
                Some(FileProviderStateSnapshot::read(&state_path)?)
            } else {
                None
            };
            let (report, snapshot) =
                FileProviderStateInvalidationReport::evaluate(previous.as_ref(), paths)?;
            snapshot.write(&state_path)?;
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
            let mut access = vec![preflight_access_scope(
                write_probe_path(&state_path),
                AccessIntent::Write,
                "fileprovider invalidation event",
            )?];
            let previous = if state_path.is_file() {
                Some(FileProviderStateSnapshot::read(&state_path)?)
            } else {
                None
            };
            access.extend(retain_fileprovider_event_access(
                &event,
                previous.as_ref(),
                "fileprovider invalidation event",
            )?);
            let (observed, snapshot) =
                FileProviderObservedInvalidation::evaluate(previous.as_ref(), [event])?;
            snapshot.write(&state_path)?;
            println!("{}", observed.as_tsv());
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
            let _root_access =
                preflight_access_scope(&root, AccessIntent::Index, "fileprovider observer root")?;
            let _target_access = preflight_access_scope(
                &target,
                AccessIntent::Write,
                "fileprovider observer target",
            )?;
            let _state_access = retain_fileprovider_snapshot_access(
                &state_path,
                std::slice::from_ref(&target),
                "fileprovider observer state",
            )?;
            let previous = if state_path.is_file() {
                Some(FileProviderStateSnapshot::read(&state_path)?)
            } else {
                None
            };
            let mut observer =
                FileProviderStateObserver::watch(&[WatchRoot::tree(root)], previous)?;
            std::fs::write(&target, b"observer-probe").map_err(|err| GfmError::io(&target, err))?;
            let observed = drain_fileprovider_observer_probe(&mut observer)?;
            let snapshot = FileProviderStateSnapshot::from_paths(observed.paths.clone())?;
            snapshot.write(&state_path)?;
            println!("{}", observed.as_tsv());
        }
        "volume-discovery" => {
            let paths: Vec<PathBuf> = args.map(PathBuf::from).collect();
            let report = volume_discovery_report(paths);
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
        "volume-event-invalidation" => {
            let kind = parse_volume_event_kind(&required_string(
                args.next(),
                "volume-event-invalidation requires an event kind",
            )?)?;
            let path = args.next().map(PathBuf::from);
            let descriptor = path
                .as_ref()
                .filter(|path| path.exists())
                .map(VolumeDescriptor::for_path)
                .transpose()?;
            let native_status = if descriptor.is_some() {
                gfm_mac::NativeVolumeStatus::Available
            } else if path.is_some() {
                gfm_mac::NativeVolumeStatus::Missing
            } else {
                gfm_mac::NativeVolumeStatus::Unavailable
            };
            println!(
                "{}",
                VolumeEventInvalidationReport::from_parts(
                    kind,
                    native_status,
                    path,
                    descriptor.as_ref(),
                    None,
                )
                .as_tsv()
            );
        }
        "volume-event-index-invalidation" => {
            println!(
                "{}",
                volume_event_index_invalidation_from_args(args)?.as_tsv()
            );
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
        "volume-operation" => {
            let operation = VolumeOperation::parse(&required_string(
                args.next(),
                "volume-operation requires an operation",
            )?)?;
            let path = required_path(args.next(), "volume-operation requires a path")?;
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
            let volumes = volume_discovery_report(paths)
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
            let current = VolumeDiscoveryReport::from_paths(vec![previous_path.clone()])
                .volumes
                .into_iter()
                .next()
                .map(|volume| index_volume_descriptor(&volume));
            let previous = IndexVolumeDescriptor::new(
                previous_path
                    .file_name()
                    .and_then(|label| label.to_str())
                    .unwrap_or("Volume"),
                previous_path.clone(),
                previous_class,
                previous_mount,
            );
            println!(
                "{}",
                VolumeInvalidationReport::evaluate(Some(&previous), current.as_ref()).as_tsv()
            );
        }
        "volume-topology-diff" => {
            let (previous_paths, current_paths) = split_topology_paths(args)?;
            let previous = VolumeDiscoveryReport::from_paths(previous_paths);
            let current = VolumeDiscoveryReport::from_paths(current_paths);
            println!(
                "{}",
                VolumeTopologyDiff::evaluate(&previous, &current).as_tsv()
            );
        }
        "spotlight-reconcile" => {
            let path = required_path(args.next(), "spotlight-reconcile requires a path")?;
            let fixture_path = args.next().map(PathBuf::from);
            let record =
                record_for_path_with_access(&path, AccessIntent::Index, "spotlight reconcile")?;
            let snapshot = match fixture_path {
                Some(fixture_path) => {
                    let _fixture_access = preflight_access_scope(
                        &fixture_path,
                        AccessIntent::Read,
                        "spotlight fixture",
                    )?;
                    let text = std::fs::read_to_string(&fixture_path)
                        .map_err(|err| GfmError::io(&fixture_path, err))?;
                    parse_spotlight_fixture(&path, &text)?
                }
                None => SpotlightMetadataReader.read_path(&path)?,
            };
            println!(
                "{}",
                SpotlightReconciliationReport::reconcile(record, snapshot).as_tsv()
            );
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
            let record = record_for_path_with_access(&path, AccessIntent::Preview, "icon preview")?;
            let input = IconPreviewInput::new(
                PreviewRequestKey::new(record.id, path.clone(), PreviewKind::Icon),
                record,
            )
            .with_invalidation(PreviewInvalidationEvent {
                tags_changed: true,
                ..PreviewInvalidationEvent::default()
            });
            println!("{}", IconPreviewContract::from_input(input).as_tsv());
        }
        "quicklook-session" => {
            let path = required_path(args.next(), "quicklook-session requires a path")?;
            let _access =
                preflight_access_scope(&path, AccessIntent::Preview, "quicklook preview")?;
            let record = record_for_path(&path, None, false)?;
            let rect = Rect::new(0, 0, 640, 480);
            let viewport = Viewport::new(Rect::new(0, 0, 1024, 768), 256);
            let volume = detect_volume_id(&path).ok();
            let contract = run_preview_contract_cancellable(
                volume,
                "quicklook preview",
                move |cancellation| {
                    cancellation.check()?;
                    let cloud = FileProviderStateReport::read_path(&path)?.materialization;
                    let input = QuickLookSessionInput::new(
                        PreviewRequestKey::new(record.id, path.clone(), PreviewKind::QuickLook),
                        rect,
                        viewport,
                    )
                    .with_cloud_materialization(cloud)
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
            )?;
            println!("{}", contract.as_tsv());
        }
        "quicklook-session-adaptive" => {
            let path = required_path(args.next(), "quicklook-session-adaptive requires a path")?;
            let pressure = parse_required_scheduling_pressure(args, "quicklook preview")?;
            let _access =
                preflight_access_scope(&path, AccessIntent::Preview, "adaptive quicklook preview")?;
            let record = record_for_path(&path, None, false)?;
            let rect = Rect::new(0, 0, 640, 480);
            let viewport = Viewport::new(Rect::new(0, 0, 1024, 768), 256);
            let volume = detect_volume_id(&path).ok();
            let outcome = run_preview_contract_adaptive(
                volume,
                Priority::Visible,
                "quicklook preview",
                pressure,
                move |cancellation| {
                    cancellation.check()?;
                    let cloud = FileProviderStateReport::read_path(&path)?.materialization;
                    let input = QuickLookSessionInput::new(
                        PreviewRequestKey::new(record.id, path.clone(), PreviewKind::QuickLook),
                        rect,
                        viewport,
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
            )?;
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
            let record = record_for_path(&path, None, false)?;
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
            let _access =
                preflight_access_scope(&path, AccessIntent::Preview, "thumbnail generation")?;
            let record = record_for_path(&path, None, false)?;
            let rect = Rect::new(0, 0, 160, 160);
            let viewport = Viewport::new(Rect::new(0, 0, 1024, 768), 256);
            let volume = detect_volume_id(&path).ok();
            let contract = run_preview_contract_cancellable(
                volume,
                "thumbnail generation",
                move |cancellation| {
                    cancellation.check()?;
                    let cloud = FileProviderStateReport::read_path(&path)?.materialization;
                    let input = ThumbnailGenerationInput::new(
                        PreviewRequestKey::new(record.id, path.clone(), PreviewKind::Thumbnail),
                        rect,
                        viewport,
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
                },
            )?;
            println!("{}", contract.as_tsv());
        }
        "thumbnail-generation-adaptive" => {
            let path = required_path(args.next(), "thumbnail-generation-adaptive requires a path")?;
            let pressure = parse_required_scheduling_pressure(args, "thumbnail generation")?;
            let _access = preflight_access_scope(
                &path,
                AccessIntent::Preview,
                "adaptive thumbnail generation",
            )?;
            let record = record_for_path(&path, None, false)?;
            let rect = Rect::new(0, 0, 160, 160);
            let viewport = Viewport::new(Rect::new(0, 0, 1024, 768), 256);
            let volume = detect_volume_id(&path).ok();
            let outcome = run_preview_contract_adaptive(
                volume,
                Priority::Background,
                "thumbnail generation",
                pressure,
                move |cancellation| {
                    cancellation.check()?;
                    let cloud = FileProviderStateReport::read_path(&path)?.materialization;
                    let input = ThumbnailGenerationInput::new(
                        PreviewRequestKey::new(record.id, path.clone(), PreviewKind::Thumbnail),
                        rect,
                        viewport,
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
            )?;
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
            let record = record_for_path(&path, None, false)?;
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

fn publish_fileprovider_progress_job(path: PathBuf) -> Result<FileProviderProgressReport> {
    let report = FileProviderProgressReport::read_path(&path)?;
    let mut scheduler = Scheduler::new();
    let label = fileprovider_progress_label(report.state.progress.direction);
    let volume = detect_volume_id(&path).ok();
    let job = if let Some(volume) = volume {
        scheduler.schedule_on_volume_in_class(Priority::Visible, JobClass::Visible, label, volume)
    } else {
        scheduler.schedule_in_class(Priority::Visible, JobClass::Visible, label)
    };
    let detail = fileprovider_progress_detail(&report);
    let runtime = RuntimeJobHandle::begin(
        &job,
        JobPayloadKind::Operation,
        label,
        fileprovider_progress_total_units(&report),
        detail.clone(),
    )?;
    runtime.progress(
        fileprovider_progress_job_state(&report),
        u64::from(report.state.progress.percent_milli.unwrap_or(0)),
        detail,
    )?;
    Ok(report)
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
    let descriptor = path
        .as_ref()
        .filter(|path| path.exists())
        .map(VolumeDescriptor::for_path)
        .transpose()?;
    let native_status = if descriptor.is_some() {
        gfm_mac::NativeVolumeStatus::Available
    } else if path.is_some() {
        gfm_mac::NativeVolumeStatus::Missing
    } else {
        gfm_mac::NativeVolumeStatus::Unavailable
    };
    let event_report = VolumeEventInvalidationReport::from_parts(
        kind,
        native_status,
        path.clone(),
        descriptor.as_ref(),
        None,
    );
    let current = descriptor.as_ref().map(index_volume_descriptor);
    Ok(VolumeEventIndexInvalidationReport::from_event(
        index_volume_event_kind(kind),
        path,
        current.as_ref(),
        event_report.invalidate_index_admission,
        event_report.rescan_index,
    ))
}

fn runtime_volume_cancellation(
    report: &VolumeEventIndexInvalidationReport,
) -> Option<gfm_jobs::VolumeCancellationReport> {
    if !report.cancel_index_jobs {
        return None;
    }
    let volume = report.current_volume_id?;
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
            | CloudStorageState::Unknown => JobProgressState::Paused,
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

fn volume_discovery_report(paths: Vec<PathBuf>) -> VolumeDiscoveryReport {
    if paths.is_empty() {
        VolumeDiscoveryReport::discover()
    } else {
        VolumeDiscoveryReport::from_paths(paths)
    }
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

fn drain_fileprovider_observer_probe(
    observer: &mut FileProviderStateObserver,
) -> Result<FileProviderObservedInvalidation> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Some(observed) = observer.drain_available(64)? {
            if !observed.paths.is_empty() {
                return Ok(observed);
            }
        }
        thread::sleep(Duration::from_millis(25));
    }
    Err(GfmError::Format(
        "fileprovider observer probe timed out waiting for a provider event".to_string(),
    ))
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

fn index_volume_event_kind(kind: VolumeEventKind) -> IndexVolumeEventKind {
    match kind {
        VolumeEventKind::Appeared => IndexVolumeEventKind::Appeared,
        VolumeEventKind::DescriptionChanged => IndexVolumeEventKind::DescriptionChanged,
        VolumeEventKind::Disappeared => IndexVolumeEventKind::Disappeared,
        VolumeEventKind::Unavailable => IndexVolumeEventKind::Unavailable,
    }
}

fn retain_fileprovider_snapshot_access(
    state_path: &Path,
    paths: &[PathBuf],
    worker: &str,
) -> Result<Vec<ScopedAccessGuard>> {
    let mut guards = Vec::with_capacity(paths.len() + 1);
    guards.push(preflight_access_scope(
        write_probe_path(state_path),
        AccessIntent::Write,
        worker,
    )?);
    for path in paths {
        guards.push(preflight_access_scope(path, AccessIntent::Read, worker)?);
    }
    Ok(guards)
}

fn retain_fileprovider_event_access(
    event: &FileEvent,
    previous: Option<&FileProviderStateSnapshot>,
    worker: &str,
) -> Result<Vec<ScopedAccessGuard>> {
    let mut guards = Vec::new();
    for path in fileprovider_event_access_paths(event, previous) {
        guards.push(preflight_access_scope(&path, AccessIntent::Read, worker)?);
    }
    Ok(guards)
}

fn fileprovider_event_access_paths(
    event: &FileEvent,
    previous: Option<&FileProviderStateSnapshot>,
) -> Vec<PathBuf> {
    match &event.kind {
        FileEventKind::Rename { from, to } => [from, to]
            .into_iter()
            .map(|path| fileprovider_event_access_path(path, previous))
            .collect(),
        FileEventKind::Remove => vec![fileprovider_event_access_path(&event.path, previous)],
        FileEventKind::Create
        | FileEventKind::Metadata
        | FileEventKind::Modify
        | FileEventKind::Rescan
        | FileEventKind::Other => vec![event.path.clone()],
    }
}

fn fileprovider_event_access_path(
    path: &Path,
    previous: Option<&FileProviderStateSnapshot>,
) -> PathBuf {
    if path.exists() || !snapshot_contains_path(previous, path) {
        return path.to_path_buf();
    }
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(path)
        .to_path_buf()
}

fn snapshot_contains_path(previous: Option<&FileProviderStateSnapshot>, path: &Path) -> bool {
    previous.is_some_and(|snapshot| snapshot.entries.iter().any(|entry| entry.path == path))
}

fn write_probe_path(path: &Path) -> &Path {
    if path.is_dir() {
        return path;
    }
    path.parent().unwrap_or(path)
}

fn record_for_path_with_access(
    path: &Path,
    intent: AccessIntent,
    worker: &str,
) -> Result<FileRecord> {
    let _access = preflight_access_scope(path, intent, worker)?;
    record_for_path(path, None, false)
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
