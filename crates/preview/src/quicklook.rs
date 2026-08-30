use crate::{
    cloud_materialization_for_state, decide_cloud_preview_for_materialization, decide_invalidation,
    decide_preview_security, preview_invalidation_for_fileprovider, security_input_for_path,
    CloudPreviewDecision, PreviewInvalidationDecision, PreviewInvalidationEvent, PreviewKind,
    PreviewRequestKey, PreviewScheduler, PreviewSchedulingPolicy, PreviewSecurityDecision,
    PreviewSecurityPolicy, PreviewTask, PreviewTaskDecision, Rect, Viewport,
};
use gfm_mac::{CloudMaterialization, CloudStorageState};
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
    pub scheduling_policy: PreviewSchedulingPolicy,
    pub invalidation_event: PreviewInvalidationEvent,
    pub cloud_state: CloudStorageState,
    pub cloud_materialization: CloudMaterialization,
}

impl QuickLookSessionInput {
    pub fn new(key: PreviewRequestKey, rect: Rect, viewport: Viewport) -> Self {
        Self {
            key,
            rect,
            viewport,
            scheduling_policy: PreviewSchedulingPolicy {
                max_visible: 1,
                max_prefetch: 1,
                cancel_offscreen: true,
            },
            invalidation_event: PreviewInvalidationEvent::default(),
            cloud_state: CloudStorageState::LocalOnly,
            cloud_materialization: CloudMaterialization::NotProviderBacked,
        }
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
pub struct QuickLookSessionContract {
    pub key: PreviewRequestKey,
    pub security: PreviewSecurityDecision,
    pub cloud: CloudPreviewDecision,
    pub controller_mode: QuickLookControllerMode,
    pub invalidation: PreviewInvalidationDecision,
    pub schedule_decision: PreviewTaskDecision,
}

impl QuickLookSessionContract {
    pub fn from_input(
        policy: &PreviewSecurityPolicy,
        input: QuickLookSessionInput,
    ) -> Result<Self> {
        Self::from_input_checked(policy, input, || Ok(()))
    }

    pub fn from_input_checked(
        policy: &PreviewSecurityPolicy,
        input: QuickLookSessionInput,
        mut check: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        check()?;
        let security_input = security_input_for_path(&input.key.path, PreviewKind::QuickLook);
        check()?;
        let security = decide_preview_security(policy, &security_input);
        let cloud = decide_cloud_preview_for_materialization(input.cloud_materialization);
        let controller_mode = controller_mode(security, cloud);
        let invalidation = decide_invalidation(input.invalidation_event);
        check()?;
        let schedule_decision = match (cloud, controller_mode) {
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
            (_, QuickLookControllerMode::MetadataOnly) => PreviewTaskDecision::Cancelled {
                key: input.key.clone(),
                reason: "metadata-only",
            },
            (
                CloudPreviewDecision::NativeEligible,
                QuickLookControllerMode::NativePreviewController
                | QuickLookControllerMode::SandboxedGenerator,
            ) => {
                let mut scheduler = PreviewScheduler::new(input.scheduling_policy)?;
                check()?;
                scheduler
                    .schedule(
                        input.viewport,
                        [PreviewTask::new(input.key.clone(), input.rect)],
                    )
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| PreviewTaskDecision::Cancelled {
                        key: input.key.clone(),
                        reason: "outside-preview-budget",
                    })
            }
            (_, QuickLookControllerMode::Denied) => PreviewTaskDecision::Cancelled {
                key: input.key.clone(),
                reason: "denied",
            },
        };
        check()?;

        Ok(Self {
            key: input.key,
            security,
            cloud,
            controller_mode,
            invalidation,
            schedule_decision,
        })
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "quicklook-session\t{}\t{}\t{}\tcloud={}\t{}\t{}:{}\tinvalidate-memory={}\tinvalidate-disk={}\tschedule={}",
            self.key.kind.as_str(),
            self.key.path.display(),
            self.security.as_str(),
            self.cloud.as_str(),
            self.controller_mode.as_str(),
            self.key.file_id.volume.0,
            self.key.file_id.node,
            self.invalidation.invalidate_memory,
            self.invalidation.invalidate_disk,
            schedule_tsv(&self.schedule_decision)
        )
    }
}

