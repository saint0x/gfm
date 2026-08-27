use gfm_types::{GfmError, Result};
use gpui::{
    div, prelude::*, px, rgb, size, App, AppContext, Application, Bounds, Context, IntoElement,
    Render, Styled, Subscription, Window, WindowBounds, WindowOptions,
};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};

mod column;
mod context;
mod dialog;
mod gallery;
mod icon;
mod list;
mod menu;
mod results;
mod session;
mod sidebar;
mod titlebar;
mod toolbar;
mod trash;
mod virtualize;

pub use column::{
    render as render_column_view, ColumnKeyboardFlow, ColumnRowSpec, ColumnSortMode, ColumnSource,
    ColumnSpec, ColumnViewContract, ColumnViewOptions, PreviewColumnSpec, PreviewRole,
};
pub use context::{
    ContextItemKind, ContextMenuContract, ContextMenuInput, ContextMenuItemSpec, ContextSurface,
};
pub use dialog::{
    render as render_dialog, DialogButtonRole, DialogButtonSpec, DialogContract, DialogFieldKind,
    DialogFieldSpec, DialogPresentation, DialogSurface, OperationConflictContract,
    OperationConflictInput, OperationConflictPaths, OperationProgressCommand,
    OperationProgressCommandSpec, OperationProgressContract, OperationProgressInput,
    OperationProgressState, PermissionPromptKind, ProviderConflictContract, ProviderConflictInput,
};
pub use gallery::{
    render as render_gallery_view, GalleryFilmstripItemSpec, GalleryKeyboardFlow,
    GalleryMetadataSpec, GalleryPreviewRole, GalleryPreviewSpec, GalleryQuickActionSpec,
    GallerySortMode, GalleryViewContract, GalleryViewOptions,
};
pub use icon::{
    IconBadge, IconCellSpec, IconRole, IconSortMode, IconViewContract, IconViewOptions,
};
pub use list::{
    render as render_list_view, ListCellSpec, ListColumnKind, ListColumnSpec, ListRowSpec,
    ListSortMode, ListViewContract, ListViewOptions,
};
pub use menu::{MenuCommandSpec, MenuCommandState, MenuContract};
pub use results::{
    render as render_search_results_view, SearchResultRowSpec, SearchResultsBatch,
    SearchResultsContract, SearchResultsGroupSpec, SearchResultsGrouping, SearchResultsOptions,
    SearchResultsScope, SearchResultsStage,
};
pub use session::{
    ActivationPolicy, PlacementPolicy, RestorePolicy, TabPolicy, WindowPlacement,
    WindowSessionContract, WindowSessionStore, WindowSessionWriter,
};
pub use sidebar::{
    SidebarCloudState, SidebarContract, SidebarItemKind, SidebarItemSpec, SidebarVolumeSpec,
};
pub use titlebar::{
    FullScreenPolicy, TitlebarContract, TitlebarFocusPolicy, TitlebarMaterialPolicy,
};
pub use toolbar::{ToolbarContract, ToolbarControlKind, ToolbarControlSpec};
pub use trash::{
    render as render_trash_view, TrashCommandSpec, TrashEntryMetadata, TrashRowSpec, TrashSortMode,
    TrashViewContract, TrashViewOptions,
};
pub use virtualize::{VirtualSurface, VirtualWindow, VirtualizationContract};

const DEFAULT_WIDTH: f32 = 1040.0;
const DEFAULT_HEIGHT: f32 = 720.0;
const MIN_WIDTH: f32 = 640.0;
const MIN_HEIGHT: f32 = 420.0;

