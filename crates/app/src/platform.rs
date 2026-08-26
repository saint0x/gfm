use crate::{detect_volume_id, index_volume_descriptor, run_preview_contract};
use gfm_fs::record_for_path;
use gfm_index::{parse_volume_indexing_policy, VolumeIndexPolicy};
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
                None => SpotlightMetadataReader::default().read_path(&path)?,
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
            let contract = run_preview_contract(volume, "quicklook preview", move || {
                QuickLookSessionContract::from_input(&PreviewSecurityPolicy::default(), input)
            })?;
            println!("{}", contract.as_tsv());
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
            let contract = run_preview_contract(volume, "thumbnail generation", move || {
                ThumbnailGenerationContract::from_input(&PreviewSecurityPolicy::default(), input)
            })?;
            println!("{}", contract.as_tsv());
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
