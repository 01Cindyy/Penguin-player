//! Every shipped colour combination, and every one the pywal theme generates, must meet
//! WCAG AA: 4.5:1 for text on its
//! background and 3:1 for the accent where it is used as a graphic, such as the seek bar
//! fill. Format badges count as text, on the tint of their own colour they lay over each
//! surface.

use std::path::Path;

use mp_theme::{Palette, Theme, ThemeSource, builtin_themes, load_theme, pywal_manifest};

const TEXT: f64 = 4.5;
const GRAPHICS: f64 = 3.0;

/// Text colour libadwaita puts on destructive and success backgrounds.
const WHITE: &str = "#ffffff";
/// Text colour libadwaita puts on warning backgrounds (black at 80% over a light bg).
const NEAR_BLACK: &str = "#333333";

/// How much of its text colour each badge lays over the surface behind it, as in
/// `.mp-format-badge` in the app's style.css.
const HIRES_TINT: f64 = 0.12;
const LOSSLESS_TINT: f64 = 0.10;

/// libadwaita's colours for what a theme leaves unset.
struct Defaults {
    dark: bool,
    window: &'static str,
    view: &'static str,
    card: &'static str,
    headerbar: &'static str,
    accent: &'static str,
    warning: &'static str,
}

const LIGHT: Defaults = Defaults {
    dark: false,
    window: "#fafafb",
    view: "#ffffff",
    card: "#ffffff",
    headerbar: "#ffffff",
    accent: "#3584e4",
    warning: "#e5a50a",
};

const DARK: Defaults = Defaults {
    dark: true,
    window: "#222226",
    view: "#1d1d20",
    // White at 8% over the window.
    card: "#343437",
    headerbar: "#2e2e32",
    accent: "#3584e4",
    warning: "#cd9309",
};

/// sRGB channels in `0.0..=1.0`.
type Rgb = [f64; 3];

fn shipped_themes() -> Vec<Theme> {
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/themes/sunset");
    let mut themes = builtin_themes();
    themes.push(load_theme(&example).expect("example theme loads"));
    themes.extend(pywal_themes());
    themes
}

/// Background, foreground and color1 to color6 of pywal palettes, from typical ones to
/// ones that are hard to read as they come.
const PYWAL_PALETTES: [(&str, &str, &str, [&str; 6]); 5] = [
    (
        "pywal-dusk",
        "#0b0b1a",
        "#c2c2c5",
        [
            "#554658", "#4E5570", "#6C5C71", "#6D6172", "#AE5B59", "#8D5C6A",
        ],
    ),
    (
        "pywal-paper",
        "#f2ece4",
        "#3a3530",
        [
            "#b0563f", "#6f8a4c", "#c79a3b", "#4f6d8f", "#8e5a8c", "#4a8a86",
        ],
    ),
    (
        "pywal-neon",
        "#05010a",
        "#f5e9ff",
        [
            "#ff2bd6", "#00f0ff", "#faff00", "#7cff00", "#ff6a00", "#9d00ff",
        ],
    ),
    (
        "pywal-murky",
        "#202020",
        "#404040",
        [
            "#303030", "#2a2f2a", "#352b2b", "#2b2b35", "#333333", "#2e2e2e",
        ],
    ),
    (
        "pywal-washed-out",
        "#e8e8e8",
        "#c8c8c8",
        [
            "#f0d0d0", "#d0f0d0", "#d0d0f0", "#e0e0c0", "#ffff66", "#eeeeee",
        ],
    ),
];

fn pywal_themes() -> Vec<Theme> {
    PYWAL_PALETTES
        .iter()
        .map(|(id, background, foreground, colors)| {
            let colors: Vec<String> = colors
                .iter()
                .enumerate()
                .map(|(index, color)| format!(r#""color{}": "{color}""#, index + 1))
                .collect();
            let json = format!(
                r#"{{ "special": {{ "background": "{background}", "foreground": "{foreground}" }},
                     "colors": {{ {} }} }}"#,
                colors.join(", ")
            );
            Theme {
                id: (*id).to_owned(),
                manifest: pywal_manifest(&json).expect("palette parses"),
                source: ThemeSource::Builtin,
                extra_css: None,
                stylesheet_problem: None,
            }
        })
        .collect()
}

