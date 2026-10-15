//! The user's appearance options, layered above whichever theme is active: how
//! rounded the interface is, whether the window is translucent, and how defined the
//! edges between the bars and the page are.

use std::fmt::Write;

use mp_core::CornerStyle;

use crate::Theme;
use crate::css::{SQUARE_WINDOW_SELECTORS, percent};
use crate::sanitize::{self, MAX_WINDOW_RADIUS, MIN_OPACITY};

/// libadwaita's window corner radius, used when a theme doesn't set its own.
const LIBADWAITA_WINDOW_RADIUS: u32 = 15;

/// Window background opacity with transparency on, unless the theme suggests its own.
const DEFAULT_WINDOW_OPACITY: f32 = 0.84;
/// Opacity of the sidebar, queue panel and now-playing bar, unless the theme suggests its own.
const DEFAULT_PANEL_OPACITY: f32 = 0.76;

/// Appearance options from the user's settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Appearance {
    pub corners: CornerStyle,
    pub transparency: bool,
    /// Ask GTK to blur behind the translucent window (GTK 4.24 passes a window's
    /// `backdrop-filter` on to the compositor).
    pub blur: bool,
    /// Crisper lines and a soft shadow between the bars and the page.
    pub edges: bool,
}

const BLUR_CSS: &str = "\n/* Blur behind the window, done by the compositor */\n\
                        window.mp-main-window {\n  backdrop-filter: blur(24px);\n}\n";

/// Lines in the theme's text colour show on light and dark themes alike, where
/// libadwaita's dark-mode sidebar line is dark on dark. The shadows use libadwaita's
/// shade, which is stronger in dark mode, and fall on the page: from the player bar's
/// container, which is drawn over the page, and on strips along the page's own edges
/// beside the sidebar and the queue (`mp-edge-shade`).
const EDGES_CSS: &str = "\n/* Defined edges */\n\
:root {
  --mp-edge-color: color-mix(in srgb, var(--window-fg-color) 16%, transparent);
  --mp-edge-shade: var(--shade-color);
}

/* The queue panel paints over its pane, so it draws its own line. */
.mp-library-split > .sidebar-pane:dir(ltr),
.mp-queue-panel:dir(rtl) {
  box-shadow: inset -1px 0 var(--mp-edge-color);
}

.mp-library-split > .sidebar-pane:dir(rtl),
.mp-queue-panel:dir(ltr) {
  box-shadow: inset 1px 0 var(--mp-edge-color);
}

.mp-edge-shade {
  min-width: 12px;
}

.mp-edge-shade.mp-start:dir(ltr),
.mp-edge-shade.mp-end:dir(rtl) {
  background-image: linear-gradient(to right, var(--mp-edge-shade), transparent);
}

.mp-edge-shade.mp-start:dir(rtl),
.mp-edge-shade.mp-end:dir(ltr) {
  background-image: linear-gradient(to left, var(--mp-edge-shade), transparent);
}

.mp-player-bar {
  box-shadow: inset 0 1px var(--mp-edge-color);
}

.mp-main-window toolbarview > .bottom-bar {
  box-shadow: 0 -2px 10px -2px var(--mp-edge-shade);
}

.mp-sidebar-footer {
  border-top-color: var(--mp-edge-color);
}
";

/// Corner radii in pixels for one [`CornerStyle`].
struct Radii {
    window: u32,
    panel: u32,
    cover: u32,
    button: u32,
    small: u32,
    pill: u32,
}

impl Radii {
    /// `None` for [`CornerStyle::Round`], which keeps the theme's own radii.
    fn for_style(corners: CornerStyle) -> Option<Self> {
        match corners {
            CornerStyle::Round => None,
            CornerStyle::Slight => Some(Self {
                window: 10,
                panel: 8,
                cover: 6,
                button: 10,
                small: 5,
                pill: 10,
            }),
            CornerStyle::Square => Some(Self {
                window: 4,
                panel: 3,
                cover: 2,
                button: 4,
                small: 2,
                pill: 4,
            }),
        }
    }
}

