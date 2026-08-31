use gpui::{div, prelude::*, px, rgb, IntoElement, Styled};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const SIDEBAR_WIDTH: f32 = 188.0;
const ROW_HEIGHT: f32 = 28.0;
const SECTION_HEADER_HEIGHT: f32 = 26.0;

const SECTIONS: [&str; 4] = ["Favorites", "iCloud", "Locations", "Tags"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarItemKind {
    Favorite,
    Cloud,
    Location,
    Tag,
}

impl SidebarItemKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Favorite => "favorite",
            Self::Cloud => "cloud",
            Self::Location => "location",
            Self::Tag => "tag",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarCloudState {
    None,
    AvailableOffline,
    CloudOnly,
    Downloading,
    Syncing,
    Waiting,
    Unavailable,
    Conflict,
}

impl SidebarCloudState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::AvailableOffline => "available-offline",
            Self::CloudOnly => "cloud-only",
            Self::Downloading => "downloading",
            Self::Syncing => "syncing",
            Self::Waiting => "waiting",
            Self::Unavailable => "unavailable",
            Self::Conflict => "conflict",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarPathState {
    Available,
    Missing,
    Unavailable,
    Virtual,
}

impl SidebarPathState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Missing => "missing",
            Self::Unavailable => "unavailable",
            Self::Virtual => "virtual",
        }
    }

    const fn enables_row(self) -> bool {
        matches!(self, Self::Available | Self::Virtual)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarItemSpec {
    pub section: &'static str,
    pub id: String,
    pub label: String,
    pub role: &'static str,
    pub kind: SidebarItemKind,
    pub path: Option<PathBuf>,
    pub icon: &'static str,
    pub depth: u8,
    pub enabled: bool,
    pub selected: bool,
    pub ejectable: bool,
    pub virtual_item: bool,
    pub path_state: SidebarPathState,
    pub cloud_state: SidebarCloudState,
    pub cloud_progress_milli: Option<u32>,
    pub volume_kind: Option<SidebarVolumeKind>,
    pub volume_mount_state: Option<SidebarVolumeMountState>,
    pub volume_read_only: Option<bool>,
    pub volume_network: Option<bool>,
    pub volume_reachable: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarCloudInvalidation {
    pub row_id: String,
    pub path: PathBuf,
    pub previous: SidebarCloudState,
    pub current: SidebarCloudState,
    pub progress_milli: Option<u32>,
    pub invalidate_row: bool,
    pub reason: String,
}

impl SidebarCloudInvalidation {
    pub fn new(
        path: impl Into<PathBuf>,
        previous: SidebarCloudState,
        current: SidebarCloudState,
        progress_milli: Option<u32>,
        provider_invalidated_sidebar: bool,
        provider_reason: impl Into<String>,
    ) -> Self {
        let path = path.into();
        let visible = previous != SidebarCloudState::None || current != SidebarCloudState::None;
        let invalidate_row = visible && provider_invalidated_sidebar;
        let reason = if !visible {
            "sidebar-cloud-not-visible".to_string()
        } else if !provider_invalidated_sidebar {
            "provider-did-not-invalidate-sidebar".to_string()
        } else if previous != current {
            "sidebar-cloud-state-changed".to_string()
        } else {
            provider_reason.into()
        };

        Self {
            row_id: "icloud-drive".to_string(),
            path,
            previous,
            current,
            progress_milli,
            invalidate_row,
            reason,
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "sidebar-cloud-invalidation\t{}\tpath={}\tprevious={}\tcurrent={}\tprogress={}\tinvalidate-row={}\treason={}",
            self.row_id,
            self.path.display(),
            self.previous.as_str(),
            self.current.as_str(),
            self.progress_milli
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.invalidate_row,
            self.reason
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarVolumeEventKind {
    Appeared,
    DescriptionChanged,
    Disappeared,
    Unavailable,
}

impl SidebarVolumeEventKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Appeared => "appeared",
            Self::DescriptionChanged => "description-changed",
            Self::Disappeared => "disappeared",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarVolumeInvalidation {
    pub row_id: Option<String>,
    pub path: Option<PathBuf>,
    pub kind: SidebarVolumeEventKind,
    pub previous_kind: Option<SidebarVolumeKind>,
    pub previous_mount_state: Option<SidebarVolumeMountState>,
    pub previous_read_only: Option<bool>,
    pub previous_network: Option<bool>,
    pub previous_reachable: Option<bool>,
    pub previous_native_status: Option<String>,
    pub previous_resource_status: Option<String>,
    pub previous_mount_status: Option<String>,
    pub current_kind: Option<SidebarVolumeKind>,
    pub current_mount_state: Option<SidebarVolumeMountState>,
    pub current_read_only: Option<bool>,
    pub current_network: Option<bool>,
    pub current_reachable: Option<bool>,
    pub current_native_status: Option<String>,
    pub current_resource_status: Option<String>,
    pub current_mount_status: Option<String>,
    pub invalidate_row: bool,
    pub invalidate_section: bool,
    pub remove_row: bool,
    pub disable_row: bool,
    pub reason: String,
}

impl SidebarVolumeInvalidation {
    pub fn from_event(
        kind: SidebarVolumeEventKind,
        path: Option<PathBuf>,
        previous: Option<&SidebarVolumeSpec>,
        current: Option<&SidebarVolumeSpec>,
        platform_invalidated_sidebar: bool,
        platform_reason: impl Into<String>,
    ) -> Self {
        let row = if matches!(kind, SidebarVolumeEventKind::Disappeared) {
            previous.or(current)
        } else {
            current.or(previous)
        };
        let row_id = row.map(|volume| volume.id.clone());
        let previous_kind = previous.map(|volume| volume.kind);
        let previous_mount_state = previous.map(|volume| volume.mount_state);
        let previous_read_only = previous.map(|volume| volume.read_only);
        let previous_network = previous.map(|volume| volume.network);
        let previous_reachable = previous.and_then(|volume| volume.reachable);
        let current_kind = current.map(|volume| volume.kind);
        let current_mount_state = current.map(|volume| volume.mount_state);
        let current_read_only = current.map(|volume| volume.read_only);
        let current_network = current.map(|volume| volume.network);
        let current_reachable = current.and_then(|volume| volume.reachable);
        let remove_row = matches!(kind, SidebarVolumeEventKind::Disappeared);
        let disable_row = matches!(kind, SidebarVolumeEventKind::Unavailable)
            || current_mount_state.is_some_and(|state| {
                matches!(
                    state,
                    SidebarVolumeMountState::Unmounted | SidebarVolumeMountState::Stale
                )
            })
            || current_reachable == Some(false);
        let visible =
            row_id.is_some() || path.is_some() || kind == SidebarVolumeEventKind::Unavailable;
        let invalidate_section = visible && platform_invalidated_sidebar;
        let invalidate_row = invalidate_section && (row_id.is_some() || remove_row || disable_row);
        let reason = if !visible {
            "sidebar-volume-not-visible".to_string()
        } else if !platform_invalidated_sidebar {
            "platform-did-not-invalidate-sidebar".to_string()
        } else if remove_row {
            "sidebar-volume-disappeared".to_string()
        } else if disable_row {
            "sidebar-volume-disabled".to_string()
        } else {
            platform_reason.into()
        };

        Self {
            row_id,
            path,
            kind,
            previous_kind,
            previous_mount_state,
            previous_read_only,
            previous_network,
            previous_reachable,
            previous_native_status: None,
            previous_resource_status: None,
            previous_mount_status: None,
            current_kind,
            current_mount_state,
            current_read_only,
            current_network,
            current_reachable,
            current_native_status: None,
            current_resource_status: None,
            current_mount_status: None,
            invalidate_row,
            invalidate_section,
            remove_row,
            disable_row,
            reason,
        }
    }

    pub fn with_platform_statuses(
        mut self,
        previous_native_status: Option<String>,
        previous_resource_status: Option<String>,
        previous_mount_status: Option<String>,
        current_native_status: Option<String>,
        current_resource_status: Option<String>,
        current_mount_status: Option<String>,
    ) -> Self {
        self.previous_native_status = previous_native_status;
        self.previous_resource_status = previous_resource_status;
        self.previous_mount_status = previous_mount_status;
        self.current_native_status = current_native_status;
        self.current_resource_status = current_resource_status;
        self.current_mount_status = current_mount_status;
        if self.current_platform_status_disables_row() {
            self.disable_row = true;
            if self.invalidate_section && self.row_id.is_some() && !self.remove_row {
                self.invalidate_row = true;
            }
        }
        self
    }

    fn current_platform_status_disables_row(&self) -> bool {
        [
            self.current_native_status.as_deref(),
            self.current_resource_status.as_deref(),
            self.current_mount_status.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|status| matches!(status, "missing" | "unavailable"))
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "sidebar-volume-invalidation\trow={}\tpath={}\tkind={}\tprevious-kind={}\tprevious-mount={}\tprevious-read-only={}\tprevious-network={}\tprevious-reachable={}\tprevious-native-status={}\tprevious-resource-status={}\tprevious-mount-status={}\tcurrent-kind={}\tcurrent-mount={}\tread-only={}\tnetwork={}\treachable={}\tcurrent-native-status={}\tcurrent-resource-status={}\tcurrent-mount-status={}\tinvalidate-row={}\tinvalidate-section={}\tremove-row={}\tdisable-row={}\treason={}",
            self.row_id.as_deref().unwrap_or("-"),
            self.path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.kind.as_str(),
            self.previous_kind.map(SidebarVolumeKind::as_str).unwrap_or("-"),
            self.previous_mount_state
                .map(SidebarVolumeMountState::as_str)
                .unwrap_or("-"),
            self.previous_read_only
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.previous_network
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.previous_reachable
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.previous_native_status.as_deref().unwrap_or("-"),
            self.previous_resource_status.as_deref().unwrap_or("-"),
            self.previous_mount_status.as_deref().unwrap_or("-"),
            self.current_kind.map(SidebarVolumeKind::as_str).unwrap_or("-"),
            self.current_mount_state
                .map(SidebarVolumeMountState::as_str)
                .unwrap_or("-"),
            self.current_read_only
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.current_network
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.current_reachable
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.current_native_status.as_deref().unwrap_or("-"),
            self.current_resource_status.as_deref().unwrap_or("-"),
            self.current_mount_status.as_deref().unwrap_or("-"),
            self.invalidate_row,
            self.invalidate_section,
            self.remove_row,
            self.disable_row,
            self.reason
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarVolumeSpec {
    pub id: String,
    pub label: String,
    pub path: PathBuf,
    pub ejectable: bool,
    pub kind: SidebarVolumeKind,
    pub mount_state: SidebarVolumeMountState,
    pub read_only: bool,
    pub network: bool,
    pub reachable: Option<bool>,
}

impl SidebarVolumeSpec {
    pub fn from_native_seed(
        seed: impl AsRef<str>,
        label: impl Into<String>,
        path: impl Into<PathBuf>,
        ejectable: bool,
    ) -> Self {
        Self {
            id: stable_id("volume", seed.as_ref()),
            label: label.into(),
            path: path.into(),
            ejectable,
            kind: SidebarVolumeKind::External,
            mount_state: SidebarVolumeMountState::Mounted,
            read_only: false,
            network: false,
            reachable: Some(true),
        }
    }

    pub fn with_volume_state(
        mut self,
        kind: SidebarVolumeKind,
        mount_state: SidebarVolumeMountState,
        read_only: bool,
        network: bool,
        reachable: Option<bool>,
    ) -> Self {
        self.kind = kind;
        self.mount_state = mount_state;
        self.read_only = read_only;
        self.network = network;
        self.reachable = reachable;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarVolumeKind {
    Internal,
    External,
    Removable,
    Network,
    DiskImage,
    Unknown,
}

impl SidebarVolumeKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Internal => "internal",
            Self::External => "external",
            Self::Removable => "removable",
            Self::Network => "network",
            Self::DiskImage => "disk-image",
            Self::Unknown => "unknown",
        }
    }

    const fn role(self) -> &'static str {
        match self {
            Self::Network => "network-volume",
            Self::DiskImage => "disk-image",
            _ => "mounted-volume",
        }
    }

    const fn icon(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::DiskImage => "disk-image",
            Self::Internal => "internal-disk",
            _ => "external-disk",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarVolumeMountState {
    Mounted,
    Unmounted,
    Stale,
}

impl SidebarVolumeMountState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mounted => "mounted",
            Self::Unmounted => "unmounted",
            Self::Stale => "stale",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SidebarEnvironment {
    home: PathBuf,
    icloud_drive: Option<PathBuf>,
    icloud_state: SidebarCloudState,
    icloud_progress_milli: Option<u32>,
    volumes: Vec<SidebarVolumeSpec>,
}

impl SidebarEnvironment {
    pub fn discover() -> Self {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| PathBuf::from("/"));
        let icloud_drive = existing_path(home.join("Library/Mobile Documents/com~apple~CloudDocs"));

        Self {
            home,
            icloud_drive,
            icloud_state: SidebarCloudState::None,
            icloud_progress_milli: None,
            volumes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarContract {
    pub width_px: u16,
    pub row_height_px: u16,
    pub section_header_height_px: u16,
    pub sections: Vec<&'static str>,
    pub rows: Vec<SidebarItemSpec>,
}

impl SidebarContract {
    pub fn discover(current_path: impl AsRef<Path>) -> Self {
        Self::from_environment(current_path, SidebarEnvironment::discover())
    }

    pub fn discover_with_volumes(
        current_path: impl AsRef<Path>,
        volumes: Vec<SidebarVolumeSpec>,
    ) -> Self {
        let mut environment = SidebarEnvironment::discover();
        environment.volumes = volumes;
        Self::from_environment(current_path, environment)
    }

    pub fn discover_with_icloud_state(
        current_path: impl AsRef<Path>,
        icloud_drive: impl Into<PathBuf>,
        cloud_state: SidebarCloudState,
        progress_milli: Option<u32>,
    ) -> Self {
        let mut environment = SidebarEnvironment::discover();
        environment.icloud_drive = Some(icloud_drive.into());
        environment.icloud_state = cloud_state;
        environment.icloud_progress_milli = progress_milli;
        Self::from_environment(current_path, environment)
    }

    fn from_environment(current_path: impl AsRef<Path>, environment: SidebarEnvironment) -> Self {
        let current_path = current_path.as_ref();
        let mut rows = Vec::new();

        rows.extend(favorite_rows(&environment.home, current_path));
        rows.push(icloud_row(
            environment.icloud_drive.as_deref(),
            environment.icloud_state,
            environment.icloud_progress_milli,
            current_path,
        ));
        rows.extend(location_rows(&environment.volumes, current_path));
        rows.extend(tag_rows());

        Self {
            width_px: SIDEBAR_WIDTH as u16,
            row_height_px: ROW_HEIGHT as u16,
            section_header_height_px: SECTION_HEADER_HEIGHT as u16,
            sections: SECTIONS.to_vec(),
            rows,
        }
    }

    pub fn as_tsv(&self) -> String {
        let mut lines = Vec::with_capacity(self.rows.len() + 1);
        lines.push(format!(
            "sidebar\twidth={}\trow-height={}\tsection-header-height={}\tsections={}",
            self.width_px,
            self.row_height_px,
            self.section_header_height_px,
            self.sections.join(",")
        ));
        lines.extend(self.rows.iter().map(|row| {
            format!(
                "row\t{}\t{}\t{}\t{}\t{}\t{}\tdepth={}\tenabled={}\tselected={}\tejectable={}\tvirtual={}\tpath-state={}\tcloud={}\tcloud-progress={}\tvolume-kind={}\tvolume-mount={}\tvolume-read-only={}\tvolume-network={}\tvolume-reachable={}",
                row.section,
                row.id,
                row.label,
                row.role,
                row.kind.as_str(),
                row.path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "-".to_string()),
                row.depth,
                row.enabled,
                row.selected,
                row.ejectable,
                row.virtual_item,
                row.path_state.as_str(),
                row.cloud_state.as_str(),
                row.cloud_progress_milli
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                row.volume_kind
                    .map(SidebarVolumeKind::as_str)
                    .unwrap_or("-"),
                row.volume_mount_state
                    .map(SidebarVolumeMountState::as_str)
                    .unwrap_or("-"),
                row.volume_read_only
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                row.volume_network
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                row.volume_reachable
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string())
            )
        }));
        lines.join("\n")
    }
}

pub fn render(contract: &SidebarContract) -> impl IntoElement {
    div()
        .id("gfm-sidebar")
        .flex()
        .flex_col()
        .flex_shrink_0()
        .w(px(SIDEBAR_WIDTH))
        .h_full()
        .pt(px(10.0))
        .px_2()
        .bg(rgb(0x242424))
        .text_color(rgb(0xd0d0d0))
        .child(render_section("Favorites", &contract.rows))
        .child(render_section("iCloud", &contract.rows))
        .child(render_section("Locations", &contract.rows))
        .child(render_section("Tags", &contract.rows))
}

fn render_section(section: &'static str, rows: &[SidebarItemSpec]) -> gpui::Div {
    let mut container = div().flex().flex_col().w_full().mb_2();
    container = container.child(
        div()
            .flex()
            .items_center()
            .h(px(SECTION_HEADER_HEIGHT))
            .pl(px(8.0))
            .text_xs()
            .text_color(rgb(0x8b8b8b))
            .child(section),
    );

    for row in rows.iter().filter(|row| row.section == section) {
        container = container.child(render_row(row));
    }

    container
}

fn render_row(row: &SidebarItemSpec) -> gpui::Div {
    let background = if row.selected {
        rgb(0x4a4a4a)
    } else {
        rgb(0x242424)
    };
    let text_color = if row.enabled {
        rgb(0xd8d8d8)
    } else {
        rgb(0x777777)
    };

    div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .h(px(ROW_HEIGHT))
        .pl(px(8.0 + f32::from(row.depth) * 12.0))
        .pr(px(6.0))
        .gap_2()
        .rounded(px(6.0))
        .bg(background)
        .text_color(text_color)
        .child(icon_cell(row))
        .child(div().flex_1().truncate().text_sm().child(row.label.clone()))
        .child(cloud_cell(row))
        .child(eject_cell(row))
}

fn icon_cell(row: &SidebarItemSpec) -> gpui::Div {
    let color = match row.kind {
        SidebarItemKind::Favorite => rgb(0x8f8f8f),
        SidebarItemKind::Cloud => rgb(0x8f8f8f),
        SidebarItemKind::Location => rgb(0x8f8f8f),
        SidebarItemKind::Tag => tag_color(&row.id),
    };

    div()
        .flex()
        .items_center()
        .justify_center()
        .size(px(16.0))
        .rounded(px(8.0))
        .bg(color)
        .text_xs()
}

fn cloud_cell(row: &SidebarItemSpec) -> gpui::Div {
    let color = match row.cloud_state {
        SidebarCloudState::None => return div().w(px(12.0)).h(px(ROW_HEIGHT)),
        SidebarCloudState::AvailableOffline => rgb(0x8f8f8f),
        SidebarCloudState::CloudOnly => rgb(0x8f8f8f),
        SidebarCloudState::Downloading => rgb(0x0a84ff),
        SidebarCloudState::Syncing => rgb(0x32d74b),
        SidebarCloudState::Waiting => rgb(0xffd60a),
        SidebarCloudState::Unavailable => rgb(0xff453a),
        SidebarCloudState::Conflict => rgb(0xff9f0a),
    };

    div()
        .flex()
        .items_center()
        .justify_center()
        .size(px(8.0))
        .rounded(px(4.0))
        .bg(color)
}

fn eject_cell(row: &SidebarItemSpec) -> gpui::Div {
    if row.ejectable {
        div()
            .flex()
            .items_center()
            .justify_center()
            .w(px(14.0))
            .h(px(ROW_HEIGHT))
            .text_xs()
            .text_color(rgb(0x8b8b8b))
            .child("^")
    } else {
        div().w(px(14.0)).h(px(ROW_HEIGHT))
    }
}

fn favorite_rows(home: &Path, current_path: &Path) -> Vec<SidebarItemSpec> {
    vec![
        row(RowDescriptor::new(
            "Favorites",
            "macintosh-hd",
            "Macintosh HD",
            "filesystem-root",
            SidebarItemKind::Favorite,
            "internal-disk",
        )
        .path(PathBuf::from("/"))
        .state(RowState::path(
            SidebarPathState::Available,
            current_path == Path::new("/"),
        ))),
        row(RowDescriptor::new(
            "Favorites",
            "home",
            home.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("Home"),
            "home-folder",
            SidebarItemKind::Favorite,
            "home",
        )
        .path(home.to_path_buf())
        .state({
            let state = path_state(home);
            RowState::path(state, state.enables_row() && same_path(home, current_path))
        })),
        row(RowDescriptor::new(
            "Favorites",
            "recents",
            "Recents",
            "recents",
            SidebarItemKind::Favorite,
            "clock",
        )
        .state(RowState::virtual_item(true, false))),
        favorite_path(
            "desktop",
            "Desktop",
            "desktop-folder",
            home.join("Desktop"),
            current_path,
        ),
        favorite_path(
            "applications",
            "Applications",
            "applications-folder",
            PathBuf::from("/Applications"),
            current_path,
        ),
        favorite_path(
            "documents",
            "Documents",
            "documents-folder",
            home.join("Documents"),
            current_path,
        ),
        favorite_path(
            "downloads",
            "Downloads",
            "downloads-folder",
            home.join("Downloads"),
            current_path,
        ),
    ]
}

fn favorite_path(
    id: &'static str,
    label: &'static str,
    role: &'static str,
    path: PathBuf,
    current_path: &Path,
) -> SidebarItemSpec {
    let state = path_state(&path);
    let selected = state.enables_row() && same_path(&path, current_path);
    row(
        RowDescriptor::new("Favorites", id, label, role, SidebarItemKind::Favorite, id)
            .path(path)
            .state(RowState::path(state, selected)),
    )
}

fn icloud_row(
    icloud_drive: Option<&Path>,
    cloud_state: SidebarCloudState,
    progress_milli: Option<u32>,
    current_path: &Path,
) -> SidebarItemSpec {
    let path = icloud_drive.map(Path::to_path_buf);
    let path_state = path
        .as_ref()
        .map_or(SidebarPathState::Missing, |path| path_state(path));
    let selected = path_state.enables_row()
        && path
            .as_ref()
            .is_some_and(|path| same_path(path, current_path));
    row(RowDescriptor::new(
        "iCloud",
        "icloud-drive",
        "iCloud Drive",
        "icloud-drive",
        SidebarItemKind::Cloud,
        "icloud",
    )
    .optional_path(path)
    .cloud(cloud_state, progress_milli)
    .state(RowState::path(path_state, selected)))
}

fn location_rows(volumes: &[SidebarVolumeSpec], current_path: &Path) -> Vec<SidebarItemSpec> {
    let mut rows = vec![row(RowDescriptor::new(
        "Locations",
        "computer",
        "Computer",
        "computer",
        SidebarItemKind::Location,
        "computer",
    )
    .state(RowState::virtual_item(true, false)))];

    rows.extend(volumes.iter().map(|volume| {
        let state = path_state(&volume.path);
        let enabled = volume.mount_state == SidebarVolumeMountState::Mounted
            && volume.reachable != Some(false)
            && state.enables_row();
        row(RowDescriptor::new(
            "Locations",
            volume.id.clone(),
            volume.label.clone(),
            volume.kind.role(),
            SidebarItemKind::Location,
            volume.kind.icon(),
        )
        .path(volume.path.clone())
        .volume(volume)
        .state(RowState {
            path_state: state,
            enabled,
            selected: enabled && same_path(&volume.path, current_path),
            ejectable: volume.ejectable,
            virtual_item: false,
        }))
    }));

    rows
}

fn tag_rows() -> Vec<SidebarItemSpec> {
    [
        ("tag-red", "Red"),
        ("tag-orange", "Orange"),
        ("tag-yellow", "Yellow"),
        ("tag-green", "Green"),
        ("tag-blue", "Blue"),
        ("tag-purple", "Purple"),
        ("tag-gray", "Gray"),
        ("tag-all", "All Tags..."),
    ]
    .into_iter()
    .map(|(id, label)| {
        row(
            RowDescriptor::new("Tags", id, label, "finder-tag", SidebarItemKind::Tag, "tag")
                .state(RowState::virtual_item(true, false)),
        )
    })
    .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RowState {
    enabled: bool,
    selected: bool,
    ejectable: bool,
    virtual_item: bool,
    path_state: SidebarPathState,
}

impl RowState {
    const fn path(path_state: SidebarPathState, selected: bool) -> Self {
        Self {
            enabled: path_state.enables_row(),
            selected,
            ejectable: false,
            virtual_item: false,
            path_state,
        }
    }

    const fn virtual_item(enabled: bool, selected: bool) -> Self {
        Self {
            enabled,
            selected,
            ejectable: false,
            virtual_item: true,
            path_state: SidebarPathState::Virtual,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RowDescriptor {
    section: &'static str,
    id: String,
    label: String,
    role: &'static str,
    kind: SidebarItemKind,
    icon: &'static str,
    path: Option<PathBuf>,
    state: RowState,
    cloud_state: SidebarCloudState,
    cloud_progress_milli: Option<u32>,
    volume_kind: Option<SidebarVolumeKind>,
    volume_mount_state: Option<SidebarVolumeMountState>,
    volume_read_only: Option<bool>,
    volume_network: Option<bool>,
    volume_reachable: Option<bool>,
}

impl RowDescriptor {
    fn new(
        section: &'static str,
        id: impl Into<String>,
        label: impl Into<String>,
        role: &'static str,
        kind: SidebarItemKind,
        icon: &'static str,
    ) -> Self {
        Self {
            section,
            id: id.into(),
            label: label.into(),
            role,
            kind,
            icon,
            path: None,
            state: RowState::path(SidebarPathState::Available, false),
            cloud_state: SidebarCloudState::None,
            cloud_progress_milli: None,
            volume_kind: None,
            volume_mount_state: None,
            volume_read_only: None,
            volume_network: None,
            volume_reachable: None,
        }
    }

    fn path(mut self, path: PathBuf) -> Self {
        self.path = Some(path);
        self
    }

    fn optional_path(mut self, path: Option<PathBuf>) -> Self {
        self.path = path;
        self
    }

    fn state(mut self, state: RowState) -> Self {
        self.state = state;
        self
    }

    fn cloud(mut self, state: SidebarCloudState, progress_milli: Option<u32>) -> Self {
        self.cloud_state = state;
        self.cloud_progress_milli = progress_milli;
        self
    }

    fn volume(mut self, volume: &SidebarVolumeSpec) -> Self {
        self.volume_kind = Some(volume.kind);
        self.volume_mount_state = Some(volume.mount_state);
        self.volume_read_only = Some(volume.read_only);
        self.volume_network = Some(volume.network);
        self.volume_reachable = volume.reachable;
        self
    }
}

fn row(descriptor: RowDescriptor) -> SidebarItemSpec {
    SidebarItemSpec {
        section: descriptor.section,
        id: descriptor.id,
        label: descriptor.label,
        role: descriptor.role,
        kind: descriptor.kind,
        path: descriptor.path,
        icon: descriptor.icon,
        depth: 0,
        enabled: descriptor.state.enabled,
        selected: descriptor.state.selected,
        ejectable: descriptor.state.ejectable,
        virtual_item: descriptor.state.virtual_item,
        path_state: descriptor.state.path_state,
        cloud_state: descriptor.cloud_state,
        cloud_progress_milli: descriptor.cloud_progress_milli,
        volume_kind: descriptor.volume_kind,
        volume_mount_state: descriptor.volume_mount_state,
        volume_read_only: descriptor.volume_read_only,
        volume_network: descriptor.volume_network,
        volume_reachable: descriptor.volume_reachable,
    }
}

#[cfg(test)]
fn is_system_volume_label(label: &str) -> bool {
    matches!(label, "Macintosh HD" | "Recovery" | "Preboot" | "VM")
}

fn stable_id(prefix: &str, label: &str) -> String {
    let mut id = String::with_capacity(prefix.len() + label.len() + 1);
    id.push_str(prefix);
    id.push('-');
    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() {
            id.push(ch.to_ascii_lowercase());
        } else if !id.ends_with('-') {
            id.push('-');
        }
    }
    id.trim_end_matches('-').to_string()
}

fn existing_path(path: PathBuf) -> Option<PathBuf> {
    (path_state(&path) == SidebarPathState::Available).then_some(path)
}

#[cfg(test)]
fn volume_path_directory_state(path: &Path) -> SidebarPathState {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => SidebarPathState::Available,
        Ok(_) => SidebarPathState::Missing,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => SidebarPathState::Missing,
        Err(_) => SidebarPathState::Unavailable,
    }
}

fn path_state(path: &Path) -> SidebarPathState {
    match fs::metadata(path) {
        Ok(_) => SidebarPathState::Available,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => SidebarPathState::Missing,
        Err(_) => SidebarPathState::Unavailable,
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    left == right
        || match (fs::canonicalize(left), fs::canonicalize(right)) {
            (Ok(left), Ok(right)) => left == right,
            _ => false,
        }
}

fn tag_color(id: &str) -> gpui::Rgba {
    match id {
        "tag-red" => rgb(0xff453a),
        "tag-orange" => rgb(0xff9f0a),
        "tag-yellow" => rgb(0xffd60a),
        "tag-green" => rgb(0x32d74b),
        "tag-blue" => rgb(0x0a84ff),
        "tag-purple" => rgb(0xbf5af2),
        _ => rgb(0x8e8e93),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_contains_finder_sidebar_sections_and_rows() {
        let contract = SidebarContract::from_environment(
            "/Users/tester/Desktop",
            SidebarEnvironment {
                home: PathBuf::from("/Users/tester"),
                icloud_drive: Some(PathBuf::from(
                    "/Users/tester/Library/Mobile Documents/com~apple~CloudDocs",
                )),
                icloud_state: SidebarCloudState::None,
                icloud_progress_milli: None,
                volumes: vec![SidebarVolumeSpec::from_native_seed(
                    "work",
                    "Work",
                    "/Volumes/Work",
                    true,
                )],
            },
        );
        let ids: Vec<_> = contract.rows.iter().map(|row| row.id.as_str()).collect();

        assert_eq!(contract.width_px, 188);
        assert_eq!(contract.row_height_px, 28);
        assert_eq!(contract.sections, SECTIONS);
        assert!(ids.contains(&"home"));
        assert!(ids.contains(&"icloud-drive"));
        assert!(ids.contains(&"volume-work"));
        assert!(ids.contains(&"tag-red"));
    }

    #[test]
    fn contract_output_is_stable_for_cli_and_fozzy() {
        let contract = SidebarContract::from_environment(
            "/Users/tester",
            SidebarEnvironment {
                home: PathBuf::from("/Users/tester"),
                icloud_drive: None,
                icloud_state: SidebarCloudState::None,
                icloud_progress_milli: None,
                volumes: Vec::new(),
            },
        );
        let output = contract.as_tsv();

        assert!(output.starts_with(
            "sidebar\twidth=188\trow-height=28\tsection-header-height=26\tsections=Favorites,iCloud,Locations,Tags"
        ));
        assert!(output.contains(
            "row\tFavorites\thome\ttester\thome-folder\tfavorite\t/Users/tester\tdepth=0"
        ));
        assert!(output.contains(
            "row\tiCloud\ticloud-drive\tiCloud Drive\ticloud-drive\tcloud\t-\tdepth=0\tenabled=false\tselected=false\tejectable=false\tvirtual=false\tpath-state=missing\tcloud=none\tcloud-progress=-\tvolume-kind=-\tvolume-mount=-\tvolume-read-only=-\tvolume-network=-\tvolume-reachable=-"
        ));
        assert!(output.contains(
            "row\tTags\ttag-all\tAll Tags...\tfinder-tag\ttag\t-\tdepth=0\tenabled=true"
        ));
    }

    #[test]
    fn path_state_distinguishes_missing_from_unavailable() {
        let root =
            std::env::temp_dir().join(format!("gfm-sidebar-path-state-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let missing = root.join("Missing");
        let unprobeable = root.join("sidebar-path-unavailable".repeat(16));

        assert_eq!(path_state(&root), SidebarPathState::Available);
        assert_eq!(path_state(&missing), SidebarPathState::Missing);
        assert_eq!(path_state(&unprobeable), SidebarPathState::Unavailable);
        assert_eq!(existing_path(root.join("icloud-missing")), None);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn volume_path_directory_state_distinguishes_unavailable_probe() {
        let root = std::env::temp_dir().join(format!(
            "gfm-sidebar-volume-path-state-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let file = root.join("Plain File");
        fs::write(&file, b"not a volume directory").unwrap();
        let missing = root.join("Missing");
        let unprobeable = root.join("sidebar-volume-path-unavailable".repeat(16));

        assert_eq!(
            volume_path_directory_state(&root),
            SidebarPathState::Available
        );
        assert_eq!(
            volume_path_directory_state(&file),
            SidebarPathState::Missing
        );
        assert_eq!(
            volume_path_directory_state(&missing),
            SidebarPathState::Missing
        );
        assert_eq!(
            volume_path_directory_state(&unprobeable),
            SidebarPathState::Unavailable
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn contract_surfaces_unavailable_path_state_for_disabled_rows() {
        let root = std::env::temp_dir().join(format!(
            "gfm-sidebar-unavailable-home-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let home = root.join("sidebar-home-unavailable".repeat(16));
        let contract = SidebarContract::from_environment(
            &root,
            SidebarEnvironment {
                home: home.clone(),
                icloud_drive: None,
                icloud_state: SidebarCloudState::None,
                icloud_progress_milli: None,
                volumes: Vec::new(),
            },
        );

        let row = contract.rows.iter().find(|row| row.id == "home").unwrap();
        assert_eq!(row.path.as_deref(), Some(home.as_path()));
        assert_eq!(row.path_state, SidebarPathState::Unavailable);
        assert!(!row.enabled);
        assert!(!row.selected);
        assert!(contract
            .as_tsv()
            .contains("\tvirtual=false\tpath-state=unavailable\tcloud=none\t"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn volume_rows_carry_typed_volume_state() {
        let contract = SidebarContract::from_environment(
            "/Volumes/Team",
            SidebarEnvironment {
                home: PathBuf::from("/Users/tester"),
                icloud_drive: None,
                icloud_state: SidebarCloudState::None,
                icloud_progress_milli: None,
                volumes: vec![SidebarVolumeSpec::from_native_seed(
                    "network-volume",
                    "Team",
                    "/Volumes/Team",
                    true,
                )
                .with_volume_state(
                    SidebarVolumeKind::Network,
                    SidebarVolumeMountState::Stale,
                    true,
                    true,
                    Some(false),
                )],
            },
        );

        let row = contract
            .rows
            .iter()
            .find(|row| row.id == "volume-network-volume")
            .unwrap();
        assert_eq!(row.role, "network-volume");
        assert_eq!(row.icon, "network");
        assert_eq!(row.volume_kind, Some(SidebarVolumeKind::Network));
        assert_eq!(row.volume_mount_state, Some(SidebarVolumeMountState::Stale));
        assert_eq!(row.volume_read_only, Some(true));
        assert_eq!(row.volume_network, Some(true));
        assert_eq!(row.volume_reachable, Some(false));
        assert!(!row.enabled);
        assert!(contract.as_tsv().contains(
            "\tvolume-kind=network\tvolume-mount=stale\tvolume-read-only=true\tvolume-network=true\tvolume-reachable=false"
        ));
    }

    #[test]
    fn volume_invalidation_repaints_and_disables_stale_location_row() {
        let volume = SidebarVolumeSpec::from_native_seed(
            "diskarbitration:uuid:Team",
            "Team",
            "/Volumes/Team",
            true,
        )
        .with_volume_state(
            SidebarVolumeKind::Network,
            SidebarVolumeMountState::Stale,
            true,
            true,
            Some(false),
        );

        let invalidation = SidebarVolumeInvalidation::from_event(
            SidebarVolumeEventKind::DescriptionChanged,
            Some(PathBuf::from("/Volumes/Team")),
            None,
            Some(&volume),
            true,
            "volume-event-description-changed",
        );

        assert_eq!(
            invalidation.row_id.as_deref(),
            Some("volume-diskarbitration-uuid-team")
        );
        assert_eq!(invalidation.current_kind, Some(SidebarVolumeKind::Network));
        assert_eq!(
            invalidation.current_mount_state,
            Some(SidebarVolumeMountState::Stale)
        );
        assert_eq!(invalidation.current_reachable, Some(false));
        assert_eq!(invalidation.previous_kind, None);
        assert!(invalidation.invalidate_row);
        assert!(invalidation.invalidate_section);
        assert!(!invalidation.remove_row);
        assert!(invalidation.disable_row);
        assert_eq!(invalidation.reason, "sidebar-volume-disabled");
        assert_eq!(
            invalidation.as_tsv(),
            "sidebar-volume-invalidation\trow=volume-diskarbitration-uuid-team\tpath=/Volumes/Team\tkind=description-changed\tprevious-kind=-\tprevious-mount=-\tprevious-read-only=-\tprevious-network=-\tprevious-reachable=-\tprevious-native-status=-\tprevious-resource-status=-\tprevious-mount-status=-\tcurrent-kind=network\tcurrent-mount=stale\tread-only=true\tnetwork=true\treachable=false\tcurrent-native-status=-\tcurrent-resource-status=-\tcurrent-mount-status=-\tinvalidate-row=true\tinvalidate-section=true\tremove-row=false\tdisable-row=true\treason=sidebar-volume-disabled"
        );
    }

    #[test]
    fn volume_invalidation_disables_unreachable_network_location_row() {
        let volume = SidebarVolumeSpec::from_native_seed(
            "diskarbitration:uuid:Team",
            "Team",
            "/Volumes/Team",
            true,
        )
        .with_volume_state(
            SidebarVolumeKind::Network,
            SidebarVolumeMountState::Mounted,
            false,
            true,
            Some(false),
        );

        let invalidation = SidebarVolumeInvalidation::from_event(
            SidebarVolumeEventKind::DescriptionChanged,
            Some(PathBuf::from("/Volumes/Team")),
            None,
            Some(&volume),
            true,
            "volume-event-description-changed",
        );

        assert_eq!(invalidation.current_reachable, Some(false));
        assert!(invalidation.invalidate_row);
        assert!(invalidation.disable_row);
        assert_eq!(invalidation.reason, "sidebar-volume-disabled");
        assert_eq!(
            invalidation.as_tsv(),
            "sidebar-volume-invalidation\trow=volume-diskarbitration-uuid-team\tpath=/Volumes/Team\tkind=description-changed\tprevious-kind=-\tprevious-mount=-\tprevious-read-only=-\tprevious-network=-\tprevious-reachable=-\tprevious-native-status=-\tprevious-resource-status=-\tprevious-mount-status=-\tcurrent-kind=network\tcurrent-mount=mounted\tread-only=false\tnetwork=true\treachable=false\tcurrent-native-status=-\tcurrent-resource-status=-\tcurrent-mount-status=-\tinvalidate-row=true\tinvalidate-section=true\tremove-row=false\tdisable-row=true\treason=sidebar-volume-disabled"
        );
    }

    #[test]
    fn volume_invalidation_serializes_platform_status_changes() {
        let previous = SidebarVolumeSpec::from_native_seed(
            "diskarbitration:uuid:API",
            "API",
            "/Volumes/API",
            true,
        )
        .with_volume_state(
            SidebarVolumeKind::External,
            SidebarVolumeMountState::Mounted,
            false,
            false,
            Some(true),
        );
        let current = previous.clone();

        let invalidation = SidebarVolumeInvalidation::from_event(
            SidebarVolumeEventKind::DescriptionChanged,
            Some(PathBuf::from("/Volumes/API")),
            Some(&previous),
            Some(&current),
            true,
            "volume-api-status-changed",
        )
        .with_platform_statuses(
            Some("unavailable".to_string()),
            Some("unavailable".to_string()),
            Some("unavailable".to_string()),
            Some("available".to_string()),
            Some("available".to_string()),
            Some("available".to_string()),
        );

        assert!(invalidation.invalidate_row);
        assert!(invalidation.invalidate_section);
        assert!(invalidation.as_tsv().contains(
            "\tprevious-native-status=unavailable\tprevious-resource-status=unavailable\tprevious-mount-status=unavailable\t"
        ));
        assert!(invalidation.as_tsv().contains(
            "\tcurrent-native-status=available\tcurrent-resource-status=available\tcurrent-mount-status=available\t"
        ));
        assert!(invalidation
            .as_tsv()
            .ends_with("reason=volume-api-status-changed"));
    }

    #[test]
    fn volume_invalidation_disables_row_when_current_platform_status_is_unavailable() {
        let volume = SidebarVolumeSpec::from_native_seed(
            "diskarbitration:uuid:API",
            "API",
            "/Volumes/API",
            true,
        )
        .with_volume_state(
            SidebarVolumeKind::External,
            SidebarVolumeMountState::Mounted,
            false,
            false,
            Some(true),
        );

        let invalidation = SidebarVolumeInvalidation::from_event(
            SidebarVolumeEventKind::DescriptionChanged,
            Some(PathBuf::from("/Volumes/API")),
            Some(&volume),
            Some(&volume),
            true,
            "volume-api-status-changed",
        )
        .with_platform_statuses(
            Some("available".to_string()),
            Some("available".to_string()),
            Some("available".to_string()),
            Some("unavailable".to_string()),
            Some("available".to_string()),
            Some("available".to_string()),
        );

        assert!(invalidation.invalidate_row);
        assert!(invalidation.invalidate_section);
        assert!(invalidation.disable_row);
        assert_eq!(invalidation.reason, "volume-api-status-changed");
        assert!(invalidation.as_tsv().contains(
            "\tcurrent-native-status=unavailable\tcurrent-resource-status=available\tcurrent-mount-status=available\t"
        ));
        assert!(invalidation.as_tsv().contains("\tdisable-row=true\t"));
    }

    #[test]
    fn volume_invalidation_disables_row_for_explicit_unavailable_event() {
        let previous = SidebarVolumeSpec::from_native_seed(
            "diskarbitration:uuid:API",
            "API",
            "/Volumes/API",
            true,
        )
        .with_volume_state(
            SidebarVolumeKind::External,
            SidebarVolumeMountState::Mounted,
            false,
            false,
            Some(true),
        );

        let invalidation = SidebarVolumeInvalidation::from_event(
            SidebarVolumeEventKind::Unavailable,
            Some(PathBuf::from("/Volumes/API")),
            Some(&previous),
            None,
            true,
            "volume-event-unavailable",
        );

        assert!(invalidation.invalidate_row);
        assert!(invalidation.invalidate_section);
        assert!(!invalidation.remove_row);
        assert!(invalidation.disable_row);
        assert_eq!(invalidation.reason, "sidebar-volume-disabled");
        assert!(invalidation
            .as_tsv()
            .contains("\tkind=unavailable\tprevious-kind=external\tprevious-mount=mounted\t"));
        assert!(invalidation
            .as_tsv()
            .contains("\tcurrent-kind=-\tcurrent-mount=-\tread-only=-\tnetwork=-\treachable=-\t"));
        assert!(invalidation.as_tsv().contains(
            "\tinvalidate-row=true\tinvalidate-section=true\tremove-row=false\tdisable-row=true\t"
        ));
    }

    #[test]
    fn volume_invalidation_marks_disappeared_location_for_removal() {
        let previous = SidebarVolumeSpec::from_native_seed(
            "diskarbitration:uuid:Team",
            "Team",
            "/Volumes/Team",
            true,
        )
        .with_volume_state(
            SidebarVolumeKind::Network,
            SidebarVolumeMountState::Mounted,
            false,
            true,
            Some(true),
        );

        let invalidation = SidebarVolumeInvalidation::from_event(
            SidebarVolumeEventKind::Disappeared,
            Some(PathBuf::from("/Volumes/Team")),
            Some(&previous),
            None,
            true,
            "volume-event-disappeared",
        );

        assert_eq!(
            invalidation.row_id.as_deref(),
            Some("volume-diskarbitration-uuid-team")
        );
        assert_eq!(invalidation.previous_kind, Some(SidebarVolumeKind::Network));
        assert_eq!(
            invalidation.previous_mount_state,
            Some(SidebarVolumeMountState::Mounted)
        );
        assert_eq!(invalidation.previous_reachable, Some(true));
        assert_eq!(invalidation.current_kind, None);
        assert!(invalidation.invalidate_row);
        assert!(invalidation.invalidate_section);
        assert!(invalidation.remove_row);
        assert!(!invalidation.disable_row);
        assert_eq!(invalidation.reason, "sidebar-volume-disappeared");
        assert_eq!(
            invalidation.as_tsv(),
            "sidebar-volume-invalidation\trow=volume-diskarbitration-uuid-team\tpath=/Volumes/Team\tkind=disappeared\tprevious-kind=network\tprevious-mount=mounted\tprevious-read-only=false\tprevious-network=true\tprevious-reachable=true\tprevious-native-status=-\tprevious-resource-status=-\tprevious-mount-status=-\tcurrent-kind=-\tcurrent-mount=-\tread-only=-\tnetwork=-\treachable=-\tcurrent-native-status=-\tcurrent-resource-status=-\tcurrent-mount-status=-\tinvalidate-row=true\tinvalidate-section=true\tremove-row=true\tdisable-row=false\treason=sidebar-volume-disappeared"
        );
    }

    #[test]
    fn stable_volume_ids_are_ascii_and_deterministic() {
        assert_eq!(
            SidebarVolumeSpec::from_native_seed("Work Drive", "Work", "/Volumes/Work", true).id,
            "volume-work-drive"
        );
        assert_eq!(
            SidebarVolumeSpec::from_native_seed(
                "diskarbitration:uuid:Media+Backup",
                "Media",
                "/Volumes/Media",
                true
            )
            .id,
            "volume-diskarbitration-uuid-media-backup"
        );
    }

    #[test]
    fn system_volumes_are_not_reported_as_ejectable_locations() {
        assert!(is_system_volume_label("Macintosh HD"));
        assert!(is_system_volume_label("Recovery"));
        assert!(!is_system_volume_label("Hex"));
    }

    #[test]
    fn icloud_sidebar_row_carries_typed_cloud_state() {
        let root =
            std::env::temp_dir().join(format!("gfm-sidebar-icloud-state-{}", std::process::id()));
        let path = root.join("Library/Mobile Documents/com~apple~CloudDocs");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&path).unwrap();
        let contract = SidebarContract::discover_with_icloud_state(
            &path,
            &path,
            SidebarCloudState::Downloading,
            Some(12_500),
        );
        let row = contract
            .rows
            .iter()
            .find(|row| row.id == "icloud-drive")
            .unwrap();

        assert!(row.selected);
        assert_eq!(row.path_state, SidebarPathState::Available);
        assert_eq!(row.cloud_state, SidebarCloudState::Downloading);
        assert_eq!(row.cloud_progress_milli, Some(12_500));
        assert!(contract
            .as_tsv()
            .contains("\tcloud=downloading\tcloud-progress=12500\tvolume-kind=-"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fileprovider_invalidation_repaints_icloud_row_for_visible_state_change() {
        let invalidation = SidebarCloudInvalidation::new(
            "/Users/tester/Library/Mobile Documents/com~apple~CloudDocs/Report.md",
            SidebarCloudState::CloudOnly,
            SidebarCloudState::AvailableOffline,
            None,
            true,
            "fileprovider-state-changed",
        );

        assert_eq!(invalidation.row_id, "icloud-drive");
        assert!(invalidation.invalidate_row);
        assert_eq!(invalidation.reason, "sidebar-cloud-state-changed");
        assert_eq!(
            invalidation.as_tsv(),
            "sidebar-cloud-invalidation\ticloud-drive\tpath=/Users/tester/Library/Mobile Documents/com~apple~CloudDocs/Report.md\tprevious=cloud-only\tcurrent=available-offline\tprogress=-\tinvalidate-row=true\treason=sidebar-cloud-state-changed"
        );
    }

    #[test]
    fn fileprovider_invalidation_keeps_local_paths_out_of_sidebar_repaint() {
        let invalidation = SidebarCloudInvalidation::new(
            "/Users/tester/Desktop/Local.md",
            SidebarCloudState::None,
            SidebarCloudState::None,
            None,
            true,
            "fileprovider-state-changed",
        );

        assert!(!invalidation.invalidate_row);
        assert_eq!(invalidation.reason, "sidebar-cloud-not-visible");
    }
}