fn controller_mode(
    security: PreviewSecurityDecision,
    cloud: CloudPreviewDecision,
) -> QuickLookControllerMode {
    match cloud {
        CloudPreviewDecision::MetadataOnly => return QuickLookControllerMode::MetadataOnly,
        CloudPreviewDecision::Defer => return QuickLookControllerMode::MetadataOnly,
        CloudPreviewDecision::Unavailable => return QuickLookControllerMode::Denied,
        CloudPreviewDecision::NativeEligible => {}
    }
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
        assert!(matches!(
            contract.schedule_decision,
            PreviewTaskDecision::Cancelled {
                reason: "metadata-only",
                ..
            }
        ));
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
    fn pressure_policy_preserves_visible_quicklook_preview() {
        let contract = QuickLookSessionContract::from_input(
            &PreviewSecurityPolicy::default(),
            input("Report.pdf", Rect::new(0, 0, 400, 300)).with_scheduling_policy(
                PreviewSchedulingPolicy {
                    max_visible: 1,
                    max_prefetch: 1,
                    cancel_offscreen: true,
                }
                .adapted_for_pressure(gfm_jobs::SchedulingPressure {
                    thermal: gfm_jobs::JobThermalState::Critical,
                    ..gfm_jobs::SchedulingPressure::default()
                }),
            ),
        )
        .unwrap();

        assert!(matches!(
            contract.schedule_decision,
            PreviewTaskDecision::Scheduled {
                priority: crate::PreviewPriority::Visible,
                ..
            }
        ));
    }

    #[test]
    fn evicted_fileprovider_items_are_metadata_only_for_quicklook() {
        let contract = QuickLookSessionContract::from_input(
            &PreviewSecurityPolicy::default(),
            input("Remote.icloud", Rect::new(0, 0, 400, 300))
                .with_cloud_state(CloudStorageState::Evicted),
        )
        .unwrap();

        assert_eq!(contract.cloud, CloudPreviewDecision::MetadataOnly);
        assert_eq!(
            contract.controller_mode,
            QuickLookControllerMode::MetadataOnly
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
    fn unknown_fileprovider_items_are_metadata_only_for_quicklook() {
        let contract = QuickLookSessionContract::from_input(
            &PreviewSecurityPolicy::default(),
            input("Unknown.icloud", Rect::new(0, 0, 400, 300))
                .with_cloud_state(CloudStorageState::Unknown),
        )
        .unwrap();

        assert_eq!(contract.cloud, CloudPreviewDecision::MetadataOnly);
        assert_eq!(
            contract.controller_mode,
            QuickLookControllerMode::MetadataOnly
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
    fn in_flight_fileprovider_items_defer_quicklook() {
        let contract = QuickLookSessionContract::from_input(
            &PreviewSecurityPolicy::default(),
            input("Downloading.icloud", Rect::new(0, 0, 400, 300))
                .with_cloud_state(CloudStorageState::Downloading),
        )
        .unwrap();

        assert_eq!(contract.cloud, CloudPreviewDecision::Defer);
        assert_eq!(
            contract.controller_mode,
            QuickLookControllerMode::MetadataOnly
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
    fn offline_fileprovider_items_deny_quicklook() {
        let contract = QuickLookSessionContract::from_input(
            &PreviewSecurityPolicy::default(),
            input("Offline.icloud", Rect::new(0, 0, 400, 300))
                .with_cloud_state(CloudStorageState::Offline),
        )
        .unwrap();

        assert_eq!(contract.cloud, CloudPreviewDecision::Unavailable);
        assert_eq!(contract.controller_mode, QuickLookControllerMode::Denied);
        assert!(matches!(
            contract.schedule_decision,
            PreviewTaskDecision::Cancelled {
                reason: "fileprovider-unavailable",
                ..
            }
        ));
    }

    #[test]
    fn fileprovider_invalidation_updates_quicklook_cloud_and_cache_state() {
        let contract = QuickLookSessionContract::from_input(
            &PreviewSecurityPolicy::default(),
            input("Remote.icloud", Rect::new(0, 0, 400, 300)).with_fileprovider_invalidation(
                &fileprovider_report(
                    CloudStorageState::Downloaded,
                    CloudStorageState::Downloading,
                ),
            ),
        )
        .unwrap();

        assert_eq!(contract.cloud, CloudPreviewDecision::Defer);
        assert_eq!(contract.invalidation.reason, "content-or-icloud");
        assert!(contract.invalidation.invalidate_memory);
        assert!(contract.invalidation.invalidate_disk);
        assert!(matches!(
            contract.schedule_decision,
            PreviewTaskDecision::Cancelled {
                reason: "fileprovider-in-flight",
                ..
            }
        ));
    }

    #[test]
    fn checked_contract_honors_pre_cancelled_work() {
        let err = QuickLookSessionContract::from_input_checked(
            &PreviewSecurityPolicy::default(),
            input("Report.pdf", Rect::new(0, 0, 400, 300)),
            || Err(gfm_types::GfmError::Cancelled),
        )
        .expect_err("pre-cancelled quicklook contract fails before planning");

        assert!(matches!(err, gfm_types::GfmError::Cancelled));
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
            "quicklook-session\tquick-look\t/tmp/Report.pdf\tallow-native\tcloud=native-eligible\tnative-preview-controller\t1:10\tinvalidate-memory=true\tinvalidate-disk=true\tschedule=scheduled:visible"
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
