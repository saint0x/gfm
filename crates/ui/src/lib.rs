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
    render as render_dialog, render_permission as render_permission_dialog, DialogButtonRole,
    DialogButtonSpec, DialogContract, DialogFieldKind, DialogFieldSpec, DialogPresentation,
    DialogSurface, OperationConflictContract, OperationConflictInput, OperationConflictPaths,
    OperationProgressCommand, OperationProgressCommandSpec, OperationProgressContract,
    OperationProgressDetailKind, OperationProgressInput, OperationProgressPayloadKind,
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
    SidebarCloudInvalidation, SidebarCloudState, SidebarContract, SidebarItemKind, SidebarItemSpec,
    SidebarPathSnapshot, SidebarPathState, SidebarVolumeEventKind, SidebarVolumeInvalidation,
    SidebarVolumeKind, SidebarVolumeMountState, SidebarVolumeSpec,
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
    pub sidebar_paths: SidebarPathSnapshot,
    pub sidebar_volumes: Vec<SidebarVolumeSpec>,
    pub progress_surfaces: Vec<OperationProgressContract>,
    pub operation_conflicts: Vec<OperationConflictContract>,
    pub permission_dialog: Option<DialogContract>,
    pub permission_prompt: Option<PermissionPromptKind>,
    pub permission_onboarding: Option<PermissionOnboardingContract>,
    pub permission_access: Option<PermissionAccessContract>,
    pub permission_refresh: Option<PermissionRefreshContract>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionOnboardingContract {
    pub action: String,
    pub prompt_kind: PermissionPromptKind,
    pub prompt_mode: String,
    pub finder_parity_default: bool,
    pub machine_search_ready: bool,
    pub scopes: Vec<PermissionOnboardingScopeContract>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionOnboardingScopeContract {
    pub scope: String,
    pub state: String,
    pub path: String,
    pub reason: String,
}

impl PermissionOnboardingContract {
    pub fn new(
        action: impl Into<String>,
        prompt_kind: PermissionPromptKind,
        prompt_mode: impl Into<String>,
        finder_parity_default: bool,
        machine_search_ready: bool,
    ) -> Self {
        Self {
            action: action.into(),
            prompt_kind,
            prompt_mode: prompt_mode.into(),
            finder_parity_default,
            machine_search_ready,
            scopes: Vec::new(),
        }
    }

    pub fn with_scopes(mut self, scopes: Vec<PermissionOnboardingScopeContract>) -> Self {
        self.scopes = scopes;
        self
    }

    pub fn requires_surface(&self) -> bool {
        self.finder_parity_default
            || !self.machine_search_ready
            || self.prompt_kind != PermissionPromptKind::General
            || self.action != "continue-normally"
    }

    pub fn as_tsv(&self) -> String {
        let mut lines = vec![format!(
            "permission-onboarding\taction={}\tprompt-kind={}\tprompt-mode={}\tfinder-parity-default={}\tmachine-search-ready={}",
            escape_contract_field(&self.action),
            self.prompt_kind.as_str(),
            escape_contract_field(&self.prompt_mode),
            self.finder_parity_default,
            self.machine_search_ready
        )];
        lines.extend(
            self.scopes
                .iter()
                .map(PermissionOnboardingScopeContract::as_tsv),
        );
        lines.join("\n")
    }
}

impl PermissionOnboardingScopeContract {
    pub fn new(
        scope: impl Into<String>,
        state: impl Into<String>,
        path: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            scope: scope.into(),
            state: state.into(),
            path: path.into(),
            reason: reason.into(),
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "permission-scope\t{}\tstate={}\tpath={}\treason={}",
            escape_contract_field(&self.scope),
            escape_contract_field(&self.state),
            escape_contract_field(&self.path),
            escape_contract_field(&self.reason)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionAccessContract {
    pub path: String,
    pub intent: String,
    pub scope: String,
    pub probe: String,
    pub mode: String,
    pub access_action: String,
    pub worker_action: String,
    pub can_touch_filesystem: bool,
    pub bookmark_required: bool,
    pub bookmark_access: bool,
    pub refresh_on_permission_change: bool,
    pub prompt_kind: PermissionPromptKind,
    pub prompt_action: String,
    pub promptable: bool,
    pub prompt_source: String,
    pub reason: String,
}

impl PermissionAccessContract {
    pub fn with_bookmark_state(mut self, required: bool, access: bool) -> Self {
        self.bookmark_required = required;
        self.bookmark_access = access;
        self
    }

    pub fn with_refresh_on_permission_change(mut self, refresh: bool) -> Self {
        self.refresh_on_permission_change = refresh;
        self
    }

    pub fn with_prompt_orchestration(
        mut self,
        promptable: bool,
        source: impl Into<String>,
    ) -> Self {
        self.promptable = promptable;
        self.prompt_source = source.into();
        self
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "permission-access\tpath={}\tintent={}\tscope={}\tprobe={}\tmode={}\taccess-action={}\tworker-action={}\tcan-touch-filesystem={}\tbookmark-required={}\tbookmark-access={}\trefresh-on-permission-change={}\tprompt-kind={}\tprompt-action={}\tpromptable={}\tprompt-source={}\treason={}",
            escape_contract_field(&self.path),
            escape_contract_field(&self.intent),
            escape_contract_field(&self.scope),
            escape_contract_field(&self.probe),
            escape_contract_field(&self.mode),
            escape_contract_field(&self.access_action),
            escape_contract_field(&self.worker_action),
            self.can_touch_filesystem,
            self.bookmark_required,
            self.bookmark_access,
            self.refresh_on_permission_change,
            self.prompt_kind.as_str(),
            escape_contract_field(&self.prompt_action),
            self.promptable,
            escape_contract_field(&self.prompt_source),
            escape_contract_field(&self.reason)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRefreshContract {
    pub initialized: bool,
    pub changed: usize,
    pub refresh_ui: bool,
    pub refresh_workers: bool,
    pub refresh_operations: bool,
    pub first_change_scope: Option<String>,
    pub first_change_kind: Option<String>,
    pub first_change_previous: Option<String>,
    pub first_change_current: Option<String>,
    pub first_change_path: Option<String>,
    pub first_change_reason: Option<String>,
    pub changes: Vec<PermissionRefreshChangeContract>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRefreshChangeContract {
    pub scope: String,
    pub kind: String,
    pub previous: String,
    pub current: String,
    pub path: String,
    pub reason: String,
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
            first_change_scope: None,
            first_change_kind: None,
            first_change_previous: None,
            first_change_current: None,
            first_change_path: None,
            first_change_reason: None,
            changes: Vec::new(),
        }
    }

    pub fn with_changes(mut self, changes: Vec<PermissionRefreshChangeContract>) -> Self {
        self.changed = changes.len();
        if let Some(first) = changes.first() {
            self.first_change_scope = Some(first.scope.clone());
            self.first_change_kind = Some(first.kind.clone());
            self.first_change_previous = Some(first.previous.clone());
            self.first_change_current = Some(first.current.clone());
            self.first_change_path = Some(first.path.clone());
            self.first_change_reason = Some(first.reason.clone());
        } else {
            self.first_change_scope = None;
            self.first_change_kind = None;
            self.first_change_previous = None;
            self.first_change_current = None;
            self.first_change_path = None;
            self.first_change_reason = None;
        }
        self.changes = changes;
        self
    }

    pub fn as_tsv(&self) -> String {
        let mut lines = vec![format!(
            "permission-refresh\taudience=ui\tinitialized={}\tchanged={}\trefresh-ui={}\trefresh-workers={}\trefresh-operations={}\tfirst-change-scope={}\tfirst-change-kind={}\tfirst-change-previous={}\tfirst-change-current={}\tfirst-change-path={}\tfirst-change-reason={}",
            self.initialized,
            self.changed,
            self.refresh_ui,
            self.refresh_workers,
            self.refresh_operations,
            self.first_change_scope
                .as_deref()
                .map(escape_contract_field)
                .unwrap_or_else(|| "-".to_string()),
            self.first_change_kind
                .as_deref()
                .map(escape_contract_field)
                .unwrap_or_else(|| "-".to_string()),
            self.first_change_previous
                .as_deref()
                .map(escape_contract_field)
                .unwrap_or_else(|| "-".to_string()),
            self.first_change_current
                .as_deref()
                .map(escape_contract_field)
                .unwrap_or_else(|| "-".to_string()),
            self.first_change_path
                .as_deref()
                .map(escape_contract_field)
                .unwrap_or_else(|| "-".to_string()),
            self.first_change_reason
                .as_deref()
                .map(escape_contract_field)
                .unwrap_or_else(|| "-".to_string())
        )];
        lines.extend(
            self.changes
                .iter()
                .map(PermissionRefreshChangeContract::as_tsv),
        );
        lines.join("\n")
    }
}

impl PermissionRefreshChangeContract {
    pub fn as_tsv(&self) -> String {
        format!(
            "permission-refresh-change\tscope={}\tkind={}\tprevious={}\tcurrent={}\tpath={}\treason={}",
            escape_contract_field(&self.scope),
            escape_contract_field(&self.kind),
            escape_contract_field(&self.previous),
            escape_contract_field(&self.current),
            escape_contract_field(&self.path),
            escape_contract_field(&self.reason)
        )
    }
}

fn escape_contract_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
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
        if self.permission_access.is_some() && self.permission_dialog.is_none() {
            return Err(GfmError::Format(
                "native app permission access binding requires a permission dialog".to_string(),
            ));
        }
        if let Some(access) = &self.permission_access {
            validate_permission_prompt_orchestration(access)?;
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

    pub fn with_permission_access(mut self, access: PermissionAccessContract) -> Self {
        let prompt = access.prompt_kind;
        let dialog = DialogContract::permission_prompt_for_action(prompt, &access.prompt_action);
        if self.permission_prompt.is_none()
            || self.permission_prompt == Some(PermissionPromptKind::General)
        {
            self.permission_dialog = Some(dialog);
            self.permission_prompt = Some(prompt);
        } else {
            self.permission_dialog.get_or_insert(dialog);
        }
        self.permission_access = Some(access);
        self
    }

    pub fn with_permission_onboarding(mut self, onboarding: PermissionOnboardingContract) -> Self {
        if onboarding.requires_surface() {
            self.permission_dialog =
                Some(DialogContract::permission_prompt(onboarding.prompt_kind));
            self.permission_prompt = Some(onboarding.prompt_kind);
        }
        self.permission_onboarding = Some(onboarding);
        self
    }

    pub fn with_sidebar_volumes(mut self, volumes: Vec<SidebarVolumeSpec>) -> Self {
        self.sidebar_volumes = volumes;
        self
    }

    pub fn with_sidebar_path_snapshot(mut self, paths: SidebarPathSnapshot) -> Self {
        self.sidebar_paths = paths;
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

fn validate_permission_prompt_orchestration(access: &PermissionAccessContract) -> Result<()> {
    let has_prompt_action = concrete_permission_value(&access.prompt_action);
    let has_prompt_source = concrete_permission_value(&access.prompt_source);
    let is_blocked_prompt = access.prompt_kind == PermissionPromptKind::Blocked;

    if access.promptable && (!has_prompt_action || !has_prompt_source || is_blocked_prompt) {
        return Err(GfmError::Format(format!(
            "native app permission access for `{}` is promptable without a concrete prompt action and source",
            access.path
        )));
    }
    if !access.promptable && has_prompt_action && !is_blocked_prompt {
        return Err(GfmError::Format(format!(
            "native app permission access for `{}` exposes an interactive prompt action without prompt orchestration",
            access.path
        )));
    }
    if !access.promptable && is_blocked_prompt && has_prompt_action && !has_prompt_source {
        return Err(GfmError::Format(format!(
            "native app permission access for `{}` blocks access without a concrete failure source",
            access.path
        )));
    }
    if !access.promptable
        && is_blocked_prompt
        && has_prompt_action
        && !access.prompt_action.starts_with("blocked-")
    {
        return Err(GfmError::Format(format!(
            "native app permission access for `{}` blocks access with an interactive prompt action",
            access.path
        )));
    }
    if has_prompt_source && !has_prompt_action {
        return Err(GfmError::Format(format!(
            "native app permission access for `{}` names a prompt source without a prompt action",
            access.path
        )));
    }
    if has_prompt_action
        && !permission_prompt_action_matches_kind(access.prompt_kind, &access.prompt_action)
    {
        return Err(GfmError::Format(format!(
            "native app permission access for `{}` pairs a prompt action with the wrong prompt kind",
            access.path
        )));
    }
    Ok(())
}

fn permission_prompt_action_matches_kind(kind: PermissionPromptKind, action: &str) -> bool {
    match kind {
        PermissionPromptKind::General => action == "none",
        PermissionPromptKind::FullDiskAccess => action == "open-settings",
        PermissionPromptKind::BookmarkAcquisition => action == "choose-location",
        PermissionPromptKind::DegradedSearch => action == "continue-metadata-only",
        PermissionPromptKind::Blocked => action.starts_with("blocked-"),
    }
}

fn concrete_permission_value(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty() && trimmed != "none"
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
            sidebar_paths: SidebarPathSnapshot::default(),
            sidebar_volumes: Vec::new(),
            progress_surfaces: Vec::new(),
            operation_conflicts: Vec::new(),
            permission_dialog: None,
            permission_prompt: None,
            permission_onboarding: None,
            permission_access: None,
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
    pub sidebar_paths: SidebarPathSnapshot,
    pub sidebar_volumes: Vec<SidebarVolumeSpec>,
    pub progress_surfaces: Vec<OperationProgressContract>,
    pub operation_conflicts: Vec<OperationConflictContract>,
    pub permission_dialog: Option<DialogSurface>,
    pub permission_prompt: Option<PermissionPromptKind>,
    pub permission_onboarding: Option<PermissionOnboardingContract>,
    pub permission_access: Option<PermissionAccessContract>,
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
            sidebar_paths: spec.sidebar_paths.clone(),
            sidebar_volumes: spec.sidebar_volumes.clone(),
            progress_surfaces: spec.progress_surfaces.clone(),
            operation_conflicts: spec.operation_conflicts.clone(),
            permission_dialog: spec.permission_dialog.as_ref().map(|dialog| dialog.surface),
            permission_prompt: spec.permission_prompt,
            permission_onboarding: spec.permission_onboarding.clone(),
            permission_access: spec.permission_access.clone(),
            permission_refresh: spec.permission_refresh.clone(),
        })
    }

    pub fn as_tsv(&self) -> String {
        let mut lines = vec![format!(
            "window\t{}\t{}\t{}x{}\tmin={}x{}\ttransparent-titlebar={}\tactivate={}\ttabs={}\tsidebar-home={}\tsidebar-icloud={}\tpermission-dialog={}",
            escape_contract_field(&self.title),
            escape_contract_field(&self.initial_path.display().to_string()),
            self.width,
            self.height,
            self.min_width,
            self.min_height,
            self.transparent_titlebar,
            self.activate_on_launch,
            escape_contract_field(&self.tabbing_identifier),
            self.sidebar_paths.home_state.as_str(),
            self.sidebar_paths.icloud_drive_state.as_str(),
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
        if let Some(access) = &self.permission_access {
            lines.push(access.as_tsv());
        }
        if let Some(onboarding) = &self.permission_onboarding {
            lines.push(onboarding.as_tsv());
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
            sidebar: sidebar::SidebarContract::from_path_snapshot(
                &spec.initial_path,
                spec.sidebar_paths.clone(),
                spec.sidebar_volumes.clone(),
            ),
            icon_view: IconViewContract::from_records(&[], IconViewOptions::default()),
            progress_surfaces: spec.progress_surfaces,
            operation_conflicts: spec.operation_conflicts,
            permission_dialog: spec.permission_dialog,
            permission_onboarding: spec.permission_onboarding,
            permission_access: spec.permission_access,
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
    permission_onboarding: Option<PermissionOnboardingContract>,
    permission_access: Option<PermissionAccessContract>,
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
            root = root.child(dialog::render_permission(
                dialog,
                self.permission_access.as_ref(),
            ));
        }
        if let Some(access) = &self.permission_access {
            root = root.child(
                div()
                    .id("permission-access-state")
                    .invisible()
                    .child(access.as_tsv()),
            );
        }
        if let Some(onboarding) = &self.permission_onboarding {
            root = root.child(
                div()
                    .id("permission-onboarding-state")
                    .invisible()
                    .child(onboarding.as_tsv()),
            );
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
        assert_eq!(contract.permission_onboarding, None);
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
            "window\tGFM\t/tmp/gfm\t1040x720\tmin=640x420\ttransparent-titlebar=true\tactivate=true\ttabs=gfm-main-window\tsidebar-home=available\tsidebar-icloud=missing\tpermission-dialog=none"
        );
    }

    #[test]
    fn lifecycle_window_tsv_escapes_control_characters_in_text_fields() {
        let mut spec = AppLaunchSpec::new("/tmp/Window\tPath\nRoot\r");
        spec.title = "GFM\tWindow\nTitle\r".to_string();
        spec.tabbing_identifier = "gfm\tmain\nwindow\r".to_string();
        let contract = WindowLifecycleContract::from_spec(&spec).unwrap();
        let tsv = contract.as_tsv();
        let window = tsv.lines().next().unwrap();

        assert!(window.contains("GFM\\tWindow\\nTitle\\r\t"), "{tsv}");
        assert!(window.contains("\t/tmp/Window\\tPath\\nRoot\\r\t"), "{tsv}");
        assert!(window.contains("\ttabs=gfm\\tmain\\nwindow\\r\t"), "{tsv}");
        assert_eq!(window.split('\t').count(), 11, "{tsv}");
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
    fn lifecycle_contract_tracks_permission_onboarding_state() {
        let onboarding = PermissionOnboardingContract::new(
            "open-full-disk-access",
            PermissionPromptKind::FullDiskAccess,
            "first-run",
            true,
            false,
        )
        .with_scopes(vec![PermissionOnboardingScopeContract::new(
            "desktop",
            "denied",
            "/Users/me/Desktop",
            "full disk access required",
        )]);
        let spec =
            AppLaunchSpec::new("/Users/me/Desktop").with_permission_onboarding(onboarding.clone());
        let contract = WindowLifecycleContract::from_spec(&spec).unwrap();

        assert_eq!(contract.permission_dialog, Some(DialogSurface::Permission));
        assert_eq!(
            contract.permission_prompt,
            Some(PermissionPromptKind::FullDiskAccess)
        );
        assert_eq!(contract.permission_onboarding, Some(onboarding));
        assert!(contract.as_tsv().contains(
            "\npermission-onboarding\taction=open-full-disk-access\tprompt-kind=full-disk-access\tprompt-mode=first-run\tfinder-parity-default=true\tmachine-search-ready=false"
        ));
        assert!(contract.as_tsv().contains(
            "\npermission-scope\tdesktop\tstate=denied\tpath=/Users/me/Desktop\treason=full disk access required"
        ));
    }

    #[test]
    fn lifecycle_contract_keeps_ready_permission_onboarding_nonmodal() {
        let onboarding = PermissionOnboardingContract::new(
            "continue-normally",
            PermissionPromptKind::General,
            "ready",
            false,
            true,
        );
        let spec = AppLaunchSpec::new("/Users/me").with_permission_onboarding(onboarding.clone());
        let contract = WindowLifecycleContract::from_spec(&spec).unwrap();

        assert_eq!(contract.permission_dialog, None);
        assert_eq!(contract.permission_prompt, None);
        assert_eq!(contract.permission_onboarding, Some(onboarding));
        assert!(contract
            .as_tsv()
            .contains("\npermission-onboarding\taction=continue-normally\t"));
    }

    #[test]
    fn lifecycle_contract_tracks_permission_access_state() {
        let access = PermissionAccessContract {
            path: "/Users/me/Documents/Plan.md".to_string(),
            intent: "read".to_string(),
            scope: "documents".to_string(),
            probe: "granted".to_string(),
            mode: "security-scoped-bookmark".to_string(),
            access_action: "allow".to_string(),
            worker_action: "start".to_string(),
            can_touch_filesystem: true,
            bookmark_required: false,
            bookmark_access: false,
            refresh_on_permission_change: false,
            prompt_kind: PermissionPromptKind::BookmarkAcquisition,
            prompt_action: "choose-location".to_string(),
            promptable: false,
            prompt_source: "none".to_string(),
            reason: "window initial path may start after retained security-scoped bookmark access"
                .to_string(),
        }
        .with_bookmark_state(true, true)
        .with_refresh_on_permission_change(false)
        .with_prompt_orchestration(true, "security-scoped-bookmark");
        let spec = AppLaunchSpec::new("/Users/me/Documents/Plan.md")
            .with_permission_access(access.clone());
        let contract = WindowLifecycleContract::from_spec(&spec).unwrap();

        assert_eq!(contract.permission_dialog, Some(DialogSurface::Permission));
        assert_eq!(
            contract.permission_prompt,
            Some(PermissionPromptKind::BookmarkAcquisition)
        );
        assert_eq!(contract.permission_access, Some(access));
        assert!(contract.as_tsv().contains(
            "\npermission-access\tpath=/Users/me/Documents/Plan.md\tintent=read\tscope=documents\t"
        ));
        assert!(contract.as_tsv().contains(
            "\tcan-touch-filesystem=true\tbookmark-required=true\tbookmark-access=true\t"
        ));
        assert!(contract
            .as_tsv()
            .contains("\tprompt-kind=bookmark-acquisition\tprompt-action=choose-location\tpromptable=true\tprompt-source=security-scoped-bookmark\t"));
    }

    #[test]
    fn lifecycle_permission_tsv_escapes_control_characters_in_text_fields() {
        let onboarding = PermissionOnboardingContract::new(
            "open\tsettings",
            PermissionPromptKind::FullDiskAccess,
            "first\nrun",
            true,
            false,
        )
        .with_scopes(vec![PermissionOnboardingScopeContract::new(
            "desktop\tfolder",
            "denied\nnow",
            "/Users/me/Desktop\tProjects\nDraft\r",
            "full disk\taccess\nrequired\\soon",
        )]);
        let access = PermissionAccessContract {
            path: "/Users/me/Documents\tDraft\nPlan\r.md".to_string(),
            intent: "read\tpreview".to_string(),
            scope: "documents\nfolder".to_string(),
            probe: "granted\tnative".to_string(),
            mode: "security-scoped\tbookmark".to_string(),
            access_action: "allow\nread".to_string(),
            worker_action: "start\rworker".to_string(),
            can_touch_filesystem: true,
            bookmark_required: true,
            bookmark_access: true,
            refresh_on_permission_change: true,
            prompt_kind: PermissionPromptKind::BookmarkAcquisition,
            prompt_action: "choose-location".to_string(),
            promptable: true,
            prompt_source: "security-scoped\nbookmark".to_string(),
            reason: "retained\tbookmark\nrequired\\now".to_string(),
        };
        let refresh =
            PermissionRefreshContract::new(false, 1, true, true, true).with_changes(vec![
                PermissionRefreshChangeContract {
                    scope: "desktop\tfolder".to_string(),
                    kind: "granted\nread".to_string(),
                    previous: "denied\rbefore".to_string(),
                    current: "granted\\now".to_string(),
                    path: "/Users/me/Desktop\tProjects\nDraft\r".to_string(),
                    reason: "macOS\tgranted\nread\\access".to_string(),
                },
            ]);
        let spec = AppLaunchSpec::new("/tmp/gfm")
            .with_permission_onboarding(onboarding)
            .with_permission_access(access)
            .with_permission_refresh(refresh);
        let contract = WindowLifecycleContract::from_spec(&spec).unwrap();
        let tsv = contract.as_tsv();
        let onboarding = tsv
            .lines()
            .find(|line| line.starts_with("permission-onboarding\t"))
            .unwrap();
        let scope = tsv
            .lines()
            .find(|line| line.starts_with("permission-scope\t"))
            .unwrap();
        let access = tsv
            .lines()
            .find(|line| line.starts_with("permission-access\t"))
            .unwrap();
        let refresh = tsv
            .lines()
            .find(|line| line.starts_with("permission-refresh\t"))
            .unwrap();
        let change = tsv
            .lines()
            .find(|line| line.starts_with("permission-refresh-change\t"))
            .unwrap();

        assert!(onboarding.contains("action=open\\tsettings\t"), "{tsv}");
        assert!(onboarding.contains("prompt-mode=first\\nrun\t"), "{tsv}");
        assert!(
            scope.contains("permission-scope\tdesktop\\tfolder\t"),
            "{tsv}"
        );
        assert!(
            scope.contains("path=/Users/me/Desktop\\tProjects\\nDraft\\r\t"),
            "{tsv}"
        );
        assert!(
            scope.contains("reason=full disk\\taccess\\nrequired\\\\soon"),
            "{tsv}"
        );
        assert!(
            access.contains("path=/Users/me/Documents\\tDraft\\nPlan\\r.md\t"),
            "{tsv}"
        );
        assert!(
            access.contains("prompt-source=security-scoped\\nbookmark\t"),
            "{tsv}"
        );
        assert!(
            access.contains("reason=retained\\tbookmark\\nrequired\\\\now"),
            "{tsv}"
        );
        assert!(
            refresh.contains("first-change-path=/Users/me/Desktop\\tProjects\\nDraft\\r\t"),
            "{tsv}"
        );
        assert!(
            refresh.contains("first-change-reason=macOS\\tgranted\\nread\\\\access"),
            "{tsv}"
        );
        assert!(change.contains("scope=desktop\\tfolder\t"), "{tsv}");
        assert!(change.contains("current=granted\\\\now\t"), "{tsv}");
        assert_eq!(onboarding.split('\t').count(), 6, "{tsv}");
        assert_eq!(scope.split('\t').count(), 5, "{tsv}");
        assert_eq!(access.split('\t').count(), 17, "{tsv}");
        assert_eq!(refresh.split('\t').count(), 13, "{tsv}");
        assert_eq!(change.split('\t').count(), 7, "{tsv}");
    }

    #[test]
    fn permission_dialog_renderer_accepts_access_action_state() {
        let access = PermissionAccessContract {
            path: "/Users/me/Documents/Plan.md".to_string(),
            intent: "preview".to_string(),
            scope: "documents".to_string(),
            probe: "granted".to_string(),
            mode: "security-scoped-bookmark".to_string(),
            access_action: "allow".to_string(),
            worker_action: "start".to_string(),
            can_touch_filesystem: true,
            bookmark_required: true,
            bookmark_access: true,
            refresh_on_permission_change: false,
            prompt_kind: PermissionPromptKind::BookmarkAcquisition,
            prompt_action: "choose-location".to_string(),
            promptable: true,
            prompt_source: "security-scoped-bookmark".to_string(),
            reason: "preview worker may start after retained bookmark access".to_string(),
        };
        let dialog = DialogContract::permission_prompt(PermissionPromptKind::BookmarkAcquisition);

        let _element = dialog::render_permission(&dialog, Some(&access));

        assert!(access
            .as_tsv()
            .contains("\tprompt-action=choose-location\tpromptable=true\tprompt-source=security-scoped-bookmark\t"));
    }

    #[test]
    fn rejects_promptable_permission_access_without_prompt_source() {
        let access = PermissionAccessContract {
            path: "/Users/me/Documents/Plan.md".to_string(),
            intent: "preview".to_string(),
            scope: "documents".to_string(),
            probe: "granted".to_string(),
            mode: "security-scoped-bookmark".to_string(),
            access_action: "prompt".to_string(),
            worker_action: "prompt".to_string(),
            can_touch_filesystem: false,
            bookmark_required: true,
            bookmark_access: false,
            refresh_on_permission_change: false,
            prompt_kind: PermissionPromptKind::BookmarkAcquisition,
            prompt_action: "choose-location".to_string(),
            promptable: true,
            prompt_source: "none".to_string(),
            reason: "preview worker needs retained bookmark access".to_string(),
        };
        let spec = AppLaunchSpec::new("/Users/me/Documents/Plan.md").with_permission_access(access);

        let err = WindowLifecycleContract::from_spec(&spec).unwrap_err();

        assert!(err
            .to_string()
            .contains("promptable without a concrete prompt action and source"));
    }

    #[test]
    fn rejects_non_promptable_permission_access_with_interactive_prompt_action() {
        let access = PermissionAccessContract {
            path: "/Users/me/Documents/Plan.md".to_string(),
            intent: "preview".to_string(),
            scope: "documents".to_string(),
            probe: "granted".to_string(),
            mode: "security-scoped-bookmark".to_string(),
            access_action: "prompt".to_string(),
            worker_action: "prompt".to_string(),
            can_touch_filesystem: false,
            bookmark_required: true,
            bookmark_access: false,
            refresh_on_permission_change: false,
            prompt_kind: PermissionPromptKind::BookmarkAcquisition,
            prompt_action: "choose-location".to_string(),
            promptable: false,
            prompt_source: "none".to_string(),
            reason: "preview worker needs retained bookmark access".to_string(),
        };
        let spec = AppLaunchSpec::new("/Users/me/Documents/Plan.md").with_permission_access(access);

        let err = WindowLifecycleContract::from_spec(&spec).unwrap_err();

        assert!(err
            .to_string()
            .contains("interactive prompt action without prompt orchestration"));
    }

    #[test]
    fn rejects_permission_access_without_permission_dialog() {
        let mut spec = AppLaunchSpec::new("/tmp/gfm");
        spec.permission_access = Some(PermissionAccessContract {
            path: "/tmp/gfm".to_string(),
            intent: "read".to_string(),
            scope: "none".to_string(),
            probe: "missing".to_string(),
            mode: "denied".to_string(),
            access_action: "deny".to_string(),
            worker_action: "deny".to_string(),
            can_touch_filesystem: false,
            bookmark_required: false,
            bookmark_access: false,
            refresh_on_permission_change: false,
            prompt_kind: PermissionPromptKind::Blocked,
            prompt_action: "blocked-missing-path".to_string(),
            promptable: false,
            prompt_source: "missing-path".to_string(),
            reason: "path is not present on this host".to_string(),
        });

        let err = WindowLifecycleContract::from_spec(&spec).unwrap_err();
        assert!(err
            .to_string()
            .contains("permission access binding requires a permission dialog"));
    }

    #[test]
    fn rejects_blocked_permission_access_without_failure_source() {
        let access = PermissionAccessContract {
            path: "/tmp/gfm".to_string(),
            intent: "read".to_string(),
            scope: "none".to_string(),
            probe: "missing".to_string(),
            mode: "denied".to_string(),
            access_action: "deny".to_string(),
            worker_action: "deny".to_string(),
            can_touch_filesystem: false,
            bookmark_required: false,
            bookmark_access: false,
            refresh_on_permission_change: false,
            prompt_kind: PermissionPromptKind::Blocked,
            prompt_action: "blocked-missing-path".to_string(),
            promptable: false,
            prompt_source: "none".to_string(),
            reason: "path is not present on this host".to_string(),
        };
        let spec = AppLaunchSpec::new("/tmp/gfm").with_permission_access(access);

        let err = WindowLifecycleContract::from_spec(&spec).unwrap_err();
        assert!(err
            .to_string()
            .contains("blocks access without a concrete failure source"));
    }

    #[test]
    fn rejects_blocked_permission_access_with_blank_failure_source() {
        let access = PermissionAccessContract {
            path: "/tmp/gfm".to_string(),
            intent: "read".to_string(),
            scope: "none".to_string(),
            probe: "missing".to_string(),
            mode: "denied".to_string(),
            access_action: "deny".to_string(),
            worker_action: "deny".to_string(),
            can_touch_filesystem: false,
            bookmark_required: false,
            bookmark_access: false,
            refresh_on_permission_change: false,
            prompt_kind: PermissionPromptKind::Blocked,
            prompt_action: "blocked-missing-path".to_string(),
            promptable: false,
            prompt_source: "   ".to_string(),
            reason: "path is not present on this host".to_string(),
        };
        let spec = AppLaunchSpec::new("/tmp/gfm").with_permission_access(access);

        let err = WindowLifecycleContract::from_spec(&spec).unwrap_err();
        assert!(err
            .to_string()
            .contains("blocks access without a concrete failure source"));
    }

    #[test]
    fn rejects_blocked_permission_access_with_interactive_prompt_action() {
        let access = PermissionAccessContract {
            path: "/tmp/gfm".to_string(),
            intent: "read".to_string(),
            scope: "none".to_string(),
            probe: "missing".to_string(),
            mode: "denied".to_string(),
            access_action: "deny".to_string(),
            worker_action: "deny".to_string(),
            can_touch_filesystem: false,
            bookmark_required: false,
            bookmark_access: false,
            refresh_on_permission_change: false,
            prompt_kind: PermissionPromptKind::Blocked,
            prompt_action: "choose-location".to_string(),
            promptable: false,
            prompt_source: "missing-path".to_string(),
            reason: "path is not present on this host".to_string(),
        };
        let spec = AppLaunchSpec::new("/tmp/gfm").with_permission_access(access);

        let err = WindowLifecycleContract::from_spec(&spec).unwrap_err();
        assert!(err
            .to_string()
            .contains("blocks access with an interactive prompt action"));
    }

    #[test]
    fn rejects_permission_access_with_mismatched_prompt_kind_and_action() {
        let access = PermissionAccessContract {
            path: "/Users/me/Library/Mail".to_string(),
            intent: "index".to_string(),
            scope: "full-disk-access".to_string(),
            probe: "denied".to_string(),
            mode: "full-disk-access".to_string(),
            access_action: "prompt".to_string(),
            worker_action: "prompt".to_string(),
            can_touch_filesystem: false,
            bookmark_required: false,
            bookmark_access: false,
            refresh_on_permission_change: true,
            prompt_kind: PermissionPromptKind::FullDiskAccess,
            prompt_action: "choose-location".to_string(),
            promptable: true,
            prompt_source: "full-disk-access".to_string(),
            reason: "protected root requires Full Disk Access guidance".to_string(),
        };
        let spec = AppLaunchSpec::new("/Users/me/Library/Mail").with_permission_access(access);

        let err = WindowLifecycleContract::from_spec(&spec).unwrap_err();
        assert!(err
            .to_string()
            .contains("pairs a prompt action with the wrong prompt kind"));
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
    fn lifecycle_contract_tracks_sidebar_path_snapshot_from_launch_spec() {
        let paths = SidebarPathSnapshot {
            home: PathBuf::from("/Users/tester"),
            home_state: SidebarPathState::Available,
            desktop_state: SidebarPathState::Unavailable,
            applications_state: SidebarPathState::Available,
            documents_state: SidebarPathState::Missing,
            downloads_state: SidebarPathState::Available,
            icloud_drive: None,
            icloud_drive_state: SidebarPathState::Missing,
        };
        let spec = AppLaunchSpec::new("/Users/tester").with_sidebar_path_snapshot(paths.clone());
        let contract = WindowLifecycleContract::from_spec(&spec).unwrap();

        assert_eq!(contract.sidebar_paths, paths);
        assert!(contract
            .as_tsv()
            .contains("\tsidebar-home=available\tsidebar-icloud=missing\t"));
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
        let refresh =
            PermissionRefreshContract::new(false, 1, true, true, true).with_changes(vec![
                PermissionRefreshChangeContract {
                    scope: "desktop".to_string(),
                    kind: "granted".to_string(),
                    previous: "denied".to_string(),
                    current: "granted".to_string(),
                    path: "/Users/me/Desktop".to_string(),
                    reason: "macOS granted read access".to_string(),
                },
            ]);
        let spec = AppLaunchSpec::new("/tmp/gfm").with_permission_refresh(refresh.clone());
        let contract = WindowLifecycleContract::from_spec(&spec).unwrap();

        assert_eq!(contract.permission_refresh, Some(refresh));
        assert!(contract
            .as_tsv()
            .contains("\npermission-refresh\taudience=ui\tinitialized=false\tchanged=1\t"));
        assert!(contract
            .as_tsv()
            .contains("\tfirst-change-scope=desktop\tfirst-change-kind=granted\tfirst-change-previous=denied\tfirst-change-current=granted\tfirst-change-path=/Users/me/Desktop\tfirst-change-reason=macOS granted read access"));
        assert!(contract.as_tsv().contains(
            "\npermission-refresh-change\tscope=desktop\tkind=granted\tprevious=denied\tcurrent=granted\t"
        ));
    }
}
