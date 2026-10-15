//! Themes shipped inside the binary. Their data lives in `crates/theme/themes/<id>/`
//! so it is edited exactly like a user theme.

use crate::{Theme, ThemeManifest, ThemeSource};

struct BuiltinTheme {
    id: &'static str,
    manifest: &'static str,
    stylesheet: Option<&'static str>,
}

/// A built-in theme whose data is `themes/<id>/theme.json` (and `style.css` if it has one).
macro_rules! builtin {
    ($id:literal) => {
        BuiltinTheme {
            id: $id,
            manifest: include_str!(concat!("../themes/", $id, "/theme.json")),
            stylesheet: None,
        }
    };
    ($id:literal, with_stylesheet) => {
        BuiltinTheme {
            stylesheet: Some(include_str!(concat!("../themes/", $id, "/style.css"))),
            ..builtin!($id)
        }
    };
}

/// In display order: the default theme first, then the app's own themes, then
/// popular palettes alphabetically.
const BUILTIN_THEMES: [BuiltinTheme; 14] = [
    builtin!("adwaita"),
    builtin!("midnight"),
    builtin!("rice", with_stylesheet),
    builtin!("frost"),
    builtin!("catppuccin"),
    builtin!("dracula"),
    builtin!("everforest"),
    builtin!("gruvbox"),
    builtin!("kanagawa"),
    builtin!("nord"),
    builtin!("one-dark"),
    builtin!("rose-pine"),
    builtin!("solarized"),
    builtin!("tokyo-night"),
];

/// Themes shipped with the app; the first is the default.
pub fn builtin_themes() -> Vec<Theme> {
    BUILTIN_THEMES
        .iter()
        .map(|builtin| {
            let manifest: ThemeManifest = serde_json::from_str(builtin.manifest)
                .expect("built-in theme.json files are checked by the crate's tests");
            Theme {
                id: builtin.id.to_owned(),
                manifest,
                source: ThemeSource::Builtin,
                extra_css: builtin.stylesheet.map(str::to_owned),
                stylesheet_problem: None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DEFAULT_THEME_ID, ThemeScheme};

    #[test]
    fn every_builtin_parses() {
        for builtin in &BUILTIN_THEMES {
            let parsed = serde_json::from_str::<ThemeManifest>(builtin.manifest);
            assert!(parsed.is_ok(), "{}: {:?}", builtin.id, parsed.err());
        }
        let themes = builtin_themes();
        assert_eq!(themes.len(), BUILTIN_THEMES.len());
        assert!(themes.iter().all(|theme| !theme.manifest.name.is_empty()));
    }

    #[test]
    fn default_theme_comes_first() {
        let themes = builtin_themes();
        assert_eq!(themes[0].id, DEFAULT_THEME_ID);
        assert_eq!(themes[0].id, "adwaita");
        assert_eq!(themes[0].manifest.name, "Adwaita");
        let ids: Vec<&str> = themes.iter().map(|theme| theme.id.as_str()).collect();
        assert_eq!(&ids[..4], ["adwaita", "midnight", "rice", "frost"]);
        let mut popular = ids[4..].to_vec();
        popular.sort_unstable();
        assert_eq!(
            popular,
            ids[4..],
            "popular palettes are listed alphabetically"
        );
    }

    #[test]
    fn adwaita_keeps_system_colours() {
        let adwaita = &builtin_themes()[0];
        let manifest = &adwaita.manifest;
        assert_eq!(manifest.scheme, ThemeScheme::Any);
        assert_eq!(manifest.colors, Default::default());
        assert_eq!(manifest.light, Default::default());
        assert_eq!(manifest.dark, Default::default());
        let css = adwaita.to_css();
        assert!(
            !css.contains("color:"),
            "Adwaita must not override colours:\n{css}"
        );
        assert!(css.contains("--mp-panel-radius"));
    }

    #[test]
    fn themes_have_a_palette_for_every_scheme_they_allow() {
        for theme in &builtin_themes()[1..] {
            let css = theme.to_css();
            let schemes: &[&str] = match theme.scheme() {
                ThemeScheme::Any => &["light", "dark"],
                ThemeScheme::Light => &["light"],
                ThemeScheme::Dark => &["dark"],
            };
            for scheme in schemes {
                let media = format!("@media (prefers-color-scheme: {scheme})");
                assert!(css.contains(&media), "{} lacks {media}", theme.id);
            }
            for expected in [
                "--window-bg-color",
                "--mp-player-bar-bg-color",
                "--mp-panel-radius",
            ] {
                assert!(css.contains(expected), "{} lacks {expected}", theme.id);
            }
            for dark in [false, true] {
                let css = theme.to_css_for_scheme(dark);
                assert!(!css.contains("@media"), "{} (dark: {dark})", theme.id);
            }
        }
        let rice = builtin_themes()
            .into_iter()
            .find(|theme| theme.id == "rice")
            .unwrap();
        assert_eq!(rice.manifest.name, "Soft Rice");
        assert!(rice.extra_css.is_some());
    }
}
