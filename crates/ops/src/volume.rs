use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(crate) const COPY_BUFFER_BYTES: usize = 256 * 1024;
const EXTERNAL_COPY_BUFFER_BYTES: usize = 128 * 1024;
pub(crate) const SLOW_COPY_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationVolumeClass {
    Local,
    External,
    Network,
    Slow,
}

impl OperationVolumeClass {
    const fn copy_buffer_bytes(self) -> usize {
        match self {
            Self::Local => COPY_BUFFER_BYTES,
            Self::External => EXTERNAL_COPY_BUFFER_BYTES,
            Self::Network | Self::Slow => SLOW_COPY_BUFFER_BYTES,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationVolumeCopyPolicy {
    default_class: OperationVolumeClass,
    root_classes: BTreeMap<PathBuf, OperationVolumeClass>,
}

impl Default for OperationVolumeCopyPolicy {
    fn default() -> Self {
        Self {
            default_class: OperationVolumeClass::Local,
            root_classes: BTreeMap::new(),
        }
    }
}

impl OperationVolumeCopyPolicy {
    pub fn new(default_class: OperationVolumeClass) -> Self {
        Self {
            default_class,
            root_classes: BTreeMap::new(),
        }
    }

    pub fn with_root(mut self, root: impl Into<PathBuf>, class: OperationVolumeClass) -> Self {
        self.root_classes.insert(root.into(), class);
        self
    }

    pub fn class_for_path(&self, path: &Path) -> OperationVolumeClass {
        self.root_classes
            .iter()
            .filter(|(root, _)| path.starts_with(root))
            .max_by_key(|(root, _)| root.components().count())
            .map(|(_, class)| *class)
            .unwrap_or(self.default_class)
    }

    pub fn copy_buffer_bytes_for_paths(&self, from: &Path, to: &Path) -> usize {
        let source = self.class_for_path(from);
        let destination = self.class_for_path(to);
        source
            .copy_buffer_bytes()
            .min(destination.copy_buffer_bytes())
            .max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_for_path_prefers_the_longest_matching_root() {
        let policy = OperationVolumeCopyPolicy::new(OperationVolumeClass::Local)
            .with_root("/Volumes/Media", OperationVolumeClass::External)
            .with_root("/Volumes/Media/Archive", OperationVolumeClass::Network);

        assert_eq!(
            policy.class_for_path(Path::new("/Volumes/Media/Archive/clip.mov")),
            OperationVolumeClass::Network
        );
        assert_eq!(
            policy.class_for_path(Path::new("/Volumes/Media/raw.mov")),
            OperationVolumeClass::External
        );
        assert_eq!(
            policy.class_for_path(Path::new("/Users/deepsaint/Desktop/file.txt")),
            OperationVolumeClass::Local
        );
    }

    #[test]
    fn copy_buffer_uses_the_most_constrained_participant() {
        let policy = OperationVolumeCopyPolicy::default()
            .with_root("/Volumes/Remote", OperationVolumeClass::Network)
            .with_root("/Volumes/Fast", OperationVolumeClass::External);

        assert_eq!(
            policy.copy_buffer_bytes_for_paths(
                Path::new("/Users/deepsaint/source.bin"),
                Path::new("/Volumes/Remote/source.bin")
            ),
            SLOW_COPY_BUFFER_BYTES
        );
        assert_eq!(
            policy.copy_buffer_bytes_for_paths(
                Path::new("/Volumes/Fast/source.bin"),
                Path::new("/Users/deepsaint/source.bin")
            ),
            EXTERNAL_COPY_BUFFER_BYTES
        );
        assert_eq!(
            policy.copy_buffer_bytes_for_paths(
                Path::new("/Users/deepsaint/source.bin"),
                Path::new("/Users/deepsaint/copy.bin")
            ),
            COPY_BUFFER_BYTES
        );
    }
}
