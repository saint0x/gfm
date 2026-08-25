use gpui::{actions, Action, KeyBinding, Menu, MenuItem, OsAction, SystemMenuType};

actions!(
    gfm,
    [
        NewWindow,
        CloseWindow,
        Open,
        OpenWith,
        NewFolder,
        PasteItem,
        GetInfo,
        Rename,
        Duplicate,
        MakeAlias,
        QuickLook,
        Share,
        Tags,
        CopyPath,
        MoveToTrash,
        PutBack,
        DeleteImmediately,
        EmptyTrash,
        Eject,
        Find,
        SearchInFolder,
        CleanUp,
        SortBy,
        ShowViewOptions,
        ToggleSidebar,
        IconView,
        ListView,
        ColumnView,
        GalleryView,
        Back,
        Forward,
        EnclosingFolder,
        Home,
        Desktop,
        Documents,
        Downloads,
        Applications,
        ConnectToServer,
        Minimize,
        Zoom,
        BringAllToFront,
        Help,
        Quit,
    ]
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuCommandState {
    Global,
    View,
    Selection,
    System,
}

impl MenuCommandState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::View => "view",
            Self::Selection => "selection",
            Self::System => "system",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuCommandSpec {
    pub menu: &'static str,
    pub title: &'static str,
    pub action: &'static str,
    pub shortcut: Option<&'static str>,
    pub state: MenuCommandState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuContract {
    pub menus: Vec<&'static str>,
    pub commands: Vec<MenuCommandSpec>,
    pub services_menu: bool,
}

impl MenuContract {
    pub fn finder_default() -> Self {
        Self {
            menus: vec!["GFM", "File", "Edit", "View", "Go", "Window", "Help"],
            commands: command_specs(),
            services_menu: true,
        }
    }

    pub fn as_tsv(&self) -> String {
        let mut lines = Vec::with_capacity(self.commands.len() + 2);
        lines.push(format!(
            "menus\t{}\tservices={}",
            self.menus.join(","),
            self.services_menu
        ));
        lines.extend(self.commands.iter().map(|command| {
            format!(
                "command\t{}\t{}\t{}\t{}\t{}",
                command.menu,
                command.title,
                command.action,
                command.shortcut.unwrap_or("-"),
                command.state.as_str()
            )
        }));
        lines.join("\n")
    }
}

pub fn native_menus() -> Vec<Menu> {
    vec![
        Menu {
            name: "GFM".into(),
            items: vec![
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Hide GFM", Help),
                MenuItem::action("Hide Others", Help),
                MenuItem::action("Show All", Help),
                MenuItem::separator(),
                MenuItem::action("Quit GFM", Quit),
            ],
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Window", NewWindow),
                MenuItem::action("Close Window", CloseWindow),
                MenuItem::separator(),
                MenuItem::action("Open", Open),
                MenuItem::action("Open With", OpenWith),
                MenuItem::separator(),
                MenuItem::action("Get Info", GetInfo),
                MenuItem::action("Rename", Rename),
                MenuItem::action("Duplicate", Duplicate),
                MenuItem::action("Make Alias", MakeAlias),
                MenuItem::action("Quick Look", QuickLook),
                MenuItem::separator(),
                MenuItem::action("Move to Trash", MoveToTrash),
            ],
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::os_action("Undo", Help, OsAction::Undo),
                MenuItem::os_action("Redo", Help, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("Cut", Help, OsAction::Cut),
                MenuItem::os_action("Copy", Help, OsAction::Copy),
                MenuItem::os_action("Paste", Help, OsAction::Paste),
                MenuItem::os_action("Select All", Help, OsAction::SelectAll),
                MenuItem::separator(),
                MenuItem::action("Find", Find),
            ],
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("as Icons", IconView),
                MenuItem::action("as List", ListView),
                MenuItem::action("as Columns", ColumnView),
                MenuItem::action("as Gallery", GalleryView),
                MenuItem::separator(),
                MenuItem::action("Show View Options", ShowViewOptions),
                MenuItem::action("Toggle Sidebar", ToggleSidebar),
            ],
        },
        Menu {
            name: "Go".into(),
            items: vec![
                MenuItem::action("Back", Back),
                MenuItem::action("Forward", Forward),
                MenuItem::action("Enclosing Folder", EnclosingFolder),
                MenuItem::separator(),
                MenuItem::action("Home", Home),
                MenuItem::action("Desktop", Desktop),
                MenuItem::action("Documents", Documents),
                MenuItem::action("Downloads", Downloads),
                MenuItem::action("Applications", Applications),
                MenuItem::separator(),
                MenuItem::action("Connect to Server", ConnectToServer),
            ],
        },
        Menu {
            name: "Window".into(),
            items: vec![
                MenuItem::action("Minimize", Minimize),
                MenuItem::action("Zoom", Zoom),
                MenuItem::separator(),
                MenuItem::action("Bring All to Front", BringAllToFront),
            ],
        },
        Menu {
            name: "Help".into(),
            items: vec![MenuItem::action("GFM Help", Help)],
        },
    ]
}

