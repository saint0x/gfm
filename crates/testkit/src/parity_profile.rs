use gfm_types::{GfmError, Result};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacOsParityProfile {
    pub macos_build: String,
    pub appearance: ParityAppearance,
    pub scale: DisplayScale,
    pub color_profile: ColorProfile,
    pub dimensions: Vec<DimensionToken>,
    pub materials: Vec<MaterialToken>,
    pub colors: Vec<ColorToken>,
    pub typography: Vec<TypographyToken>,
    pub symbols: Vec<SymbolToken>,
    pub animations: Vec<TimingToken>,
    pub interactions: Vec<TimingToken>,
}

impl MacOsParityProfile {
    pub fn finder_default(
        macos_build: impl Into<String>,
        appearance: ParityAppearance,
        scale: DisplayScale,
        color_profile: ColorProfile,
    ) -> Result<Self> {
        let profile = Self {
            macos_build: macos_build.into(),
            appearance,
            scale,
            color_profile,
            dimensions: finder_dimensions(),
            materials: finder_materials(),
            colors: finder_colors(appearance),
            typography: finder_typography(),
            symbols: finder_symbols(),
            animations: finder_animations(),
            interactions: finder_interactions(),
        };
        profile.validate()?;
        Ok(profile)
    }

    pub fn validate(&self) -> Result<()> {
        if self.macos_build.trim().is_empty() {
            return Err(GfmError::Format(
                "parity profile macOS build must not be empty".to_string(),
            ));
        }
        validate_unique_nonempty(
            "dimension",
            self.dimensions.iter().map(|token| token.name.as_str()),
        )?;
        validate_unique_nonempty("material", self.materials.iter().map(|token| token.name))?;
        validate_unique_nonempty("color", self.colors.iter().map(|token| token.name))?;
        validate_unique_nonempty("typography", self.typography.iter().map(|token| token.name))?;
        validate_unique_nonempty("symbol", self.symbols.iter().map(|token| token.name))?;
        validate_unique_nonempty("animation", self.animations.iter().map(|token| token.name))?;
        validate_unique_nonempty(
            "interaction",
            self.interactions.iter().map(|token| token.name),
        )?;
        if self.dimensions.iter().any(|token| token.px == 0) {
            return Err(GfmError::Format(
                "parity profile dimensions must be positive".to_string(),
            ));
        }
        if self
            .typography
            .iter()
            .any(|token| token.size_px == 0 || token.line_height_px == 0)
        {
            return Err(GfmError::Format(
                "parity profile typography sizes must be positive".to_string(),
            ));
        }
        Ok(())
    }

