//! Compiles a [`Theme`] into a GTK stylesheet.

use std::borrow::Cow;
use std::fmt::Write;

use crate::media::resolve_color_scheme;
use crate::sanitize::{self, MAX_RADIUS, MAX_SPACING, MAX_WINDOW_RADIUS};
use crate::{Effects, Palette, Shape, Theme};

type Declarations = Vec<(&'static str, String)>;
type PaletteField = fn(&Palette) -> &Option<String>;

/// The CSS variables each palette key sets. Backdrop and secondary variants follow
/// their main colour so unfocused windows and secondary sidebars match the theme.
const PALETTE_VARIABLES: [(&str, PaletteField, &[&str]); 23] = [
    ("accent", |p| &p.accent, &["--accent-bg-color"]),
    ("accent_fg", |p| &p.accent_fg, &["--accent-fg-color"]),
    (
        "destructive",
        |p| &p.destructive,
        &["--destructive-bg-color", "--error-bg-color"],
    ),
    ("success", |p| &p.success, &["--success-bg-color"]),
    ("warning", |p| &p.warning, &["--warning-bg-color"]),
    (
        "window_bg",
        |p| &p.window_bg,
        &["--window-bg-color", "--headerbar-backdrop-color"],
    ),
    ("window_fg", |p| &p.window_fg, &["--window-fg-color"]),
    ("view_bg", |p| &p.view_bg, &["--view-bg-color"]),
    ("view_fg", |p| &p.view_fg, &["--view-fg-color"]),
    (
        "headerbar_bg",
        |p| &p.headerbar_bg,
        &["--headerbar-bg-color"],
    ),
    (
        "headerbar_fg",
        |p| &p.headerbar_fg,
        &["--headerbar-fg-color"],
    ),
    (
        "sidebar_bg",
        |p| &p.sidebar_bg,
        &[
            "--sidebar-bg-color",
            "--sidebar-backdrop-color",
            "--secondary-sidebar-bg-color",
            "--secondary-sidebar-backdrop-color",
        ],
    ),
    (
        "sidebar_fg",
        |p| &p.sidebar_fg,
        &["--sidebar-fg-color", "--secondary-sidebar-fg-color"],
    ),
    ("card_bg", |p| &p.card_bg, &["--card-bg-color"]),
    ("card_fg", |p| &p.card_fg, &["--card-fg-color"]),
    ("popover_bg", |p| &p.popover_bg, &["--popover-bg-color"]),
    ("popover_fg", |p| &p.popover_fg, &["--popover-fg-color"]),
    ("dialog_bg", |p| &p.dialog_bg, &["--dialog-bg-color"]),
    ("dialog_fg", |p| &p.dialog_fg, &["--dialog-fg-color"]),
    (
        "player_bar_bg",
        |p| &p.player_bar_bg,
        &["--mp-player-bar-bg-color"],
    ),
    (
        "player_bar_fg",
        |p| &p.player_bar_fg,
        &["--mp-player-bar-fg-color"],
    ),
    ("border", |p| &p.border, &["--border-color"]),
    (
        "hires_badge",
        |p| &p.hires_badge,
        &["--mp-hires-badge-color"],
    ),
];

/// libadwaita zeroes the window radius for these window states; the theme's own
/// `:root` rule outranks that, so the reset has to be repeated.
pub(crate) const SQUARE_WINDOW_SELECTORS: [&str; 8] = [
    "window.csd.maximized",
    "window.csd.fullscreen",
    "window.csd.tiled",
    "window.csd.tiled-top",
    "window.csd.tiled-left",
    "window.csd.tiled-right",
    "window.csd.tiled-bottom",
    "window.solid-csd",
];

/// Which colour schemes a stylesheet is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    /// Both, each palette under its `@media (prefers-color-scheme: ...)` query.
    BothSchemes,
    Light,
    Dark,
}

/// The stylesheet, as blocks separated by blank lines: header comment, shared
/// `:root` values, the light and/or dark palette, fixes and the theme's own stylesheet.
pub(crate) fn generate(theme: &Theme, target: Target) -> String {
    let manifest = &theme.manifest;
    let mut blocks = vec![header(theme)];
    blocks.extend(rule(":root", &shared_declarations(theme)));

    let palette_rule = |palette| rule(":root", &palette_declarations(&theme.id, palette));
    match target {
        Target::BothSchemes => {
            for (scheme, palette) in [("light", &manifest.light), ("dark", &manifest.dark)] {
                if let Some(root) = palette_rule(palette) {
                    blocks.push(format!(
                        "@media (prefers-color-scheme: {scheme}) {{\n{}}}\n",
                        indent(&root)
                    ));
                }
            }
        }
        Target::Light => blocks.extend(palette_rule(&manifest.light)),
        Target::Dark => blocks.extend(palette_rule(&manifest.dark)),
    }

    if manifest.shape.window_radius.is_some() {
        let reset = [("--window-radius", "0px".to_owned())];
        blocks.extend(rule(&SQUARE_WINDOW_SELECTORS.join(",\n"), &reset));
    }

    if let Some(font) = manifest.font_family.as_deref() {
        match sanitize::font_family(font) {
            Some(family) => blocks.extend(rule("window", &[("font-family", family)])),
            None => log::warn!("Theme '{}': ignoring empty font_family", theme.id),
        }
    }

    if let Some(extra) = theme.extra_css.as_deref() {
        let extra = match target {
            Target::BothSchemes => Cow::Borrowed(extra),
            Target::Light => Cow::Owned(resolve_color_scheme(extra, false)),
            Target::Dark => Cow::Owned(resolve_color_scheme(extra, true)),
        };
        let newline = if extra.ends_with('\n') { "" } else { "\n" };
        blocks.push(format!("/* Theme stylesheet */\n{extra}{newline}"));
    }
    blocks.join("\n")
}

