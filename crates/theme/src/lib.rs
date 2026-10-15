//! Themes: a folder with `theme.json` (colours, shape and effect values) and an
//! optional `style.css`, compiled into a GTK stylesheet.
//!
//! The generated CSS sets libadwaita's CSS variables (`--accent-bg-color`,
//! `--window-bg-color`, ...) plus the app's own `--mp-*` variables.
//!
//! libadwaita only switches the colour scheme of its own stylesheet, so the
//! `gtk::CssProvider` holding a theme has to follow `adw::StyleManager:dark` itself.
//! There are two ways:
//!
//! - [`Theme::to_css_for_scheme`] works with every GTK 4 version: it holds only the
//!   current scheme's palette, without media queries. Reload the provider with it
//!   whenever `StyleManager:dark` changes. `examples/check_css.rs` does exactly that,
//!   and checks every built-in theme in both schemes.
//! - [`Theme::to_css`] holds both palettes under `@media (prefers-color-scheme: ...)`.
//!   It needs GTK 4.20, whose `CssProvider::set_prefers_color_scheme` must then be kept
//!   in step with `StyleManager:dark`.
//!
//! Theme values are user input, so every value is validated before it reaches the
//! stylesheet: a broken theme can look odd, but it can never break the app.

mod appearance;
mod builtin;
mod css;
mod css_scan;
mod loader;
mod media;
mod pywal;
mod sanitize;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use appearance::Appearance;
pub use builtin::builtin_themes;
pub use loader::{MANIFEST_FILE, STYLESHEET_FILE, discover, load_theme};
pub use pywal::{PYWAL_COLORS_FILE, PYWAL_THEME_ID, load_pywal_theme, pywal_dir, pywal_manifest};

/// Id of the default, Adwaita-native theme.
pub const DEFAULT_THEME_ID: &str = mp_core::settings::DEFAULT_THEME_ID;

/// Which colour schemes a theme supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeScheme {
    /// Works in light and dark; follows the user's preference.
    #[default]
    Any,
    /// Forces the light scheme.
    Light,
    /// Forces the dark scheme.
    Dark,
}

/// Colours as CSS colour strings (`#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`,
/// `rgb()`/`rgba()`, `hsl()`/`hsla()`); anything else is ignored with a warning.
/// Unset colours keep libadwaita's (or the app's) defaults. See `docs/themes.md` for
/// the CSS variables each key sets.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Palette {
    pub accent: Option<String>,
    pub accent_fg: Option<String>,
    pub destructive: Option<String>,
    pub success: Option<String>,
    pub warning: Option<String>,
    pub window_bg: Option<String>,
    pub window_fg: Option<String>,
    pub view_bg: Option<String>,
    pub view_fg: Option<String>,
    pub headerbar_bg: Option<String>,
    pub headerbar_fg: Option<String>,
    pub sidebar_bg: Option<String>,
    pub sidebar_fg: Option<String>,
    pub card_bg: Option<String>,
    pub card_fg: Option<String>,
    pub popover_bg: Option<String>,
    pub popover_fg: Option<String>,
    pub dialog_bg: Option<String>,
    pub dialog_fg: Option<String>,
    pub player_bar_bg: Option<String>,
    pub player_bar_fg: Option<String>,
    pub border: Option<String>,
    pub hires_badge: Option<String>,
}

/// Corner radii and spacing in pixels. Unset values keep the app defaults.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shape {
    pub window_radius: Option<u32>,
    pub panel_radius: Option<u32>,
    pub cover_radius: Option<u32>,
    pub button_radius: Option<u32>,
    pub spacing: Option<u32>,
}

/// Visual effects. Opacities are `0.5..=1.0` and only apply while the user has
/// transparency turned on (see [`Appearance`]); without them the defaults are used.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Effects {
    pub window_opacity: Option<f32>,
    pub panel_opacity: Option<f32>,
    /// Strength of the shadow under album covers, `0.0` disables it.
    pub cover_shadow: Option<f32>,
}

/// Contents of `theme.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeManifest {
    pub name: String,
    pub author: String,
    pub description: String,
    pub version: String,
    pub scheme: ThemeScheme,
    /// Colours shared by both schemes.
    pub colors: Palette,
    /// Overrides applied in the light scheme.
    pub light: Palette,
    /// Overrides applied in the dark scheme.
    pub dark: Palette,
    pub shape: Shape,
    pub effects: Effects,
    pub font_family: Option<String>,
    /// Extra stylesheet relative to the theme folder, appended after the generated CSS.
    pub stylesheet: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeSource {
    Builtin,
    User(PathBuf),
    /// Made from pywal's palette in this cache folder.
    Pywal(PathBuf),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    /// Built-in id or the user theme's folder name.
    pub id: String,
    pub manifest: ThemeManifest,
    pub source: ThemeSource,
    /// Contents of the theme's extra stylesheet, if any.
    pub extra_css: Option<String>,
    /// Why the theme's stylesheet was left out, for telling its author.
    pub stylesheet_problem: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    #[error("could not read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid theme.json in {path}: {message}")]
    Invalid { path: PathBuf, message: String },
}

impl Theme {
    /// The complete stylesheet for this theme.
    ///
    /// Invalid values are left out (with a logged warning), so the result is always
    /// a stylesheet GTK can load.
    pub fn to_css(&self) -> String {
        css::generate(self, css::Target::BothSchemes)
    }

    /// The stylesheet for one colour scheme: the shared values, then only the dark
    /// (`dark == true`) or only the light palette, with no `@media` blocks, then the
    /// theme's own stylesheet with its `prefers-color-scheme` blocks resolved the same
    /// way. Works with every GTK 4 version; reload it when the scheme changes.
    pub fn to_css_for_scheme(&self, dark: bool) -> String {
        let target = if dark {
            css::Target::Dark
        } else {
            css::Target::Light
        };
        css::generate(self, target)
    }

    pub fn scheme(&self) -> ThemeScheme {
        self.manifest.scheme
    }
}
