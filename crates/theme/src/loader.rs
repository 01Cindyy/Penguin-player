//! Loading user themes from disk.

use std::io;
use std::path::{Path, PathBuf};

use mp_core::files::{file_inside, is_hidden_name, read_text_limited};

use crate::sanitize::file_reference;
use crate::{Theme, ThemeError, ThemeManifest, ThemeSource, builtin_themes, load_pywal_theme};

/// Name of the manifest file inside a theme folder.
pub const MANIFEST_FILE: &str = "theme.json";

/// Stylesheet picked up automatically when `theme.json` doesn't name one.
pub const STYLESHEET_FILE: &str = "style.css";

/// Stylesheets above this size are ignored; real themes are a few kilobytes.
const MAX_STYLESHEET_BYTES: u64 = 512 * 1024;

/// `theme.json` files are a few kilobytes; anything far larger is not a theme.
const MAX_MANIFEST_BYTES: u64 = 256 * 1024;

/// Loads one user theme from its folder.
pub fn load_theme(dir: &Path) -> Result<Theme, ThemeError> {
    let id = folder_id(dir).ok_or_else(|| ThemeError::Invalid {
        path: dir.to_path_buf(),
        message: "the folder name is not valid UTF-8".to_owned(),
    })?;
    let manifest_path = dir.join(MANIFEST_FILE);
    let text =
        read_text_limited(&manifest_path, MAX_MANIFEST_BYTES).map_err(|source| ThemeError::Io {
            path: manifest_path.clone(),
            source,
        })?;
    let mut manifest: ThemeManifest =
        serde_json::from_str(&text).map_err(|err| ThemeError::Invalid {
            path: manifest_path,
            message: err.to_string(),
        })?;
    if manifest.name.trim().is_empty() {
        manifest.name = id.clone();
    }
    let stylesheet = read_stylesheet(dir, manifest.stylesheet.as_deref())
        .inspect_err(|problem| log::warn!("Theme '{id}': {problem}"));
    let (extra_css, stylesheet_problem) = match stylesheet {
        Ok(css) => (css, None),
        Err(problem) => (None, Some(problem)),
    };
    Ok(Theme {
        id,
        manifest,
        source: ThemeSource::User(dir.to_path_buf()),
        extra_css,
        stylesheet_problem,
    })
}

/// Built-in themes, then the pywal theme if `pywal_dir` holds a palette, then valid user
/// themes from `themes_dir`. A user theme whose id matches one of the others replaces it.
/// Broken themes are logged and skipped.
pub fn discover(themes_dir: &Path, pywal_dir: Option<&Path>) -> Vec<Theme> {
    let mut user_themes: Vec<Theme> = theme_folders(themes_dir)
        .into_iter()
        .filter_map(|dir| match load_theme(&dir) {
            Ok(theme) => Some(theme),
            Err(err) => {
                log::warn!("Skipping theme: {err}");
                None
            }
        })
        .collect();
    user_themes.sort_by_cached_key(|theme| (theme.manifest.name.to_lowercase(), theme.id.clone()));

    let mut themes = builtin_themes();
    themes.extend(pywal_dir.and_then(pywal_theme));
    for theme in user_themes {
        match themes.iter_mut().find(|existing| existing.id == theme.id) {
            Some(builtin) => *builtin = theme,
            None => themes.push(theme),
        }
    }
    themes
}

fn pywal_theme(dir: &Path) -> Option<Theme> {
    match load_pywal_theme(dir) {
        Ok(theme) => Some(theme),
        Err(ThemeError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => None,
        Err(err) => {
            log::warn!("Skipping the pywal theme: {err}");
            None
        }
    }
}

/// Sub-folders of `themes_dir` that contain a manifest, skipping hidden folders.
fn theme_folders(themes_dir: &Path) -> Vec<PathBuf> {
    let entries = match std::fs::read_dir(themes_dir) {
        Ok(entries) => entries,
        Err(err) => {
            if err.kind() != io::ErrorKind::NotFound {
                log::warn!("Could not list themes in {}: {err}", themes_dir.display());
            }
            return Vec::new();
        }
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            let hidden = path.file_name().is_some_and(is_hidden_name);
            !hidden && path.join(MANIFEST_FILE).is_file()
        })
        .collect()
}

fn folder_id(dir: &Path) -> Option<String> {
    dir.file_name()?.to_str().map(str::to_owned)
}

