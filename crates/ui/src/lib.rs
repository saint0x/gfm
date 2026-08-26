use gfm_types::{GfmError, Result};
use gpui::{
    div, prelude::*, px, rgb, size, App, AppContext, Application, Bounds, Context, IntoElement,
    Render, Styled, Subscription, Window, WindowBounds, WindowOptions,
};
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
    DialogButtonRole, DialogButtonSpec, DialogContract, DialogFieldKind, DialogFieldSpec,
    DialogPresentation, DialogSurface, OperationProgressContract, OperationProgressInput,
    OperationProgressState,
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
pub use sidebar::{SidebarContract, SidebarItemKind, SidebarItemSpec, SidebarVolumeSpec};
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
        titlebar::TitlebarContract::from_spec(self)?;
        Ok(())
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
        })
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "window\t{}\t{}\t{}x{}\tmin={}x{}\ttransparent-titlebar={}\tactivate={}\ttabs={}",
            self.title,
            self.initial_path.display(),
            self.width,
            self.height,
            self.min_width,
            self.min_height,
            self.transparent_titlebar,
            self.activate_on_launch,
            self.tabbing_identifier
        )
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
            sidebar: sidebar::SidebarContract::discover(&spec.initial_path),
            icon_view: IconViewContract::from_records(&[], IconViewOptions::default()),
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
    initial_path: PathBuf,
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.bounds_subscription.is_none() {
            self.bounds_subscription = Some(cx.observe_window_bounds(window, |this, window, _| {
                this.session_writer.save_bounds(window.window_bounds());
            }));
        }

        div()
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
            )
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
            "window\tGFM\t/tmp/gfm\t1040x720\tmin=640x420\ttransparent-titlebar=true\tactivate=true\ttabs=gfm-main-window"
        );
    }
}
