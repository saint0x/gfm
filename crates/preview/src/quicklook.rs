use crate::{
    decide_invalidation, decide_preview_security, security_input_for_path,
    PreviewInvalidationDecision, PreviewInvalidationEvent, PreviewKind, PreviewRequestKey,
    PreviewScheduler, PreviewSchedulingPolicy, PreviewSecurityDecision, PreviewSecurityPolicy,
    PreviewTask, PreviewTaskDecision, Rect, Viewport,
};
use gfm_types::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickLookControllerMode {
    NativePreviewController,
    SandboxedGenerator,
    MetadataOnly,
    Denied,
}

impl QuickLookControllerMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NativePreviewController => "native-preview-controller",
            Self::SandboxedGenerator => "sandboxed-generator",
            Self::MetadataOnly => "metadata-only",
            Self::Denied => "denied",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickLookSessionInput {
    pub key: PreviewRequestKey,
    pub rect: Rect,
    pub viewport: Viewport,
    pub invalidation_event: PreviewInvalidationEvent,
}

impl QuickLookSessionInput {
    pub fn new(key: PreviewRequestKey, rect: Rect, viewport: Viewport) -> Self {
        Self {
            key,
            rect,
            viewport,
            invalidation_event: PreviewInvalidationEvent::default(),
        }
    }

    pub fn with_invalidation(mut self, event: PreviewInvalidationEvent) -> Self {
        self.invalidation_event = event;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickLookSessionContract {
    pub key: PreviewRequestKey,
    pub security: PreviewSecurityDecision,
    pub controller_mode: QuickLookControllerMode,
    pub invalidation: PreviewInvalidationDecision,
    pub schedule_decision: PreviewTaskDecision,
}

impl QuickLookSessionContract {
    pub fn from_input(
        policy: &PreviewSecurityPolicy,
        input: QuickLookSessionInput,
    ) -> Result<Self> {
        let security_input = security_input_for_path(&input.key.path, PreviewKind::QuickLook);
        let security = decide_preview_security(policy, &security_input);
        let controller_mode = controller_mode(security);
        let invalidation = decide_invalidation(input.invalidation_event);
        let mut scheduler = PreviewScheduler::new(PreviewSchedulingPolicy {
            max_visible: 1,
            max_prefetch: 1,
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
                reason: "outside-preview-budget",
            });

        Ok(Self {
            key: input.key,
            security,
            controller_mode,
            invalidation,
            schedule_decision,
        })
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "quicklook-session\t{}\t{}\t{}\t{}\t{}:{}\tinvalidate-memory={}\tinvalidate-disk={}\tschedule={}",
            self.key.kind.as_str(),
            self.key.path.display(),
            self.security.as_str(),
            self.controller_mode.as_str(),
            self.key.file_id.volume.0,
            self.key.file_id.node,
            self.invalidation.invalidate_memory,
            self.invalidation.invalidate_disk,
            schedule_tsv(&self.schedule_decision)
        )
    }
}

fn controller_mode(security: PreviewSecurityDecision) -> QuickLookControllerMode {
    match security {
        PreviewSecurityDecision::AllowNative => QuickLookControllerMode::NativePreviewController,
        PreviewSecurityDecision::Sandbox => QuickLookControllerMode::SandboxedGenerator,
        PreviewSecurityDecision::MetadataOnly => QuickLookControllerMode::MetadataOnly,
        PreviewSecurityDecision::Deny => QuickLookControllerMode::Denied,
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
    fn trusted_documents_use_native_preview_controller() {
        let contract = QuickLookSessionContract::from_input(
            &PreviewSecurityPolicy::default(),
            input("Report.pdf", Rect::new(0, 0, 400, 300)),
        )
        .unwrap();

        assert_eq!(contract.security, PreviewSecurityDecision::AllowNative);
        assert_eq!(
            contract.controller_mode,
            QuickLookControllerMode::NativePreviewController
        );
        assert!(matches!(
            contract.schedule_decision,
            PreviewTaskDecision::Scheduled { .. }
        ));
    }

    #[test]
    fn executable_packages_are_metadata_only() {
        let contract = QuickLookSessionContract::from_input(
            &PreviewSecurityPolicy::default(),
            input("Danger.app", Rect::new(0, 0, 400, 300)),
        )
        .unwrap();

        assert_eq!(contract.security, PreviewSecurityDecision::MetadataOnly);
        assert_eq!(
            contract.controller_mode,
            QuickLookControllerMode::MetadataOnly
        );
    }

    #[test]
    fn offscreen_sessions_are_cancelled_by_schedule() {
        let contract = QuickLookSessionContract::from_input(
            &PreviewSecurityPolicy::default(),
            input("Report.pdf", Rect::new(10_000, 10_000, 400, 300)),
        )
        .unwrap();

        assert!(matches!(
            contract.schedule_decision,
            PreviewTaskDecision::Cancelled { .. }
        ));
    }

    #[test]
    fn tsv_output_is_stable_for_cli_and_fozzy() {
        let contract = QuickLookSessionContract::from_input(
            &PreviewSecurityPolicy::default(),
            input("Report.pdf", Rect::new(0, 0, 400, 300)).with_invalidation(
                PreviewInvalidationEvent {
                    content_changed: true,
                    ..PreviewInvalidationEvent::default()
                },
            ),
        )
        .unwrap();

        assert_eq!(
            contract.as_tsv(),
            "quicklook-session\tquick-look\t/tmp/Report.pdf\tallow-native\tnative-preview-controller\t1:10\tinvalidate-memory=true\tinvalidate-disk=true\tschedule=scheduled:visible"
        );
    }

    fn input(name: &str, rect: Rect) -> QuickLookSessionInput {
        QuickLookSessionInput::new(
            PreviewRequestKey::new(
                FileId::new(VolumeId(1), 10),
                PathBuf::from("/tmp").join(name),
                PreviewKind::QuickLook,
            ),
            rect,
            Viewport::new(Rect::new(0, 0, 1_000, 800), 256),
        )
    }
}