#[test]
fn shipped_and_pywal_palettes_meet_wcag_aa() {
    let mut failures = Vec::new();
    for theme in shipped_themes() {
        for (scheme, overrides) in [
            ("light", &theme.manifest.light),
            ("dark", &theme.manifest.dark),
        ] {
            let palette = merged(&theme.manifest.colors, overrides);
            for (label, fg, bg, minimum) in pairs(&palette) {
                let ratio = contrast(rgb(fg), rgb(bg));
                if ratio < minimum {
                    failures.push(format!(
                        "{}/{scheme}: {label} {fg} on {bg} is {ratio:.2}, needs {minimum}",
                        theme.id
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn badge_text_meets_wcag_aa_wherever_badges_are_shown() {
    let mut failures = Vec::new();
    for theme in shipped_themes() {
        for (scheme, overrides, defaults) in [
            ("light", &theme.manifest.light, &LIGHT),
            ("dark", &theme.manifest.dark, &DARK),
        ] {
            let palette = merged(&theme.manifest.colors, overrides);
            let color = |own: &Option<String>, default| rgb(own.as_deref().unwrap_or(default));
            // The app's defaults: libadwaita's warning and accent text colours.
            let hires = palette.hires_badge.as_deref().map_or_else(
                || standalone(color(&palette.warning, defaults.warning), defaults.dark),
                rgb,
            );
            let lossless = standalone(color(&palette.accent, defaults.accent), defaults.dark);
            let headerbar = palette
                .headerbar_bg
                .as_deref()
                .unwrap_or(defaults.headerbar);
            let surfaces = [
                ("window", color(&palette.window_bg, defaults.window)),
                ("view", color(&palette.view_bg, defaults.view)),
                ("card", color(&palette.card_bg, defaults.card)),
                ("player bar", color(&palette.player_bar_bg, headerbar)),
            ];
            for (badge, text, tint) in [
                ("hi-res", hires, HIRES_TINT),
                ("lossless", lossless, LOSSLESS_TINT),
            ] {
                for (surface, background) in surfaces {
                    let ratio = contrast(text, mix(text, background, tint));
                    if ratio < TEXT {
                        failures.push(format!(
                            "{}/{scheme}: {badge} badge on the {surface} is {ratio:.2}, needs {TEXT}",
                            theme.id
                        ));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn foregrounds_are_never_left_without_backgrounds() {
    for theme in shipped_themes() {
        for overrides in [&theme.manifest.light, &theme.manifest.dark] {
            let palette = merged(&theme.manifest.colors, overrides);
            for (fg, bg) in surfaces(&palette) {
                assert_eq!(
                    fg.is_some(),
                    bg.is_some(),
                    "{}: every surface sets both colours or neither",
                    theme.id
                );
            }
        }
    }
}

fn surfaces(p: &Palette) -> [(Option<&str>, Option<&str>); 9] {
    [
        (p.window_fg.as_deref(), p.window_bg.as_deref()),
        (p.view_fg.as_deref(), p.view_bg.as_deref()),
        (p.headerbar_fg.as_deref(), p.headerbar_bg.as_deref()),
        (p.sidebar_fg.as_deref(), p.sidebar_bg.as_deref()),
        (p.card_fg.as_deref(), p.card_bg.as_deref()),
        (p.popover_fg.as_deref(), p.popover_bg.as_deref()),
        (p.dialog_fg.as_deref(), p.dialog_bg.as_deref()),
        (p.player_bar_fg.as_deref(), p.player_bar_bg.as_deref()),
        (p.accent_fg.as_deref(), p.accent.as_deref()),
    ]
}

fn pairs(p: &Palette) -> Vec<(&'static str, &str, &str, f64)> {
    let labels = [
        "window",
        "view",
        "headerbar",
        "sidebar",
        "card",
        "popover",
        "dialog",
        "player bar",
        "accent",
    ];
    let mut pairs: Vec<_> = labels
        .into_iter()
        .zip(surfaces(p))
        .filter_map(|(label, (fg, bg))| Some((label, fg?, bg?, TEXT)))
        .collect();
    let backgrounds = [p.window_bg.as_deref(), p.view_bg.as_deref()];
    for bg in backgrounds.into_iter().flatten() {
        if let Some(accent) = p.accent.as_deref() {
            pairs.push(("accent graphics", accent, bg, GRAPHICS));
        }
    }
    if let Some(destructive) = p.destructive.as_deref() {
        pairs.push(("destructive button", WHITE, destructive, TEXT));
    }
    if let Some(success) = p.success.as_deref() {
        pairs.push(("success button", WHITE, success, TEXT));
    }
    if let Some(warning) = p.warning.as_deref() {
        pairs.push(("warning button", NEAR_BLACK, warning, TEXT));
    }
    pairs
}

fn merged(shared: &Palette, overrides: &Palette) -> Palette {
    let pick = |own: &Option<String>, fallback: &Option<String>| own.clone().or(fallback.clone());
    Palette {
        accent: pick(&overrides.accent, &shared.accent),
        accent_fg: pick(&overrides.accent_fg, &shared.accent_fg),
        destructive: pick(&overrides.destructive, &shared.destructive),
        success: pick(&overrides.success, &shared.success),
        warning: pick(&overrides.warning, &shared.warning),
        window_bg: pick(&overrides.window_bg, &shared.window_bg),
        window_fg: pick(&overrides.window_fg, &shared.window_fg),
        view_bg: pick(&overrides.view_bg, &shared.view_bg),
        view_fg: pick(&overrides.view_fg, &shared.view_fg),
        headerbar_bg: pick(&overrides.headerbar_bg, &shared.headerbar_bg),
        headerbar_fg: pick(&overrides.headerbar_fg, &shared.headerbar_fg),
        sidebar_bg: pick(&overrides.sidebar_bg, &shared.sidebar_bg),
        sidebar_fg: pick(&overrides.sidebar_fg, &shared.sidebar_fg),
        card_bg: pick(&overrides.card_bg, &shared.card_bg),
        card_fg: pick(&overrides.card_fg, &shared.card_fg),
        popover_bg: pick(&overrides.popover_bg, &shared.popover_bg),
        popover_fg: pick(&overrides.popover_fg, &shared.popover_fg),
        dialog_bg: pick(&overrides.dialog_bg, &shared.dialog_bg),
        dialog_fg: pick(&overrides.dialog_fg, &shared.dialog_fg),
        player_bar_bg: pick(&overrides.player_bar_bg, &shared.player_bar_bg),
        player_bar_fg: pick(&overrides.player_bar_fg, &shared.player_bar_fg),
        border: pick(&overrides.border, &shared.border),
        hires_badge: pick(&overrides.hires_badge, &shared.hires_badge),
    }
}

fn rgb(hex: &str) -> Rgb {
    let digits = hex
        .strip_prefix('#')
        .filter(|digits| digits.len() == 6)
        .unwrap_or_else(|| panic!("shipped text colours must be opaque #rrggbb, got {hex}"));
    [0, 2, 4].map(|index| {
        let value = u8::from_str_radix(&digits[index..index + 2], 16).expect("hex digits");
        f64::from(value) / 255.0
    })
}

/// `color` at `amount` over `background`, as a translucent `color-mix(in srgb, …)` gives.
fn mix(color: Rgb, background: Rgb, amount: f64) -> Rgb {
    [0, 1, 2].map(|channel| color[channel] * amount + background[channel] * (1.0 - amount))
}

/// WCAG 2 contrast ratio between two opaque colours.
fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

fn luminance(color: Rgb) -> f64 {
    let [r, g, b] = color.map(to_linear);
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// libadwaita's text colour for a background colour, such as `--accent-color` for
/// `--accent-bg-color`: the same hue and chroma at an OKLab lightness of at most 0.5 in
/// light mode and at least 0.85 in dark mode.
fn standalone(background: Rgb, dark: bool) -> Rgb {
    let [lightness, a, b] = to_oklab(background);
    let lightness = if dark {
        lightness.max(0.85)
    } else {
        lightness.min(0.5)
    };
    from_oklab([lightness, a, b])
}

fn to_oklab(color: Rgb) -> [f64; 3] {
    let [r, g, b] = color.map(to_linear);
    let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
    let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
    let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
    [
        0.210_454_255_3 * l + 0.793_617_785 * m - 0.004_072_046_8 * s,
        1.977_998_495_1 * l - 2.428_592_205 * m + 0.450_593_709_9 * s,
        0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766 * s,
    ]
}

/// Converts back to sRGB, clipping colours outside it.
fn from_oklab([lightness, a, b]: [f64; 3]) -> Rgb {
    let l = (lightness + 0.396_337_777_4 * a + 0.215_803_757_3 * b).powi(3);
    let m = (lightness - 0.105_561_345_8 * a - 0.063_854_172_8 * b).powi(3);
    let s = (lightness - 0.089_484_177_5 * a - 1.291_485_548 * b).powi(3);
    [
        4.076_741_662_1 * l - 3.307_711_591_3 * m + 0.230_969_929_2 * s,
        -1.268_438_004_6 * l + 2.609_757_401_1 * m - 0.341_319_396_5 * s,
        -0.004_196_086_3 * l - 0.703_418_614_7 * m + 1.707_614_701 * s,
    ]
    .map(|channel| to_encoded(channel.clamp(0.0, 1.0)))
}

fn to_linear(channel: f64) -> f64 {
    if channel <= 0.04045 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

fn to_encoded(channel: f64) -> f64 {
    if channel <= 0.003_130_8 {
        channel * 12.92
    } else {
        1.055 * channel.powf(1.0 / 2.4) - 0.055
    }
}
