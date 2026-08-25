use crate::AppLaunchSpec;
use gfm_types::{GfmError, Result};
use gpui::{px, SharedString, TitlebarOptions};

const TRAFFIC_LIGHT_X: f32 = 20.0;
const TRAFFIC_LIGHT_Y: f32 = 20.0;
const DEFAULT_TITLEBAR_HEIGHT: f32 = 54.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitlebarMaterialPolicy {
    TransparentSystemTitlebar,
    OpaqueSystemTitlebar,
}

impl TitlebarMaterialPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TransparentSystemTitlebar => "transparent-system-titlebar",
            Self::OpaqueSystemTitlebar => "opaque-system-titlebar",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitlebarFocusPolicy {
    SystemActiveInactive,
}

impl TitlebarFocusPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SystemActiveInactive => "system-active-inactive",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullScreenPolicy {
    NativeMacosZoomAndFullScreen,
}

impl FullScreenPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NativeMacosZoomAndFullScreen => "native-macos-zoom-and-full-screen",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TitlebarContract {
    pub title: String,
    pub height_px: u16,
    pub traffic_light_x_px: u16,
    pub traffic_light_y_px: u16,
    pub material: TitlebarMaterialPolicy,
    pub focus_policy: TitlebarFocusPolicy,
    pub full_screen_policy: FullScreenPolicy,
    pub tabbing_identifier: String,
}

impl TitlebarContract {
    pub fn from_spec(spec: &AppLaunchSpec) -> Result<Self> {
        let contract = Self {
            title: spec.title.clone(),
            height_px: DEFAULT_TITLEBAR_HEIGHT as u16,
            traffic_light_x_px: TRAFFIC_LIGHT_X as u16,
            traffic_light_y_px: TRAFFIC_LIGHT_Y as u16,
            material: if spec.transparent_titlebar {
                TitlebarMaterialPolicy::TransparentSystemTitlebar
            } else {
                TitlebarMaterialPolicy::OpaqueSystemTitlebar
            },
            focus_policy: TitlebarFocusPolicy::SystemActiveInactive,
            full_screen_policy: FullScreenPolicy::NativeMacosZoomAndFullScreen,
            tabbing_identifier: spec.tabbing_identifier.clone(),
        };
        contract.validate()?;
        Ok(contract)
    }

    pub fn validate(&self) -> Result<()> {
        if self.title.trim().is_empty() {
            return Err(GfmError::Format(
                "native titlebar title must not be empty".to_string(),
            ));
        }
        if self.height_px < 44 {
            return Err(GfmError::Format(
                "native titlebar height is below Finder chrome minimum".to_string(),
            ));
        }
        if self.traffic_light_x_px < 12 || self.traffic_light_y_px < 12 {
            return Err(GfmError::Format(
                "native titlebar traffic-light anchor is too close to the window edge".to_string(),
            ));
        }
        if self.tabbing_identifier.trim().is_empty() {
            return Err(GfmError::Format(
                "native titlebar tabbing identifier must not be empty".to_string(),
            ));
        }
        Ok(())
    }

    pub fn into_options(self) -> TitlebarOptions {
        TitlebarOptions {
            title: Some(SharedString::from(self.title)),
            appears_transparent: matches!(
                self.material,
                TitlebarMaterialPolicy::TransparentSystemTitlebar
            ),
            traffic_light_position: Some(gpui::point(
                px(f32::from(self.traffic_light_x_px)),
                px(f32::from(self.traffic_light_y_px)),
            )),
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "titlebar\t{}\theight={}\ttraffic-light={}x{}\tmaterial={}\tfocus={}\tfull-screen={}\ttabs={}",
            self.title,
            self.height_px,
            self.traffic_light_x_px,
            self.traffic_light_y_px,
            self.material.as_str(),
            self.focus_policy.as_str(),
            self.full_screen_policy.as_str(),
            self.tabbing_identifier
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppLaunchSpec;

    #[test]
    fn default_contract_matches_native_chrome_policy() {
        let spec = AppLaunchSpec::new("/tmp/gfm");
        let contract = TitlebarContract::from_spec(&spec).unwrap();

        assert_eq!(contract.title, "GFM");
        assert_eq!(contract.height_px, 54);
        assert_eq!(contract.traffic_light_x_px, 20);
        assert_eq!(contract.traffic_light_y_px, 20);
        assert_eq!(
            contract.material,
            TitlebarMaterialPolicy::TransparentSystemTitlebar
        );
        assert_eq!(
            contract.full_screen_policy,
            FullScreenPolicy::NativeMacosZoomAndFullScreen
        );
    }

    #[test]
    fn contract_output_is_stable_for_cli_and_fozzy() {
        let spec = AppLaunchSpec::new("/tmp/gfm");
        let contract = TitlebarContract::from_spec(&spec).unwrap();

        assert_eq!(
            contract.as_tsv(),
            "titlebar\tGFM\theight=54\ttraffic-light=20x20\tmaterial=transparent-system-titlebar\tfocus=system-active-inactive\tfull-screen=native-macos-zoom-and-full-screen\ttabs=gfm-main-window"
        );
    }

    #[test]
    fn rejects_empty_titlebar_title() {
        let spec = AppLaunchSpec {
            title: " ".to_string(),
            ..Default::default()
        };

        assert!(TitlebarContract::from_spec(&spec).is_err());
    }
}