impl Theme {
    /// CSS for the user's appearance options, to be loaded above the theme's own
    /// stylesheet. Empty when the options leave the theme as it is.
    pub fn appearance_css(&self, appearance: Appearance) -> String {
        let mut css = String::new();
        if let Some(radii) = Radii::for_style(appearance.corners) {
            css.push_str(&corner_css(&radii));
        }
        if appearance.transparency {
            css.push_str(&self.transparency_css());
            if appearance.blur {
                css.push_str(BLUR_CSS);
            }
        }
        if appearance.edges {
            css.push_str(EDGES_CSS);
        }
        css
    }

    /// The window's corner radius in pixels with these options, e.g. to shape the
    /// blur behind it.
    pub fn window_radius(&self, corners: CornerStyle) -> u32 {
        match Radii::for_style(corners) {
            Some(radii) => radii.window,
            None => self
                .manifest
                .shape
                .window_radius
                .map_or(LIBADWAITA_WINDOW_RADIUS, |radius| {
                    radius.min(MAX_WINDOW_RADIUS)
                }),
        }
    }

    fn transparency_css(&self) -> String {
        let effects = &self.manifest.effects;
        let opacity = |hint: Option<f32>, default| {
            hint.and_then(|value| sanitize::opacity(value, MIN_OPACITY))
                .unwrap_or(default)
        };
        let window = opacity(effects.window_opacity, DEFAULT_WINDOW_OPACITY);
        let panel = opacity(effects.panel_opacity, DEFAULT_PANEL_OPACITY);
        format!(
            "/* Transparency */\n\
             :root {{\n\
             \x20 --mp-window-bg-color: {};\n\
             \x20 --mp-panel-bg-color: {};\n\
             \x20 --mp-player-bar-surface: {};\n\
             }}\n\n\
             /* Content views let the translucent window show through. */\n\
             .mp-main-window .view,\n\
             .mp-main-window columnview,\n\
             .mp-main-window listview,\n\
             .mp-main-window gridview {{\n\
             \x20 background-color: transparent;\n\
             }}\n",
            translucent("var(--window-bg-color)", window),
            translucent("var(--sidebar-bg-color)", panel),
            translucent("var(--mp-player-bar-bg-color)", panel),
        )
    }
}

fn translucent(color: &str, opacity: f32) -> String {
    format!(
        "color-mix(in srgb, {color} {}%, transparent)",
        percent(opacity)
    )
}

