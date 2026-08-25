use gfm_types::{FileEvent, FileEventKind, GfmError, Result};
use notify::event::{CreateKind, ModifyKind, RemoveKind, RenameMode};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver};

mod permissions;

pub use permissions::{
    current_permission_onboarding, PermissionAction, PermissionOnboardingPlan, PermissionPolicy,
    PermissionPromptMode, PermissionReadiness, PermissionScope, PermissionState,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MacOsVersion {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl MacOsVersion {
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    pub fn parse(input: &str) -> Result<Self> {
        let mut parts = input.trim().split('.');
        let major = parse_version_part(parts.next(), "major")?;
        let minor = parts
            .next()
            .map(|value| parse_version_part(Some(value), "minor"))
            .transpose()?
            .unwrap_or(0);
        let patch = parts
            .next()
            .map(|value| parse_version_part(Some(value), "patch"))
            .transpose()?
            .unwrap_or(0);
        if parts.next().is_some() {
            return Err(GfmError::Format(format!(
                "unsupported macOS version format `{input}`"
            )));
        }
        Ok(Self {
            major,
            minor,
            patch,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuArchitecture {
    AppleSilicon,
    Intel64,
    Unsupported,
}

impl CpuArchitecture {
    pub fn parse(input: &str) -> Self {
        match input.trim() {
            "arm64" | "arm64e" => Self::AppleSilicon,
            "x86_64" => Self::Intel64,
            _ => Self::Unsupported,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AppleSilicon => "apple-silicon",
            Self::Intel64 => "intel64",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportTier {
    Primary,
    Compatible,
    Unsupported,
}

impl SupportTier {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Compatible => "compatible",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardwareProfile {
    pub architecture: CpuArchitecture,
    pub memory_bytes: u64,
    pub logical_cpus: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostProfile {
    pub macos_version: MacOsVersion,
    pub build: String,
    pub hardware: HardwareProfile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportEvaluation {
    pub tier: SupportTier,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportMatrix {
    pub primary_minimum: MacOsVersion,
    pub compatible_minimum: MacOsVersion,
    pub minimum_memory_bytes: u64,
    pub minimum_logical_cpus: u16,
    pub primary_architectures: Vec<CpuArchitecture>,
    pub compatible_architectures: Vec<CpuArchitecture>,
}

impl Default for SupportMatrix {
    fn default() -> Self {
        Self {
            primary_minimum: MacOsVersion::new(15, 0, 0),
            compatible_minimum: MacOsVersion::new(14, 0, 0),
            minimum_memory_bytes: 8 * 1024 * 1024 * 1024,
            minimum_logical_cpus: 4,
            primary_architectures: vec![CpuArchitecture::AppleSilicon],
            compatible_architectures: vec![CpuArchitecture::Intel64],
        }
    }
}

impl SupportMatrix {
    pub fn evaluate(&self, host: &HostProfile) -> SupportEvaluation {
        let mut reasons = Vec::new();
        if host.macos_version < self.compatible_minimum {
            reasons.push(format!(
                "macOS {}.{}.{} is below minimum {}.{}.{}",
                host.macos_version.major,
                host.macos_version.minor,
                host.macos_version.patch,
                self.compatible_minimum.major,
                self.compatible_minimum.minor,
                self.compatible_minimum.patch
            ));
        }
        if host.hardware.architecture == CpuArchitecture::Unsupported {
            reasons.push("CPU architecture is unsupported".to_string());
        }
        if host.hardware.memory_bytes < self.minimum_memory_bytes {
            reasons.push(format!(
                "memory {} bytes is below minimum {} bytes",
                host.hardware.memory_bytes, self.minimum_memory_bytes
            ));
        }
        if host.hardware.logical_cpus < self.minimum_logical_cpus {
            reasons.push(format!(
                "logical CPU count {} is below minimum {}",
                host.hardware.logical_cpus, self.minimum_logical_cpus
            ));
        }
        if !reasons.is_empty() {
            return SupportEvaluation {
                tier: SupportTier::Unsupported,
                reasons,
            };
        }

        if host.macos_version >= self.primary_minimum
            && self
                .primary_architectures
                .contains(&host.hardware.architecture)
        {
            SupportEvaluation {
                tier: SupportTier::Primary,
                reasons: Vec::new(),
            }
        } else if self
            .compatible_architectures
            .contains(&host.hardware.architecture)
            || host.macos_version < self.primary_minimum
        {
            SupportEvaluation {
                tier: SupportTier::Compatible,
                reasons: Vec::new(),
            }
        } else {
            SupportEvaluation {
                tier: SupportTier::Unsupported,
                reasons: vec!["host does not match a supported architecture tier".to_string()],
            }
        }
    }
}

pub fn current_host_profile() -> Result<HostProfile> {
    Ok(HostProfile {
        macos_version: MacOsVersion::parse(&command_output("sw_vers", &["-productVersion"])?)?,
        build: command_output("sw_vers", &["-buildVersion"])?,
        hardware: HardwareProfile {
            architecture: CpuArchitecture::parse(&command_output("uname", &["-m"])?),
            memory_bytes: command_output("sysctl", &["-n", "hw.memsize"])?
                .trim()
                .parse()
                .map_err(|err| GfmError::Format(format!("invalid hw.memsize: {err}")))?,
            logical_cpus: command_output("sysctl", &["-n", "hw.logicalcpu"])?
                .trim()
                .parse()
                .map_err(|err| GfmError::Format(format!("invalid hw.logicalcpu: {err}")))?,
        },
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchDepth {
    Directory,
    Tree,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchRoot {
    pub path: PathBuf,
    pub depth: WatchDepth,
}

impl WatchRoot {
    pub fn tree(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            depth: WatchDepth::Tree,
        }
    }

    pub fn directory(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            depth: WatchDepth::Directory,
        }
    }
}

pub struct FileEventStream {
    _watcher: RecommendedWatcher,
    receiver: Receiver<Result<FileEvent>>,
}

impl FileEventStream {
    pub fn watch(roots: &[WatchRoot]) -> Result<Self> {
        let (tx, rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |event| {
            for mapped in map_notify_result(event) {
                let _ = tx.send(mapped);
            }
        })
        .map_err(|err| GfmError::Format(format!("failed to create watcher: {err}")))?;

        for root in roots {
            watcher
                .watch(&root.path, recursive_mode(root.depth))
                .map_err(|err| GfmError::io(&root.path, err))?;
        }

        Ok(Self {
            _watcher: watcher,
            receiver: rx,
        })
    }

    pub fn recv(&self) -> Result<FileEvent> {
        self.receiver
            .recv()
            .map_err(|err| GfmError::Format(format!("file event stream closed: {err}")))?
    }

    pub fn try_recv(&self) -> Option<Result<FileEvent>> {
        self.receiver.try_recv().ok()
    }
}

pub fn map_notify_event(event: Event) -> Vec<FileEvent> {
    if is_rescan(&event) {
        return paths_or_current(event.paths)
            .into_iter()
            .map(|path| FileEvent::new(path, FileEventKind::Rescan))
            .collect();
    }

    match event.kind {
        EventKind::Create(
            CreateKind::File | CreateKind::Folder | CreateKind::Any | CreateKind::Other,
        ) => event
            .paths
            .into_iter()
            .map(|path| FileEvent::new(path, FileEventKind::Create))
            .collect(),
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) if event.paths.len() >= 2 => {
            vec![FileEvent::new(
                event.paths[1].clone(),
                FileEventKind::Rename {
                    from: event.paths[0].clone(),
                    to: event.paths[1].clone(),
                },
            )]
        }
        EventKind::Modify(ModifyKind::Name(RenameMode::From)) => event
            .paths
            .into_iter()
            .map(|path| FileEvent::new(path, FileEventKind::Remove))
            .collect(),
        EventKind::Modify(ModifyKind::Name(RenameMode::To)) => event
            .paths
            .into_iter()
            .map(|path| FileEvent::new(path, FileEventKind::Create))
            .collect(),
        EventKind::Modify(_) => event
            .paths
            .into_iter()
            .map(|path| FileEvent::new(path, FileEventKind::Modify))
            .collect(),
        EventKind::Remove(
            RemoveKind::File | RemoveKind::Folder | RemoveKind::Any | RemoveKind::Other,
        ) => event
            .paths
            .into_iter()
            .map(|path| FileEvent::new(path, FileEventKind::Remove))
            .collect(),
        EventKind::Any | EventKind::Other => event
            .paths
            .into_iter()
            .map(|path| FileEvent::new(path, FileEventKind::Other))
            .collect(),
        EventKind::Access(_) => Vec::new(),
    }
}

fn map_notify_result(event: notify::Result<Event>) -> Vec<Result<FileEvent>> {
    match event {
        Ok(event) => map_notify_event(event).into_iter().map(Ok).collect(),
        Err(err) => vec![Err(GfmError::Format(format!("watcher event error: {err}")))],
    }
}

fn recursive_mode(depth: WatchDepth) -> RecursiveMode {
    match depth {
        WatchDepth::Directory => RecursiveMode::NonRecursive,
        WatchDepth::Tree => RecursiveMode::Recursive,
    }
}

fn is_rescan(event: &Event) -> bool {
    event.need_rescan()
}

fn paths_or_current(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    if paths.is_empty() {
        vec![Path::new(".").to_path_buf()]
    } else {
        paths
    }
}

fn parse_version_part(value: Option<&str>, label: &str) -> Result<u16> {
    let value =
        value.ok_or_else(|| GfmError::Format(format!("missing macOS version {label} part")))?;
    value
        .parse()
        .map_err(|err| GfmError::Format(format!("invalid macOS version {label} `{value}`: {err}")))
}

fn command_output(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|err| GfmError::Format(format!("failed to run {program}: {err}")))?;
    if !output.status.success() {
        return Err(GfmError::Format(format!(
            "{program} {:?} failed with status {}: {}",
            args,
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_string())
        .map_err(|err| GfmError::Format(format!("{program} returned non-UTF-8 output: {err}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{DataChange, ModifyKind};
    use std::fs;
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    #[test]
    fn maps_rename_pair_to_single_domain_event() {
        let event = Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
            .add_path(PathBuf::from("/tmp/from.txt"))
            .add_path(PathBuf::from("/tmp/to.txt"));

        let mapped = map_notify_event(event);

        assert_eq!(mapped.len(), 1);
        assert_eq!(
            mapped[0].kind,
            FileEventKind::Rename {
                from: PathBuf::from("/tmp/from.txt"),
                to: PathBuf::from("/tmp/to.txt")
            }
        );
        assert_eq!(mapped[0].path, PathBuf::from("/tmp/to.txt"));
    }

    #[test]
    fn maps_data_change_to_modify_events() {
        let event = Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Content)))
            .add_path(PathBuf::from("/tmp/file.txt"));

        let mapped = map_notify_event(event);

        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].kind, FileEventKind::Modify);
    }

    #[test]
    fn parses_macos_versions() {
        assert_eq!(
            MacOsVersion::parse("15.6.1").unwrap(),
            MacOsVersion::new(15, 6, 1)
        );
        assert_eq!(
            MacOsVersion::parse("14").unwrap(),
            MacOsVersion::new(14, 0, 0)
        );
        assert!(MacOsVersion::parse("15.6.1.2").is_err());
    }

    #[test]
    fn evaluates_primary_compatible_and_unsupported_hosts() {
        let matrix = SupportMatrix::default();
        let primary = HostProfile {
            macos_version: MacOsVersion::new(15, 1, 0),
            build: "24B83".to_string(),
            hardware: HardwareProfile {
                architecture: CpuArchitecture::AppleSilicon,
                memory_bytes: 16 * 1024 * 1024 * 1024,
                logical_cpus: 8,
            },
        };
        let compatible = HostProfile {
            macos_version: MacOsVersion::new(14, 7, 0),
            build: "23H124".to_string(),
            hardware: HardwareProfile {
                architecture: CpuArchitecture::Intel64,
                memory_bytes: 16 * 1024 * 1024 * 1024,
                logical_cpus: 8,
            },
        };
        let unsupported = HostProfile {
            macos_version: MacOsVersion::new(13, 6, 0),
            build: "22G120".to_string(),
            hardware: HardwareProfile {
                architecture: CpuArchitecture::AppleSilicon,
                memory_bytes: 4 * 1024 * 1024 * 1024,
                logical_cpus: 2,
            },
        };

        assert_eq!(matrix.evaluate(&primary).tier, SupportTier::Primary);
        assert_eq!(matrix.evaluate(&compatible).tier, SupportTier::Compatible);
        let rejected = matrix.evaluate(&unsupported);
        assert_eq!(rejected.tier, SupportTier::Unsupported);
        assert_eq!(rejected.reasons.len(), 3);
    }

    #[test]
    fn native_watcher_observes_real_file_mutation() {
        let root = unique_temp_dir("gfm-watch-root");
        let stream = FileEventStream::watch(&[WatchRoot::tree(&root)]).unwrap();
        let target = root.join("created.txt");
        fs::write(&target, "hello").unwrap();

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut observed = Vec::new();
        while Instant::now() < deadline {
            if let Some(event) = stream.try_recv() {
                let event = event.unwrap();
                observed.push(event.clone());
                if event.path == target || event.path == root {
                    fs::remove_dir_all(root).unwrap();
                    return;
                }
            }
            thread::sleep(Duration::from_millis(25));
        }

        fs::remove_dir_all(root).unwrap();
        panic!("watcher did not observe mutation; observed events: {observed:?}");
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "{}-{}",
            prefix,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
