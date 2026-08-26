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
    CloudMaterializationSource, CloudStorageState, CloudTransferDirection, CloudTransferProgress,
    FileProviderConflictReport, FileProviderDomain, FileProviderDomainEnumerationReport,
    FileProviderDomainReport, FileProviderInvalidationReport, FileProviderOperation,
    FileProviderOperationDisposition, FileProviderOperationReport, FileProviderProgressReport,
    FileProviderRegisteredDomain, FileProviderStateReport,
};
pub use finder::{FinderKindSource, NativeFinderKindReport};
pub use host::{
    current_host_profile, CpuArchitecture, HardwareProfile, HostProfile, MacOsVersion,
    SupportEvaluation, SupportMatrix, SupportTier,
};
pub use icon::{
    NativeIconBadge, NativeIconBridgeContract, NativeIconBridgeDecision, NativeIconDescriptor,
    NativeIconProvider, NativeIconRole,
};
pub use permissions::{
    current_permission_onboarding, PermissionAction, PermissionOnboardingPlan, PermissionPolicy,
    PermissionPromptMode, PermissionReadiness, PermissionScope, PermissionState,
};
pub use security::{
    AccessIntent, AccessProbeState, ProtectedScope, SecurityAccessMode, SecurityDecisionAction,
    SecurityScopedAccessReport, SecurityScopedBookmark, SecurityScopedBookmarkAccess,
    SecurityScopedBookmarkAccessLookup, SecurityScopedBookmarkLookup, SecurityScopedBookmarkRecord,
    SecurityScopedBookmarkReport, SecurityScopedBookmarkResolution, SecurityScopedBookmarkStatus,
    SecurityScopedBookmarkStore, SecurityScopedBookmarkStoreReport,
};
pub use spotlight::{
    parse_spotlight_fixture, SpotlightField, SpotlightFieldDecision, SpotlightFieldReconciliation,
    SpotlightIndexHealth, SpotlightIngestionAction, SpotlightIngestionDecision,
    SpotlightIngestionPlan, SpotlightIngestionPolicy, SpotlightMetadataReader,
    SpotlightReconciliationReport, SpotlightSnapshot, SpotlightStatus,
};
pub use volume::{
    MountState, VolumeCapacity, VolumeCommandPolicy, VolumeCommandState, VolumeDescriptor,
    VolumeDiscoveryReport, VolumeKind, VolumeOperation, VolumeOperationDisposition,
    VolumeOperationReport, VolumeTopologyChange, VolumeTopologyChangeKind, VolumeTopologyDiff,
};
pub use watch::{map_notify_event, FileEventStream, WatchDepth, WatchRoot};
