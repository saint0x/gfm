use std::str::FromStr;

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationProgressContract {
    pub dialog: DialogContract,
    pub label: String,
    pub state: OperationProgressState,
    pub completed_units: u64,
    pub total_units: u64,
    pub detail: String,
    pub percent_complete: u64,
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
            label: input.label,
            state: input.state,
            completed_units: input.completed_units,
            total_units: input.total_units,
            detail: input.detail,
            percent_complete,
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "{}\noperation-progress\tlabel={}\tstate={}\tcompleted={}\ttotal={}\tpercent={}\tdetail={}",
            self.dialog.as_tsv(),
            escape_tsv(&self.label),
            self.state.as_str(),
            self.completed_units,
            self.total_units,
            self.percent_complete,
            escape_tsv(&self.detail),
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
        message: "Finder-compatible conflict sheet for replace, keep both, stop, skip, and apply-to-all decisions.",
        icon: "system-alert",
        buttons: vec![
            button("replace", "Replace", DialogButtonRole::Destructive, true),
            button("keep-both", "Keep Both", DialogButtonRole::Default, true),
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
        assert!(contract
            .fields
            .iter()
            .any(|field| field.id == "apply-to-all"));
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
            "operation-progress\tlabel=copy selected files\tstate=paused\tcompleted=42\ttotal=100\tpercent=42\tdetail=pressure:throttled"
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
}
