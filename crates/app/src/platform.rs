use crate::{
    detect_volume_id, index_volume_descriptor, parse_required_scheduling_pressure,
    run_preview_contract_adaptive, run_preview_contract_cancellable,
};
use gfm_fs::record_for_path;
use gfm_index::{parse_volume_indexing_policy, VolumeIndexPolicy};
use gfm_jobs::{Cancellation, Priority, SchedulingAction};
use gfm_mac::{
    parse_spotlight_fixture, AccessIntent, FileProviderStateReport, MacBridgeContract,
    NativeIconDescriptor, SecurityScopedAccessReport, SpotlightMetadataReader,
    SpotlightReconciliationReport, VolumeDiscoveryReport,
};
use gfm_preview::{
    decide_invalidation, decide_preview_security, security_input_for_path,
    PreviewInvalidationEvent, PreviewKind, PreviewRequestKey, PreviewScheduler,
    PreviewSchedulingPolicy, PreviewSecurityPolicy, PreviewTask, QuickLookSessionContract,
    QuickLookSessionInput, Rect, ThumbnailGenerationContract, ThumbnailGenerationInput, Viewport,
};
use gfm_types::{FileId, GfmError, Result, VolumeId};
use std::path::PathBuf;

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
        "mac-bridges" => {
            println!("{}", MacBridgeContract::finder_required().as_tsv());
        }
        "native-icon" => {
            let path = required_path(args.next(), "native-icon requires a path")?;
            let record = record_for_path(&path, None, false)?;
            println!("{}", NativeIconDescriptor::for_record(&record).as_tsv());
        }
        "fileprovider-state" => {
            let path = required_path(args.next(), "fileprovider-state requires a path")?;
            println!("{}", FileProviderStateReport::read_path(path)?.as_tsv());
        }
        "volume-discovery" => {
            let paths: Vec<PathBuf> = args.map(PathBuf::from).collect();
            let report = volume_discovery_report(paths);
            println!("{}", report.as_tsv());
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
        "spotlight-reconcile" => {
            let path = required_path(args.next(), "spotlight-reconcile requires a path")?;
            let fixture_path = args.next().map(PathBuf::from);
            let record = record_for_path(&path, None, false)?;
            let snapshot = match fixture_path {
                Some(fixture_path) => {
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
        "quicklook-session" => {
            let path = required_path(args.next(), "quicklook-session requires a path")?;
            let record = record_for_path(&path, None, false)?;
            let rect = Rect::new(0, 0, 640, 480);
            let viewport = Viewport::new(Rect::new(0, 0, 1024, 768), 256);
            let volume = detect_volume_id(&path).ok();
            let input = QuickLookSessionInput::new(
                PreviewRequestKey::new(record.id, path.clone(), PreviewKind::QuickLook),
                rect,
                viewport,
            )
            .with_invalidation(PreviewInvalidationEvent {
                content_changed: true,
                ..PreviewInvalidationEvent::default()
            });
            let contract = run_preview_contract_cancellable(
                volume,
                "quicklook preview",
                move |cancellation| {
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
            let record = record_for_path(&path, None, false)?;
            let rect = Rect::new(0, 0, 640, 480);
            let viewport = Viewport::new(Rect::new(0, 0, 1024, 768), 256);
            let volume = detect_volume_id(&path).ok();
            let input = QuickLookSessionInput::new(
                PreviewRequestKey::new(record.id, path.clone(), PreviewKind::QuickLook),
                rect,
                viewport,
            )
            .with_invalidation(PreviewInvalidationEvent {
                content_changed: true,
                ..PreviewInvalidationEvent::default()
            });
            let outcome = run_preview_contract_adaptive(
                volume,
                Priority::Visible,
                "quicklook preview",
                pressure,
                move |cancellation| {
                    QuickLookSessionContract::from_input_checked(
                        &PreviewSecurityPolicy::default(),
                        input.clone(),
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
            let record = record_for_path(&path, None, false)?;
            let cancellation = Cancellation::default();
            cancellation.cancel();
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
            let record = record_for_path(&path, None, false)?;
            let rect = Rect::new(0, 0, 160, 160);
            let viewport = Viewport::new(Rect::new(0, 0, 1024, 768), 256);
            let volume = detect_volume_id(&path).ok();
            let input = ThumbnailGenerationInput::new(
                PreviewRequestKey::new(record.id, path.clone(), PreviewKind::Thumbnail),
                rect,
                viewport,
            )
            .with_size(512, 2_000)
            .with_invalidation(PreviewInvalidationEvent {
                metadata_changed: true,
                ..PreviewInvalidationEvent::default()
            });
            let contract = run_preview_contract_cancellable(
                volume,
                "thumbnail generation",
                move |cancellation| {
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
            let record = record_for_path(&path, None, false)?;
            let rect = Rect::new(0, 0, 160, 160);
            let viewport = Viewport::new(Rect::new(0, 0, 1024, 768), 256);
            let volume = detect_volume_id(&path).ok();
            let input = ThumbnailGenerationInput::new(
                PreviewRequestKey::new(record.id, path.clone(), PreviewKind::Thumbnail),
                rect,
                viewport,
            )
            .with_size(512, 2_000)
            .with_invalidation(PreviewInvalidationEvent {
                metadata_changed: true,
                ..PreviewInvalidationEvent::default()
            });
            let outcome = run_preview_contract_adaptive(
                volume,
                Priority::Background,
                "thumbnail generation",
                pressure,
                move |cancellation| {
                    ThumbnailGenerationContract::from_input_checked(
                        &PreviewSecurityPolicy::default(),
                        input.clone(),
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
            let record = record_for_path(&path, None, false)?;
            let cancellation = Cancellation::default();
            cancellation.cancel();
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

fn volume_discovery_report(paths: Vec<PathBuf>) -> VolumeDiscoveryReport {
    if paths.is_empty() {
        VolumeDiscoveryReport::discover()
    } else {
        VolumeDiscoveryReport::from_paths(paths)
    }
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
