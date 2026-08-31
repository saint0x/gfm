use crate::{
    cloud_materialization_for_state, decide_cloud_preview_for_materialization, decide_invalidation,
    decide_preview_security, preview_invalidation_for_fileprovider, security_input_for_path,
    volume_descriptor_is_remote_for_preview, CloudPreviewDecision, PreviewInvalidationDecision,
    PreviewInvalidationEvent, PreviewKind, PreviewRequestKey, PreviewScheduler,
    PreviewSchedulingPolicy, PreviewSecurityDecision, PreviewSecurityPolicy, PreviewTask,
    PreviewTaskDecision, Rect, Viewport,
};
use gfm_mac::{CloudMaterialization, CloudStorageState, VolumeDescriptor};
use gfm_types::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbnailGeneratorMode {
    QuickLookThumbnailing,
    SandboxedGenerator,
    MetadataOnly,
    Denied,
}

impl ThumbnailGeneratorMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::QuickLookThumbnailing => "quicklook-thumbnailing",
            Self::SandboxedGenerator => "sandboxed-generator",
            Self::MetadataOnly => "metadata-only",
            Self::Denied => "denied",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbnailCacheDisposition {
    ReadThrough,
    RefreshMemoryAndDisk,
    RefreshMemoryOnly,
    Bypass,
}

impl ThumbnailCacheDisposition {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadThrough => "read-through",
            Self::RefreshMemoryAndDisk => "refresh-memory-and-disk",
            Self::RefreshMemoryOnly => "refresh-memory-only",
            Self::Bypass => "bypass",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThumbnailGenerationInput {
    pub key: PreviewRequestKey,
    pub rect: Rect,
    pub viewport: Viewport,
    pub scheduling_policy: PreviewSchedulingPolicy,
    pub is_remote: bool,
    pub max_pixel_size: u16,
    pub scale_factor_milli: u16,
    pub invalidation_event: PreviewInvalidationEvent,
    pub cloud_state: CloudStorageState,
    pub cloud_materialization: CloudMaterialization,
}

impl ThumbnailGenerationInput {
    pub fn new(key: PreviewRequestKey, rect: Rect, viewport: Viewport) -> Self {
        Self {
            key,
            rect,
            viewport,
            scheduling_policy: PreviewSchedulingPolicy::default(),
            is_remote: false,
            max_pixel_size: 512,
            scale_factor_milli: 2_000,
            invalidation_event: PreviewInvalidationEvent::default(),
            cloud_state: CloudStorageState::LocalOnly,
            cloud_materialization: CloudMaterialization::NotProviderBacked,
        }
    }

    pub fn with_size(mut self, max_pixel_size: u16, scale_factor_milli: u16) -> Self {
        self.max_pixel_size = max_pixel_size.max(1);
        self.scale_factor_milli = scale_factor_milli.max(1);
        self.key.pixel_size = self.max_pixel_size;
        self.key.scale_factor_milli = self.scale_factor_milli;
        self
    }

    pub fn with_invalidation(mut self, event: PreviewInvalidationEvent) -> Self {
        self.invalidation_event = event;
        self
    }

    pub fn with_fileprovider_invalidation(
        mut self,
        report: &gfm_mac::FileProviderInvalidationReport,
    ) -> Self {
        self.invalidation_event = preview_invalidation_for_fileprovider(report);
        self.cloud_state = report.current.storage_state;
        self.cloud_materialization = report.current.materialization;
        self
    }

    pub fn with_scheduling_policy(mut self, policy: PreviewSchedulingPolicy) -> Self {
        self.scheduling_policy = policy;
        self
    }

    pub fn with_volume_descriptor(mut self, volume: Option<&VolumeDescriptor>) -> Self {
        self.is_remote = volume.is_some_and(volume_descriptor_is_remote_for_preview);
        self
    }

    pub fn with_cloud_state(mut self, state: CloudStorageState) -> Self {
        self.cloud_state = state;
        self.cloud_materialization = cloud_materialization_for_state(state);
        self
    }

