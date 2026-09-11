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
        Self::finder_for_view_mode(path, "icon")
    }

    pub fn finder_for_view_mode(path: impl AsRef<Path>, mode: &str) -> Self {
        Self::finder_for_view_mode_with_search(path, mode, None)
    }

    pub fn finder_for_view_mode_with_search(
        path: impl AsRef<Path>,
        mode: &str,
        search_query: Option<&str>,
    ) -> Self {
        Self::finder_for_view_mode_with_search_and_selection(path, mode, search_query, false)
    }

    pub fn finder_for_view_mode_with_search_and_selection(
        path: impl AsRef<Path>,
        mode: &str,
        search_query: Option<&str>,
        has_selection: bool,
    ) -> Self {
        let title = toolbar_title(path.as_ref());
        let search_label = search_query
            .filter(|query| mode == "search" && !query.is_empty())
            .unwrap_or("Search");
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
                    ControlState::new(34, true, mode == "icon"),
                ),
                control(
                    "view",
                    "list-view",
                    "list",
                    "view-as-list",
                    ToolbarControlKind::SegmentedButton,
                    ControlState::new(34, true, mode == "list"),
                ),
                control(
                    "view",
                    "column-view",
                    "columns",
                    "view-as-columns",
                    ToolbarControlKind::SegmentedButton,
                    ControlState::new(34, true, mode == "column"),
                ),
                control(
                    "view",
                    "gallery-view",
                    "gallery",
                    "view-as-gallery",
                    ToolbarControlKind::SegmentedButton,
                    ControlState::new(34, true, mode == "gallery"),
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
                    ControlState::new(28, has_selection, false),
                ),
                control(
                    "actions",
                    "tags",
                    "tags",
                    "tags",
                    ToolbarControlKind::Button,
                    ControlState::new(28, has_selection, false),
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
                    search_label,
                    "machine-search",
                    ToolbarControlKind::SearchField,
                    ControlState::new(SEARCH_WIDTH as u16, true, mode == "search"),
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

pub fn render(contract: &ToolbarContract) -> impl IntoElement {
    let mut toolbar = div()
        .id("gfm-toolbar")
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .h(px(contract.height_px as f32))
        .pl(px(contract.traffic_light_gutter_px as f32))
        .pr(px(12.0))
        .gap_3()
        .bg(rgb(0x2c2c2c))
        .text_color(rgb(0xd7d7d7));

    for group in grouped_controls(&contract.controls) {
        toolbar = toolbar.child(render_toolbar_group(group));
    }

    toolbar
}

fn grouped_controls(controls: &[ToolbarControlSpec]) -> Vec<&[ToolbarControlSpec]> {
    let mut groups = Vec::new();
    let mut start = 0;
    while start < controls.len() {
        let group = controls[start].group;
        let mut end = start + 1;
        while end < controls.len() && controls[end].group == group {
            end += 1;
        }
        groups.push(&controls[start..end]);
        start = end;
    }
    groups
}

fn render_toolbar_group(controls: &[ToolbarControlSpec]) -> impl IntoElement {
    let mut group = div()
        .id(controls[0].group)
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .flex_shrink_0();

    for control in controls {
        group = group.child(render_control(control));
    }

    group
}

fn render_control(control: &ToolbarControlSpec) -> gpui::Stateful<gpui::Div> {
    match control.kind {
        ToolbarControlKind::Button | ToolbarControlKind::MenuButton => button(control),
        ToolbarControlKind::SegmentedButton => segment(control),
        ToolbarControlKind::PathTitle => path_title(control),
        ToolbarControlKind::SearchField => search_field(control),
    }
}

fn button(control: &ToolbarControlSpec) -> gpui::Stateful<gpui::Div> {
    let text_color = if control.enabled {
        rgb(0xd7d7d7)
    } else {
        rgb(0x777777)
    };
    div()
        .id(control.id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(control.width_px as f32))
        .h(px(BUTTON_SIZE))
        .rounded(px(6.0))
        .text_xs()
        .text_color(text_color)
        .bg(rgb(0x303030))
        .child(control.label.clone())
}

fn segment(control: &ToolbarControlSpec) -> gpui::Stateful<gpui::Div> {
    let background = if control.selected {
        rgb(0x3f3f3f)
    } else {
        rgb(0x303030)
    };
    div()
        .id(control.id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(control.width_px as f32))
        .h(px(28.0))
        .rounded(px(6.0))
        .text_xs()
        .bg(background)
        .child(control.label.clone())
}

fn path_title(control: &ToolbarControlSpec) -> gpui::Stateful<gpui::Div> {
    div()
        .id(control.id)
        .flex()
        .items_center()
        .h(px(32.0))
        .min_w(px(170.0))
        .w(px(control.width_px as f32))
        .flex_1()
        .truncate()
        .text_sm()
        .child(control.label.clone())
}

fn search_field(control: &ToolbarControlSpec) -> gpui::Stateful<gpui::Div> {
    div()
        .id(control.id)
        .flex()
        .items_center()
        .w(px(control.width_px as f32))
        .h(px(30.0))
        .px_2()
        .rounded(px(7.0))
        .bg(rgb(0x242424))
        .text_color(rgb(0x8f8f8f))
        .text_sm()
        .child(control.label.clone())
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
    fn finder_for_view_mode_selects_visible_view_segment() {
        let contract = ToolbarContract::finder_for_view_mode("/tmp/gfm", "list");
        let tsv = contract.as_tsv();

        assert!(tsv.contains(
            "control\tview\ticon-view\tgrid\tview-as-icons\tsegmented-button\t34px\tenabled=true\tselected=false"
        ));
        assert!(tsv.contains(
            "control\tview\tlist-view\tlist\tview-as-list\tsegmented-button\t34px\tenabled=true\tselected=true"
        ));
    }

    #[test]
    fn finder_for_view_mode_renders_active_search_query() {
        let contract =
            ToolbarContract::finder_for_view_mode_with_search("/tmp/gfm", "search", Some("Needle"));
        let tsv = contract.as_tsv();

        assert!(tsv.contains(
            "control\tsearch\tsearch-field\tNeedle\tmachine-search\tsearch-field\t232px\tenabled=true\tselected=true"
        ));
    }

    #[test]
    fn finder_for_selection_enables_share_and_tags_actions() {
        let contract = ToolbarContract::finder_for_view_mode_with_search_and_selection(
            "/tmp/gfm", "list", None, true,
        );
        let tsv = contract.as_tsv();

        assert!(tsv.contains(
            "control\tactions\tshare\tshare\tshare\tbutton\t28px\tenabled=true\tselected=false"
        ));
        assert!(tsv.contains(
            "control\tactions\ttags\ttags\ttags\tbutton\t28px\tenabled=true\tselected=false"
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
