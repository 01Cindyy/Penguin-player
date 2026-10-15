//! A theme made from the colours [pywal](https://github.com/eylles/pywal16) picks from
//! the user's wallpaper. pywal rewrites `colors.json` in its cache folder every time it
//! runs, so watching that file keeps the theme in step with the wallpaper.
//!
//! Wallpapers can give any colours at all, so every colour is nudged towards white or
//! black until it is readable where the app uses it (WCAG AA, as for the built-in themes).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use mp_core::files::read_text_limited;
use serde::Deserialize;

use crate::{Palette, Theme, ThemeError, ThemeManifest, ThemeScheme, ThemeSource};

/// Id of the theme, which a user theme of the same id replaces like any built-in.
pub const PYWAL_THEME_ID: &str = "pywal";

/// The file pywal writes its palette to, inside its cache folder.
pub const PYWAL_COLORS_FILE: &str = "colors.json";

/// pywal's `colors.json` is under a kilobyte.
const MAX_COLORS_BYTES: u64 = 64 * 1024;

const TEXT_CONTRAST: f64 = 4.5;
const GRAPHIC_CONTRAST: f64 = 3.0;

/// How far each surface is mixed from the background towards the text colour.
const SIDEBAR_MIX: f64 = 0.05;
const CARD_MIX: f64 = 0.08;
const POPOVER_MIX: f64 = 0.10;

/// How much of their own colour format badges lay over a surface, as in the app's
/// `.mp-format-badge` style.
const HIRES_TINT: f64 = 0.12;
const LOSSLESS_TINT: f64 = 0.10;

