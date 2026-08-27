use std::str::FromStr;

use gpui::{div, prelude::*, px, rgb, IntoElement, Styled};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogSurface {
    Alert,
    Rename,
    Popover,
    Disclosure,
    Progress,
    Conflict,
    Permission,
}

impl DialogSurface {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Alert => "alert",
            Self::Rename => "rename",
            Self::Popover => "popover",
            Self::Disclosure => "disclosure",
            Self::Progress => "progress",
            Self::Conflict => "conflict",
            Self::Permission => "permission",
        }
    }
}

impl FromStr for DialogSurface {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "alert" => Ok(Self::Alert),
            "rename" => Ok(Self::Rename),
            "popover" => Ok(Self::Popover),
            "disclosure" => Ok(Self::Disclosure),
            "progress" => Ok(Self::Progress),
            "conflict" => Ok(Self::Conflict),
            "permission" => Ok(Self::Permission),
            _ => Err(format!("unknown dialog surface: {value}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogPresentation {
    WindowSheet,
    InlineEditor,
    AnchoredPopover,
    InlineDisclosure,
    ProgressSheet,
}

impl DialogPresentation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WindowSheet => "window-sheet",
            Self::InlineEditor => "inline-editor",
            Self::AnchoredPopover => "anchored-popover",
            Self::InlineDisclosure => "inline-disclosure",
            Self::ProgressSheet => "progress-sheet",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogButtonRole {
    Default,
    Cancel,
    Destructive,
    Alternate,
}

impl DialogButtonRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Cancel => "cancel",
            Self::Destructive => "destructive",
            Self::Alternate => "alternate",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogFieldKind {
    None,
    Text,
    Checkbox,
    Progress,
    Disclosure,
}

impl DialogFieldKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Text => "text",
            Self::Checkbox => "checkbox",
            Self::Progress => "progress",
            Self::Disclosure => "disclosure",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionPromptKind {
    General,
    FullDiskAccess,
    BookmarkAcquisition,
    DegradedSearch,
    Blocked,
}

impl PermissionPromptKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::FullDiskAccess => "full-disk-access",
            Self::BookmarkAcquisition => "bookmark-acquisition",
            Self::DegradedSearch => "degraded-search",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogButtonSpec {
    pub id: &'static str,
    pub title: &'static str,
    pub role: DialogButtonRole,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogFieldSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub kind: DialogFieldKind,
    pub required: bool,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogContract {
    pub surface: DialogSurface,
    pub presentation: DialogPresentation,
    pub title: &'static str,
    pub message: &'static str,
    pub icon: &'static str,
    pub buttons: Vec<DialogButtonSpec>,
    pub fields: Vec<DialogFieldSpec>,
    pub blocks_parent_window: bool,
    pub escape_cancels: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationProgressState {
    Planned,
    Running,
    Paused,
    Completed,
    Cancelled,
    Failed,
}

impl OperationProgressState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }

    pub const fn is_paused(self) -> bool {
        matches!(self, Self::Paused)
    }

    pub const fn is_cancellable(self) -> bool {
        matches!(self, Self::Planned | Self::Running | Self::Paused)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationProgressInput {
    pub job_id: Option<u64>,
    pub label: String,
    pub state: OperationProgressState,
    pub completed_units: u64,
    pub total_units: u64,
    pub detail: String,
}

impl OperationProgressInput {
    pub fn new(
        label: impl Into<String>,
        state: OperationProgressState,
        completed_units: u64,
        total_units: u64,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            job_id: None,
            label: label.into(),
            state,
            completed_units: completed_units.min(total_units),
            total_units,
            detail: detail.into(),
        }
    }

    pub fn percent_complete(&self) -> u64 {
        if self.total_units == 0 {
            0
        } else {
            self.completed_units.saturating_mul(100) / self.total_units
        }
    }

    pub fn with_job_id(mut self, job_id: u64) -> Self {
        self.job_id = Some(job_id);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationProgressContract {
    pub dialog: DialogContract,
    pub job_id: Option<u64>,
    pub label: String,
    pub state: OperationProgressState,
    pub completed_units: u64,
    pub total_units: u64,
    pub detail: String,
    pub percent_complete: u64,
    pub commands: Vec<OperationProgressCommandSpec>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationProgressCommand {
    Pause,
    Resume,
    Stop,
}

impl OperationProgressCommand {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pause => "pause",
            Self::Resume => "resume",
            Self::Stop => "stop",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationProgressCommandSpec {
    pub command: OperationProgressCommand,
    pub job_id: Option<u64>,
    pub enabled: bool,
}

impl OperationProgressContract {
    pub fn from_input(input: OperationProgressInput) -> Self {
        let dialog = DialogContract::operation_progress(
            input.state.is_paused(),
            input.state.is_cancellable(),
        );
        let percent_complete = input.percent_complete();

        Self {
            dialog,
            job_id: input.job_id,
            label: input.label,
            state: input.state,
            completed_units: input.completed_units,
            total_units: input.total_units,
            detail: input.detail,
            percent_complete,
            commands: operation_progress_commands(input.job_id, input.state),
        }
    }

    pub fn as_tsv(&self) -> String {
        let mut lines = vec![format!(
            "{}\noperation-progress\tjob={}\tlabel={}\tstate={}\tcompleted={}\ttotal={}\tpercent={}\tdetail={}",
            self.dialog.as_tsv(),
            self.job_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "-".to_string()),
            escape_tsv(&self.label),
            self.state.as_str(),
            self.completed_units,
            self.total_units,
            self.percent_complete,
            escape_tsv(&self.detail),
        )];
        lines.extend(self.commands.iter().map(|command| {
            format!(
                "operation-progress-command\t{}\tjob={}\tenabled={}",
                command.command.as_str(),
                command
                    .job_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                command.enabled
            )
        }));
        lines.join("\n")
    }
}

fn operation_progress_commands(
    job_id: Option<u64>,
    state: OperationProgressState,
) -> Vec<OperationProgressCommandSpec> {
    let addressable = job_id.is_some();
    vec![
        OperationProgressCommandSpec {
            command: OperationProgressCommand::Pause,
            job_id,
            enabled: addressable
                && matches!(
                    state,
                    OperationProgressState::Planned | OperationProgressState::Running
                ),
        },
        OperationProgressCommandSpec {
            command: OperationProgressCommand::Resume,
            job_id,
            enabled: addressable && state == OperationProgressState::Paused,
        },
        OperationProgressCommandSpec {
            command: OperationProgressCommand::Stop,
            job_id,
            enabled: addressable && state.is_cancellable(),
        },
    ]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderConflictInput {
    pub path: String,
    pub has_unresolved_conflict: bool,
    pub affected_count: usize,
    pub affected_paths: Vec<String>,
    pub reveal_enabled: bool,
    pub operations_blocked: bool,
    pub reason: String,
}

impl ProviderConflictInput {
    pub fn new(
        path: impl Into<String>,
        has_unresolved_conflict: bool,
        affected_paths: Vec<String>,
        reveal_enabled: bool,
        operations_blocked: bool,
        reason: impl Into<String>,
    ) -> Self {
        let affected_count = affected_paths.len();
        Self {
            path: path.into(),
            has_unresolved_conflict,
            affected_count,
            affected_paths,
            reveal_enabled,
            operations_blocked,
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderConflictContract {
    pub dialog: DialogContract,
    pub path: String,
    pub has_unresolved_conflict: bool,
    pub affected_count: usize,
    pub affected_paths: Vec<String>,
    pub reveal_enabled: bool,
    pub operations_blocked: bool,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationConflictInput {
    pub operation: String,
    pub target: String,
    pub target_kind: String,
    pub selected_policy: String,
    pub available_policies: Vec<String>,
    pub blocks_operation: bool,
    pub reason: String,
}

impl OperationConflictInput {
    pub fn new(
        operation: impl Into<String>,
        target: impl Into<String>,
        target_kind: impl Into<String>,
        selected_policy: impl Into<String>,
        available_policies: Vec<String>,
        blocks_operation: bool,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            operation: operation.into(),
            target: target.into(),
            target_kind: target_kind.into(),
            selected_policy: selected_policy.into(),
            available_policies,
            blocks_operation,
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationConflictContract {
    pub dialog: DialogContract,
    pub operation: String,
    pub target: String,
    pub target_kind: String,
    pub selected_policy: String,
    pub available_policies: Vec<String>,
    pub blocks_operation: bool,
    pub reason: String,
    pub initial_focus: String,
    pub default_action: String,
    pub cancel_action: String,
    pub keyboard_model: String,
    pub review_rows: Vec<OperationConflictReviewRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationConflictReviewRow {
    pub ordinal: usize,
    pub operation: String,
    pub target: String,
    pub target_kind: String,
    pub selected_policy: String,
    pub reason: String,
}

impl OperationConflictContract {
    pub fn from_input(input: OperationConflictInput) -> Self {
        Self::from_inputs(vec![input]).expect("operation conflict input must produce a contract")
    }

    pub fn from_inputs(inputs: Vec<OperationConflictInput>) -> Option<Self> {
        let first = inputs.first()?;
        let available_policies = shared_available_policies(&inputs);
        let blocks_operation = inputs.iter().any(|input| input.blocks_operation);
        let operation = if inputs.len() == 1 {
            first.operation.clone()
        } else {
            "batch".to_string()
        };
        let target = if inputs.len() == 1 {
            first.target.clone()
        } else {
            format!("{} items", inputs.len())
        };
        let target_kind = if inputs.len() == 1 {
            first.target_kind.clone()
        } else {
            "mixed".to_string()
        };
        let selected_policy = if inputs.len() == 1 {
            first.selected_policy.clone()
        } else {
            "fail".to_string()
        };
        let reason = if inputs.len() == 1 {
            first.reason.clone()
        } else {
            format!(
                "{}-operation-conflicts-require-user-resolution",
                inputs.len()
            )
        };
        let mut dialog = DialogContract::finder_default(DialogSurface::Conflict);
        dialog.buttons = dialog
            .buttons
            .into_iter()
            .map(|button| DialogButtonSpec {
                enabled: if button.id == "stop" {
                    blocks_operation
                } else {
                    available_policies.iter().any(|policy| policy == button.id)
                },
                ..button
            })
            .collect();
        if !blocks_operation {
            dialog.blocks_parent_window = false;
        }
        let default_action = dialog
            .buttons
            .iter()
            .find(|button| button.role == DialogButtonRole::Default && button.enabled)
            .map(|button| button.id)
            .or_else(|| {
                dialog
                    .buttons
                    .iter()
                    .find(|button| button.enabled)
                    .map(|button| button.id)
            })
            .unwrap_or("-")
            .to_string();
        let cancel_action = dialog
            .buttons
            .iter()
            .find(|button| button.role == DialogButtonRole::Cancel && button.enabled)
            .map(|button| button.id)
            .unwrap_or("-")
            .to_string();
        let initial_focus = if blocks_operation {
            default_action.clone()
        } else {
            "-".to_string()
        };
        let review_rows = inputs
            .iter()
            .enumerate()
            .map(|(ordinal, input)| OperationConflictReviewRow {
                ordinal,
                operation: input.operation.clone(),
                target: input.target.clone(),
                target_kind: input.target_kind.clone(),
                selected_policy: input.selected_policy.clone(),
                reason: input.reason.clone(),
            })
            .collect();

        Some(Self {
            dialog,
            operation,
            target,
            target_kind,
            selected_policy,
            available_policies,
            blocks_operation,
            reason,
            initial_focus,
            default_action,
            cancel_action,
            keyboard_model: "finder-conflict-sheet-return-default-escape-cancel-tab-cycle"
                .to_string(),
            review_rows,
        })
    }

    pub fn as_tsv(&self) -> String {
        let mut lines = vec![format!(
            "{}\noperation-conflict-ui\toperation={}\ttarget={}\tkind={}\tpolicy={}\tavailable={}\tblocks-operation={}\tfocus={}\tdefault-action={}\tcancel-action={}\tkeyboard={}\treason={}",
            self.dialog.as_tsv(),
            escape_tsv(&self.operation),
            escape_tsv(&self.target),
            escape_tsv(&self.target_kind),
            escape_tsv(&self.selected_policy),
            self.available_policies.join(","),
            self.blocks_operation,
            escape_tsv(&self.initial_focus),
            escape_tsv(&self.default_action),
            escape_tsv(&self.cancel_action),
            escape_tsv(&self.keyboard_model),
            escape_tsv(&self.reason)
        )];
        lines.extend(self.review_rows.iter().map(|row| {
            format!(
                "operation-conflict-row\t{}\toperation={}\ttarget={}\tkind={}\tpolicy={}\treason={}",
                row.ordinal,
                escape_tsv(&row.operation),
                escape_tsv(&row.target),
                escape_tsv(&row.target_kind),
                escape_tsv(&row.selected_policy),
                escape_tsv(&row.reason)
            )
        }));
        lines.join("\n")
    }
}

fn shared_available_policies(inputs: &[OperationConflictInput]) -> Vec<String> {
    let Some(first) = inputs.first() else {
        return Vec::new();
    };
    first
        .available_policies
        .iter()
        .filter(|policy| {
            inputs
                .iter()
                .all(|input| input.available_policies.iter().any(|item| item == *policy))
        })
        .cloned()
        .collect()
}

impl ProviderConflictContract {
    pub fn from_input(input: ProviderConflictInput) -> Self {
        let mut dialog = DialogContract::provider_conflict(input.reveal_enabled);
        if !input.has_unresolved_conflict {
            dialog.buttons = dialog
                .buttons
                .into_iter()
                .map(|button| DialogButtonSpec {
                    enabled: false,
                    ..button
                })
                .collect();
        }

        Self {
            dialog,
            path: input.path,
            has_unresolved_conflict: input.has_unresolved_conflict,
            affected_count: input.affected_count,
            affected_paths: input.affected_paths,
            reveal_enabled: input.reveal_enabled,
            operations_blocked: input.operations_blocked,
            reason: input.reason,
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "{}\nprovider-conflict\tpath={}\tconflict={}\taffected={}\taffected-paths={}\treveal={}\toperations-blocked={}\treason={}",
            self.dialog.as_tsv(),
            escape_tsv(&self.path),
            self.has_unresolved_conflict,
            self.affected_count,
            affected_paths_tsv(&self.affected_paths),
            self.reveal_enabled,
            self.operations_blocked,
            escape_tsv(&self.reason),
        )
    }
}

impl DialogContract {
    pub fn finder_default(surface: DialogSurface) -> Self {
        match surface {
            DialogSurface::Alert => alert_contract(),
            DialogSurface::Rename => rename_contract(),
            DialogSurface::Popover => popover_contract(),
            DialogSurface::Disclosure => disclosure_contract(),
            DialogSurface::Progress => progress_contract(),
            DialogSurface::Conflict => conflict_contract(),
            DialogSurface::Permission => permission_contract(),
        }
    }

    pub fn operation_progress(paused: bool, cancellable: bool) -> Self {
        let mut contract = progress_contract();
        contract.message = if paused {
            "Finder-compatible operation progress sheet for paused foreground work that can resume or stop."
        } else {
            "Finder-compatible operation progress sheet for running foreground work that can pause or stop."
        };
        contract.buttons = vec![
            button("pause", "Pause", DialogButtonRole::Alternate, !paused),
            button("resume", "Resume", DialogButtonRole::Default, paused),
            button("stop", "Stop", DialogButtonRole::Cancel, cancellable),
        ];
        contract
    }

    pub fn provider_conflict(reveal_enabled: bool) -> Self {
        let mut contract = conflict_contract();
        contract.title = "Resolve FileProvider Conflict";
        contract.message =
            "Finder-compatible FileProvider conflict sheet backed by unresolved provider state.";
        contract.buttons = vec![
            button(
                "reveal-conflict",
                "Reveal Conflict",
                DialogButtonRole::Default,
                reveal_enabled,
            ),
            button("skip", "Skip", DialogButtonRole::Alternate, true),
            button("stop", "Stop", DialogButtonRole::Cancel, true),
        ];
        contract.fields.clear();
        contract
    }

    pub fn permission_prompt(kind: PermissionPromptKind) -> Self {
        let mut contract = permission_contract();
        match kind {
            PermissionPromptKind::General => {}
            PermissionPromptKind::FullDiskAccess => {
                contract.title = "Allow Full Disk Access";
                contract.message =
                    "Open macOS Privacy settings to grant Full Disk Access for protected locations.";
                contract.buttons = vec![
                    button(
                        "open-settings",
                        "Open Settings",
                        DialogButtonRole::Default,
                        true,
                    ),
                    button("not-now", "Not Now", DialogButtonRole::Cancel, true),
                ];
            }
            PermissionPromptKind::BookmarkAcquisition => {
                contract.title = "Choose a Folder to Continue";
                contract.message =
                    "Select the protected location so GFM can retain least-privilege access.";
                contract.buttons = vec![
                    button(
                        "choose-location",
                        "Choose...",
                        DialogButtonRole::Default,
                        true,
                    ),
                    button("not-now", "Not Now", DialogButtonRole::Cancel, true),
                ];
            }
            PermissionPromptKind::DegradedSearch => {
                contract.title = "Search Will Use Metadata Only";
                contract.message =
                    "Some protected locations are unavailable, so content search will continue in degraded mode.";
                contract.buttons = vec![
                    button("continue", "Continue", DialogButtonRole::Default, true),
                    button(
                        "open-settings",
                        "Open Settings",
                        DialogButtonRole::Alternate,
                        true,
                    ),
                ];
            }
            PermissionPromptKind::Blocked => {
                contract.title = "Permission Required";
                contract.message =
                    "Grant access before continuing with this protected file operation.";
                contract.buttons = vec![
                    button(
                        "choose-location",
                        "Choose...",
                        DialogButtonRole::Default,
                        true,
                    ),
                    button(
                        "open-settings",
                        "Open Settings",
                        DialogButtonRole::Alternate,
                        true,
                    ),
                    button("not-now", "Not Now", DialogButtonRole::Cancel, true),
                ];
            }
        }
        contract
    }

    pub fn as_tsv(&self) -> String {
        let mut lines = Vec::with_capacity(self.buttons.len() + self.fields.len() + 1);
        lines.push(format!(
            "dialog\tsurface={}\tpresentation={}\ttitle={}\tmessage={}\ticon={}\tblocks-parent={}\tescape-cancels={}",
            self.surface.as_str(),
            self.presentation.as_str(),
            self.title,
            self.message,
            self.icon,
            self.blocks_parent_window,
            self.escape_cancels
        ));
        lines.extend(self.fields.iter().map(|field| {
            format!(
                "field\t{}\t{}\t{}\trequired={}\tenabled={}",
                field.id,
                field.label,
                field.kind.as_str(),
                field.required,
                field.enabled
            )
        }));
        lines.extend(self.buttons.iter().map(|button| {
            format!(
                "button\t{}\t{}\t{}\tenabled={}",
                button.id,
                button.title,
                button.role.as_str(),
                button.enabled
            )
        }));
        lines.join("\n")
    }
}

pub fn render(contract: &DialogContract) -> impl IntoElement {
    let sheet_id = match contract.surface {
        DialogSurface::Alert => "alert-sheet",
        DialogSurface::Rename => "rename-sheet",
        DialogSurface::Popover => "popover-sheet",
        DialogSurface::Disclosure => "disclosure-sheet",
        DialogSurface::Progress => "progress-sheet",
        DialogSurface::Conflict => "conflict-sheet",
        DialogSurface::Permission => "permission-sheet",
    };
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_start()
        .justify_center()
        .pt(px(58.0))
        .bg(rgb(0x111113))
        .child(
            div()
                .id(sheet_id)
                .w(px(420.0))
                .p(px(18.0))
                .rounded(px(8.0))
                .border_1()
                .border_color(rgb(0x5f6368))
                .bg(rgb(0x2d2d30))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.0))
                                .child(
                                    div()
                                        .w(px(28.0))
                                        .h(px(28.0))
                                        .rounded(px(6.0))
                                        .bg(rgb(0x4f8cff)),
                                )
                                .child(
                                    div()
                                        .text_size(px(16.0))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(rgb(0xf2f2f2))
                                        .child(contract.title),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(13.0))
                                .line_height(px(18.0))
                                .text_color(rgb(0xd4d4d4))
                                .child(contract.message),
                        )
                        .child(render_buttons(contract)),
                ),
        )
}

fn render_buttons(contract: &DialogContract) -> impl IntoElement {
    let mut row = div().flex().justify_end().gap(px(8.0));
    for button in &contract.buttons {
        let bg = if button.role == DialogButtonRole::Default {
            rgb(0x0a84ff)
        } else {
            rgb(0x3a3a3c)
        };
        row = row.child(
            div()
                .px(px(12.0))
                .h(px(28.0))
                .rounded(px(6.0))
                .flex()
                .items_center()
                .justify_center()
                .bg(if button.enabled { bg } else { rgb(0x2a2a2c) })
                .text_size(px(12.0))
                .text_color(if button.enabled {
                    rgb(0xffffff)
                } else {
                    rgb(0x8a8a8d)
                })
                .child(button.title),
        );
    }
    row
}

fn alert_contract() -> DialogContract {
    DialogContract {
        surface: DialogSurface::Alert,
        presentation: DialogPresentation::WindowSheet,
        title: "The operation cannot be completed",
        message: "Finder-compatible alert sheet for recoverable file-operation failures.",
        icon: "system-alert",
        buttons: vec![
            button("ok", "OK", DialogButtonRole::Default, true),
            button("cancel", "Cancel", DialogButtonRole::Cancel, true),
        ],
        fields: Vec::new(),
        blocks_parent_window: true,
        escape_cancels: true,
    }
}

fn rename_contract() -> DialogContract {
    DialogContract {
        surface: DialogSurface::Rename,
        presentation: DialogPresentation::InlineEditor,
        title: "Rename",
        message:
            "Inline Finder-compatible filename editor committed by Return and cancelled by Escape.",
        icon: "file-text",
        buttons: Vec::new(),
        fields: vec![field(
            "filename",
            "Filename",
            DialogFieldKind::Text,
            true,
            true,
        )],
        blocks_parent_window: false,
        escape_cancels: true,
    }
}

fn popover_contract() -> DialogContract {
    DialogContract {
        surface: DialogSurface::Popover,
        presentation: DialogPresentation::AnchoredPopover,
        title: "View Options",
        message: "Finder-compatible anchored options popover with live view settings.",
        icon: "slider-horizontal",
        buttons: vec![button(
            "use-as-defaults",
            "Use as Defaults",
            DialogButtonRole::Alternate,
            true,
        )],
        fields: vec![
            field(
                "always-open-in",
                "Always open in view",
                DialogFieldKind::Checkbox,
                false,
                true,
            ),
            field(
                "browse-in-view",
                "Browse in view",
                DialogFieldKind::Checkbox,
                false,
                true,
            ),
        ],
        blocks_parent_window: false,
        escape_cancels: true,
    }
}

fn disclosure_contract() -> DialogContract {
    DialogContract {
        surface: DialogSurface::Disclosure,
        presentation: DialogPresentation::InlineDisclosure,
        title: "Details",
        message: "Finder-compatible disclosure region for expandable metadata or advanced choices.",
        icon: "disclosure-triangle",
        buttons: Vec::new(),
        fields: vec![field(
            "details",
            "Details",
            DialogFieldKind::Disclosure,
            false,
            true,
        )],
        blocks_parent_window: false,
        escape_cancels: true,
    }
}

fn progress_contract() -> DialogContract {
    DialogContract {
        surface: DialogSurface::Progress,
        presentation: DialogPresentation::ProgressSheet,
        title: "Copying",
        message: "Finder-compatible operation progress sheet for running foreground work that can pause or stop.",
        icon: "progress",
        buttons: vec![
            button("pause", "Pause", DialogButtonRole::Alternate, true),
            button("resume", "Resume", DialogButtonRole::Default, false),
            button("stop", "Stop", DialogButtonRole::Cancel, true),
        ],
        fields: vec![field(
            "progress",
            "Progress",
            DialogFieldKind::Progress,
            false,
            true,
        )],
        blocks_parent_window: false,
        escape_cancels: false,
    }
}

fn conflict_contract() -> DialogContract {
    DialogContract {
        surface: DialogSurface::Conflict,
        presentation: DialogPresentation::WindowSheet,
        title: "An item with the same name already exists",
        message: "Finder-compatible conflict sheet for replace, keep both, merge, stop, skip, and apply-to-all decisions.",
        icon: "system-alert",
        buttons: vec![
            button("replace", "Replace", DialogButtonRole::Destructive, true),
            button("keep-both", "Keep Both", DialogButtonRole::Default, true),
            button("merge", "Merge", DialogButtonRole::Alternate, true),
            button("skip", "Skip", DialogButtonRole::Alternate, true),
            button("stop", "Stop", DialogButtonRole::Cancel, true),
        ],
        fields: vec![field(
            "apply-to-all",
            "Apply to All",
            DialogFieldKind::Checkbox,
            false,
            true,
        )],
        blocks_parent_window: true,
        escape_cancels: true,
    }
}

fn permission_contract() -> DialogContract {
    DialogContract {
        surface: DialogSurface::Permission,
        presentation: DialogPresentation::WindowSheet,
        title: "GFM needs permission to continue",
        message: "Finder-compatible permission prompt for protected locations and degraded machine search.",
        icon: "privacy",
        buttons: vec![
            button("open-settings", "Open Settings", DialogButtonRole::Default, true),
            button("not-now", "Not Now", DialogButtonRole::Cancel, true),
        ],
        fields: Vec::new(),
        blocks_parent_window: true,
        escape_cancels: true,
    }
}

fn button(
    id: &'static str,
    title: &'static str,
    role: DialogButtonRole,
    enabled: bool,
) -> DialogButtonSpec {
    DialogButtonSpec {
        id,
        title,
        role,
        enabled,
    }
}

fn field(
    id: &'static str,
    label: &'static str,
    kind: DialogFieldKind,
    required: bool,
    enabled: bool,
) -> DialogFieldSpec {
    DialogFieldSpec {
        id,
        label,
        kind,
        required,
        enabled,
    }
}

fn affected_paths_tsv(paths: &[String]) -> String {
    if paths.is_empty() {
        "-".to_string()
    } else {
        paths
            .iter()
            .map(|path| escape_tsv(path))
            .collect::<Vec<_>>()
            .join(",")
    }
}

fn escape_tsv(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '\t' | '\n' | '\r' => ' ',
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_contract_is_inline_and_keyboard_cancelable() {
        let contract = DialogContract::finder_default(DialogSurface::Rename);

        assert_eq!(contract.presentation, DialogPresentation::InlineEditor);
        assert!(!contract.blocks_parent_window);
        assert!(contract.escape_cancels);
        assert_eq!(contract.fields[0].kind, DialogFieldKind::Text);
        assert!(contract.fields[0].required);
    }

    #[test]
    fn conflict_contract_contains_required_decisions() {
        let contract = DialogContract::finder_default(DialogSurface::Conflict);

        assert!(contract.blocks_parent_window);
        assert!(contract
            .buttons
            .iter()
            .any(|button| button.id == "replace" && button.role == DialogButtonRole::Destructive));
        assert!(contract
            .buttons
            .iter()
            .any(|button| button.id == "keep-both"));
        assert!(contract.buttons.iter().any(|button| button.id == "merge"));
        assert!(contract.buttons.iter().any(|button| button.id == "skip"));
        assert!(contract.buttons.iter().any(|button| button.id == "stop"));
        assert!(contract
            .fields
            .iter()
            .any(|field| field.id == "apply-to-all"));
    }

    #[test]
    fn provider_conflict_contract_binds_reveal_intent_and_blocks_operations() {
        let contract = ProviderConflictContract::from_input(ProviderConflictInput::new(
            "/tmp/Conflict.icloud-conflict.md",
            true,
            vec!["/tmp/Conflict.icloud-conflict.md".to_string()],
            true,
            true,
            "conflict-requires-user-resolution",
        ));

        assert!(contract.dialog.blocks_parent_window);
        assert!(contract.operations_blocked);
        assert!(contract
            .dialog
            .buttons
            .iter()
            .any(|button| button.id == "reveal-conflict" && button.enabled));
        assert!(contract.as_tsv().contains(
            "provider-conflict\tpath=/tmp/Conflict.icloud-conflict.md\tconflict=true\taffected=1\taffected-paths=/tmp/Conflict.icloud-conflict.md\treveal=true\toperations-blocked=true\treason=conflict-requires-user-resolution"
        ));
    }

    #[test]
    fn provider_conflict_contract_disables_actions_without_unresolved_conflict() {
        let contract = ProviderConflictContract::from_input(ProviderConflictInput::new(
            "/tmp/Downloaded.icloud.md",
            false,
            Vec::new(),
            false,
            false,
            "no-provider-conflict",
        ));

        assert!(contract.dialog.buttons.iter().all(|button| !button.enabled));
        assert!(contract
            .as_tsv()
            .contains("\tconflict=false\taffected=0\taffected-paths=-\treveal=false\toperations-blocked=false\t"));
    }

    #[test]
    fn operation_conflict_contract_enables_only_available_resolutions() {
        let contract = OperationConflictContract::from_input(OperationConflictInput::new(
            "copy",
            "/tmp/target",
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

        assert!(contract
            .dialog
            .buttons
            .iter()
            .any(|button| button.id == "replace" && button.enabled));
        assert!(contract
            .dialog
            .buttons
            .iter()
            .any(|button| button.id == "merge" && !button.enabled));
        assert!(contract
            .dialog
            .buttons
            .iter()
            .any(|button| button.id == "stop" && button.enabled));
        assert_eq!(contract.initial_focus, "keep-both");
        assert_eq!(contract.default_action, "keep-both");
        assert_eq!(contract.cancel_action, "stop");
        assert_eq!(contract.review_rows.len(), 1);
        assert!(contract
            .as_tsv()
            .contains("\noperation-conflict-ui\toperation=copy\ttarget=/tmp/target\tkind=file\t"));
        assert!(contract.as_tsv().contains(
            "\noperation-conflict-row\t0\toperation=copy\ttarget=/tmp/target\tkind=file\t"
        ));
    }

    #[test]
    fn operation_conflict_contract_reports_no_focus_when_not_blocking() {
        let contract = OperationConflictContract::from_input(OperationConflictInput::new(
            "copy",
            "/tmp/new-target",
            "none",
            "fail",
            Vec::new(),
            false,
            "target-available",
        ));

        assert!(!contract.dialog.blocks_parent_window);
        assert!(contract.dialog.buttons.iter().all(|button| !button.enabled));
        assert_eq!(contract.initial_focus, "-");
        assert_eq!(contract.default_action, "-");
        assert_eq!(contract.cancel_action, "-");
        assert!(contract
            .as_tsv()
            .contains("\tfocus=-\tdefault-action=-\tcancel-action=-\t"));
    }

    #[test]
    fn operation_conflict_contract_batches_review_rows_and_shared_actions() {
        let contract = OperationConflictContract::from_inputs(vec![
            OperationConflictInput::new(
                "copy",
                "/tmp/file-target",
                "file",
                "fail",
                vec![
                    "replace".to_string(),
                    "keep-both".to_string(),
                    "skip".to_string(),
                ],
                true,
                "destination-conflict-requires-user-resolution",
            ),
            OperationConflictInput::new(
                "move",
                "/tmp/directory-target",
                "directory",
                "fail",
                vec![
                    "replace".to_string(),
                    "keep-both".to_string(),
                    "merge".to_string(),
                    "skip".to_string(),
                ],
                true,
                "destination-conflict-requires-user-resolution",
            ),
        ])
        .unwrap();

        assert_eq!(contract.operation, "batch");
        assert_eq!(contract.target, "2 items");
        assert_eq!(
            contract.available_policies,
            vec![
                "replace".to_string(),
                "keep-both".to_string(),
                "skip".to_string()
            ]
        );
        assert_eq!(contract.review_rows.len(), 2);
        assert!(contract
            .dialog
            .buttons
            .iter()
            .any(|button| button.id == "merge" && !button.enabled));
        assert!(contract
            .as_tsv()
            .contains("\noperation-conflict-row\t1\toperation=move\ttarget=/tmp/directory-target\tkind=directory\t"));
    }

    #[test]
    fn progress_sheet_is_not_escape_cancelled() {
        let contract = DialogContract::finder_default(DialogSurface::Progress);

        assert_eq!(contract.presentation, DialogPresentation::ProgressSheet);
        assert!(!contract.escape_cancels);
        assert!(contract
            .fields
            .iter()
            .any(|field| field.kind == DialogFieldKind::Progress));
        assert!(contract
            .buttons
            .iter()
            .any(|button| button.id == "pause" && button.enabled));
        assert!(contract
            .buttons
            .iter()
            .any(|button| button.id == "resume" && !button.enabled));
        assert!(contract
            .buttons
            .iter()
            .any(|button| button.id == "stop" && button.enabled));
    }

    #[test]
    fn paused_progress_sheet_enables_resume_and_disables_pause() {
        let contract = DialogContract::operation_progress(true, true);

        assert!(contract.message.contains("paused"));
        assert!(contract
            .buttons
            .iter()
            .any(|button| button.id == "pause" && !button.enabled));
        assert!(contract
            .buttons
            .iter()
            .any(|button| button.id == "resume" && button.enabled));
        assert!(contract
            .buttons
            .iter()
            .any(|button| button.id == "stop" && button.enabled));
    }

    #[test]
    fn noncancellable_progress_sheet_disables_stop() {
        let contract = DialogContract::operation_progress(false, false);

        assert!(contract
            .buttons
            .iter()
            .any(|button| button.id == "stop" && !button.enabled));
    }

    #[test]
    fn operation_progress_contract_reflects_paused_job_progress() {
        let contract = OperationProgressContract::from_input(OperationProgressInput::new(
            "copy selected files",
            OperationProgressState::Paused,
            42,
            100,
            "pressure:throttled",
        ));

        assert_eq!(contract.percent_complete, 42);
        assert!(contract.commands.iter().any(|command| {
            command.command == OperationProgressCommand::Resume && !command.enabled
        }));
        assert!(contract
            .dialog
            .buttons
            .iter()
            .any(|button| button.id == "pause" && !button.enabled));
        assert!(contract
            .dialog
            .buttons
            .iter()
            .any(|button| button.id == "resume" && button.enabled));
        assert!(contract.as_tsv().contains(
            "operation-progress\tjob=-\tlabel=copy selected files\tstate=paused\tcompleted=42\ttotal=100\tpercent=42\tdetail=pressure:throttled"
        ));
    }

    #[test]
    fn operation_progress_contract_disables_stop_for_terminal_jobs() {
        let contract = OperationProgressContract::from_input(OperationProgressInput::new(
            "compact content",
            OperationProgressState::Completed,
            7,
            7,
            "done",
        ));

        assert!(contract
            .dialog
            .buttons
            .iter()
            .any(|button| button.id == "stop" && !button.enabled));
    }

    #[test]
    fn output_is_stable_for_cli_and_fozzy() {
        let tsv = DialogContract::finder_default(DialogSurface::Permission).as_tsv();

        assert!(tsv.starts_with("dialog\tsurface=permission\tpresentation=window-sheet"));
        assert!(tsv.contains("button\topen-settings\tOpen Settings\tdefault\tenabled=true"));
        assert!(tsv.contains("button\tnot-now\tNot Now\tcancel\tenabled=true"));
    }

    #[test]
    fn permission_prompt_variants_expose_distinct_actions() {
        let full_disk =
            DialogContract::permission_prompt(PermissionPromptKind::FullDiskAccess).as_tsv();
        assert!(full_disk.contains("\ttitle=Allow Full Disk Access\t"));
        assert!(full_disk.contains("button\topen-settings\tOpen Settings\tdefault"));

        let bookmark =
            DialogContract::permission_prompt(PermissionPromptKind::BookmarkAcquisition).as_tsv();
        assert!(bookmark.contains("\ttitle=Choose a Folder to Continue\t"));
        assert!(bookmark.contains("button\tchoose-location\tChoose...\tdefault"));

        let degraded =
            DialogContract::permission_prompt(PermissionPromptKind::DegradedSearch).as_tsv();
        assert!(degraded.contains("\ttitle=Search Will Use Metadata Only\t"));
        assert!(degraded.contains("button\tcontinue\tContinue\tdefault"));
    }
}
