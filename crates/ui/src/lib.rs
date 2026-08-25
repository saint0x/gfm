use gfm_types::{GfmError, Result};
use gpui::{
    div, px, rgb, size, App, AppContext, Application, Bounds, Context, IntoElement, Render,
    SharedString, Styled, TitlebarOptions, Window, WindowBounds, WindowOptions,
};
use std::path::PathBuf;

mod menu;

pub use menu::{MenuCommandSpec, MenuCommandState, MenuContract};

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
        install_native_menu(cx, spec.clone());
        if let Err(err) = open_main_window(cx, spec) {
            eprintln!("gfm-ui: {err}");
            cx.quit();
        }
    });
    Ok(())
}

fn open_main_window(cx: &mut App, spec: AppLaunchSpec) -> anyhow::Result<()> {
    let options = window_options(cx, &spec);
    let activate = spec.activate_on_launch;
    cx.open_window(options, |_, cx| {
        cx.new(|_| RootView {
            initial_path: spec.initial_path,
        })
    })?;
    if activate {
        cx.activate(true);
    }
    Ok(())
}

fn install_native_menu(cx: &mut App, spec: AppLaunchSpec) {
    cx.bind_keys(menu::key_bindings());
    cx.on_action({
        let spec = spec.clone();
        move |_: &menu::NewWindow, cx| {
            if let Err(err) = open_main_window(cx, spec.clone()) {
                eprintln!("gfm-ui: {err}");
            }
        }
    });
    cx.on_action(|_: &menu::CloseWindow, cx| {
        if let Some(active_window) = cx.active_window() {
            let _ = active_window.update(cx, |_, window, _| window.remove_window());
        }
    });
    cx.on_action(|_: &menu::Quit, cx| cx.quit());
    cx.set_menus(menu::native_menus());
}

fn window_options(cx: &App, spec: &AppLaunchSpec) -> WindowOptions {
    let bounds = Bounds::centered(None, size(px(spec.width), px(spec.height)), cx);
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(SharedString::from(spec.title.clone())),
            appears_transparent: spec.transparent_titlebar,
            traffic_light_position: Some(gpui::point(px(20.0), px(20.0))),
        }),
        window_min_size: Some(size(px(spec.min_width), px(spec.min_height))),
        tabbing_identifier: Some(spec.tabbing_identifier.clone()),
        ..Default::default()
    }
}

struct RootView {
    initial_path: PathBuf,
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let _ = &self.initial_path;
        div().size_full().bg(rgb(0x1e1e1e))
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