/// The colours pywal generates from one wallpaper (`color0` to `color15`).
#[derive(Deserialize)]
struct WalColors {
    special: Special,
    colors: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct Special {
    background: String,
    foreground: String,
}

/// pywal's cache folder, found the way pywal finds it: `$PYWAL_CACHE_DIR`, else `wal`
/// in `$XDG_CACHE_HOME`, else `~/.cache/wal`.
pub fn pywal_dir() -> Option<PathBuf> {
    pywal_dir_from(|name| std::env::var_os(name))
}

fn pywal_dir_from(var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let path = |name| {
        var(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    if let Some(dir) = path("PYWAL_CACHE_DIR") {
        return Some(dir);
    }
    let cache = path("XDG_CACHE_HOME").or_else(|| {
        path("HOME")
            .or_else(|| path("USERPROFILE"))
            .map(|home| home.join(".cache"))
    })?;
    Some(cache.join("wal"))
}

/// Builds the theme from `colors.json` in pywal's cache folder `dir`.
pub fn load_pywal_theme(dir: &Path) -> Result<Theme, ThemeError> {
    let path = dir.join(PYWAL_COLORS_FILE);
    let text = read_text_limited(&path, MAX_COLORS_BYTES).map_err(|source| ThemeError::Io {
        path: path.clone(),
        source,
    })?;
    let manifest = pywal_manifest(&text).map_err(|message| ThemeError::Invalid {
        path: path.clone(),
        message,
    })?;
    Ok(Theme {
        id: PYWAL_THEME_ID.to_owned(),
        manifest,
        source: ThemeSource::Pywal(dir.to_path_buf()),
        extra_css: None,
        stylesheet_problem: None,
    })
}

/// The theme manifest for the contents of pywal's `colors.json`.
pub fn pywal_manifest(json: &str) -> Result<ThemeManifest, String> {
    let wal: WalColors = serde_json::from_str(json).map_err(|err| err.to_string())?;
    let background = Rgb::parse(&wal.special.background)
        .ok_or_else(|| format!("background {:?} is not a colour", wal.special.background))?;
    let foreground = Rgb::parse(&wal.special.foreground)
        .ok_or_else(|| format!("foreground {:?} is not a colour", wal.special.foreground))?;
    // color1 to color6 are the wallpaper's own colours; the rest are greys.
    let accents: Vec<Rgb> = (1..=6)
        .filter_map(|index| wal.colors.get(&format!("color{index}")))
        .filter_map(|color| Rgb::parse(color))
        .collect();

    let dark = background.contrast(WHITE) >= background.contrast(BLACK);
    let palette = palette(background, foreground, &accents, dark);
    let mut manifest = ThemeManifest {
        name: "Pywal".to_owned(),
        author: "pywal".to_owned(),
        description: "Follows the colours pywal picks from your wallpaper.".to_owned(),
        ..ThemeManifest::default()
    };
    if dark {
        manifest.scheme = ThemeScheme::Dark;
        manifest.dark = palette;
    } else {
        manifest.scheme = ThemeScheme::Light;
        manifest.light = palette;
    }
    Ok(manifest)
}

fn palette(background: Rgb, foreground: Rgb, accents: &[Rgb], dark: bool) -> Palette {
    let away = if dark { WHITE } else { BLACK };
    let sidebar = background.mix(foreground, SIDEBAR_MIX);
    let card = background.mix(foreground, CARD_MIX);
    let popover = background.mix(foreground, POPOVER_MIX);
    let surfaces = [background, sidebar, card, popover];
    let readable_on_all = |color: Rgb, minimum: f64| {
        surfaces
            .iter()
            .all(|surface| color.contrast(*surface) >= minimum)
    };

    let text = towards(foreground, away, |color| {
        readable_on_all(color, TEXT_CONTRAST)
    });
    let border = format!(
        "rgba({}, {}, {}, 0.10)",
        text.channel(0),
        text.channel(1),
        text.channel(2)
    );

    let mut palette = Palette {
        window_bg: Some(background.hex()),
        view_bg: Some(background.hex()),
        headerbar_bg: Some(background.hex()),
        sidebar_bg: Some(sidebar.hex()),
        player_bar_bg: Some(sidebar.hex()),
        dialog_bg: Some(sidebar.hex()),
        card_bg: Some(card.hex()),
        popover_bg: Some(popover.hex()),
        border: Some(border),
        ..Palette::default()
    };
    for fg in [
        &mut palette.window_fg,
        &mut palette.view_fg,
        &mut palette.headerbar_fg,
        &mut palette.sidebar_fg,
        &mut palette.player_bar_fg,
        &mut palette.dialog_fg,
        &mut palette.card_fg,
        &mut palette.popover_fg,
    ] {
        *fg = Some(text.hex());
    }

    // The most colourful of the wallpaper's colours; without any, libadwaita's accent.
    let Some(&vivid) = accents
        .iter()
        .max_by(|a, b| a.chroma().total_cmp(&b.chroma()))
    else {
        return palette;
    };
    let badge_readable = |color: Rgb, tint: f64| {
        surfaces
            .iter()
            .all(|surface| color.contrast(color.mix_over(*surface, tint)) >= TEXT_CONTRAST)
    };
    // In light mode libadwaita writes accent-coloured text (such as the Lossless badge)
    // in the accent itself, or darker; in dark mode it lightens it enough on its own.
    let accent = towards(vivid, away, |color| {
        readable_on_all(color, GRAPHIC_CONTRAST) && (dark || badge_readable(color, LOSSLESS_TINT))
    });
    let accent_fg = if accent.contrast(WHITE) >= accent.contrast(BLACK) {
        WHITE
    } else {
        BLACK
    };
    let badge = towards(accent, away, |color| badge_readable(color, HIRES_TINT));
    palette.accent = Some(accent.hex());
    palette.accent_fg = Some(accent_fg.hex());
    palette.hires_badge = Some(badge.hex());
    palette
}

/// `color` mixed towards `target` just far enough to be `readable`.
fn towards(color: Rgb, target: Rgb, readable: impl Fn(Rgb) -> bool) -> Rgb {
    (0..=50)
        .map(|step| color.mix(target, f64::from(step) / 50.0))
        .find(|candidate| readable(*candidate))
        .unwrap_or(target)
}

const WHITE: Rgb = Rgb([1.0; 3]);
const BLACK: Rgb = Rgb([0.0; 3]);

/// An opaque sRGB colour with channels in `0.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Rgb([f64; 3]);

impl Rgb {
    /// `#rrggbb` or `#rgb`, as pywal writes them.
    fn parse(text: &str) -> Option<Self> {
        let hex = text.trim().strip_prefix('#')?;
        if !hex.is_ascii() {
            return None;
        }
        let channel = |digits: &str| u8::from_str_radix(digits, 16).ok().map(f64::from);
        let [r, g, b] = match hex.len() {
            6 => [&hex[0..2], &hex[2..4], &hex[4..6]].map(channel),
            3 => [0, 1, 2].map(|index| channel(&hex[index..=index]).map(|value| value * 17.0)),
            _ => return None,
        };
        Some(Self([r?, g?, b?].map(|value| value / 255.0)))
    }

    fn channel(self, index: usize) -> u8 {
        // Always in 0..=255 after the clamp, so the cast can't truncate.
        (self.0[index].clamp(0.0, 1.0) * 255.0).round() as u8
    }

    fn hex(self) -> String {
        format!(
            "#{:02x}{:02x}{:02x}",
            self.channel(0),
            self.channel(1),
            self.channel(2)
        )
    }

    /// `amount` of the way from `self` to `other`.
    fn mix(self, other: Self, amount: f64) -> Self {
        Self([0, 1, 2].map(|i| self.0[i] + (other.0[i] - self.0[i]) * amount))
    }

    /// `self` at `amount` opacity over `background`.
    fn mix_over(self, background: Self, amount: f64) -> Self {
        background.mix(self, amount)
    }

    fn chroma(self) -> f64 {
        let [r, g, b] = self.0;
        r.max(g).max(b) - r.min(g).min(b)
    }

    /// WCAG 2 contrast ratio.
    fn contrast(self, other: Self) -> f64 {
        let (a, b) = (self.luminance(), other.luminance());
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    fn luminance(self) -> f64 {
        let [r, g, b] = self.0.map(|channel| {
            if channel <= 0.04045 {
                channel / 12.92
            } else {
                ((channel + 0.055) / 1.055).powf(2.4)
            }
        });
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wal_json(background: &str, foreground: &str, colors: [&str; 6]) -> String {
        let mut entries: Vec<String> = colors
            .iter()
            .enumerate()
            .map(|(index, color)| format!("\"color{}\": \"{color}\"", index + 1))
            .collect();
        entries.push(format!("\"color0\": \"{background}\""));
        entries.push(format!("\"color7\": \"{foreground}\""));
        format!(
            r#"{{
                "wallpaper": "/wallpapers/sunset.png",
                "alpha": "100",
                "special": {{ "background": "{background}", "foreground": "{foreground}", "cursor": "{foreground}" }},
                "colors": {{ {} }}
            }}"#,
            entries.join(", ")
        )
    }

    fn color(value: &Option<String>) -> Rgb {
        Rgb::parse(value.as_deref().expect("colour is set")).expect("colour parses")
    }

    #[test]
    fn dark_wallpaper_gives_a_dark_theme_with_its_most_colourful_accent() {
        let json = wal_json(
            "#0b0b1a",
            "#c2c2c5",
            [
                "#554658", "#4E5570", "#6C5C71", "#6D6172", "#AE5B59", "#8D5C6A",
            ],
        );
        let manifest = pywal_manifest(&json).unwrap();
        assert_eq!(manifest.scheme, ThemeScheme::Dark);
        assert_eq!(manifest.light, Palette::default());
        let palette = &manifest.dark;
        assert_eq!(palette.window_bg.as_deref(), Some("#0b0b1a"));
        assert_eq!(palette.window_fg.as_deref(), Some("#c2c2c5"));
        assert_eq!(palette.accent.as_deref(), Some("#ae5b59"));
        assert_eq!(palette.accent_fg.as_deref(), Some("#ffffff"));
    }

    #[test]
    fn light_wallpaper_gives_a_light_theme() {
        let json = wal_json(
            "#f2ece4",
            "#3a3530",
            [
                "#b0563f", "#6f8a4c", "#c79a3b", "#4f6d8f", "#8e5a8c", "#4a8a86",
            ],
        );
        let manifest = pywal_manifest(&json).unwrap();
        assert_eq!(manifest.scheme, ThemeScheme::Light);
        assert_eq!(manifest.dark, Palette::default());
        assert_eq!(manifest.light.view_bg.as_deref(), Some("#f2ece4"));
    }

    #[test]
    fn unreadable_palettes_are_made_readable() {
        let murky = wal_json(
            "#202020",
            "#404040",
            [
                "#303030", "#2a2f2a", "#352b2b", "#2b2b35", "#333333", "#2e2e2e",
            ],
        );
        let washed_out = wal_json(
            "#e8e8e8",
            "#c8c8c8",
            [
                "#f0d0d0", "#d0f0d0", "#d0d0f0", "#e0e0c0", "#ddeedd", "#eeeeee",
            ],
        );
        for json in [murky, washed_out] {
            let manifest = pywal_manifest(&json).unwrap();
            let palette = if manifest.scheme == ThemeScheme::Dark {
                &manifest.dark
            } else {
                &manifest.light
            };
            let accent = color(&palette.accent);
            let badge = color(&palette.hires_badge);
            for surface in [
                &palette.window_bg,
                &palette.sidebar_bg,
                &palette.card_bg,
                &palette.popover_bg,
            ] {
                let surface = color(surface);
                assert!(color(&palette.window_fg).contrast(surface) >= TEXT_CONTRAST);
                assert!(accent.contrast(surface) >= GRAPHIC_CONTRAST);
                assert!(badge.contrast(badge.mix_over(surface, HIRES_TINT)) >= TEXT_CONTRAST);
            }
            assert!(accent.contrast(color(&palette.accent_fg)) >= TEXT_CONTRAST);
        }
    }

    #[test]
    fn missing_accent_colours_keep_the_default_accent() {
        let json = r##"{ "special": { "background": "#101010", "foreground": "#e0e0e0" },
                         "colors": { "color1": "not a colour" } }"##;
        let manifest = pywal_manifest(json).unwrap();
        assert_eq!(manifest.dark.accent, None);
        assert_eq!(manifest.dark.hires_badge, None);
        assert!(manifest.dark.window_bg.is_some());
    }

    #[test]
    fn broken_files_are_errors() {
        assert!(pywal_manifest("{").is_err());
        assert!(pywal_manifest(r#"{ "colors": {} }"#).is_err());
        let bad_background =
            r##"{ "special": { "background": "red", "foreground": "#fff" }, "colors": {} }"##;
        assert!(pywal_manifest(bad_background).is_err());
    }

    #[test]
    fn colours_parse_like_pywal_writes_them() {
        assert_eq!(Rgb::parse("#AE5B59").unwrap().hex(), "#ae5b59");
        assert_eq!(Rgb::parse("#fa0").unwrap().hex(), "#ffaa00");
        for invalid in ["AE5B59", "#ae5b5", "#gggggg", "#ééé", ""] {
            assert_eq!(Rgb::parse(invalid), None, "{invalid}");
        }
    }

    #[test]
    fn cache_folder_is_found_like_pywal_does() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| OsString::from(value))
            }
        };
        let dir = |pairs| pywal_dir_from(env(pairs));
        assert_eq!(
            dir(&[("PYWAL_CACHE_DIR", "/wal"), ("HOME", "/home/me")]),
            Some(PathBuf::from("/wal"))
        );
        assert_eq!(
            dir(&[("XDG_CACHE_HOME", "/cache"), ("HOME", "/home/me")]),
            Some(PathBuf::from("/cache/wal"))
        );
        assert_eq!(
            dir(&[("XDG_CACHE_HOME", ""), ("HOME", "/home/me")]),
            Some(PathBuf::from("/home/me/.cache/wal"))
        );
        assert_eq!(
            dir(&[("USERPROFILE", "C:\\Users\\me")]),
            Some(PathBuf::from("C:\\Users\\me").join(".cache").join("wal"))
        );
        assert_eq!(dir(&[]), None);
    }

    #[test]
    fn loads_from_the_cache_folder() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            load_pywal_theme(dir.path()),
            Err(ThemeError::Io { .. })
        ));
        let json = wal_json("#000000", "#ffffff", ["#ff0000"; 6]);
        std::fs::write(dir.path().join(PYWAL_COLORS_FILE), json).unwrap();
        let theme = load_pywal_theme(dir.path()).unwrap();
        assert_eq!(theme.id, PYWAL_THEME_ID);
        assert_eq!(theme.source, ThemeSource::Pywal(dir.path().to_path_buf()));
    }
}
