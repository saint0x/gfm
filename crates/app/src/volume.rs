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
            let descriptor = VolumeDescriptor::for_path(&path)?;
            let native_status = native_status_for_event_descriptor(&descriptor);
            Ok(VolumeEventPathResolution {
                path: Some(path),
                descriptor: Some(descriptor),
                native_status,
                native_reason: None,
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
}