#[derive(Debug, Clone, PartialEq)]
pub struct AppLaunchSpec {
    pub title: String,
    pub initial_path: PathBuf,
    pub width: f32,
    pub height: f32,
    pub min_width: f32,
    pub min_height: f32,
    pub transparent_titlebar: bool,
    pub activate_on_launch: bool,
    pub tabbing_identifier: String,
    pub sidebar_volumes: Vec<SidebarVolumeSpec>,
    pub progress_surfaces: Vec<OperationProgressContract>,
    pub operation_conflicts: Vec<OperationConflictContract>,
    pub permission_dialog: Option<DialogContract>,
    pub permission_prompt: Option<PermissionPromptKind>,
    pub permission_refresh: Option<PermissionRefreshContract>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRefreshContract {
    pub initialized: bool,
    pub changed: usize,
    pub refresh_ui: bool,
    pub refresh_workers: bool,
    pub refresh_operations: bool,
}

impl PermissionRefreshContract {
    pub fn new(
        initialized: bool,
        changed: usize,
        refresh_ui: bool,
        refresh_workers: bool,
        refresh_operations: bool,
    ) -> Self {
        Self {
            initialized,
            changed,
            refresh_ui,
            refresh_workers,
            refresh_operations,
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "permission-refresh\taudience=ui\tinitialized={}\tchanged={}\trefresh-ui={}\trefresh-workers={}\trefresh-operations={}",
            self.initialized,
            self.changed,
            self.refresh_ui,
            self.refresh_workers,
            self.refresh_operations
        )
    }
}

impl AppLaunchSpec {
    pub fn new(initial_path: impl Into<PathBuf>) -> Self {
        Self {
            initial_path: initial_path.into(),
            ..Self::default()
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.title.trim().is_empty() {
            return Err(GfmError::Format(
                "native app title must not be empty".to_string(),
            ));
        }
        if self.width < self.min_width || self.height < self.min_height {
            return Err(GfmError::Format(format!(
                "native app window {}x{} is below minimum {}x{}",
                self.width, self.height, self.min_width, self.min_height
            )));
        }
        if self.min_width < 320.0 || self.min_height < 240.0 {
            return Err(GfmError::Format(
                "native app minimum window is too small for Finder-parity chrome".to_string(),
            ));
        }
        if self.tabbing_identifier.trim().is_empty() {
            return Err(GfmError::Format(
                "native app tabbing identifier must not be empty".to_string(),
            ));
        }
        let mut sidebar_volume_ids = BTreeSet::new();
        for volume in &self.sidebar_volumes {
            if volume.id.trim().is_empty() {
                return Err(GfmError::Format(
                    "native app sidebar volume id must not be empty".to_string(),
                ));
            }
            if !sidebar_volume_ids.insert(volume.id.as_str()) {
                return Err(GfmError::Format(format!(
                    "native app sidebar volume id `{}` is duplicated",
                    volume.id
                )));
            }
        }
        for progress in &self.progress_surfaces {
            if !progress.state.is_cancellable() {
                return Err(GfmError::Format(format!(
                    "native app progress surface `{}` is not restorable",
                    progress.label
                )));
            }
        }
        for conflict in &self.operation_conflicts {
            if conflict.dialog.surface != DialogSurface::Conflict {
                return Err(GfmError::Format(
                    "native app operation conflict must use the conflict surface".to_string(),
                ));
            }
            if conflict.dialog.presentation != DialogPresentation::WindowSheet {
                return Err(GfmError::Format(
                    "native app operation conflict must be a window sheet".to_string(),
                ));
            }
        }
        if let Some(dialog) = &self.permission_dialog {
            if dialog.surface != DialogSurface::Permission {
                return Err(GfmError::Format(
                    "native app permission dialog must use the permission surface".to_string(),
                ));
            }
            if dialog.presentation != DialogPresentation::WindowSheet {
                return Err(GfmError::Format(
                    "native app permission dialog must be a window sheet".to_string(),
                ));
            }
        }
        if self.permission_prompt.is_some() && self.permission_dialog.is_none() {
            return Err(GfmError::Format(
                "native app permission prompt binding requires a permission dialog".to_string(),
            ));
        }
        titlebar::TitlebarContract::from_spec(self)?;
        Ok(())
    }

    pub fn with_permission_dialog(mut self, dialog: DialogContract) -> Self {
        self.permission_dialog = Some(dialog);
        self
    }

    pub fn with_permission_prompt(mut self, kind: PermissionPromptKind) -> Self {
        self.permission_dialog = Some(DialogContract::permission_prompt(kind));
        self.permission_prompt = Some(kind);
        self
    }

    pub fn with_sidebar_volumes(mut self, volumes: Vec<SidebarVolumeSpec>) -> Self {
        self.sidebar_volumes = volumes;
        self
    }

    pub fn with_progress_surfaces(mut self, surfaces: Vec<OperationProgressContract>) -> Self {
        self.progress_surfaces = surfaces;
        self
    }

    pub fn with_operation_conflicts(mut self, conflicts: Vec<OperationConflictContract>) -> Self {
        self.operation_conflicts = conflicts;
        self
    }

    pub fn with_permission_refresh(mut self, refresh: PermissionRefreshContract) -> Self {
        self.permission_refresh = Some(refresh);
        self
    }
}

impl Default for AppLaunchSpec {
    fn default() -> Self {
        Self {
            title: "GFM".to_string(),
            initial_path: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            min_width: MIN_WIDTH,
            min_height: MIN_HEIGHT,
            transparent_titlebar: true,
            activate_on_launch: true,
            tabbing_identifier: "gfm-main-window".to_string(),
            sidebar_volumes: Vec::new(),
            progress_surfaces: Vec::new(),
            operation_conflicts: Vec::new(),
            permission_dialog: None,
            permission_prompt: None,
            permission_refresh: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowLifecycleContract {
    pub title: String,
    pub initial_path: PathBuf,
    pub width: f32,
    pub height: f32,
    pub min_width: f32,
    pub min_height: f32,
    pub transparent_titlebar: bool,
    pub activate_on_launch: bool,
    pub tabbing_identifier: String,
    pub sidebar_volumes: Vec<SidebarVolumeSpec>,
    pub progress_surfaces: Vec<OperationProgressContract>,
    pub operation_conflicts: Vec<OperationConflictContract>,
    pub permission_dialog: Option<DialogSurface>,
    pub permission_prompt: Option<PermissionPromptKind>,
    pub permission_refresh: Option<PermissionRefreshContract>,
}

impl WindowLifecycleContract {
    pub fn from_spec(spec: &AppLaunchSpec) -> Result<Self> {
        spec.validate()?;
        Ok(Self {
            title: spec.title.clone(),
            initial_path: spec.initial_path.clone(),
            width: spec.width,
            height: spec.height,
            min_width: spec.min_width,
            min_height: spec.min_height,
            transparent_titlebar: spec.transparent_titlebar,
            activate_on_launch: spec.activate_on_launch,
            tabbing_identifier: spec.tabbing_identifier.clone(),
            sidebar_volumes: spec.sidebar_volumes.clone(),
            progress_surfaces: spec.progress_surfaces.clone(),
            operation_conflicts: spec.operation_conflicts.clone(),
            permission_dialog: spec.permission_dialog.as_ref().map(|dialog| dialog.surface),
            permission_prompt: spec.permission_prompt,
            permission_refresh: spec.permission_refresh.clone(),
        })
    }

    pub fn as_tsv(&self) -> String {
        let mut lines = vec![format!(
            "window\t{}\t{}\t{}x{}\tmin={}x{}\ttransparent-titlebar={}\tactivate={}\ttabs={}\tpermission-dialog={}",
            self.title,
            self.initial_path.display(),
            self.width,
            self.height,
            self.min_width,
            self.min_height,
            self.transparent_titlebar,
            self.activate_on_launch,
            self.tabbing_identifier,
            self.permission_dialog
                .map(DialogSurface::as_str)
                .unwrap_or("none")
        )];
        lines.extend(
            self.progress_surfaces
                .iter()
                .map(|progress| progress.as_tsv()),
        );
        lines.extend(
            self.operation_conflicts
                .iter()
                .map(|conflict| conflict.as_tsv()),
        );
        if let Some(prompt) = self.permission_prompt {
            lines.push(format!(
                "permission-prompt\tkind={}\tsurface=permission",
                prompt.as_str()
            ));
        }
        if let Some(refresh) = &self.permission_refresh {
            lines.push(refresh.as_tsv());
        }
        lines.join("\n")
    }
}

pub fn run_native(spec: AppLaunchSpec) -> Result<()> {
    spec.validate()?;
    Application::new().run(move |cx: &mut App| {
        let window_counter = Arc::new(AtomicU32::new(1));
        let session_store = WindowSessionStore::platform_default();
        install_native_menu(cx);
        install_new_window_action(cx, window_counter, spec.clone());
        if let Err(err) = open_main_window(cx, spec, session_store, 0) {
            eprintln!("gfm-ui: {err}");
            cx.quit();
        }
    });
    Ok(())
}

fn open_main_window(
    cx: &mut App,
    spec: AppLaunchSpec,
    session_store: WindowSessionStore,
    ordinal: u32,
) -> anyhow::Result<()> {
    let options = window_options(cx, &spec, &session_store, ordinal);
    let activate = spec.activate_on_launch;
    cx.open_window(options, |_, cx| {
        cx.new(|_| RootView {
            bounds_subscription: None,
            session_writer: WindowSessionWriter::new(session_store),
            sidebar: sidebar::SidebarContract::discover_with_volumes(
                &spec.initial_path,
                spec.sidebar_volumes.clone(),
            ),
            icon_view: IconViewContract::from_records(&[], IconViewOptions::default()),
            progress_surfaces: spec.progress_surfaces,
            operation_conflicts: spec.operation_conflicts,
            permission_dialog: spec.permission_dialog,
            permission_refresh: spec.permission_refresh,
            initial_path: spec.initial_path,
        })
    })?;
    if activate {
        cx.activate(true);
    }
    Ok(())
}

fn install_native_menu(cx: &mut App) {
    cx.bind_keys(menu::key_bindings());
    cx.on_action(|_: &menu::CloseWindow, cx| {
        if let Some(active_window) = cx.active_window() {
            let _ = active_window.update(cx, |_, window, _| window.remove_window());
        }
    });
    cx.on_action(|_: &menu::Quit, cx| cx.quit());
    cx.set_menus(menu::native_menus());
}

fn install_new_window_action(cx: &mut App, window_counter: Arc<AtomicU32>, spec: AppLaunchSpec) {
    cx.on_action(move |_: &menu::NewWindow, cx| {
        let ordinal = window_counter.fetch_add(1, Ordering::Relaxed);
        let session_store = WindowSessionStore::platform_default();
        if let Err(err) = open_main_window(cx, spec.clone(), session_store, ordinal) {
            eprintln!("gfm-ui: {err}");
        }
    });
}

fn window_options(
    cx: &App,
    spec: &AppLaunchSpec,
    session_store: &WindowSessionStore,
    ordinal: u32,
) -> WindowOptions {
    let session = WindowSessionContract::from_spec(spec, session_store, ordinal);
    let bounds = session
        .placement
        .and_then(WindowPlacement::to_bounds)
        .unwrap_or_else(|| Bounds::centered(None, size(px(spec.width), px(spec.height)), cx));
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(
            titlebar::TitlebarContract::from_spec(spec)
                .expect("validated titlebar")
                .into_options(),
        ),
        window_min_size: Some(size(px(spec.min_width), px(spec.min_height))),
        tabbing_identifier: Some(session.tabbing_identifier.clone()),
        focus: session.focus_new_window,
        show: session.show_on_open,
        is_movable: session.movable,
        is_resizable: session.resizable,
        is_minimizable: session.minimizable,
        ..Default::default()
    }
}

struct RootView {
    bounds_subscription: Option<Subscription>,
    session_writer: WindowSessionWriter,
    sidebar: SidebarContract,
    icon_view: IconViewContract,
    progress_surfaces: Vec<OperationProgressContract>,
    operation_conflicts: Vec<OperationConflictContract>,
    permission_dialog: Option<DialogContract>,
    permission_refresh: Option<PermissionRefreshContract>,
    initial_path: PathBuf,
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.bounds_subscription.is_none() {
            self.bounds_subscription = Some(cx.observe_window_bounds(window, |this, window, _| {
                this.session_writer.save_bounds(window.window_bounds());
            }));
        }

        let mut root = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x1e1e1e))
            .child(toolbar::render(&self.initial_path))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .w_full()
                    .child(sidebar::render(&self.sidebar))
                    .child(div().flex_1().h_full().child(icon::render(&self.icon_view))),
            );
        if let Some(dialog) = &self.permission_dialog {
            root = root.child(dialog::render(dialog));
        }
        for progress in &self.progress_surfaces {
            root = root
                .child(dialog::render(&progress.dialog))
                .child(div().invisible().child(progress.as_tsv()));
        }
        for conflict in &self.operation_conflicts {
            root = root
                .child(dialog::render(&conflict.dialog))
                .child(div().invisible().child(conflict.as_tsv()));
        }
        if let Some(refresh) = &self.permission_refresh {
            root = root.child(
                div()
                    .id("permission-refresh-state")
                    .invisible()
                    .child(refresh.as_tsv()),
            );
        }
        root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_contract_is_valid_for_native_window_lifecycle() {
        let spec = AppLaunchSpec::new("/Users/deepsaint/Desktop");
        let contract = WindowLifecycleContract::from_spec(&spec).unwrap();

        assert_eq!(contract.title, "GFM");
        assert_eq!(contract.width, DEFAULT_WIDTH);
        assert_eq!(contract.height, DEFAULT_HEIGHT);
        assert!(contract.transparent_titlebar);
        assert_eq!(contract.tabbing_identifier, "gfm-main-window");
        assert!(contract.sidebar_volumes.is_empty());
        assert!(contract.progress_surfaces.is_empty());
        assert_eq!(contract.permission_dialog, None);
        assert_eq!(contract.permission_refresh, None);
    }

