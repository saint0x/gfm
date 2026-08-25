use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextSurface {
    File,
    Folder,
    Volume,
    Sidebar,
    Empty,
    Selection,
    SearchResult,
    Trash,
}

impl ContextSurface {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Folder => "folder",
            Self::Volume => "volume",
            Self::Sidebar => "sidebar",
            Self::Empty => "empty",
            Self::Selection => "selection",
            Self::SearchResult => "search-result",
            Self::Trash => "trash",
        }
    }
}

impl FromStr for ContextSurface {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "file" => Ok(Self::File),
            "folder" => Ok(Self::Folder),
            "volume" => Ok(Self::Volume),
            "sidebar" => Ok(Self::Sidebar),
            "empty" => Ok(Self::Empty),
            "selection" => Ok(Self::Selection),
            "search-result" => Ok(Self::SearchResult),
            "trash" => Ok(Self::Trash),
            _ => Err(format!("unknown context surface: {value}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextItemKind {
    Command,
    Submenu,
    Separator,
}

impl ContextItemKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::Submenu => "submenu",
            Self::Separator => "separator",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextMenuItemSpec {
    pub id: &'static str,
    pub title: &'static str,
    pub action: &'static str,
    pub kind: ContextItemKind,
    pub enabled: bool,
    pub destructive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextMenuInput {
    pub surface: ContextSurface,
    pub selection_count: u16,
    pub writable: bool,
    pub ejectable: bool,
    pub has_clipboard_items: bool,
}

impl ContextMenuInput {
    pub const fn new(surface: ContextSurface) -> Self {
        Self {
            surface,
            selection_count: 1,
            writable: true,
            ejectable: false,
            has_clipboard_items: true,
        }
    }

    pub const fn empty_space() -> Self {
        Self {
            surface: ContextSurface::Empty,
            selection_count: 0,
            writable: true,
            ejectable: false,
            has_clipboard_items: true,
        }
    }

    pub fn with_selection_count(mut self, selection_count: u16) -> Self {
        self.selection_count = selection_count;
        self
    }

    pub fn with_writable(mut self, writable: bool) -> Self {
        self.writable = writable;
        self
    }

    pub fn with_ejectable(mut self, ejectable: bool) -> Self {
        self.ejectable = ejectable;
        self
    }

    pub fn with_clipboard_items(mut self, has_clipboard_items: bool) -> Self {
        self.has_clipboard_items = has_clipboard_items;
        self
    }

    pub const fn has_selection(&self) -> bool {
        self.selection_count > 0
    }

    pub const fn single_selection(&self) -> bool {
        self.selection_count == 1
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextMenuContract {
    pub surface: ContextSurface,
    pub selection_count: u16,
    pub items: Vec<ContextMenuItemSpec>,
}

impl ContextMenuContract {
    pub fn finder_default(input: ContextMenuInput) -> Self {
        Self {
            surface: input.surface,
            selection_count: input.selection_count,
            items: context_items(&input),
        }
    }

    pub fn as_tsv(&self) -> String {
        let mut lines = Vec::with_capacity(self.items.len() + 1);
        lines.push(format!(
            "context-menu\tsurface={}\tselection={}\titems={}",
            self.surface.as_str(),
            self.selection_count,
            self.items.len()
        ));
        lines.extend(self.items.iter().map(|item| {
            format!(
                "item\t{}\t{}\t{}\t{}\tenabled={}\tdestructive={}",
                item.id,
                item.title,
                item.action,
                item.kind.as_str(),
                item.enabled,
                item.destructive
            )
        }));
        lines.join("\n")
    }
}

fn context_items(input: &ContextMenuInput) -> Vec<ContextMenuItemSpec> {
    match input.surface {
        ContextSurface::File | ContextSurface::Folder | ContextSurface::SearchResult => {
            selected_item_menu(input)
        }
        ContextSurface::Selection => selected_set_menu(input),
        ContextSurface::Volume => volume_menu(input),
        ContextSurface::Sidebar => sidebar_menu(input),
        ContextSurface::Empty => empty_space_menu(input),
        ContextSurface::Trash => trash_menu(input),
    }
}

fn selected_item_menu(input: &ContextMenuInput) -> Vec<ContextMenuItemSpec> {
    let mut items = vec![
        command("open", "Open", "gfm::Open", input.has_selection()),
        submenu(
            "open-with",
            "Open With",
            "gfm::OpenWith",
            input.single_selection(),
        ),
        separator("sep-open"),
        command(
            "get-info",
            "Get Info",
            "gfm::GetInfo",
            input.has_selection(),
        ),
        command(
            "rename",
            "Rename",
            "gfm::Rename",
            input.single_selection() && input.writable,
        ),
        command(
            "duplicate",
            "Duplicate",
            "gfm::Duplicate",
            input.has_selection() && input.writable,
        ),
        command(
            "make-alias",
            "Make Alias",
            "gfm::MakeAlias",
            input.has_selection() && input.writable,
        ),
        command(
            "quick-look",
            "Quick Look",
            "gfm::QuickLook",
            input.has_selection(),
        ),
        separator("sep-share"),
        command("share", "Share...", "gfm::Share", input.has_selection()),
        command(
            "tags",
            "Tags...",
            "gfm::Tags",
            input.has_selection() && input.writable,
        ),
        command(
            "copy-path",
            "Copy as Pathname",
            "gfm::CopyPath",
            input.has_selection(),
        ),
    ];
    if input.surface == ContextSurface::SearchResult {
        items.push(command(
            "show-original",
            "Show in Enclosing Folder",
            "gfm::EnclosingFolder",
            input.has_selection(),
        ));
    }
    items.extend([
        separator("sep-trash"),
        destructive(
            "move-to-trash",
            "Move to Trash",
            "gfm::MoveToTrash",
            input.has_selection() && input.writable,
        ),
    ]);
    items
}

fn selected_set_menu(input: &ContextMenuInput) -> Vec<ContextMenuItemSpec> {
    vec![
        command("open", "Open", "gfm::Open", input.has_selection()),
        command(
            "get-info",
            "Get Info",
            "gfm::GetInfo",
            input.has_selection(),
        ),
        command(
            "quick-look",
            "Quick Look",
            "gfm::QuickLook",
            input.has_selection(),
        ),
        separator("sep-edit"),
        command(
            "duplicate",
            "Duplicate",
            "gfm::Duplicate",
            input.has_selection() && input.writable,
        ),
        command(
            "make-alias",
            "Make Alias",
            "gfm::MakeAlias",
            input.has_selection() && input.writable,
        ),
        command("share", "Share...", "gfm::Share", input.has_selection()),
        command(
            "tags",
            "Tags...",
            "gfm::Tags",
            input.has_selection() && input.writable,
        ),
        separator("sep-trash"),
        destructive(
            "move-to-trash",
            "Move to Trash",
            "gfm::MoveToTrash",
            input.has_selection() && input.writable,
        ),
    ]
}

fn volume_menu(input: &ContextMenuInput) -> Vec<ContextMenuItemSpec> {
    vec![
        command("open", "Open", "gfm::Open", true),
        command("get-info", "Get Info", "gfm::GetInfo", true),
        separator("sep-volume"),
        command("eject", "Eject", "gfm::Eject", input.ejectable),
    ]
}

fn sidebar_menu(input: &ContextMenuInput) -> Vec<ContextMenuItemSpec> {
    vec![
        command("open", "Open", "gfm::Open", true),
        command(
            "show-original",
            "Show in Enclosing Folder",
            "gfm::EnclosingFolder",
            true,
        ),
        command("get-info", "Get Info", "gfm::GetInfo", true),
        separator("sep-sidebar"),
        command("eject", "Eject", "gfm::Eject", input.ejectable),
    ]
}

fn empty_space_menu(input: &ContextMenuInput) -> Vec<ContextMenuItemSpec> {
    vec![
        command("new-folder", "New Folder", "gfm::NewFolder", input.writable),
        command(
            "paste-item",
            "Paste Item",
            "gfm::PasteItem",
            input.writable && input.has_clipboard_items,
        ),
        separator("sep-arrange"),
        submenu("sort-by", "Sort By", "gfm::SortBy", true),
        command("clean-up", "Clean Up", "gfm::CleanUp", true),
        command(
            "show-view-options",
            "Show View Options",
            "gfm::ShowViewOptions",
            true,
        ),
        separator("sep-search"),
        command("search-in-folder", "Search", "gfm::SearchInFolder", true),
    ]
}

fn trash_menu(input: &ContextMenuInput) -> Vec<ContextMenuItemSpec> {
    vec![
        command(
            "put-back",
            "Put Back",
            "gfm::PutBack",
            input.has_selection(),
        ),
        command(
            "get-info",
            "Get Info",
            "gfm::GetInfo",
            input.has_selection(),
        ),
        separator("sep-trash"),
        destructive(
            "delete-immediately",
            "Delete Immediately...",
            "gfm::DeleteImmediately",
            input.has_selection() && input.writable,
        ),
        destructive(
            "empty-trash",
            "Empty Trash...",
            "gfm::EmptyTrash",
            input.writable,
        ),
    ]
}

fn command(
    id: &'static str,
    title: &'static str,
    action: &'static str,
    enabled: bool,
) -> ContextMenuItemSpec {
    item(id, title, action, ContextItemKind::Command, enabled, false)
}

fn submenu(
    id: &'static str,
    title: &'static str,
    action: &'static str,
    enabled: bool,
) -> ContextMenuItemSpec {
    item(id, title, action, ContextItemKind::Submenu, enabled, false)
}

fn destructive(
    id: &'static str,
    title: &'static str,
    action: &'static str,
    enabled: bool,
) -> ContextMenuItemSpec {
    item(id, title, action, ContextItemKind::Command, enabled, true)
}

fn separator(id: &'static str) -> ContextMenuItemSpec {
    item(id, "-", "-", ContextItemKind::Separator, false, false)
}

fn item(
    id: &'static str,
    title: &'static str,
    action: &'static str,
    kind: ContextItemKind,
    enabled: bool,
    destructive: bool,
) -> ContextMenuItemSpec {
    ContextMenuItemSpec {
        id,
        title,
        action,
        kind,
        enabled,
        destructive,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_context_contains_finder_selection_actions() {
        let contract =
            ContextMenuContract::finder_default(ContextMenuInput::new(ContextSurface::File));

        assert!(contract.items.iter().any(|item| item.id == "open"));
        assert!(contract.items.iter().any(|item| item.id == "open-with"));
        assert!(contract.items.iter().any(|item| item.id == "get-info"));
        assert!(contract.items.iter().any(|item| item.id == "quick-look"));
        assert!(contract.items.iter().any(|item| item.id == "move-to-trash"));
    }

    #[test]
    fn empty_context_disables_paste_without_clipboard_items() {
        let contract = ContextMenuContract::finder_default(
            ContextMenuInput::empty_space().with_clipboard_items(false),
        );
        let paste = contract
            .items
            .iter()
            .find(|item| item.id == "paste-item")
            .expect("paste item");

        assert!(!paste.enabled);
    }

    #[test]
    fn trash_context_marks_destructive_actions() {
        let contract =
            ContextMenuContract::finder_default(ContextMenuInput::new(ContextSurface::Trash));

        assert!(contract
            .items
            .iter()
            .filter(|item| item.destructive)
            .all(|item| item.title.contains("Trash") || item.title.contains("Delete")));
    }

    #[test]
    fn output_is_stable_for_cli_and_fozzy() {
        let tsv = ContextMenuContract::finder_default(ContextMenuInput::new(
            ContextSurface::SearchResult,
        ))
        .as_tsv();

        assert!(tsv.starts_with("context-menu\tsurface=search-result\tselection=1\titems="));
        assert!(tsv.contains("item\tshow-original\tShow in Enclosing Folder\tgfm::EnclosingFolder\tcommand\tenabled=true\tdestructive=false"));
        assert!(tsv.contains(
            "item\tmove-to-trash\tMove to Trash\tgfm::MoveToTrash\tcommand\tenabled=true\tdestructive=true"
        ));
    }
}
