mod bridge;
mod fileprovider;
mod finder;
mod host;
mod icon;
mod permissions;
mod security;
mod spotlight;
mod volume;
mod watch;

pub use bridge::{
    MacBridgeContract, MacBridgeSpec, MacBridgeStatus, MacBridgeThreadPolicy, MacFramework,
};
pub use fileprovider::{
    CloudBadge, CloudCommandPolicy, CloudCommandState, CloudMaterialization,
    CloudMaterializationConfidence, CloudMaterializationSource, CloudStorageState,
    CloudTransferDirection, CloudTransferProgress, FileProviderConflictReport, FileProviderDomain,
    FileProviderDomainEnumerationReport, FileProviderDomainReport, FileProviderInvalidationReport,
    FileProviderObservedEventKind, FileProviderObservedInvalidation, FileProviderOperation,
    FileProviderOperationDisposition, FileProviderOperationReport, FileProviderProgressReport,
    FileProviderRegisteredDomain, FileProviderStateInvalidationReport, FileProviderStateObserver,
    FileProviderStateReport, FileProviderStateSnapshot, FileProviderStateSnapshotEntry,
};
pub use finder::{FinderKindSource, NativeFinderKindReport};
pub use gfm_mac_sys::NativeVolumeStatus;
pub use host::{
    current_host_profile, CpuArchitecture, HardwareProfile, HostProfile, MacOsVersion,
    SupportEvaluation, SupportMatrix, SupportTier,
};
pub use icon::{
    NativeIconBadge, NativeIconBridgeContract, NativeIconBridgeDecision, NativeIconDescriptor,
    NativeIconInvalidationReport, NativeIconProvider, NativeIconRole,
};
pub use permissions::{
    current_permission_onboarding, current_permission_onboarding_checked, permission_onboarding,
    permission_onboarding_checked, PermissionAction, PermissionOnboardingPlan, PermissionPolicy,
    PermissionPromptMode, PermissionReadiness, PermissionScope, PermissionScopeChange,
    PermissionScopeChangeKind, PermissionState, PermissionStateInvalidationReport,
    PermissionStateSnapshot,
};
pub use security::{
    AccessIntent, AccessProbeState, ProtectedScope, SecurityAccessMode, SecurityDecisionAction,
    SecurityScopedAccessReport, SecurityScopedBookmark, SecurityScopedBookmarkAccess,
    SecurityScopedBookmarkAccessLookup, SecurityScopedBookmarkLookup, SecurityScopedBookmarkRecord,
    SecurityScopedBookmarkReport, SecurityScopedBookmarkResolution, SecurityScopedBookmarkStatus,
    SecurityScopedBookmarkStore, SecurityScopedBookmarkStoreReport, SecurityWorkerAction,
    SecurityWorkerAdmissionReport,
};
pub use spotlight::{
    parse_spotlight_fixture, SpotlightField, SpotlightFieldDecision, SpotlightFieldReconciliation,
    SpotlightIndexHealth, SpotlightIngestionAction, SpotlightIngestionDecision,
    SpotlightIngestionPlan, SpotlightIngestionPolicy, SpotlightMetadataReader,
    SpotlightReconciliationReport, SpotlightSnapshot, SpotlightStatus,
};
pub use volume::{
    ApfsVolumeRole, MountState, VolumeCapacity, VolumeCommandPolicy, VolumeCommandState,
    VolumeDescriptor, VolumeDiscoveryReport, VolumeEventInvalidationReport, VolumeEventKind,
    VolumeEventReport, VolumeEventState, VolumeEventStateBatchReport, VolumeEventStateTransition,
    VolumeEventStream, VolumeKind, VolumeMountIdentityReport, VolumeOperation,
    VolumeOperationDisposition, VolumeOperationReport, VolumeTopologyChange,
    VolumeTopologyChangeKind, VolumeTopologyDiff,
};
pub use watch::{map_notify_event, FileEventStream, WatchDepth, WatchRoot};
