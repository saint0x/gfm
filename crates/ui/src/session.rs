use crate::AppLaunchSpec;
use gpui::{bounds, point, px, size, Bounds, Pixels, WindowBounds};
use std::env;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CASCADE_OFFSET_PX: f32 = 24.0;
const MAX_CASCADE_STEPS: u32 = 8;
const BOUNDS_WRITE_DEBOUNCE: Duration = Duration::from_millis(80);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestorePolicy {
    RestoreLastWindowBounds,
    CenterWhenNoStoredBounds,
}

impl RestorePolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RestoreLastWindowBounds => "restore-last-window-bounds",
            Self::CenterWhenNoStoredBounds => "center-when-no-stored-bounds",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacementPolicy {
    PersistedOrCentered,
}

impl PlacementPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PersistedOrCentered => "persisted-or-centered",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabPolicy {
    NativeMacosTabGroup,
}

impl TabPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NativeMacosTabGroup => "native-macos-tab-group",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationPolicy {
    ActivateAppAndFocusNewWindow,
}

impl ActivationPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ActivateAppAndFocusNewWindow => "activate-app-and-focus-new-window",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowPlacement {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl WindowPlacement {
    pub fn from_bounds(bounds: Bounds<Pixels>) -> Self {
        Self {
            x: f32::from(bounds.origin.x),
            y: f32::from(bounds.origin.y),
            width: f32::from(bounds.size.width),
            height: f32::from(bounds.size.height),
        }
    }

    pub fn to_bounds(self) -> Option<Bounds<Pixels>> {
        self.is_valid().then(|| {
            bounds(
                point(px(self.x), px(self.y)),
                size(px(self.width), px(self.height)),
            )
        })
    }

    pub fn cascade(self, ordinal: u32) -> Self {
        let step = ordinal.min(MAX_CASCADE_STEPS) as f32;
        Self {
            x: self.x + step * CASCADE_OFFSET_PX,
            y: self.y + step * CASCADE_OFFSET_PX,
            ..self
        }
    }

    pub fn is_valid(self) -> bool {
        self.width >= 320.0
            && self.height >= 240.0
            && self.x.is_finite()
            && self.y.is_finite()
            && self.width.is_finite()
            && self.height.is_finite()
    }

