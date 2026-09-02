use gfm_mac::{
    NativeVolumeStatus, VolumeDescriptor, VolumeEventInvalidationReport, VolumeEventKind,
};
use gfm_types::Result;
use std::io::ErrorKind;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub(crate) struct VolumeEventPathResolution {
    pub(crate) path: Option<PathBuf>,
    pub(crate) descriptor: Option<VolumeDescriptor>,
    pub(crate) native_status: NativeVolumeStatus,
    pub(crate) native_reason: Option<String>,
}

impl VolumeEventPathResolution {
    pub(crate) fn invalidation_report(
        &self,
        kind: VolumeEventKind,
    ) -> VolumeEventInvalidationReport {
        VolumeEventInvalidationReport::from_parts(
            kind,
            self.native_status,
            self.path.clone(),
            self.descriptor.as_ref(),
            self.native_reason.clone(),
        )
    }
}

pub(crate) fn volume_event_invalidation_for_descriptor(
    kind: VolumeEventKind,
    path: PathBuf,
    descriptor: &VolumeDescriptor,
) -> VolumeEventInvalidationReport {
    VolumeEventInvalidationReport::from_parts(
        kind,
        native_status_for_event_descriptor(descriptor),
        Some(path),
        Some(descriptor),
        native_reason_for_event_descriptor(descriptor),
    )
}

pub(crate) fn resolve_volume_event_path(
    kind: VolumeEventKind,
    path: Option<PathBuf>,
) -> Result<VolumeEventPathResolution> {
    let Some(path) = path else {
        return Ok(VolumeEventPathResolution {
            path: None,
            descriptor: None,
            native_status: NativeVolumeStatus::Unavailable,
            native_reason: None,
        });
    };

    if kind == VolumeEventKind::Unavailable {
        return Ok(VolumeEventPathResolution {
            path: Some(path),
            descriptor: None,
            native_status: NativeVolumeStatus::Unavailable,
            native_reason: Some("volume-event-unavailable".to_string()),
        });
    }

    match path.try_exists() {
        Ok(false) => Ok(VolumeEventPathResolution {
            path: Some(path),
            descriptor: None,
            native_status: NativeVolumeStatus::Missing,
            native_reason: None,
        }),
        Ok(true) => {
            let descriptor = VolumeDescriptor::for_path_policy_checked(&path, || Ok(()))?;
            let native_status = native_status_for_event_descriptor(&descriptor);
            let native_reason = native_reason_for_event_descriptor(&descriptor);
            Ok(VolumeEventPathResolution {
                path: Some(path),
                descriptor: Some(descriptor),
                native_status,
                native_reason,
            })
        }
        Err(err) => Ok(VolumeEventPathResolution {
            path: Some(path),
            descriptor: None,
            native_status: native_status_for_path_probe_error(err.kind()),
            native_reason: Some(format!("volume path state unavailable: {err}")),
        }),
    }
}

fn native_status_for_event_descriptor(descriptor: &VolumeDescriptor) -> NativeVolumeStatus {
    descriptor
        .native_status
        .unwrap_or(NativeVolumeStatus::Available)
}

fn native_reason_for_event_descriptor(descriptor: &VolumeDescriptor) -> Option<String> {
    status_or_reason(
        "diskarbitration-volume",
        descriptor.native_status,
        descriptor.native_reason.as_deref(),
    )
    .or_else(|| {
        status_or_reason(
            "url-resource-volume",
            descriptor.resource_status,
            descriptor.resource_reason.as_deref(),
        )
    })
    .or_else(|| {
        status_or_reason(
            "mount-table-volume",
            descriptor.mount_table_status,
            descriptor.mount_table_reason.as_deref(),
        )
    })
}

fn status_or_reason(
    prefix: &str,
    status: Option<NativeVolumeStatus>,
    reason: Option<&str>,
) -> Option<String> {
    let status = status?;
    if status == NativeVolumeStatus::Available {
        return None;
    }
    reason
        .filter(|reason| !reason.trim().is_empty())
        .map(str::to_string)
        .or_else(|| status_reason(prefix, status))
}