fn header(theme: &Theme) -> String {
    let manifest = &theme.manifest;
    let mut title = format!("{} ({})", manifest.name, theme.id);
    if !manifest.author.is_empty() {
        let _ = write!(title, " by {}", manifest.author);
    }
    format!(
        "/* Theme: {} — generated by {} */\n",
        sanitize::comment_text(&title),
        mp_core::APP_NAME
    )
}

/// Values for both schemes: shared colours, shape and effects.
fn shared_declarations(theme: &Theme) -> Declarations {
    let manifest = &theme.manifest;
    let mut declarations = palette_declarations(&theme.id, &manifest.colors);
    declarations.extend(shape_declarations(&manifest.shape));
    declarations.extend(effect_declarations(&theme.id, &manifest.effects));
    declarations
}

/// A CSS rule, or `None` when there is nothing to declare.
fn rule(selector: &str, declarations: &[(&'static str, String)]) -> Option<String> {
    if declarations.is_empty() {
        return None;
    }
    let mut css = format!("{selector} {{\n");
    for (property, value) in declarations {
        let _ = writeln!(css, "  {property}: {value};");
    }
    css.push_str("}\n");
    Some(css)
}

fn indent(css: &str) -> String {
    css.lines().map(|line| format!("  {line}\n")).collect()
}

fn palette_declarations(theme_id: &str, palette: &Palette) -> Declarations {
    let mut declarations = Vec::new();
    for (key, field, variables) in PALETTE_VARIABLES {
        let Some(raw) = field(palette).as_deref() else {
            continue;
        };
        let Some(color) = sanitize::color(raw) else {
            log::warn!("Theme '{theme_id}': ignoring invalid colour {raw:?} for '{key}'");
            continue;
        };
        for &variable in variables {
            declarations.push((variable, color.to_owned()));
        }
    }
    declarations
}

fn shape_declarations(shape: &Shape) -> Declarations {
    let values = [
        ("--window-radius", shape.window_radius, MAX_WINDOW_RADIUS),
        ("--mp-panel-radius", shape.panel_radius, MAX_RADIUS),
        ("--mp-cover-radius", shape.cover_radius, MAX_RADIUS),
        ("--mp-button-radius", shape.button_radius, MAX_RADIUS),
        ("--mp-spacing", shape.spacing, MAX_SPACING),
    ];
    values
        .into_iter()
        .filter_map(|(variable, value, max)| Some((variable, format!("{}px", value?.min(max)))))
        .collect()
}

/// Effects the theme controls on its own. Its opacities only apply when the user turns
/// transparency on (see [`crate::appearance`]).
fn effect_declarations(theme_id: &str, effects: &Effects) -> Declarations {
    let mut declarations = Vec::new();
    if let Some(strength) = effects.cover_shadow {
        match sanitize::opacity(strength, 0.0) {
            Some(strength) => declarations.push(("--mp-cover-shadow", cover_shadow(strength))),
            None => log::warn!("Theme '{theme_id}': ignoring invalid cover_shadow"),
        }
    }
    declarations
}

/// A two-layer shadow: a soft wide one for depth and a tight one for definition.
fn cover_shadow(strength: f32) -> String {
    if strength <= 0.0 {
        return "none".to_owned();
    }
    let wide = 0.32 * strength;
    let tight = 0.18 * strength;
    format!("0 6px 18px rgba(0, 0, 0, {wide:.3}), 0 1px 3px rgba(0, 0, 0, {tight:.3})")
}

pub(crate) fn percent(fraction: f32) -> String {
    let value = (fraction * 1000.0).round() / 10.0;
    format!("{value}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ThemeManifest, ThemeSource};

    fn theme(manifest: ThemeManifest) -> Theme {
        Theme {
            id: "test".into(),
            manifest,
            source: ThemeSource::Builtin,
            extra_css: None,
            stylesheet_problem: None,
        }
    }

    fn color(value: &str) -> Option<String> {
        Some(value.to_owned())
    }

    #[test]
    fn emits_shared_values_then_scheme_blocks() {
        let mut manifest = ThemeManifest {
            name: "Test".into(),
            ..ThemeManifest::default()
        };
        manifest.colors.accent = color("#8f7fd0");
        manifest.light.window_bg = color("#fafafa");
        manifest.dark.window_bg = color("#101014");
        manifest.dark.player_bar_bg = color("#202024");
        manifest.shape.panel_radius = Some(16);
        manifest.effects.panel_opacity = Some(0.8);

        let css = theme(manifest).to_css();
        let root = css.find(":root {").unwrap();
        let light = css.find("@media (prefers-color-scheme: light)").unwrap();
        let dark = css.find("@media (prefers-color-scheme: dark)").unwrap();
        assert!(css.starts_with("/* Theme: Test (test)"));
        assert!(root < light && light < dark);
        assert!(css[root..light].contains("--accent-bg-color: #8f7fd0;"));
        assert!(css[root..light].contains("--mp-panel-radius: 16px;"));
        assert!(css[light..dark].contains("--window-bg-color: #fafafa;"));
        assert!(css[light..dark].contains("--headerbar-backdrop-color: #fafafa;"));
        assert!(css[dark..].contains("--window-bg-color: #101014;"));
        assert!(css[dark..].contains("--mp-player-bar-bg-color: #202024;"));
        // Opacities are hints for the user's transparency option, not theme output.
        assert!(!css.contains("transparent"));
    }

    #[test]
    fn single_scheme_stylesheets_hold_one_palette_and_no_media_queries() {
        let mut manifest = ThemeManifest::default();
        manifest.colors.accent = color("#8f7fd0");
        manifest.colors.window_bg = color("#777777");
        manifest.light.window_bg = color("#fafafa");
        manifest.dark.window_bg = color("#101014");
        manifest.shape.panel_radius = Some(16);
        let mut theme = theme(manifest);
        theme.extra_css = Some(
            "@media (prefers-color-scheme: dark) { .only-dark { color: red; } }\n.both {}\n".into(),
        );

        let light = theme.to_css_for_scheme(false);
        let dark = theme.to_css_for_scheme(true);
        for css in [&light, &dark] {
            assert!(!css.contains("@media"), "{css}");
            assert!(css.contains("--accent-bg-color: #8f7fd0;"));
            assert!(css.contains("--mp-panel-radius: 16px;"));
            assert!(css.ends_with(".both {}\n"), "{css}");
            // The scheme's own colour comes after the shared one, so it wins.
            assert!(css.find("#777777").unwrap() < css.rfind("--window-bg-color").unwrap());
        }
        assert!(light.contains("--window-bg-color: #fafafa;") && !light.contains("#101014"));
        assert!(dark.contains("--window-bg-color: #101014;") && !dark.contains("#fafafa"));
        assert!(dark.contains(".only-dark") && !light.contains(".only-dark"));
    }

    #[test]
    fn empty_palettes_emit_no_media_blocks() {
        let css = theme(ThemeManifest::default()).to_css();
        assert!(!css.contains("@media"));
        assert!(!css.contains(":root"));
    }

    #[test]
    fn invalid_colors_are_dropped() {
        let mut manifest = ThemeManifest::default();
        manifest.colors.accent = color("red; } window { background: url(x)");
        manifest.colors.window_bg = color("#123456");
        let css = theme(manifest).to_css();
        assert!(!css.contains("url("));
        assert!(!css.contains("--accent-bg-color"));
        assert!(css.contains("--window-bg-color: #123456;"));
    }

    #[test]
    fn numbers_are_clamped() {
        let mut manifest = ThemeManifest::default();
        manifest.shape.window_radius = Some(5000);
        manifest.shape.spacing = Some(500);
        manifest.effects.cover_shadow = Some(0.0);
        let css = theme(manifest).to_css();
        assert!(css.contains(&format!("--window-radius: {MAX_WINDOW_RADIUS}px;")));
        assert!(css.contains(&format!("--mp-spacing: {MAX_SPACING}px;")));
        assert!(css.contains("--mp-cover-shadow: none;"));
        assert!(css.contains("window.csd.maximized"));
    }

    #[test]
    fn font_and_extra_stylesheet_come_last() {
        let manifest = ThemeManifest {
            font_family: Some("Inter, sans-serif".into()),
            ..ThemeManifest::default()
        };
        let mut theme = theme(manifest);
        theme.extra_css = Some(".mp-player-bar { padding: 4px; }".into());
        let css = theme.to_css();
        let font = css.find("font-family: \"Inter\", sans-serif;").unwrap();
        let extra = css.find(".mp-player-bar { padding: 4px; }").unwrap();
        assert!(font < extra);
        assert!(css.ends_with('\n'));
    }

    #[test]
    fn header_cannot_break_out_of_its_comment() {
        let manifest = ThemeManifest {
            name: "x */ window { color: red; } /*".into(),
            ..ThemeManifest::default()
        };
        let css = theme(manifest).to_css();
        let header_end = css.find("*/").unwrap();
        assert!(css[header_end + 2..].trim().is_empty());
    }
}