/// The app's radius variables plus libadwaita's own widgets, whose radii aren't
/// variables.
fn corner_css(radii: &Radii) -> String {
    let Radii {
        window,
        panel,
        cover,
        button,
        small,
        pill,
    } = radii;
    let mut css = format!(
        "/* Corners */\n\
         :root {{\n\
         \x20 --window-radius: {window}px;\n\
         \x20 --mp-panel-radius: {panel}px;\n\
         \x20 --mp-cover-radius: {cover}px;\n\
         \x20 --mp-button-radius: {button}px;\n\
         \x20 --mp-small-radius: {small}px;\n\
         \x20 --mp-pill-radius: {pill}px;\n\
         }}\n\n"
    );
    let _ = writeln!(
        css,
        "{} {{\n  --window-radius: 0px;\n}}\n",
        SQUARE_WINDOW_SELECTORS.join(",\n")
    );
    let rules: [(&str, &u32); 4] = [
        (
            "button,\nentry,\nspinbutton,\ntooltip,\n.navigation-sidebar > row,\nmodelbutton",
            small,
        ),
        (
            "button.pill,\nbutton.circular,\nswitch,\nswitch > slider,\ntoast",
            pill,
        ),
        (
            ".card,\n.boxed-list,\n.boxed-list-separate > row,\npopover > contents,\ndialog.floating sheet",
            panel,
        ),
        // Rows paint their own corners at the ends of a boxed list.
        (
            ".boxed-list > row:first-child,\n.boxed-list > row:last-child",
            panel,
        ),
    ];
    for (selector, radius) in rules {
        let _ = writeln!(css, "{selector} {{\n  border-radius: {radius}px;\n}}\n");
    }
    css.push_str(
        ".boxed-list > row:first-child:not(:last-child) {\n  \
         border-bottom-left-radius: 0;\n  border-bottom-right-radius: 0;\n}\n\n\
         .boxed-list > row:last-child:not(:first-child) {\n  \
         border-top-left-radius: 0;\n  border-top-right-radius: 0;\n}\n\n\
         .boxed-list > row:not(:first-child):not(:last-child) {\n  border-radius: 0;\n}\n",
    );
    css
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

    #[test]
    fn default_options_leave_the_theme_alone() {
        let css = theme(ThemeManifest::default()).appearance_css(Appearance::default());
        assert!(css.is_empty());
    }

    #[test]
    fn corner_styles_set_every_radius() {
        let theme = theme(ThemeManifest::default());
        let square = theme.appearance_css(Appearance {
            corners: CornerStyle::Square,
            transparency: false,
            blur: false,
            edges: false,
        });
        let slight = theme.appearance_css(Appearance {
            corners: CornerStyle::Slight,
            transparency: false,
            blur: false,
            edges: false,
        });
        for css in [&square, &slight] {
            for variable in [
                "--window-radius",
                "--mp-panel-radius",
                "--mp-cover-radius",
                "--mp-button-radius",
                "--mp-small-radius",
                "--mp-pill-radius",
            ] {
                assert!(css.contains(variable), "{variable} missing:\n{css}");
            }
            assert!(css.contains("window.csd.maximized"));
            assert!(!css.contains("transparent"));
        }
        assert!(square.contains("--mp-panel-radius: 3px;"));
        assert!(slight.contains("--mp-panel-radius: 8px;"));
    }

    #[test]
    fn window_radius_follows_the_style_then_the_theme() {
        let mut manifest = ThemeManifest::default();
        assert_eq!(
            theme(manifest.clone()).window_radius(CornerStyle::Round),
            15
        );
        manifest.shape.window_radius = Some(18);
        let themed = theme(manifest);
        assert_eq!(themed.window_radius(CornerStyle::Round), 18);
        assert_eq!(themed.window_radius(CornerStyle::Slight), 10);
        assert_eq!(themed.window_radius(CornerStyle::Square), 4);
    }

    #[test]
    fn transparency_uses_the_theme_hints_or_defaults() {
        let on = Appearance {
            corners: CornerStyle::Round,
            transparency: true,
            blur: false,
            edges: false,
        };
        let plain = theme(ThemeManifest::default()).appearance_css(on);
        assert!(plain.contains("var(--window-bg-color) 84%, transparent"));
        assert!(plain.contains("--mp-player-bar-surface"));
        assert!(!plain.contains("--mp-panel-radius"));

        let mut manifest = ThemeManifest::default();
        manifest.effects.window_opacity = Some(0.9);
        manifest.effects.panel_opacity = Some(0.1);
        let hinted = theme(manifest).appearance_css(on);
        assert!(hinted.contains("var(--window-bg-color) 90%, transparent"));
        assert!(hinted.contains("var(--sidebar-bg-color) 50%, transparent"));
        assert!(!hinted.contains("backdrop-filter"));
    }

    #[test]
    fn blur_needs_transparency() {
        let theme = theme(ThemeManifest::default());
        let blur = |transparency| {
            theme.appearance_css(Appearance {
                corners: CornerStyle::Round,
                transparency,
                blur: true,
                edges: false,
            })
        };
        assert!(blur(true).contains("backdrop-filter: blur("));
        assert!(blur(false).is_empty());
    }

    #[test]
    fn edges_outline_the_sidebar_queue_and_player_bar() {
        let css = theme(ThemeManifest::default()).appearance_css(Appearance {
            edges: true,
            ..Appearance::default()
        });
        for selector in [
            ".mp-library-split > .sidebar-pane:dir(ltr)",
            ".mp-queue-panel:dir(ltr)",
            ".mp-player-bar {",
        ] {
            assert!(css.contains(selector), "{selector} missing:\n{css}");
        }
        assert!(!css.contains("--mp-panel-radius"));
    }
}