pub fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("cmd-n", NewWindow, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
        KeyBinding::new("cmd-o", Open, None),
        KeyBinding::new("cmd-i", GetInfo, None),
        KeyBinding::new("enter", Rename, None),
        KeyBinding::new("cmd-d", Duplicate, None),
        KeyBinding::new("cmd-l", MakeAlias, None),
        KeyBinding::new("space", QuickLook, None),
        KeyBinding::new("cmd-backspace", MoveToTrash, None),
        KeyBinding::new("cmd-f", Find, None),
        KeyBinding::new("cmd-j", ShowViewOptions, None),
        KeyBinding::new("cmd-1", IconView, None),
        KeyBinding::new("cmd-2", ListView, None),
        KeyBinding::new("cmd-3", ColumnView, None),
        KeyBinding::new("cmd-4", GalleryView, None),
        KeyBinding::new("cmd-left", Back, None),
        KeyBinding::new("cmd-right", Forward, None),
        KeyBinding::new("cmd-up", EnclosingFolder, None),
        KeyBinding::new("shift-cmd-h", Home, None),
        KeyBinding::new("shift-cmd-d", Desktop, None),
        KeyBinding::new("shift-cmd-o", Documents, None),
        KeyBinding::new("alt-cmd-l", Downloads, None),
        KeyBinding::new("shift-cmd-a", Applications, None),
        KeyBinding::new("cmd-k", ConnectToServer, None),
        KeyBinding::new("cmd-m", Minimize, None),
        KeyBinding::new("cmd-?", Help, None),
        KeyBinding::new("cmd-q", Quit, None),
    ]
}