/// Reads the theme's extra stylesheet: the one named in `theme.json`, or `style.css`
/// if present. The error says why it can't be used; the theme then goes without it.
fn read_stylesheet(dir: &Path, declared: Option<&str>) -> Result<Option<String>, String> {
    let name = match declared {
        Some(name) => name,
        None if dir.join(STYLESHEET_FILE).exists() => STYLESHEET_FILE,
        None => return Ok(None),
    };
    let path = file_inside(dir, Path::new(name))
        .ok_or_else(|| format!("the stylesheet {name:?} must be a file inside the theme folder"))?;
    let css = read_text_limited(&path, MAX_STYLESHEET_BYTES)
        .map_err(|err| format!("couldn't read {name}: {err}"))?;
    if let Some(reference) = file_reference(&css) {
        return Err(format!(
            "{name} uses {reference}, but themes can't load other files"
        ));
    }
    Ok(Some(css))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DEFAULT_THEME_ID, ThemeScheme};

    fn write_theme(root: &Path, id: &str, manifest: &str) -> PathBuf {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(MANIFEST_FILE), manifest).unwrap();
        dir
    }

    #[test]
    fn missing_fields_use_defaults() {
        let root = tempfile::tempdir().unwrap();
        let dir = write_theme(root.path(), "plain", "{}");
        let theme = load_theme(&dir).unwrap();
        assert_eq!(theme.id, "plain");
        assert_eq!(theme.manifest.name, "plain");
        assert_eq!(theme.manifest.scheme, ThemeScheme::Any);
        assert_eq!(theme.manifest.colors, Default::default());
        assert_eq!(theme.extra_css, None);
        assert_eq!(theme.source, ThemeSource::User(dir));
    }

    #[test]
    fn stylesheet_is_loaded() {
        let root = tempfile::tempdir().unwrap();
        let named = write_theme(
            root.path(),
            "named",
            r#"{ "stylesheet": "extra/main.css" }"#,
        );
        std::fs::create_dir_all(named.join("extra")).unwrap();
        std::fs::write(named.join("extra/main.css"), ".mp-cover { opacity: 0.9; }").unwrap();
        let theme = load_theme(&named).unwrap();
        assert_eq!(
            theme.extra_css.as_deref(),
            Some(".mp-cover { opacity: 0.9; }")
        );
        assert!(theme.to_css().ends_with(".mp-cover { opacity: 0.9; }\n"));
        assert_eq!(theme.stylesheet_problem, None);

        let implicit = write_theme(root.path(), "implicit", "{}");
        std::fs::write(implicit.join(STYLESHEET_FILE), "window {}").unwrap();
        assert_eq!(
            load_theme(&implicit).unwrap().extra_css.as_deref(),
            Some("window {}")
        );
    }

    #[test]
    fn stylesheet_outside_the_folder_is_refused() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("secret.css"), "window {}").unwrap();
        let dir = write_theme(
            root.path(),
            "sneaky",
            r#"{ "stylesheet": "../secret.css" }"#,
        );
        assert_eq!(load_theme(&dir).unwrap().extra_css, None);
        let absolute = format!(
            r#"{{ "stylesheet": {:?} }}"#,
            root.path().join("secret.css").display().to_string()
        );
        let dir = write_theme(root.path(), "absolute", &absolute);
        assert_eq!(load_theme(&dir).unwrap().extra_css, None);
    }

    /// An implicit `style.css` is checked like a named one: a symlink out of the theme
    /// folder is refused.
    #[cfg(unix)]
    #[test]
    fn implicit_stylesheet_outside_the_folder_is_refused() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("secret.css"), "window {}").unwrap();
        let dir = write_theme(root.path(), "linked", "{}");
        std::os::unix::fs::symlink(root.path().join("secret.css"), dir.join(STYLESHEET_FILE))
            .unwrap();
        assert_eq!(load_theme(&dir).unwrap().extra_css, None);
    }

    /// `@import` and `url()` would let a theme make GTK read any file, such as
    /// `/dev/zero`, which freezes the app.
    #[test]
    fn stylesheets_that_load_files_are_refused() {
        let root = tempfile::tempdir().unwrap();
        for (id, css) in [
            ("imports", "@import url(\"file:///dev/zero\");\nwindow {}"),
            (
                "images",
                ".mp-cover { background-image: url(\"file:///dev/zero\"); }",
            ),
            (
                "hidden-between-strings",
                ".mp-page-title { font-family: \"/*\"; }\n\
                 .mp-player-bar { background-image: url(\"file:///dev/zero\"); }\n\
                 .mp-page-subtitle { font-family: \"*/\"; }",
            ),
        ] {
            let dir = write_theme(root.path(), id, r#"{ "name": "Loader" }"#);
            std::fs::write(dir.join(STYLESHEET_FILE), css).unwrap();
            let theme = load_theme(&dir).unwrap();
            assert_eq!(theme.manifest.name, "Loader");
            assert_eq!(theme.extra_css, None, "{id}");
            let problem = theme.stylesheet_problem.unwrap();
            assert!(
                problem.contains("can't load other files"),
                "{id}: {problem}"
            );
        }
    }

    #[test]
    fn oversized_stylesheet_is_ignored() {
        let root = tempfile::tempdir().unwrap();
        let dir = write_theme(root.path(), "huge", r#"{ "name": "Huge" }"#);
        let huge = "window {}\n".repeat(MAX_STYLESHEET_BYTES as usize / 10 + 1);
        std::fs::write(dir.join(STYLESHEET_FILE), huge).unwrap();
        let theme = load_theme(&dir).unwrap();
        assert_eq!(theme.manifest.name, "Huge");
        assert_eq!(theme.extra_css, None);
        assert!(theme.stylesheet_problem.is_some());
    }

    #[cfg(unix)]
    #[test]
    fn special_files_are_refused_without_blocking() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("device");
        std::fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink("/dev/zero", dir.join(MANIFEST_FILE)).unwrap();
        assert!(matches!(load_theme(&dir), Err(ThemeError::Io { .. })));

        let named = write_theme(root.path(), "named", r#"{ "stylesheet": "zero.css" }"#);
        std::os::unix::fs::symlink("/dev/zero", named.join("zero.css")).unwrap();
        assert_eq!(load_theme(&named).unwrap().extra_css, None);
    }

    #[test]
    fn invalid_manifest_is_an_error() {
        let root = tempfile::tempdir().unwrap();
        let dir = write_theme(root.path(), "broken", "{ not json");
        assert!(matches!(load_theme(&dir), Err(ThemeError::Invalid { .. })));
        let dir = write_theme(
            root.path(),
            "wrong-type",
            r#"{ "shape": { "panel_radius": "big" } }"#,
        );
        assert!(matches!(load_theme(&dir), Err(ThemeError::Invalid { .. })));
        assert!(matches!(
            load_theme(&root.path().join("missing")),
            Err(ThemeError::Io { .. })
        ));
    }

    #[test]
    fn discover_skips_broken_and_replaces_builtins() {
        let root = tempfile::tempdir().unwrap();
        write_theme(root.path(), "broken", "{ not json");
        write_theme(root.path(), "zebra", r#"{ "name": "Zebra" }"#);
        write_theme(root.path(), "apple", r#"{ "name": "Apple" }"#);
        write_theme(root.path(), "rice", r#"{ "name": "My Rice" }"#);
        write_theme(root.path(), ".hidden", r#"{ "name": "Hidden" }"#);
        std::fs::create_dir_all(root.path().join("not-a-theme")).unwrap();

        let themes = discover(root.path(), None);
        let ids: Vec<&str> = themes.iter().map(|theme| theme.id.as_str()).collect();
        let builtins = builtin_themes().len();
        assert_eq!(ids.len(), builtins + 2);
        assert_eq!(ids[builtins..], ["apple", "zebra"]);
        assert_eq!(themes[0].id, DEFAULT_THEME_ID);
        let rice = themes.iter().find(|theme| theme.id == "rice").unwrap();
        assert_eq!(rice.manifest.name, "My Rice");
        assert!(matches!(rice.source, ThemeSource::User(_)));
    }

    #[test]
    fn discover_without_folder_lists_builtins() {
        let root = tempfile::tempdir().unwrap();
        let themes = discover(&root.path().join("does-not-exist"), None);
        assert_eq!(themes, builtin_themes());
    }

    #[test]
    fn discover_lists_pywal_after_builtins_unless_a_user_theme_replaces_it() {
        let root = tempfile::tempdir().unwrap();
        let wal = tempfile::tempdir().unwrap();
        std::fs::write(
            wal.path().join(crate::PYWAL_COLORS_FILE),
            r##"{ "special": { "background": "#101018", "foreground": "#d0d0d8" },
                 "colors": { "color1": "#c05050" } }"##,
        )
        .unwrap();
        write_theme(root.path(), "apple", r#"{ "name": "Apple" }"#);

        let themes = discover(root.path(), Some(wal.path()));
        let builtins = builtin_themes().len();
        assert_eq!(themes[builtins].id, crate::PYWAL_THEME_ID);
        assert!(matches!(themes[builtins].source, ThemeSource::Pywal(_)));
        assert_eq!(themes[builtins + 1].id, "apple");

        write_theme(root.path(), crate::PYWAL_THEME_ID, r#"{ "name": "Mine" }"#);
        let themes = discover(root.path(), Some(wal.path()));
        assert_eq!(themes[builtins].manifest.name, "Mine");
        assert_eq!(themes.len(), builtins + 2);

        let empty = tempfile::tempdir().unwrap();
        assert_eq!(discover(empty.path(), Some(empty.path())), builtin_themes());
    }
}