fn status_reason(prefix: &str, status: NativeVolumeStatus) -> Option<String> {
    (status != NativeVolumeStatus::Available).then(|| format!("{prefix}-{}", status.as_str()))
}

fn native_status_for_path_probe_error(kind: ErrorKind) -> NativeVolumeStatus {
    match kind {
        ErrorKind::NotFound => NativeVolumeStatus::Missing,
        _ => NativeVolumeStatus::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disappeared_volume_event_keeps_missing_path_typed_without_descriptor() {
        let path =
            std::env::temp_dir().join(format!("gfm-missing-volume-event-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);

        let resolution =
            resolve_volume_event_path(VolumeEventKind::Disappeared, Some(path.clone()))
                .expect("missing disappeared volume path should resolve");
        let report = resolution.invalidation_report(VolumeEventKind::Disappeared);

        assert_eq!(resolution.path.as_deref(), Some(path.as_path()));
        assert_eq!(resolution.native_status, NativeVolumeStatus::Missing);
        assert!(resolution.descriptor.is_none());
        assert_eq!(
            report.current_mount_state,
            Some(gfm_mac::MountState::Unmounted)
        );
        assert!(report.invalidate_sidebar);
        assert!(report.invalidate_operation_policy);
        assert!(report.invalidate_index_admission);
        assert!(report.rescan_index);
    }

    #[test]
    fn unavailable_volume_event_does_not_require_path_probe() {
        let path = std::env::temp_dir().join(format!(
            "gfm-unavailable-volume-event-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);

        let resolution =
            resolve_volume_event_path(VolumeEventKind::Unavailable, Some(path.clone()))
                .expect("unavailable event path should resolve without descriptor");
        let report = resolution.invalidation_report(VolumeEventKind::Unavailable);

        assert_eq!(resolution.path.as_deref(), Some(path.as_path()));
        assert_eq!(resolution.native_status, NativeVolumeStatus::Unavailable);
        assert!(resolution.descriptor.is_none());
        assert_eq!(
            resolution.native_reason.as_deref(),
            Some("volume-event-unavailable")
        );
        assert!(report.invalidate_sidebar);
        assert!(report.invalidate_operation_policy);
        assert!(report.invalidate_index_admission);
        assert!(report.rescan_index);
    }

    #[test]
    fn descriptor_native_status_is_preserved_for_event_resolution() {
        let path = std::env::temp_dir().join(format!(
            "gfm-volume-event-descriptor-status-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        let mut descriptor = VolumeDescriptor::for_path(&path).unwrap();
        descriptor.native_status = Some(NativeVolumeStatus::Unavailable);

        assert_eq!(
            native_status_for_event_descriptor(&descriptor),
            NativeVolumeStatus::Unavailable
        );

        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn existing_volume_event_resolution_defers_capacity_reads() {
        let path = std::env::temp_dir().join(format!(
            "gfm-volume-event-policy-capacity-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join(".gfm-volume-kind"), "external-removable\n").unwrap();

        let resolution =
            resolve_volume_event_path(VolumeEventKind::DescriptionChanged, Some(path.clone()))
                .expect("existing volume event path should resolve");
        let descriptor = resolution
            .descriptor
            .as_ref()
            .expect("existing volume event path should retain descriptor");

        assert_eq!(descriptor.kind, gfm_mac::VolumeKind::External);
        assert_eq!(
            descriptor.capacity,
            gfm_mac::VolumeCapacity {
                total_bytes: 0,
                available_bytes: 0
            }
        );
        assert_eq!(
            resolution.native_status,
            descriptor
                .native_status
                .unwrap_or(NativeVolumeStatus::Available)
        );

        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn descriptor_native_failure_status_becomes_event_reason() {
        let path = std::env::temp_dir().join(format!(
            "gfm-volume-event-descriptor-reason-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        let mut descriptor = VolumeDescriptor::for_path(&path).unwrap();
        descriptor.native_status = Some(NativeVolumeStatus::Unavailable);
        descriptor.native_reason = None;

        let report = descriptor_event_invalidation(&path, &descriptor);

        assert_eq!(
            native_reason_for_event_descriptor(&descriptor).as_deref(),
            Some("diskarbitration-volume-unavailable")
        );
        assert_eq!(report.reason, "diskarbitration-volume-unavailable");
        assert!(report
            .as_tsv()
            .contains("\tcurrent-native-status=unavailable\t"));
        assert!(report
            .as_tsv()
            .ends_with("reason=diskarbitration-volume-unavailable"));

        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn descriptor_native_failure_reason_is_preserved_for_event_reason() {
        let path = std::env::temp_dir().join(format!(
            "gfm-volume-event-descriptor-native-reason-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        let mut descriptor = VolumeDescriptor::for_path(&path).unwrap();
        descriptor.native_status = Some(NativeVolumeStatus::Unavailable);
        descriptor.native_reason =
            Some("DiskArbitration did not return a disk description".to_string());

        let report = descriptor_event_invalidation(&path, &descriptor);

        assert_eq!(
            native_reason_for_event_descriptor(&descriptor).as_deref(),
            Some("DiskArbitration did not return a disk description")
        );
        assert_eq!(
            report.reason,
            "DiskArbitration did not return a disk description"
        );

        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn descriptor_blank_native_failure_reason_uses_typed_status_reason() {
        let path = std::env::temp_dir().join(format!(
            "gfm-volume-event-descriptor-blank-native-reason-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        let mut descriptor = VolumeDescriptor::for_path(&path).unwrap();
        descriptor.native_status = Some(NativeVolumeStatus::Unavailable);
        descriptor.native_reason = Some(" \t ".to_string());

        let report = descriptor_event_invalidation(&path, &descriptor);

        assert_eq!(
            native_reason_for_event_descriptor(&descriptor).as_deref(),
            Some("diskarbitration-volume-unavailable")
        );
        assert_eq!(report.reason, "diskarbitration-volume-unavailable");

        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn descriptor_resource_failure_reason_precedes_synthetic_status_reason() {
        let path = std::env::temp_dir().join(format!(
            "gfm-volume-event-descriptor-resource-native-reason-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        let mut descriptor = VolumeDescriptor::for_path(&path).unwrap();
        descriptor.native_status = Some(NativeVolumeStatus::Available);
        descriptor.resource_status = Some(NativeVolumeStatus::Unavailable);
        descriptor.resource_reason =
            Some("native volume URL resource values unavailable".to_string());

        let report = descriptor_event_invalidation(&path, &descriptor);

        assert_eq!(
            native_reason_for_event_descriptor(&descriptor).as_deref(),
            Some("native volume URL resource values unavailable")
        );
        assert_eq!(
            report.reason,
            "native volume URL resource values unavailable"
        );

        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn descriptor_resource_failure_status_becomes_event_reason() {
        let path = std::env::temp_dir().join(format!(
            "gfm-volume-event-resource-reason-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        let mut descriptor = VolumeDescriptor::for_path(&path).unwrap();
        descriptor.native_status = Some(NativeVolumeStatus::Available);
        descriptor.resource_status = Some(NativeVolumeStatus::Unavailable);

        let report = descriptor_event_invalidation(&path, &descriptor);

        assert_eq!(
            native_reason_for_event_descriptor(&descriptor).as_deref(),
            Some("url-resource-volume-unavailable")
        );
        assert_eq!(report.reason, "url-resource-volume-unavailable");
        assert!(report
            .as_tsv()
            .contains("\tcurrent-resource-status=unavailable\t"));

        std::fs::remove_dir_all(path).unwrap();
    }

    fn descriptor_event_invalidation(
        path: &std::path::Path,
        descriptor: &VolumeDescriptor,
    ) -> VolumeEventInvalidationReport {
        volume_event_invalidation_for_descriptor(
            VolumeEventKind::DescriptionChanged,
            path.to_path_buf(),
            descriptor,
        )
    }
}