    pub fn as_tsv(&self) -> String {
        let mut lines = Vec::new();
        lines.push(format!(
            "profile\tbuild={}\tappearance={}\tscale={}\tcolor-profile={}",
            self.macos_build,
            self.appearance.as_str(),
            self.scale.as_str(),
            self.color_profile.as_str()
        ));
        lines.extend(self.dimensions.iter().map(|token| {
            format!(
                "dimension\t{}\t{}px\t{}",
                token.name, token.px, token.source
            )
        }));
        lines.extend(self.materials.iter().map(|token| {
            format!(
                "material\t{}\t{}\t{}",
                token.name, token.value, token.source
            )
        }));
        lines.extend(self.colors.iter().map(|token| {
            format!(
                "color\t{}\t{}\t{}\t{}",
                token.name, token.role, token.value, token.source
            )
        }));
        lines.extend(self.typography.iter().map(|token| {
            format!(
                "typography\t{}\t{}\t{}px\t{}px\t{}\t{}",
                token.name,
                token.family,
                token.size_px,
                token.line_height_px,
                token.weight,
                token.source
            )
        }));
        lines.extend(
            self.symbols
                .iter()
                .map(|token| format!("symbol\t{}\t{}\t{}", token.name, token.symbol, token.source)),
        );
        lines.extend(self.animations.iter().map(|token| {
            format!(
                "animation\t{}\t{}ms\t{}",
                token.name, token.duration_ms, token.source
            )
        }));
        lines.extend(self.interactions.iter().map(|token| {
            format!(
                "interaction\t{}\t{}ms\t{}",
                token.name, token.duration_ms, token.source
            )
        }));
        lines.join("\n")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParityAppearance {
    System,
    Light,
    Dark,
}

impl ParityAppearance {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

impl FromStr for ParityAppearance {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "system" => Ok(Self::System),
            "light" => Ok(Self::Light),
            "dark" => Ok(Self::Dark),
            _ => Err(format!("unknown parity appearance: {value}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayScale {
    One,
    Two,
    Three,
}

impl DisplayScale {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::One => "1x",
            Self::Two => "2x",
            Self::Three => "3x",
        }
    }
}

impl FromStr for DisplayScale {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "1" | "1x" => Ok(Self::One),
            "2" | "2x" => Ok(Self::Two),
            "3" | "3x" => Ok(Self::Three),
            _ => Err(format!("unknown display scale: {value}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorProfile {
    SRgb,
    DisplayP3,
}

impl ColorProfile {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SRgb => "srgb",
            Self::DisplayP3 => "display-p3",
        }
    }
}

impl FromStr for ColorProfile {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "srgb" => Ok(Self::SRgb),
            "display-p3" => Ok(Self::DisplayP3),
            _ => Err(format!("unknown color profile: {value}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DimensionToken {
    pub name: String,
    pub px: u16,
    pub source: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterialToken {
    pub name: &'static str,
    pub value: &'static str,
    pub source: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorToken {
    pub name: &'static str,
    pub role: &'static str,
    pub value: &'static str,
    pub source: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypographyToken {
    pub name: &'static str,
    pub family: &'static str,
    pub size_px: u16,
    pub line_height_px: u16,
    pub weight: &'static str,
    pub source: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolToken {
    pub name: &'static str,
    pub symbol: &'static str,
    pub source: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimingToken {
    pub name: &'static str,
    pub duration_ms: u16,
    pub source: &'static str,
}

fn finder_dimensions() -> Vec<DimensionToken> {
    vec![
        dimension("titlebar.height", 54, "ui/titlebar"),
        dimension("titlebar.traffic-light.x", 20, "ui/titlebar"),
        dimension("titlebar.traffic-light.y", 20, "ui/titlebar"),
        dimension("toolbar.height", 54, "ui/toolbar"),
        dimension("toolbar.traffic-light-gutter", 96, "ui/toolbar"),
        dimension("toolbar.button", 28, "ui/toolbar"),
        dimension("toolbar.search-field.width", 232, "ui/toolbar"),
        dimension("sidebar.width", 188, "ui/sidebar"),
        dimension("sidebar.row-height", 28, "ui/sidebar"),
        dimension("sidebar.section-header-height", 26, "ui/sidebar"),
        dimension("icon-grid.default-icon", 64, "finder-reference-token"),
        dimension("icon-grid.label-line-height", 17, "finder-reference-token"),
        dimension("list.row-height", 20, "finder-reference-token"),
        dimension("column.default-width", 245, "finder-reference-token"),
        dimension("gallery.filmstrip-height", 112, "finder-reference-token"),
    ]
}

fn finder_materials() -> Vec<MaterialToken> {
    vec![
        material(
            "window.titlebar",
            "transparent-system-titlebar",
            "ui/titlebar",
        ),
        material(
            "sidebar.background",
            "system-sidebar-material",
            "finder-parity",
        ),
        material(
            "toolbar.background",
            "system-toolbar-material",
            "finder-parity",
        ),
        material(
            "popover.background",
            "system-popover-material",
            "finder-parity",
        ),
    ]
}

fn finder_colors(appearance: ParityAppearance) -> Vec<ColorToken> {
    match appearance {
        ParityAppearance::Light => vec![
            color("text.primary", "label", "system-label", "system-color"),
            color(
                "text.secondary",
                "secondary-label",
                "system-secondary-label",
                "system-color",
            ),
            color(
                "selection.active",
                "selected-content-background",
                "system-selected-content-background",
                "system-color",
            ),
            color(
                "focus.ring",
                "keyboard-focus-indicator",
                "system-keyboard-focus-indicator",
                "system-color",
            ),
        ],
        ParityAppearance::System | ParityAppearance::Dark => vec![
            color("text.primary", "label", "system-label", "system-color"),
            color(
                "text.secondary",
                "secondary-label",
                "system-secondary-label",
                "system-color",
            ),
            color(
                "selection.active",
                "selected-content-background",
                "system-selected-content-background",
                "system-color",
            ),
            color(
                "focus.ring",
                "keyboard-focus-indicator",
                "system-keyboard-focus-indicator",
                "system-color",
            ),
        ],
    }
}

fn finder_typography() -> Vec<TypographyToken> {
    vec![
        typography("sidebar.row", "system", 13, 17, "regular", "finder-parity"),
        typography(
            "sidebar.section",
            "system",
            11,
            14,
            "semibold",
            "finder-parity",
        ),
        typography(
            "toolbar.title",
            "system",
            13,
            17,
            "semibold",
            "finder-parity",
        ),
        typography("icon.label", "system", 13, 17, "regular", "finder-parity"),
        typography("list.row", "system", 13, 17, "regular", "finder-parity"),
        typography("search.field", "system", 13, 17, "regular", "finder-parity"),
    ]
}

fn finder_symbols() -> Vec<SymbolToken> {
    vec![
        symbol("navigation.back", "chevron.left", "sf-symbol"),
        symbol("navigation.forward", "chevron.right", "sf-symbol"),
        symbol("view.icon", "square.grid.2x2", "sf-symbol"),
        symbol("view.list", "list.bullet", "sf-symbol"),
        symbol("view.column", "rectangle.split.3x1", "sf-symbol"),
        symbol("view.gallery", "rectangle.on.rectangle", "sf-symbol"),
        symbol("toolbar.share", "square.and.arrow.up", "sf-symbol"),
        symbol("toolbar.tags", "tag", "sf-symbol"),
        symbol("toolbar.more", "ellipsis.circle", "sf-symbol"),
        symbol("sidebar.folder", "folder", "sf-symbol"),
        symbol("sidebar.icloud", "icloud", "sf-symbol"),
        symbol("sidebar.volume", "externaldrive", "sf-symbol"),
    ]
}

fn finder_animations() -> Vec<TimingToken> {
    vec![
        timing("selection.fade", 120, "finder-parity"),
        timing("popover.present", 160, "finder-parity"),
        timing("sheet.present", 180, "finder-parity"),
        timing("rename.focus", 80, "finder-parity"),
    ]
}

fn finder_interactions() -> Vec<TimingToken> {
    vec![
        timing("keyboard.repeat-navigation", 33, "finder-parity"),
        timing("hover.settle", 80, "finder-parity"),
        timing("search-keystroke-budget", 16, "performance-budget"),
        timing("selection-to-paint-budget", 16, "performance-budget"),
    ]
}

fn dimension(name: &str, px: u16, source: &'static str) -> DimensionToken {
    DimensionToken {
        name: name.to_string(),
        px,
        source,
    }
}

fn material(name: &'static str, value: &'static str, source: &'static str) -> MaterialToken {
    MaterialToken {
        name,
        value,
        source,
    }
}

fn color(
    name: &'static str,
    role: &'static str,
    value: &'static str,
    source: &'static str,
) -> ColorToken {
    ColorToken {
        name,
        role,
        value,
        source,
    }
}

fn typography(
    name: &'static str,
    family: &'static str,
    size_px: u16,
    line_height_px: u16,
    weight: &'static str,
    source: &'static str,
) -> TypographyToken {
    TypographyToken {
        name,
        family,
        size_px,
        line_height_px,
        weight,
        source,
    }
}

fn symbol(name: &'static str, symbol: &'static str, source: &'static str) -> SymbolToken {
    SymbolToken {
        name,
        symbol,
        source,
    }
}

fn timing(name: &'static str, duration_ms: u16, source: &'static str) -> TimingToken {
    TimingToken {
        name,
        duration_ms,
        source,
    }
}

fn validate_unique_nonempty<'a>(kind: &str, names: impl Iterator<Item = &'a str>) -> Result<()> {
    let mut seen = Vec::new();
    for name in names {
        if name.trim().is_empty() {
            return Err(GfmError::Format(format!(
                "parity profile {kind} token names must not be empty"
            )));
        }
        if seen.contains(&name) {
            return Err(GfmError::Format(format!(
                "parity profile duplicate {kind} token: {name}"
            )));
        }
        seen.push(name);
    }
    if seen.is_empty() {
        return Err(GfmError::Format(format!(
            "parity profile {kind} token set must not be empty"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finder_default_profile_is_stable_for_build_and_capture_inputs() {
        let profile = MacOsParityProfile::finder_default(
            "25A354",
            ParityAppearance::Dark,
            DisplayScale::Two,
            ColorProfile::DisplayP3,
        )
        .unwrap();
        let tsv = profile.as_tsv();

        assert!(tsv.starts_with(
            "profile\tbuild=25A354\tappearance=dark\tscale=2x\tcolor-profile=display-p3"
        ));
        assert!(tsv.contains("dimension\ttoolbar.height\t54px\tui/toolbar"));
        assert!(tsv.contains("material\twindow.titlebar\ttransparent-system-titlebar"));
        assert!(tsv.contains("symbol\tview.column\trectangle.split.3x1"));
        assert!(tsv.contains("interaction\tsearch-keystroke-budget\t16ms"));
    }

    #[test]
    fn profile_validation_rejects_empty_build() {
        let err = MacOsParityProfile::finder_default(
            " ",
            ParityAppearance::Light,
            DisplayScale::Two,
            ColorProfile::SRgb,
        )
        .unwrap_err();

        assert!(matches!(err, GfmError::Format(message) if message.contains("macOS build")));
    }

    #[test]
    fn parsers_accept_cli_values() {
        assert_eq!(
            "dark".parse::<ParityAppearance>().unwrap(),
            ParityAppearance::Dark
        );
        assert_eq!("2x".parse::<DisplayScale>().unwrap(), DisplayScale::Two);
        assert_eq!(
            "display-p3".parse::<ColorProfile>().unwrap(),
            ColorProfile::DisplayP3
        );
    }
}
