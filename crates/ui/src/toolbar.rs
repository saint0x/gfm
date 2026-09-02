use gpui::{div, prelude::*, px, rgb, IntoElement, Styled};
use std::path::Path;

const TOOLBAR_HEIGHT: f32 = 54.0;
const TRAFFIC_LIGHT_GUTTER: f32 = 96.0;
const BUTTON_SIZE: f32 = 28.0;
const SEARCH_WIDTH: f32 = 232.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolbarControlKind {
    Button,
    SegmentedButton,
    MenuButton,
    PathTitle,
    SearchField,
}

impl ToolbarControlKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Button => "button",
            Self::SegmentedButton => "segmented-button",
            Self::MenuButton => "menu-button",
            Self::PathTitle => "path-title",
            Self::SearchField => "search-field",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolbarControlSpec {
    pub group: &'static str,
    pub id: &'static str,
    pub label: String,
    pub role: &'static str,
    pub kind: ToolbarControlKind,
    pub width_px: u16,
    pub enabled: bool,
    pub selected: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ControlState {
    width_px: u16,
    enabled: bool,
    selected: bool,
}

impl ControlState {
    const fn new(width_px: u16, enabled: bool, selected: bool) -> Self {
        Self {
            width_px,
            enabled,
            selected,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolbarContract {
    pub height_px: u16,
    pub traffic_light_gutter_px: u16,
    pub controls: Vec<ToolbarControlSpec>,
}

impl ToolbarContract {
    pub fn finder_default(path: impl AsRef<Path>) -> Self {
        let title = toolbar_title(path.as_ref());
        Self {
            height_px: TOOLBAR_HEIGHT as u16,
            traffic_light_gutter_px: TRAFFIC_LIGHT_GUTTER as u16,
            controls: vec![
                control(
                    "navigation",
                    "back",
                    "<",
                    "go-back",
                    ToolbarControlKind::Button,
                    ControlState::new(28, true, false),
                ),
                control(
                    "navigation",
                    "forward",
                    ">",
                    "go-forward",
                    ToolbarControlKind::Button,
                    ControlState::new(28, false, false),
                ),
                ToolbarControlSpec {
                    group: "location",
                    id: "path-title",
                    label: title,
                    role: "current-folder-title",
                    kind: ToolbarControlKind::PathTitle,
                    width_px: 220,
                    enabled: true,
                    selected: false,
                },
                control(
                    "view",
                    "icon-view",
                    "grid",
                    "view-as-icons",
                    ToolbarControlKind::SegmentedButton,
                    ControlState::new(34, true, true),
                ),
                control(
                    "view",
                    "list-view",
                    "list",
                    "view-as-list",
                    ToolbarControlKind::SegmentedButton,
                    ControlState::new(34, true, false),
                ),
                control(
                    "view",
                    "column-view",
                    "columns",
                    "view-as-columns",
                    ToolbarControlKind::SegmentedButton,
                    ControlState::new(34, true, false),
                ),
                control(
                    "view",
                    "gallery-view",
                    "gallery",
                    "view-as-gallery",
                    ToolbarControlKind::SegmentedButton,
                    ControlState::new(34, true, false),
                ),
                control(
                    "arrange",
                    "arrange",
                    "arrange",
                    "arrange-or-sort",
                    ToolbarControlKind::MenuButton,
                    ControlState::new(36, true, false),
                ),
                control(
                    "actions",
                    "share",
                    "share",
                    "share",
                    ToolbarControlKind::Button,
                    ControlState::new(28, false, false),
                ),
                control(
                    "actions",
                    "tags",
                    "tags",
                    "tags",
                    ToolbarControlKind::Button,
                    ControlState::new(28, false, false),
                ),
                control(
                    "actions",
                    "more",
                    "more",
                    "more-actions",
                    ToolbarControlKind::MenuButton,
                    ControlState::new(32, true, false),
                ),
                control(
                    "search",
                    "search-field",
                    "Search",
                    "machine-search",
                    ToolbarControlKind::SearchField,
                    ControlState::new(SEARCH_WIDTH as u16, true, false),
                ),
            ],
        }
    }

    pub fn as_tsv(&self) -> String {
        let mut lines = Vec::with_capacity(self.controls.len() + 1);
        lines.push(format!(
            "toolbar\theight={}\ttraffic-light-gutter={}",
            self.height_px, self.traffic_light_gutter_px
        ));
        lines.extend(self.controls.iter().map(|control| {
            format!(
                "control\t{}\t{}\t{}\t{}\t{}\t{}px\tenabled={}\tselected={}",
                control.group,
                control.id,
                escape_field(&control.label),
                control.role,
                control.kind.as_str(),
                control.width_px,
                control.enabled,
                control.selected
            )
        }));
        lines.join("\n")
    }
}

pub fn render(path: &Path) -> impl IntoElement {
    let title = toolbar_title(path);

    div()
        .id("gfm-toolbar")
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .h(px(TOOLBAR_HEIGHT))
        .pl(px(TRAFFIC_LIGHT_GUTTER))
        .pr(px(12.0))
        .gap_3()
        .bg(rgb(0x2c2c2c))
        .text_color(rgb(0xd7d7d7))
        .child(
            toolbar_group("navigation")
                .child(button("<", true))
                .child(button(">", false)),
        )
        .child(
            div()
                .id("path-title")
                .flex()
                .items_center()
                .h(px(32.0))
                .min_w(px(170.0))
                .flex_1()
                .truncate()
                .text_sm()
                .child(title),
        )
        .child(
            toolbar_group("view")
                .child(segment("grid", true))
                .child(segment("list", false))
                .child(segment("columns", false))
                .child(segment("gallery", false)),
        )
        .child(toolbar_group("arrange").child(button("arrange", true)))
        .child(
            toolbar_group("actions")
                .child(button("share", false))
                .child(button("tags", false))
                .child(button("more", true)),
        )
        .child(search_field())
}

fn toolbar_group(id: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .flex_shrink_0()
}

fn button(label: &'static str, enabled: bool) -> gpui::Div {
    let text_color = if enabled {
        rgb(0xd7d7d7)
    } else {
        rgb(0x777777)
    };
    div()
        .flex()
        .items_center()
        .justify_center()
        .size(px(BUTTON_SIZE))
        .rounded(px(6.0))
        .text_xs()
        .text_color(text_color)
        .bg(rgb(0x303030))
        .child(label)
}

fn segment(label: &'static str, selected: bool) -> gpui::Div {
    let background = if selected {
        rgb(0x3f3f3f)
    } else {
        rgb(0x303030)
    };
    div()
        .flex()
        .items_center()
        .justify_center()
        .w(px(34.0))
        .h(px(28.0))
        .rounded(px(6.0))
        .text_xs()
        .bg(background)
        .child(label)
}

fn search_field() -> gpui::Stateful<gpui::Div> {
    div()
        .id("search-field")
        .flex()
        .items_center()
        .w(px(SEARCH_WIDTH))
        .h(px(30.0))
        .px_2()
        .rounded(px(7.0))
        .bg(rgb(0x242424))
        .text_color(rgb(0x8f8f8f))
        .text_sm()
        .child("Search")
}

fn toolbar_title(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.display().to_string())
}

fn escape_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn control(
    group: &'static str,
    id: &'static str,
    label: impl Into<String>,
    role: &'static str,
    kind: ToolbarControlKind,
    state: ControlState,
) -> ToolbarControlSpec {
    ToolbarControlSpec {
        group,
        id,
        label: label.into(),
        role,
        kind,
        width_px: state.width_px,
        enabled: state.enabled,
        selected: state.selected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finder_default_contract_contains_required_toolbar_surface() {
        let contract = ToolbarContract::finder_default("/Users/deepsaint/Desktop");
        let ids: Vec<_> = contract.controls.iter().map(|control| control.id).collect();

        assert_eq!(contract.height_px, 54);
        assert_eq!(contract.traffic_light_gutter_px, 96);
        assert_eq!(
            ids,
            vec![
                "back",
                "forward",
                "path-title",
                "icon-view",
                "list-view",
                "column-view",
                "gallery-view",
                "arrange",
                "share",
                "tags",
                "more",
                "search-field"
            ]
        );
    }

    #[test]
    fn contract_output_is_stable_for_cli_and_fozzy() {
        let contract = ToolbarContract::finder_default("/tmp/gfm");

        assert!(contract
            .as_tsv()
            .starts_with("toolbar\theight=54\ttraffic-light-gutter=96"));
        assert!(contract.as_tsv().contains(
            "control\tlocation\tpath-title\tgfm\tcurrent-folder-title\tpath-title\t220px\tenabled=true\tselected=false"
        ));
        assert!(contract.as_tsv().contains(
            "control\tsearch\tsearch-field\tSearch\tmachine-search\tsearch-field\t232px\tenabled=true\tselected=false"
        ));
    }

    #[test]
    fn toolbar_tsv_escapes_control_characters_in_path_title() {
        let contract = ToolbarContract::finder_default("/tmp/Reports\tQ3\nDraft\rToolbar");
        let tsv = contract.as_tsv();
        let title = tsv
            .lines()
            .find(|line| line.starts_with("control\tlocation\tpath-title\t"))
            .unwrap();

        assert_eq!(tsv.lines().count(), 13, "{tsv}");
        assert!(
            title.contains("\tReports\\tQ3\\nDraft\\rToolbar\tcurrent-folder-title\t"),
            "{tsv}"
        );
        assert_eq!(title.split('\t').count(), 9, "{tsv}");
    }
}