    pub fn as_field(self) -> String {
        format!(
            "{:.0},{:.0},{:.0},{:.0}",
            self.x, self.y, self.width, self.height
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowSessionContract {
    pub restore_policy: RestorePolicy,
    pub placement_policy: PlacementPolicy,
    pub tab_policy: TabPolicy,
    pub activation_policy: ActivationPolicy,
    pub tabbing_identifier: String,
    pub restore_key: String,
    pub placement_store: PathBuf,
    pub placement: Option<WindowPlacement>,
    pub cascade_ordinal: u32,
    pub focus_new_window: bool,
    pub show_on_open: bool,
    pub movable: bool,
    pub resizable: bool,
    pub minimizable: bool,
}

impl WindowSessionContract {
    pub fn from_spec(spec: &AppLaunchSpec, store: &WindowSessionStore, ordinal: u32) -> Self {
        let placement = store
            .load_window_bounds()
            .ok()
            .flatten()
            .map(|placement| placement.cascade(ordinal))
            .filter(|placement| placement.is_valid());

        Self {
            restore_policy: RestorePolicy::RestoreLastWindowBounds,
            placement_policy: PlacementPolicy::PersistedOrCentered,
            tab_policy: TabPolicy::NativeMacosTabGroup,
            activation_policy: ActivationPolicy::ActivateAppAndFocusNewWindow,
            tabbing_identifier: spec.tabbing_identifier.clone(),
            restore_key: "main-window".to_string(),
            placement_store: store.path().to_path_buf(),
            placement,
            cascade_ordinal: ordinal,
            focus_new_window: true,
            show_on_open: true,
            movable: true,
            resizable: true,
            minimizable: true,
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "session\trestore={}\tplacement-policy={}\ttab-policy={}\tactivation={}\ttabs={}\trestore-key={}\tstore={}\tplacement={}\tcascade={}\tfocus={}\tshow={}\tmovable={}\tresizable={}\tminimizable={}",
            self.restore_policy.as_str(),
            self.placement_policy.as_str(),
            self.tab_policy.as_str(),
            self.activation_policy.as_str(),
            self.tabbing_identifier,
            self.restore_key,
            self.placement_store.display(),
            self.placement
                .map(WindowPlacement::as_field)
                .unwrap_or_else(|| "centered".to_string()),
            self.cascade_ordinal,
            self.focus_new_window,
            self.show_on_open,
            self.movable,
            self.resizable,
            self.minimizable
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowSessionStore {
    path: PathBuf,
}

impl WindowSessionStore {
    pub fn platform_default() -> Self {
        Self::new(default_store_path())
    }

    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load_window_bounds(&self) -> io::Result<Option<WindowPlacement>> {
        self.load_window_bounds_checked(|| Ok(()))
    }

    pub fn load_window_bounds_checked(
        &self,
        mut check_control: impl FnMut() -> io::Result<()>,
    ) -> io::Result<Option<WindowPlacement>> {
        const CHUNK_BYTES: usize = 64 * 1024;

        check_control()?;
        let mut file = match File::open(&self.path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err),
        };
        check_control()?;
        let mut bytes = Vec::new();
        let mut buffer = [0; CHUNK_BYTES];
        loop {
            check_control()?;
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
            check_control()?;
        }
        check_control()?;
        let content = String::from_utf8(bytes)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        Ok(parse_store(&content))
    }

    pub fn save_window_bounds(&self, bounds: WindowBounds) -> io::Result<()> {
        self.save_window_bounds_checked(bounds, || Ok(()))
    }

    pub fn save_window_bounds_checked(
        &self,
        bounds: WindowBounds,
        mut check_control: impl FnMut() -> io::Result<()>,
    ) -> io::Result<()> {
        check_control()?;
        let placement = WindowPlacement::from_bounds(bounds.get_bounds());
        if !placement.is_valid() {
            return Ok(());
        }
        check_control()?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        check_control()?;
        let tmp = self.temp_path();
        let content = format!("main-window\t{}\n", placement.as_field());
        let result = (|| {
            let mut file = File::create(&tmp)?;
            check_control()?;
            for chunk in content.as_bytes().chunks(64 * 1024) {
                check_control()?;
                file.write_all(chunk)?;
            }
            check_control()?;
            file.sync_all()?;
            check_control()?;
            fs::rename(&tmp, &self.path)?;
            sync_parent(&self.path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }

    fn temp_path(&self) -> PathBuf {
        let file_name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("window-session.tsv");
        self.path.with_file_name(format!(
            ".{file_name}.{}.{nonce}.tmp",
            std::process::id(),
            nonce = now_nanos()
        ))
    }
}

#[derive(Debug, Clone)]
pub struct WindowSessionWriter {
    updates: mpsc::Sender<WindowBounds>,
}

impl WindowSessionWriter {
    pub fn new(store: WindowSessionStore) -> Self {
        let (updates, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("gfm-window-session-writer".to_string())
            .spawn(move || write_coalesced_bounds(store, receiver))
            .expect("spawn window session writer");
        Self { updates }
    }

    pub fn save_bounds(&self, bounds: WindowBounds) {
        let _ = self.updates.send(bounds);
    }
}

fn write_coalesced_bounds(store: WindowSessionStore, receiver: mpsc::Receiver<WindowBounds>) {
    while let Ok(mut latest) = receiver.recv() {
        loop {
            match receiver.recv_timeout(BOUNDS_WRITE_DEBOUNCE) {
                Ok(bounds) => latest = bounds,
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        let _ = store.save_window_bounds(latest);
    }
}

fn parse_store(content: &str) -> Option<WindowPlacement> {
    content.lines().find_map(|line| {
        let (key, value) = line.split_once('\t')?;
        (key == "main-window")
            .then(|| parse_placement(value))
            .flatten()
            .filter(|placement| placement.is_valid())
    })
}

fn parse_placement(value: &str) -> Option<WindowPlacement> {
    let mut fields = value.split(',');
    let placement = WindowPlacement {
        x: fields.next()?.parse().ok()?,
        y: fields.next()?.parse().ok()?,
        width: fields.next()?.parse().ok()?,
        height: fields.next()?.parse().ok()?,
    };
    fields.next().is_none().then_some(placement)
}

fn default_store_path() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library/Application Support/GFM/window-session.tsv")
}

fn sync_parent(path: &Path) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    File::open(parent).and_then(|file| file.sync_all())
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::WindowBounds;

    #[test]
    fn contract_reports_centered_when_no_restore_exists() {
        let store = WindowSessionStore::new("/tmp/gfm-missing-window-session.tsv");
        let contract = WindowSessionContract::from_spec(&AppLaunchSpec::default(), &store, 0);

        assert_eq!(contract.placement, None);
        assert_eq!(contract.tabbing_identifier, "gfm-main-window");
        assert_eq!(
            contract.restore_policy,
            RestorePolicy::RestoreLastWindowBounds
        );
        assert!(contract.focus_new_window);
        assert!(contract.as_tsv().contains("placement=centered"));
    }

    #[test]
    fn store_round_trips_window_bounds_atomically() {
        let root = env::temp_dir().join(format!("gfm-session-test-{}", std::process::id()));
        let path = root.join("window-session.tsv");
        let store = WindowSessionStore::new(&path);
        let bounds = WindowBounds::Windowed(bounds(
            point(px(10.0), px(20.0)),
            size(px(1040.0), px(720.0)),
        ));

        store.save_window_bounds(bounds).unwrap();
        let loaded = store.load_window_bounds().unwrap().unwrap();

        assert_eq!(loaded.as_field(), "10,20,1040,720");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn store_temp_paths_are_unique_within_process() {
        let root = unique_temp_dir("gfm-session-unique-temp");
        let store = WindowSessionStore::new(root.join("window-session.tsv"));

        let first = store.temp_path();
        let second = store.temp_path();

        assert_ne!(first, second);
        assert_eq!(first.parent(), Some(root.as_path()));
        assert_eq!(second.parent(), Some(root.as_path()));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn load_window_bounds_checked_honors_pre_cancelled_control_before_open() {
        let root = unique_temp_dir("gfm-session-load-pre-cancel");
        let store = WindowSessionStore::new(root.join("window-session.tsv"));

        let err = store
            .load_window_bounds_checked(|| Err(cancelled_io()))
            .unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
        assert!(!store.path().exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn save_window_bounds_checked_honors_pre_cancelled_control_before_file_create() {
        let root = unique_temp_dir("gfm-session-save-pre-cancel");
        let store = WindowSessionStore::new(root.join("window-session.tsv"));
        let bounds = WindowBounds::Windowed(bounds(
            point(px(10.0), px(20.0)),
            size(px(1040.0), px(720.0)),
        ));

        let err = store
            .save_window_bounds_checked(bounds, || Err(cancelled_io()))
            .unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
        assert!(!store.path().exists());
        assert_eq!(session_temp_count(store.path()), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn save_window_bounds_checked_preserves_existing_bounds_on_cancel() {
        let root = unique_temp_dir("gfm-session-save-cancel-preserve");
        let store = WindowSessionStore::new(root.join("window-session.tsv"));
        let original = WindowBounds::Windowed(bounds(
            point(px(10.0), px(20.0)),
            size(px(1040.0), px(720.0)),
        ));
        let replacement = WindowBounds::Windowed(bounds(
            point(px(30.0), px(40.0)),
            size(px(1200.0), px(800.0)),
        ));
        store.save_window_bounds(original).unwrap();
        let before = fs::read(store.path()).unwrap();
        let mut checks = 0usize;

        let err = store
            .save_window_bounds_checked(replacement, || {
                checks += 1;
                if checks >= 5 {
                    Err(cancelled_io())
                } else {
                    Ok(())
                }
            })
            .unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
        assert!(checks >= 5);
        assert_eq!(fs::read(store.path()).unwrap(), before);
        assert_eq!(
            store.load_window_bounds().unwrap().unwrap().as_field(),
            "10,20,1040,720"
        );
        assert_eq!(session_temp_count(store.path()), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restored_windows_cascade_by_ordinal() {
        let placement = WindowPlacement {
            x: 10.0,
            y: 20.0,
            width: 1040.0,
            height: 720.0,
        };

        assert_eq!(placement.cascade(2).as_field(), "58,68,1040,720");
    }

    #[test]
    fn invalid_store_content_is_ignored() {
        assert_eq!(parse_store("main-window\tbad\n"), None);
        assert_eq!(parse_store("other\t1,2,3,4\n"), None);
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let path =
            env::temp_dir().join(format!("{}-{}-{}", prefix, std::process::id(), now_nanos()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn cancelled_io() -> io::Error {
        io::Error::new(io::ErrorKind::Interrupted, "cancelled")
    }

    fn session_temp_count(path: &Path) -> usize {
        let Some(parent) = path.parent() else {
            return 0;
        };
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            return 0;
        };
        let prefix = format!(".{file_name}.{}.", std::process::id());
        fs::read_dir(parent)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
            .count()
    }
}