    #[test]
    fn rejects_windows_below_finder_chrome_minimum() {
        let spec = AppLaunchSpec {
            width: 500.0,
            ..Default::default()
        };

        let err = WindowLifecycleContract::from_spec(&spec).unwrap_err();
        assert!(err.to_string().contains("below minimum"));
    }

    #[test]
    fn contract_output_is_stable_for_cli_and_fozzy() {
        let spec = AppLaunchSpec::new("/tmp/gfm");
        let contract = WindowLifecycleContract::from_spec(&spec).unwrap();

        assert_eq!(
            contract.as_tsv(),
            "window\tGFM\t/tmp/gfm\t1040x720\tmin=640x420\ttransparent-titlebar=true\tactivate=true\ttabs=gfm-main-window\tpermission-dialog=none"
        );
    }

    #[test]
    fn lifecycle_contract_tracks_permission_sheet() {
        let spec = AppLaunchSpec::new("/tmp/gfm")
            .with_permission_prompt(PermissionPromptKind::BookmarkAcquisition);
        let contract = WindowLifecycleContract::from_spec(&spec).unwrap();

        assert_eq!(contract.permission_dialog, Some(DialogSurface::Permission));
        assert_eq!(
            contract.permission_prompt,
            Some(PermissionPromptKind::BookmarkAcquisition)
        );
        assert!(contract.as_tsv().contains("\tpermission-dialog=permission"));
        assert!(contract
            .as_tsv()
            .contains("\npermission-prompt\tkind=bookmark-acquisition\tsurface=permission"));
    }