    pub fn with_cloud_materialization(mut self, materialization: CloudMaterialization) -> Self {
        self.cloud_materialization = materialization;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThumbnailGenerationContract {
    pub key: PreviewRequestKey,
    pub generator_mode: ThumbnailGeneratorMode,
    pub security: PreviewSecurityDecision,
    pub cloud: CloudPreviewDecision,
    pub invalidation: PreviewInvalidationDecision,
    pub cache_disposition: ThumbnailCacheDisposition,
    pub schedule_decision: PreviewTaskDecision,
    pub max_pixel_size: u16,
    pub scale_factor_milli: u16,
}

impl ThumbnailGenerationContract {
    pub fn from_input(
        policy: &PreviewSecurityPolicy,
        input: ThumbnailGenerationInput,
    ) -> Result<Self> {
        Self::from_input_checked(policy, input, || Ok(()))
    }

    pub fn from_input_checked(
        policy: &PreviewSecurityPolicy,
        input: ThumbnailGenerationInput,
        mut check: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        check()?;
        let mut security_input = security_input_for_path(&input.key.path, PreviewKind::Thumbnail);
        security_input.is_remote |= input.is_remote;
        check()?;
        let security = decide_preview_security(policy, &security_input);
        let cloud = decide_cloud_preview_for_materialization(input.cloud_materialization);
        let generator_mode = generator_mode(security, cloud);
        let invalidation = decide_invalidation(input.invalidation_event);
        let cache_disposition = cache_disposition(generator_mode, &invalidation);
        check()?;
        let schedule_decision = match (cloud, generator_mode) {
            (CloudPreviewDecision::Defer, _) => PreviewTaskDecision::Cancelled {
                key: input.key.clone(),
                reason: "fileprovider-in-flight",
            },
            (CloudPreviewDecision::Unavailable, _) => PreviewTaskDecision::Cancelled {
                key: input.key.clone(),
                reason: "fileprovider-unavailable",
            },
            (CloudPreviewDecision::MetadataOnly, _) => PreviewTaskDecision::Cancelled {
                key: input.key.clone(),
                reason: "metadata-only",
            },
            (_, ThumbnailGeneratorMode::MetadataOnly) => PreviewTaskDecision::Cancelled {
                key: input.key.clone(),
                reason: "metadata-only",
            },
            (
                CloudPreviewDecision::NativeEligible,
                ThumbnailGeneratorMode::QuickLookThumbnailing
                | ThumbnailGeneratorMode::SandboxedGenerator,
            ) => {
                let mut scheduler = PreviewScheduler::new(input.scheduling_policy)?;
                check()?;
                scheduler
                    .schedule_checked(
                        input.viewport,
                        [PreviewTask::new(input.key.clone(), input.rect)],
                        &mut check,
                    )?
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| PreviewTaskDecision::Cancelled {
                        key: input.key.clone(),
                        reason: "outside-thumbnail-budget",
                    })
            }
            (_, ThumbnailGeneratorMode::Denied) => PreviewTaskDecision::Cancelled {
                key: input.key.clone(),
                reason: "denied",
            },
        };
        check()?;

        Ok(Self {
            key: input.key,
            generator_mode,
            security,
            cloud,
            invalidation,
            cache_disposition,
            schedule_decision,
            max_pixel_size: input.max_pixel_size,
            scale_factor_milli: input.scale_factor_milli,
        })
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "thumbnail-generation\t{}\t{}\tcloud={}\t{}\t{}px\tscale={}m\t{}:{}\tcache={}\tinvalidate-memory={}\tinvalidate-disk={}\tschedule={}",
            self.key.path.display(),
            self.security.as_str(),
            self.cloud.as_str(),
            self.generator_mode.as_str(),
            self.max_pixel_size,
            self.scale_factor_milli,
            self.key.file_id.volume.0,
            self.key.file_id.node,
            self.cache_disposition.as_str(),
            self.invalidation.invalidate_memory,
            self.invalidation.invalidate_disk,
            schedule_tsv(&self.schedule_decision)
        )
    }
}

fn generator_mode(
    security: PreviewSecurityDecision,
    cloud: CloudPreviewDecision,
) -> ThumbnailGeneratorMode {
    match cloud {
        CloudPreviewDecision::MetadataOnly => return ThumbnailGeneratorMode::MetadataOnly,
        CloudPreviewDecision::Defer => return ThumbnailGeneratorMode::MetadataOnly,
        CloudPreviewDecision::Unavailable => return ThumbnailGeneratorMode::Denied,
        CloudPreviewDecision::NativeEligible => {}
    }
    match security {
        PreviewSecurityDecision::AllowNative => ThumbnailGeneratorMode::QuickLookThumbnailing,
        PreviewSecurityDecision::Sandbox => ThumbnailGeneratorMode::SandboxedGenerator,
        PreviewSecurityDecision::MetadataOnly => ThumbnailGeneratorMode::MetadataOnly,
        PreviewSecurityDecision::Deny => ThumbnailGeneratorMode::Denied,
    }
}

fn cache_disposition(
    mode: ThumbnailGeneratorMode,
    invalidation: &PreviewInvalidationDecision,
) -> ThumbnailCacheDisposition {
    if matches!(
        mode,
        ThumbnailGeneratorMode::Denied | ThumbnailGeneratorMode::MetadataOnly
    ) {
        ThumbnailCacheDisposition::Bypass
    } else if invalidation.invalidate_disk {
        ThumbnailCacheDisposition::RefreshMemoryAndDisk
    } else if invalidation.invalidate_memory {
        ThumbnailCacheDisposition::RefreshMemoryOnly
    } else {
        ThumbnailCacheDisposition::ReadThrough
    }
}

fn schedule_tsv(decision: &PreviewTaskDecision) -> String {
    match decision {
        PreviewTaskDecision::Scheduled { priority, .. }
        | PreviewTaskDecision::Coalesced { priority, .. } => {
            format!("{}:{}", decision.as_str(), priority.as_str())
        }
        PreviewTaskDecision::Cancelled { reason, .. } => {
            format!("{}:{reason}", decision.as_str())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfm_types::{FileId, VolumeId};
    use std::path::PathBuf;

    #[test]
    fn trusted_documents_use_quicklook_thumbnailing() {
        let contract = ThumbnailGenerationContract::from_input(
            &PreviewSecurityPolicy::default(),
            input("Image.png", Rect::new(0, 0, 128, 128)),
        )
        .unwrap();

        assert_eq!(contract.security, PreviewSecurityDecision::AllowNative);
        assert_eq!(
            contract.generator_mode,
            ThumbnailGeneratorMode::QuickLookThumbnailing
        );
        assert_eq!(
            contract.cache_disposition,
            ThumbnailCacheDisposition::ReadThrough
        );
    }

    #[test]
    fn executable_packages_are_metadata_only_for_thumbnails() {
        let contract = ThumbnailGenerationContract::from_input(
            &PreviewSecurityPolicy::default(),
            input("Installer.pkg", Rect::new(0, 0, 128, 128)),
        )
        .unwrap();

        assert_eq!(contract.security, PreviewSecurityDecision::MetadataOnly);
        assert_eq!(
            contract.generator_mode,
            ThumbnailGeneratorMode::MetadataOnly
        );
        assert_eq!(
            contract.cache_disposition,
            ThumbnailCacheDisposition::Bypass
        );
        assert!(matches!(
            contract.schedule_decision,
            PreviewTaskDecision::Cancelled {
                reason: "metadata-only",
                ..
            }
        ));
    }

    #[test]
    fn descriptor_remote_untrusted_items_are_denied_for_thumbnails() {
        let volume = remote_volume();
        let contract = ThumbnailGenerationContract::from_input(
            &PreviewSecurityPolicy::default(),
            input("Installer.dmg", Rect::new(0, 0, 128, 128)).with_volume_descriptor(Some(&volume)),
        )
        .unwrap();

        assert_eq!(contract.security, PreviewSecurityDecision::Deny);
        assert_eq!(contract.generator_mode, ThumbnailGeneratorMode::Denied);
        assert_eq!(
            contract.cache_disposition,
            ThumbnailCacheDisposition::Bypass
        );
        assert!(matches!(
            contract.schedule_decision,
            PreviewTaskDecision::Cancelled {
                reason: "denied",
                ..
            }
        ));
    }

    #[test]
    fn content_changes_refresh_memory_and_disk() {
        let contract = ThumbnailGenerationContract::from_input(
            &PreviewSecurityPolicy::default(),
            input("Image.png", Rect::new(0, 0, 128, 128)).with_invalidation(
                PreviewInvalidationEvent {
                    content_changed: true,
                    ..PreviewInvalidationEvent::default()
                },
            ),
        )
        .unwrap();

        assert_eq!(
            contract.cache_disposition,
            ThumbnailCacheDisposition::RefreshMemoryAndDisk
        );
    }

    #[test]
    fn pressure_policy_can_drop_offscreen_thumbnail_prefetch() {
        let contract = ThumbnailGenerationContract::from_input(
            &PreviewSecurityPolicy::default(),
            input("Image.png", Rect::new(0, 900, 128, 128)).with_scheduling_policy(
                PreviewSchedulingPolicy::default().adapted_for_pressure(
                    gfm_jobs::SchedulingPressure {
                        io: gfm_jobs::JobIoPressure::Saturated,
                        ..gfm_jobs::SchedulingPressure::default()
                    },
                ),
            ),
        )
        .unwrap();

        assert!(matches!(
            contract.schedule_decision,
            PreviewTaskDecision::Cancelled {
                reason: "outside-thumbnail-budget",
                ..
            }
        ));
    }

    #[test]
    fn evicted_fileprovider_items_are_metadata_only_for_thumbnails() {
        let contract = ThumbnailGenerationContract::from_input(
            &PreviewSecurityPolicy::default(),
            input("Remote.icloud", Rect::new(0, 0, 128, 128))
                .with_cloud_state(CloudStorageState::Evicted),
        )
        .unwrap();

        assert_eq!(contract.cloud, CloudPreviewDecision::MetadataOnly);
        assert_eq!(
            contract.generator_mode,
            ThumbnailGeneratorMode::MetadataOnly
        );
        assert_eq!(
            contract.cache_disposition,
            ThumbnailCacheDisposition::Bypass
        );
        assert!(matches!(
            contract.schedule_decision,
            PreviewTaskDecision::Cancelled {
                reason: "metadata-only",
                ..
            }
        ));
    }

    #[test]
    fn unknown_fileprovider_items_are_metadata_only_for_thumbnails() {
        let contract = ThumbnailGenerationContract::from_input(
            &PreviewSecurityPolicy::default(),
            input("Unknown.icloud", Rect::new(0, 0, 128, 128))
                .with_cloud_state(CloudStorageState::Unknown),
        )
        .unwrap();

        assert_eq!(contract.cloud, CloudPreviewDecision::MetadataOnly);
        assert_eq!(
            contract.generator_mode,
            ThumbnailGeneratorMode::MetadataOnly
        );
        assert_eq!(
            contract.cache_disposition,
            ThumbnailCacheDisposition::Bypass
        );
        assert!(matches!(
            contract.schedule_decision,
            PreviewTaskDecision::Cancelled {
                reason: "metadata-only",
                ..
            }
        ));
    }

    #[test]
    fn in_flight_fileprovider_items_defer_thumbnails() {
        let contract = ThumbnailGenerationContract::from_input(
            &PreviewSecurityPolicy::default(),
            input("Downloading.icloud", Rect::new(0, 0, 128, 128))
                .with_cloud_state(CloudStorageState::Downloading),
        )
        .unwrap();

        assert_eq!(contract.cloud, CloudPreviewDecision::Defer);
        assert_eq!(
            contract.generator_mode,
            ThumbnailGeneratorMode::MetadataOnly
        );
        assert!(matches!(
            contract.schedule_decision,
            PreviewTaskDecision::Cancelled {
                reason: "fileprovider-in-flight",
                ..
            }
        ));
    }

    #[test]
    fn offline_fileprovider_items_bypass_thumbnail_cache() {
        let contract = ThumbnailGenerationContract::from_input(
            &PreviewSecurityPolicy::default(),
            input("Offline.icloud", Rect::new(0, 0, 128, 128))
                .with_cloud_state(CloudStorageState::Offline),
        )
        .unwrap();

        assert_eq!(contract.cloud, CloudPreviewDecision::Unavailable);
        assert_eq!(contract.generator_mode, ThumbnailGeneratorMode::Denied);
        assert_eq!(
            contract.cache_disposition,
            ThumbnailCacheDisposition::Bypass
        );
        assert!(matches!(
            contract.schedule_decision,
            PreviewTaskDecision::Cancelled {
                reason: "fileprovider-unavailable",
                ..
            }
        ));
    }

    #[test]
    fn fileprovider_invalidation_updates_thumbnail_cloud_and_cache_state() {
        let contract = ThumbnailGenerationContract::from_input(
            &PreviewSecurityPolicy::default(),
            input("Remote.icloud", Rect::new(0, 0, 128, 128)).with_fileprovider_invalidation(
                &fileprovider_report(CloudStorageState::Downloaded, CloudStorageState::Evicted),
            ),
        )
        .unwrap();

        assert_eq!(contract.cloud, CloudPreviewDecision::MetadataOnly);
        assert_eq!(
            contract.generator_mode,
            ThumbnailGeneratorMode::MetadataOnly
        );
        assert_eq!(contract.invalidation.reason, "content-or-icloud");
        assert_eq!(
            contract.cache_disposition,
            ThumbnailCacheDisposition::Bypass
        );
        assert!(contract.invalidation.invalidate_memory);
        assert!(contract.invalidation.invalidate_disk);
        assert!(matches!(
            contract.schedule_decision,
            PreviewTaskDecision::Cancelled {
                reason: "metadata-only",
                ..
            }
        ));
    }

    #[test]
    fn checked_contract_honors_pre_cancelled_work() {
        let err = ThumbnailGenerationContract::from_input_checked(
            &PreviewSecurityPolicy::default(),
            input("Image.png", Rect::new(0, 0, 128, 128)),
            || Err(gfm_types::GfmError::Cancelled),
        )
        .expect_err("pre-cancelled thumbnail contract fails before planning");

        assert!(matches!(err, gfm_types::GfmError::Cancelled));
    }

    #[test]
    fn tsv_output_is_stable_for_cli_and_fozzy() {
        let contract = ThumbnailGenerationContract::from_input(
            &PreviewSecurityPolicy::default(),
            input("Image.png", Rect::new(0, 0, 128, 128))
                .with_size(256, 2_000)
                .with_invalidation(PreviewInvalidationEvent {
                    tags_changed: true,
                    ..PreviewInvalidationEvent::default()
                }),
        )
        .unwrap();

        assert_eq!(
            contract.as_tsv(),
            "thumbnail-generation\t/tmp/Image.png\tallow-native\tcloud=native-eligible\tquicklook-thumbnailing\t256px\tscale=2000m\t1:11\tcache=refresh-memory-only\tinvalidate-memory=true\tinvalidate-disk=false\tschedule=scheduled:visible"
        );
    }

    fn input(name: &str, rect: Rect) -> ThumbnailGenerationInput {
        ThumbnailGenerationInput::new(
            PreviewRequestKey::new(
                FileId::new(VolumeId(1), 11),
                PathBuf::from("/tmp").join(name),
                PreviewKind::Thumbnail,
            ),
            rect,
            Viewport::new(Rect::new(0, 0, 1_000, 800), 256),
        )
    }

    fn remote_volume() -> VolumeDescriptor {
        let mut volume = VolumeDescriptor::for_path("/tmp").unwrap();
        volume.kind = gfm_mac::VolumeKind::Network;
        volume.network = true;
        volume.local = Some(false);
        volume
    }

    fn fileprovider_report(
        previous: CloudStorageState,
        current: CloudStorageState,
    ) -> gfm_mac::FileProviderInvalidationReport {
        gfm_mac::FileProviderInvalidationReport {
            path: PathBuf::from("/tmp/Remote.icloud"),
            previous,
            current: gfm_mac::FileProviderStateReport {
                path: PathBuf::from("/tmp/Remote.icloud"),
                domain: gfm_mac::FileProviderDomain::ICloudDrive,
                storage_state: current,
                materialization: crate::cloud_materialization_for_state(current),
                materialization_source: gfm_mac::CloudMaterializationSource::NativeUrlResource,
                materialization_confidence: gfm_mac::CloudMaterializationConfidence::Native,
                materialization_reason: Some("test".to_string()),
                progress: gfm_mac::CloudTransferProgress {
                    direction: gfm_mac::CloudTransferDirection::Idle,
                    percent_milli: None,
                    requested: false,
                    complete: false,
                    indeterminate: false,
                    source: "state",
                    reason: Some("test".to_string()),
                },
                badges: Vec::new(),
                commands: gfm_mac::CloudCommandPolicy {
                    download: gfm_mac::CloudCommandState::Hidden,
                    evict: gfm_mac::CloudCommandState::Hidden,
                    reveal_conflict: gfm_mac::CloudCommandState::Hidden,
                    reason: None,
                },
                offline: false,
                conflict: false,
                provider_identifier: None,
                source: "test".to_string(),
            },
            state_changed: previous != current,
            invalidate_icon: true,
            invalidate_preview_memory: true,
            invalidate_preview_disk: true,
            invalidate_sidebar: true,
            reindex_metadata: true,
            reason: "test",
        }
    }
}
