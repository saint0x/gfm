use crate::{
    decide_invalidation, decide_preview_security, security_input_for_path,
    PreviewInvalidationDecision, PreviewInvalidationEvent, PreviewKind, PreviewRequestKey,
    PreviewScheduler, PreviewSchedulingPolicy, PreviewSecurityDecision, PreviewSecurityPolicy,
    PreviewTask, PreviewTaskDecision, Rect, Viewport,
};
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
    pub max_pixel_size: u16,
    pub scale_factor_milli: u16,
    pub invalidation_event: PreviewInvalidationEvent,
}

impl ThumbnailGenerationInput {
    pub fn new(key: PreviewRequestKey, rect: Rect, viewport: Viewport) -> Self {
        Self {
            key,
            rect,
            viewport,
            max_pixel_size: 512,
            scale_factor_milli: 2_000,
            invalidation_event: PreviewInvalidationEvent::default(),
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThumbnailGenerationContract {
    pub key: PreviewRequestKey,
    pub generator_mode: ThumbnailGeneratorMode,
    pub security: PreviewSecurityDecision,
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
        let security_input = security_input_for_path(&input.key.path, PreviewKind::Thumbnail);
        let security = decide_preview_security(policy, &security_input);
        let generator_mode = generator_mode(security);
        let invalidation = decide_invalidation(input.invalidation_event);
        let cache_disposition = cache_disposition(generator_mode, &invalidation);
        let mut scheduler = PreviewScheduler::new(PreviewSchedulingPolicy {
            max_visible: 64,
            max_prefetch: 128,
            cancel_offscreen: true,
        })?;
        let schedule_decision = scheduler
            .schedule(
                input.viewport,
                [PreviewTask::new(input.key.clone(), input.rect)],
            )
            .into_iter()
            .next()
            .unwrap_or_else(|| PreviewTaskDecision::Cancelled {
                key: input.key.clone(),
                reason: "outside-thumbnail-budget",
            });

        Ok(Self {
            key: input.key,
            generator_mode,
            security,
            invalidation,
            cache_disposition,
            schedule_decision,
            max_pixel_size: input.max_pixel_size,
            scale_factor_milli: input.scale_factor_milli,
        })
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "thumbnail-generation\t{}\t{}\t{}\t{}px\tscale={}m\t{}:{}\tcache={}\tinvalidate-memory={}\tinvalidate-disk={}\tschedule={}",
            self.key.path.display(),
            self.security.as_str(),
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

fn generator_mode(security: PreviewSecurityDecision) -> ThumbnailGeneratorMode {
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
    if mode == ThumbnailGeneratorMode::Denied {
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
            "thumbnail-generation\t/tmp/Image.png\tallow-native\tquicklook-thumbnailing\t256px\tscale=2000m\t1:11\tcache=refresh-memory-only\tinvalidate-memory=true\tinvalidate-disk=false\tschedule=scheduled:visible"
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
}