    #[test]
    fn rejects_permission_prompt_without_permission_dialog() {
        let mut spec = AppLaunchSpec::new("/tmp/gfm");
        spec.permission_prompt = Some(PermissionPromptKind::FullDiskAccess);

        let err = WindowLifecycleContract::from_spec(&spec).unwrap_err();
        assert!(err
            .to_string()
            .contains("permission prompt binding requires a permission dialog"));
    }

    #[test]
    fn lifecycle_contract_tracks_sidebar_volumes_from_launch_spec() {
        let volume = SidebarVolumeSpec::from_native_seed(
            "diskarbitration-uuid-media-backup",
            "Media Backup",
            "/Volumes/Media Backup",
            true,
        );
        let spec = AppLaunchSpec::new("/tmp/gfm").with_sidebar_volumes(vec![volume.clone()]);
        let contract = WindowLifecycleContract::from_spec(&spec).unwrap();

        assert_eq!(contract.sidebar_volumes, vec![volume]);
    }

    #[test]
    fn rejects_duplicate_sidebar_volume_ids() {
        let first =
            SidebarVolumeSpec::from_native_seed("duplicate-volume", "First", "/Volumes/A", true);
        let second =
            SidebarVolumeSpec::from_native_seed("duplicate-volume", "Second", "/Volumes/B", false);
        let spec = AppLaunchSpec::new("/tmp/gfm").with_sidebar_volumes(vec![first, second]);

        let err = WindowLifecycleContract::from_spec(&spec).unwrap_err();
        assert!(err.to_string().contains("duplicated"));
    }

