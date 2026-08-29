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
    root_volume_identities: BTreeMap<PathBuf, String>,
    root_file_cloning_support: BTreeMap<PathBuf, bool>,
    root_sparse_file_support: BTreeMap<PathBuf, bool>,
}

impl Default for OperationVolumeCopyPolicy {
    fn default() -> Self {
        Self {
            default_class: OperationVolumeClass::Local,
            root_classes: BTreeMap::new(),
            root_volume_identities: BTreeMap::new(),
            root_file_cloning_support: BTreeMap::new(),
            root_sparse_file_support: BTreeMap::new(),
        }
    }
}

impl OperationVolumeCopyPolicy {
    pub fn new(default_class: OperationVolumeClass) -> Self {
        Self {
            default_class,
            root_classes: BTreeMap::new(),
            root_volume_identities: BTreeMap::new(),
            root_file_cloning_support: BTreeMap::new(),
            root_sparse_file_support: BTreeMap::new(),
        }
    }

    pub fn with_root(mut self, root: impl Into<PathBuf>, class: OperationVolumeClass) -> Self {
        self.root_classes.insert(root.into(), class);
        self
    }

    pub fn with_root_volume_identity(
        mut self,
        root: impl Into<PathBuf>,
        identity: impl Into<String>,
    ) -> Self {
        self.root_volume_identities
            .insert(root.into(), identity.into());
        self
    }

    pub fn with_root_file_cloning_support(
        mut self,
        root: impl Into<PathBuf>,
        supported: bool,
    ) -> Self {
        self.root_file_cloning_support
            .insert(root.into(), supported);
        self
    }

    pub fn with_root_sparse_file_support(
        mut self,
        root: impl Into<PathBuf>,
        supported: bool,
    ) -> Self {
        self.root_sparse_file_support.insert(root.into(), supported);
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

    fn file_cloning_support_for_path(&self, path: &Path) -> Option<bool> {
        self.root_file_cloning_support
            .iter()
            .filter(|(root, _)| path.starts_with(root))
            .max_by_key(|(root, _)| root.components().count())
            .map(|(_, supported)| *supported)
    }

    fn volume_identity_for_path(&self, path: &Path) -> Option<&str> {
        self.root_volume_identities
            .iter()
            .filter(|(root, _)| path.starts_with(root))
            .max_by_key(|(root, _)| root.components().count())
            .map(|(_, identity)| identity.as_str())
    }

    pub fn file_cloning_supported_for_paths(&self, from: &Path, to: &Path) -> bool {
        if self.file_cloning_support_for_path(from) == Some(false)
            || self.file_cloning_support_for_path(to) == Some(false)
        {
            return false;
        }
        match (
            self.volume_identity_for_path(from),
            self.volume_identity_for_path(to),
        ) {
            (Some(source), Some(destination)) => source == destination,
            _ => true,
        }
    }

    pub fn sparse_files_supported_for_path(&self, path: &Path) -> bool {
        self.root_sparse_file_support
            .iter()
            .filter(|(root, _)| path.starts_with(root))
            .max_by_key(|(root, _)| root.components().count())
            .map(|(_, supported)| *supported)
            .unwrap_or(true)
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

    #[test]
    fn file_cloning_support_uses_the_most_specific_root() {
        let policy = OperationVolumeCopyPolicy::default()
            .with_root_file_cloning_support("/Volumes/Media", false)
            .with_root_file_cloning_support("/Volumes/Media/Fast", true);

        assert!(!policy.file_cloning_supported_for_paths(
            Path::new("/Volumes/Media/source.bin"),
            Path::new("/Users/deepsaint/copy.bin")
        ));
        assert!(policy.file_cloning_supported_for_paths(
            Path::new("/Volumes/Media/Fast/source.bin"),
            Path::new("/Users/deepsaint/copy.bin")
        ));
        assert!(policy.file_cloning_supported_for_paths(
            Path::new("/Users/deepsaint/source.bin"),
            Path::new("/Users/deepsaint/copy.bin")
        ));
    }

    #[test]
    fn file_cloning_requires_same_known_volume_identity() {
        let policy = OperationVolumeCopyPolicy::default()
            .with_root_volume_identity("/Volumes/Source", "diskarbitration:uuid:SOURCE")
            .with_root_volume_identity("/Volumes/Destination", "diskarbitration:uuid:DESTINATION")
            .with_root_volume_identity("/Volumes/Source/Subvolume", "diskarbitration:uuid:NESTED");

        assert!(!policy.file_cloning_supported_for_paths(
            Path::new("/Volumes/Source/file.bin"),
            Path::new("/Volumes/Destination/file.bin")
        ));
        assert!(policy.file_cloning_supported_for_paths(
            Path::new("/Volumes/Source/file.bin"),
            Path::new("/Volumes/Source/copy.bin")
        ));
        assert!(!policy.file_cloning_supported_for_paths(
            Path::new("/Volumes/Source/Subvolume/file.bin"),
            Path::new("/Volumes/Source/copy.bin")
        ));
        assert!(policy.file_cloning_supported_for_paths(
            Path::new("/Unknown/source.bin"),
            Path::new("/Volumes/Destination/file.bin")
        ));
    }

    #[test]
    fn sparse_file_support_uses_the_most_specific_destination_root() {
        let policy = OperationVolumeCopyPolicy::default()
            .with_root_sparse_file_support("/Volumes/Media", false)
            .with_root_sparse_file_support("/Volumes/Media/Sparse", true);

        assert!(!policy.sparse_files_supported_for_path(Path::new("/Volumes/Media/copy.bin")));
        assert!(policy.sparse_files_supported_for_path(Path::new("/Volumes/Media/Sparse/copy.bin")));
        assert!(policy.sparse_files_supported_for_path(Path::new("/Users/deepsaint/copy.bin")));
    }
}