fn command_specs() -> Vec<MenuCommandSpec> {
    vec![
        command(
            "GFM",
            "Services",
            "system::Services",
            None,
            MenuCommandState::System,
        ),
        command(
            "GFM",
            "Quit GFM",
            Quit.name(),
            Some("cmd-q"),
            MenuCommandState::Global,
        ),
        command(
            "File",
            "New Window",
            NewWindow.name(),
            Some("cmd-n"),
            MenuCommandState::Global,
        ),
        command(
            "File",
            "Close Window",
            CloseWindow.name(),
            Some("cmd-w"),
            MenuCommandState::Global,
        ),
        command(
            "File",
            "Open",
            Open.name(),
            Some("cmd-o"),
            MenuCommandState::Selection,
        ),
        command(
            "File",
            "Open With",
            OpenWith.name(),
            None,
            MenuCommandState::Selection,
        ),
        command(
            "File",
            "Get Info",
            GetInfo.name(),
            Some("cmd-i"),
            MenuCommandState::Selection,
        ),
        command(
            "File",
            "Rename",
            Rename.name(),
            Some("enter"),
            MenuCommandState::Selection,
        ),
        command(
            "File",
            "Duplicate",
            Duplicate.name(),
            Some("cmd-d"),
            MenuCommandState::Selection,
        ),
        command(
            "File",
            "Make Alias",
            MakeAlias.name(),
            Some("cmd-l"),
            MenuCommandState::Selection,
        ),
        command(
            "File",
            "Quick Look",
            QuickLook.name(),
            Some("space"),
            MenuCommandState::Selection,
        ),
        command(
            "File",
            "Move to Trash",
            MoveToTrash.name(),
            Some("cmd-backspace"),
            MenuCommandState::Selection,
        ),
        command(
            "Edit",
            "Undo",
            "system::Undo",
            Some("cmd-z"),
            MenuCommandState::System,
        ),
        command(
            "Edit",
            "Redo",
            "system::Redo",
            Some("shift-cmd-z"),
            MenuCommandState::System,
        ),
        command(
            "Edit",
            "Cut",
            "system::Cut",
            Some("cmd-x"),
            MenuCommandState::System,
        ),
        command(
            "Edit",
            "Copy",
            "system::Copy",
            Some("cmd-c"),
            MenuCommandState::System,
        ),
        command(
            "Edit",
            "Paste",
            "system::Paste",
            Some("cmd-v"),
            MenuCommandState::System,
        ),
        command(
            "Edit",
            "Select All",
            "system::SelectAll",
            Some("cmd-a"),
            MenuCommandState::System,
        ),
        command(
            "Edit",
            "Find",
            Find.name(),
            Some("cmd-f"),
            MenuCommandState::View,
        ),
        command(
            "View",
            "as Icons",
            IconView.name(),
            Some("cmd-1"),
            MenuCommandState::View,
        ),
        command(
            "View",
            "as List",
            ListView.name(),
            Some("cmd-2"),
            MenuCommandState::View,
        ),
        command(
            "View",
            "as Columns",
            ColumnView.name(),
            Some("cmd-3"),
            MenuCommandState::View,
        ),
        command(
            "View",
            "as Gallery",
            GalleryView.name(),
            Some("cmd-4"),
            MenuCommandState::View,
        ),
        command(
            "View",
            "Show View Options",
            ShowViewOptions.name(),
            Some("cmd-j"),
            MenuCommandState::View,
        ),
        command(
            "View",
            "Toggle Sidebar",
            ToggleSidebar.name(),
            None,
            MenuCommandState::View,
        ),
        command(
            "Go",
            "Back",
            Back.name(),
            Some("cmd-left"),
            MenuCommandState::View,
        ),
        command(
            "Go",
            "Forward",
            Forward.name(),
            Some("cmd-right"),
            MenuCommandState::View,
        ),
        command(
            "Go",
            "Enclosing Folder",
            EnclosingFolder.name(),
            Some("cmd-up"),
            MenuCommandState::View,
        ),
        command(
            "Go",
            "Home",
            Home.name(),
            Some("shift-cmd-h"),
            MenuCommandState::Global,
        ),
        command(
            "Go",
            "Desktop",
            Desktop.name(),
            Some("shift-cmd-d"),
            MenuCommandState::Global,
        ),
        command(
            "Go",
            "Documents",
            Documents.name(),
            Some("shift-cmd-o"),
            MenuCommandState::Global,
        ),
        command(
            "Go",
            "Downloads",
            Downloads.name(),
            Some("option-cmd-l"),
            MenuCommandState::Global,
        ),
        command(
            "Go",
            "Applications",
            Applications.name(),
            Some("shift-cmd-a"),
            MenuCommandState::Global,
        ),
        command(
            "Go",
            "Connect to Server",
            ConnectToServer.name(),
            Some("cmd-k"),
            MenuCommandState::Global,
        ),
        command(
            "Window",
            "Minimize",
            Minimize.name(),
            Some("cmd-m"),
            MenuCommandState::Global,
        ),
        command(
            "Window",
            "Zoom",
            Zoom.name(),
            None,
            MenuCommandState::Global,
        ),
        command(
            "Window",
            "Bring All to Front",
            BringAllToFront.name(),
            None,
            MenuCommandState::Global,
        ),
        command(
            "Help",
            "GFM Help",
            Help.name(),
            Some("cmd-?"),
            MenuCommandState::Global,
        ),
    ]
}

fn command(
    menu: &'static str,
    title: &'static str,
    action: &'static str,
    shortcut: Option<&'static str>,
    state: MenuCommandState,
) -> MenuCommandSpec {
    MenuCommandSpec {
        menu,
        title,
        action,
        shortcut,
        state,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_contains_finder_menu_families_and_services() {
        let contract = MenuContract::finder_default();

        assert_eq!(
            contract.menus,
            vec!["GFM", "File", "Edit", "View", "Go", "Window", "Help"]
        );
        assert!(contract.services_menu);
        assert!(contract
            .commands
            .iter()
            .any(|command| command.action == "system::Services"));
    }

    #[test]
    fn gpui_menu_and_keybinding_construction_is_valid() {
        let menus = native_menus();
        let bindings = key_bindings();

        assert_eq!(menus.len(), 7);
        assert!(bindings.len() >= 20);
    }

    #[test]
    fn tsv_output_is_stable_for_binary_and_fozzy() {
        let tsv = MenuContract::finder_default().as_tsv();

        assert!(tsv.starts_with("menus\tGFM,File,Edit,View,Go,Window,Help\tservices=true\n"));
        assert!(tsv.contains("command\tFile\tNew Window\tgfm::NewWindow\tcmd-n\tglobal"));
        assert!(tsv.contains("command\tEdit\tCopy\tsystem::Copy\tcmd-c\tsystem"));
    }
}