    #[test]
    fn lifecycle_contract_tracks_restorable_progress_surfaces() {
        let progress = OperationProgressContract::from_input(
            OperationProgressInput::new(
                "copy selected files",
                OperationProgressState::Running,
                42,
                100,
                "copy:/source->/target",
            )
            .with_job_id(7),
        );
        let spec = AppLaunchSpec::new("/tmp/gfm").with_progress_surfaces(vec![progress.clone()]);
        let contract = WindowLifecycleContract::from_spec(&spec).unwrap();

        assert_eq!(contract.progress_surfaces, vec![progress]);
        assert!(contract
            .as_tsv()
            .contains("\noperation-progress\tjob=7\tlabel=copy selected files\tstate=running\t"));
        assert!(contract
            .as_tsv()
            .contains("\noperation-progress-command\tpause\tjob=7\tenabled=true"));
    }

    #[test]
    fn lifecycle_contract_tracks_operation_conflict_surfaces() {
        let conflict = OperationConflictContract::from_input(OperationConflictInput::new(
            "copy",
            OperationConflictPaths::new("/tmp/source", "/tmp/target"),
            "file",
            "fail",
            vec![
                "replace".to_string(),
                "keep-both".to_string(),
                "skip".to_string(),
            ],
            true,
            "destination-conflict-requires-user-resolution",
        ));
        let spec = AppLaunchSpec::new("/tmp/gfm").with_operation_conflicts(vec![conflict]);

        let contract = WindowLifecycleContract::from_spec(&spec).unwrap();

        assert_eq!(contract.operation_conflicts.len(), 1);
        assert!(contract
            .as_tsv()
            .contains("\noperation-conflict-ui\toperation=copy\ttarget=/tmp/target\tkind=file\t"));
        assert!(contract
            .as_tsv()
            .contains("button\tmerge\tMerge\talternate\tenabled=false"));
        assert!(contract.as_tsv().contains(
            "\noperation-conflict-row\t0\toperation=copy\tsource=/tmp/source\ttarget=/tmp/target\tkind=file\t"
        ));
    }

    #[test]
    fn rejects_terminal_progress_surfaces_on_native_launch() {
        let progress = OperationProgressContract::from_input(OperationProgressInput::new(
            "finished copy",
            OperationProgressState::Completed,
            100,
            100,
            "done",
        ));
        let spec = AppLaunchSpec::new("/tmp/gfm").with_progress_surfaces(vec![progress]);

        let err = WindowLifecycleContract::from_spec(&spec).unwrap_err();
        assert!(err.to_string().contains("not restorable"));
    }

    #[test]
    fn lifecycle_contract_tracks_permission_refresh_state() {
        let spec = AppLaunchSpec::new("/tmp/gfm")
            .with_permission_refresh(PermissionRefreshContract::new(false, 1, true, true, true));
        let contract = WindowLifecycleContract::from_spec(&spec).unwrap();

        assert_eq!(
            contract.permission_refresh,
            Some(PermissionRefreshContract::new(false, 1, true, true, true))
        );
        assert!(contract
            .as_tsv()
            .contains("\npermission-refresh\taudience=ui\tinitialized=false\tchanged=1\t"));
    }
}
