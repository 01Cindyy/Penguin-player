//! `plugin.json`, plugin discovery and validation.

use std::io;
use std::path::{Component, Path, PathBuf};

use mp_core::PluginPermissions;
use mp_core::files::{file_inside, is_hidden_name, read_text_limited};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::API_VERSION;
use crate::bundled::{self, BundledPlugin};

/// Name of the manifest file inside a plugin folder.
pub const MANIFEST_FILE: &str = "plugin.json";

/// Manifests are a few hundred bytes; anything far larger is not a manifest.
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

/// Most sites a plugin may connect to, and most settings it may offer.
const MAX_HOSTS: usize = 16;
const MAX_SETTINGS: usize = 32;

/// Contents of `plugin.json`. The plugin's id is the name of its folder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    /// Entry script relative to the plugin folder.
    #[serde(default = "default_entry")]
    pub entry: String,
    #[serde(default = "default_api_version")]
    pub api_version: u32,
    /// What the plugin does beyond the sandbox. The user allows it when switching the
    /// plugin on.
    #[serde(default)]
    pub permissions: PluginPermissions,
    /// What the plugin sends where, in its own words, shown when the user is asked to
    /// allow its permissions.
    #[serde(default)]
    pub privacy: String,
    /// The settings on the plugin's settings page.
    #[serde(default)]
    pub settings: Vec<SettingSpec>,
}

/// A setting on a plugin's settings page, read with `mp.settings.get(key)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingSpec {
    pub key: String,
    pub title: String,
    #[serde(default)]
    pub subtitle: String,
    #[serde(flatten)]
    pub kind: SettingKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SettingKind {
    /// On or off.
    Boolean {
        #[serde(default)]
        default: bool,
    },
    /// A number from `min` to `max`, in steps of `step`.
    Number {
        default: f64,
        min: f64,
        max: f64,
        #[serde(default = "default_step")]
        step: f64,
    },
    /// One of a few options, by value.
    Choice {
        default: String,
        choices: Vec<SettingChoice>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingChoice {
    pub value: String,
    pub title: String,
}

fn default_step() -> f64 {
    1.0
}

impl SettingSpec {
    pub fn default_value(&self) -> Value {
        match &self.kind {
            SettingKind::Boolean { default } => Value::Bool(*default),
            SettingKind::Number { default, .. } => number_value(*default),
            SettingKind::Choice { default, .. } => Value::String(default.clone()),
        }
    }

    /// `value` if it suits this setting, otherwise the default.
    pub fn value_or_default(&self, value: Option<&Value>) -> Value {
        let suitable = value.and_then(|value| match &self.kind {
            SettingKind::Boolean { .. } => value.as_bool().map(Value::Bool),
            SettingKind::Number { min, max, .. } => value
                .as_f64()
                .filter(|number| (*min..=*max).contains(number))
                .map(number_value),
            SettingKind::Choice { choices, .. } => value
                .as_str()
                .filter(|value| choices.iter().any(|choice| choice.value == *value))
                .map(|value| Value::String(value.to_owned())),
        });
        suitable.unwrap_or_else(|| self.default_value())
    }

    fn check(&self) -> Result<(), String> {
        let key = &self.key;
        if !is_valid_key(key) {
            return Err(format!(
                "setting key \"{key}\" must be letters, digits, '_' and '-'"
            ));
        }
        if self.title.trim().is_empty() {
            return Err(format!("setting \"{key}\" needs a \"title\""));
        }
        match &self.kind {
            SettingKind::Boolean { .. } => Ok(()),
            SettingKind::Number {
                default,
                min,
                max,
                step,
            } => {
                let finite = [default, min, max, step].iter().all(|n| n.is_finite());
                if finite && min <= default && default <= max && *step > 0.0 {
                    Ok(())
                } else {
                    Err(format!(
                        "setting \"{key}\" needs min <= default <= max and a step above 0"
                    ))
                }
            }
            SettingKind::Choice { default, choices } => {
                let mut values: Vec<&str> = choices.iter().map(|c| c.value.as_str()).collect();
                values.sort_unstable();
                values.dedup();
                if choices.is_empty() || values.len() != choices.len() {
                    Err(format!(
                        "setting \"{key}\" needs \"choices\" with different values"
                    ))
                } else if !values.contains(&default.as_str()) {
                    Err(format!(
                        "setting \"{key}\" has a default that isn't one of its choices"
                    ))
                } else {
                    Ok(())
                }
            }
        }
    }
}

/// A whole number as an integer, so Lua sees `30` rather than `30.0`.
fn number_value(number: f64) -> Value {
    const EXACT: f64 = 9_007_199_254_740_992.0;
    if number.fract() == 0.0 && number.abs() < EXACT {
        Value::from(number as i64)
    } else {
        Value::from(number)
    }
}

fn is_valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// A plain lowercase host name such as `lrclib.net`: no scheme, port or path.
fn is_valid_host(host: &str) -> bool {
    host.len() <= 253
        && host.contains('.')
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
}

fn default_entry() -> String {
    "main.lua".to_owned()
}

fn default_api_version() -> u32 {
    API_VERSION
}

/// Where a plugin's files are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginSource {
    /// A folder in the user's plugins folder.
    Folder(PathBuf),
    /// Inside the app.
    Bundled,
}

/// A plugin found on disk or shipped with the app, for listing in the UI.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginInfo {
    pub id: String,
    pub source: PluginSource,
    pub manifest: PluginManifest,
    /// Why the plugin cannot be loaded (bad manifest, missing entry script, newer API...).
    pub problem: Option<String>,
}

impl PluginInfo {
    pub fn is_bundled(&self) -> bool {
        self.source == PluginSource::Bundled
    }
}

/// The plugins shipped with the app and those in `plugins_dir`, sorted by display
/// name. Folders with a broken manifest are still listed (named after the folder) with
/// `problem` set.
pub fn discover(plugins_dir: &Path) -> Vec<PluginInfo> {
    let mut plugins: Vec<PluginInfo> = bundled::all().iter().map(inspect_bundled).collect();
    match std::fs::read_dir(plugins_dir) {
        Ok(entries) => plugins.extend(
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.is_dir() && !path.file_name().is_some_and(is_hidden_name))
                .filter_map(|dir| inspect(&dir)),
        ),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => log::warn!("Could not list plugins in {}: {err}", plugins_dir.display()),
    }
    plugins.sort_by_cached_key(|plugin| (plugin.manifest.name.to_lowercase(), plugin.id.clone()));
    plugins
}

/// The plugin `id`: shipped with the app, or a folder in `plugins_dir`.
pub(crate) fn find(plugins_dir: &Path, id: &str) -> Option<PluginInfo> {
    if let Some(plugin) = bundled::find(id) {
        return Some(inspect_bundled(plugin));
    }
    if !is_valid_id(id) {
        return None;
    }
    let dir = plugins_dir.join(id);
    if dir.is_dir() { inspect(&dir) } else { None }
}

fn inspect_bundled(plugin: &BundledPlugin) -> PluginInfo {
    let (manifest, problem) = match serde_json::from_str::<PluginManifest>(plugin.manifest) {
        Ok(manifest) => {
            let problem = check(&manifest, |entry| plugin.file(entry).is_some()).err();
            (manifest, problem)
        }
        Err(err) => (
            placeholder_manifest(plugin.id),
            Some(format!("{MANIFEST_FILE} is invalid: {err}")),
        ),
    };
    PluginInfo {
        id: plugin.id.to_owned(),
        source: PluginSource::Bundled,
        manifest,
        problem,
    }
}

/// Reads and validates one plugin folder. Returns `None` only when the folder name
/// can't be used as an id (it isn't valid UTF-8).
pub(crate) fn inspect(dir: &Path) -> Option<PluginInfo> {
    let id = dir.file_name()?.to_str()?.to_owned();
    let (manifest, problem) = match read_manifest(dir) {
        Ok(mut manifest) => {
            let problem = if bundled::find(&id).is_some() {
                Some(format!(
                    "{} includes a plugin with this folder name; rename the folder",
                    mp_core::APP_NAME
                ))
            } else {
                validate(dir, &manifest).err()
            };
            if manifest.name.trim().is_empty() {
                manifest.name = id.clone();
            }
            (manifest, problem)
        }
        Err(problem) => (placeholder_manifest(&id), Some(problem)),
    };
    Some(PluginInfo {
        id,
        source: PluginSource::Folder(dir.to_path_buf()),
        manifest,
        problem,
    })
}

/// Whether `id` is a plain folder name, so joining it to a directory stays inside it.
pub(crate) fn is_valid_id(id: &str) -> bool {
    let mut components = Path::new(id).components();
    matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none()
        && !id.contains(['/', '\\'])
}

fn read_manifest(dir: &Path) -> Result<PluginManifest, String> {
    let path = dir.join(MANIFEST_FILE);
    let text = read_text_limited(&path, MAX_MANIFEST_BYTES).map_err(|err| match err.kind() {
        io::ErrorKind::NotFound => format!("{MANIFEST_FILE} is missing"),
        _ => format!("could not read {MANIFEST_FILE}: {err}"),
    })?;
    serde_json::from_str(&text).map_err(|err| format!("{MANIFEST_FILE} is invalid: {err}"))
}

fn validate(dir: &Path, manifest: &PluginManifest) -> Result<(), String> {
    let entry = Path::new(&manifest.entry);
    let plain_relative = entry
        .components()
        .all(|component| matches!(component, Component::Normal(_)));
    if manifest.entry.is_empty() || !plain_relative {
        return Err(format!(
            "entry script \"{}\" must be a path inside the plugin folder",
            manifest.entry
        ));
    }
    check(manifest, |_| file_inside(dir, entry).is_some())
}

/// Checks everything but where the entry script is, which `has_entry` tells.
fn check(manifest: &PluginManifest, has_entry: impl Fn(&str) -> bool) -> Result<(), String> {
    if manifest.name.trim().is_empty() {
        return Err(format!("{MANIFEST_FILE} needs a non-empty \"name\""));
    }
    if manifest.api_version > API_VERSION {
        return Err(format!(
            "needs plugin API version {}, but this version of {} supports up to version {API_VERSION}; \
             updating the app may help",
            manifest.api_version,
            mp_core::APP_NAME
        ));
    }
    if !has_entry(&manifest.entry) {
        return Err(format!("entry script \"{}\" was not found", manifest.entry));
    }
    let hosts = &manifest.permissions.network;
    if hosts.len() > MAX_HOSTS {
        return Err(format!("a plugin may connect to at most {MAX_HOSTS} sites"));
    }
    if let Some(host) = hosts.iter().find(|host| !is_valid_host(host)) {
        return Err(format!(
            "\"{host}\" in \"permissions\" isn't a site's name, such as \"example.com\" \
             (lowercase, without https:// or a path)"
        ));
    }
    if manifest.settings.len() > MAX_SETTINGS {
        return Err(format!("a plugin may have at most {MAX_SETTINGS} settings"));
    }
    for (index, setting) in manifest.settings.iter().enumerate() {
        setting.check()?;
        if manifest.settings[..index]
            .iter()
            .any(|other| other.key == setting.key)
        {
            return Err(format!("there are two settings called \"{}\"", setting.key));
        }
    }
    Ok(())
}

fn placeholder_manifest(id: &str) -> PluginManifest {
    PluginManifest {
        name: id.to_owned(),
        version: String::new(),
        author: String::new(),
        description: String::new(),
        entry: default_entry(),
        api_version: API_VERSION,
        permissions: PluginPermissions::default(),
        privacy: String::new(),
        settings: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, file: &str, contents: &str) {
        let path = dir.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn discovers_valid_and_broken_plugins() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        write(
            dir,
            "good/plugin.json",
            r#"{ "name": "Good", "version": "1.0" }"#,
        );
        write(dir, "good/main.lua", "");
        write(
            dir,
            "custom-entry/plugin.json",
            r#"{ "name": "Custom", "entry": "src/init.lua" }"#,
        );
        write(dir, "custom-entry/src/init.lua", "");
        write(dir, "no-manifest/main.lua", "");
        write(dir, "bad-json/plugin.json", "{ name: ");
        write(dir, "no-name/plugin.json", r#"{ "name": "  " }"#);
        write(dir, "no-name/main.lua", "");
        write(dir, "no-entry/plugin.json", r#"{ "name": "No entry" }"#);
        write(
            dir,
            "escape/plugin.json",
            r#"{ "name": "Escape", "entry": "../good/main.lua" }"#,
        );
        write(
            dir,
            "future/plugin.json",
            r#"{ "name": "Future", "api_version": 99 }"#,
        );
        write(dir, "future/main.lua", "");
        write(dir, ".hidden/plugin.json", r#"{ "name": "Hidden" }"#);
        write(dir, "not-a-folder.json", "{}");

        let plugins: Vec<PluginInfo> = discover(dir)
            .into_iter()
            .filter(|plugin| !plugin.is_bundled())
            .collect();
        let summary: Vec<(&str, &str, bool)> = plugins
            .iter()
            .map(|p| (p.id.as_str(), p.manifest.name.as_str(), p.problem.is_some()))
            .collect();
        assert_eq!(
            summary,
            [
                ("bad-json", "bad-json", true),
                ("custom-entry", "Custom", false),
                ("escape", "Escape", true),
                ("future", "Future", true),
                ("good", "Good", false),
                ("no-entry", "No entry", true),
                ("no-manifest", "no-manifest", true),
                ("no-name", "no-name", true),
            ]
        );
        let problem = |id: &str| {
            plugins
                .iter()
                .find(|p| p.id == id)
                .and_then(|p| p.problem.clone())
                .unwrap()
        };
        assert!(problem("bad-json").contains("plugin.json is invalid"));
        assert!(problem("no-manifest").contains("plugin.json is missing"));
        assert!(problem("no-name").contains("non-empty \"name\""));
        assert!(problem("no-entry").contains("\"main.lua\" was not found"));
        assert!(problem("escape").contains("inside the plugin folder"));
        assert!(problem("future").contains("API version 99"));
        let good = plugins.iter().find(|p| p.id == "good").unwrap();
        assert_eq!(good.manifest.entry, "main.lua");
        assert_eq!(good.manifest.api_version, API_VERSION);
        assert_eq!(good.source, PluginSource::Folder(dir.join("good")));
    }

    #[test]
    fn bundled_plugins_are_valid() {
        let root = tempfile::tempdir().unwrap();
        let plugins = discover(&root.path().join("nope"));
        for plugin in &plugins {
            assert!(plugin.is_bundled());
            assert_eq!(plugin.problem, None, "{}", plugin.id);
            assert!(!plugin.manifest.description.is_empty(), "{}", plugin.id);
        }
        let first = &plugins[0];
        assert_eq!(find(root.path(), &first.id).as_ref(), Some(first));
    }

    #[test]
    fn folders_cant_take_a_bundled_plugins_id() {
        let root = tempfile::tempdir().unwrap();
        let id = bundled::all()[0].id;
        write(
            root.path(),
            &format!("{id}/plugin.json"),
            r#"{ "name": "Mine" }"#,
        );
        write(root.path(), &format!("{id}/main.lua"), "");
        let mine = inspect(&root.path().join(id)).unwrap();
        assert!(mine.problem.unwrap().contains("rename the folder"));
        assert!(find(root.path(), id).unwrap().is_bundled());
    }

    fn problem_with(json: &str) -> Option<String> {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "p/plugin.json", json);
        write(root.path(), "p/main.lua", "");
        inspect(&root.path().join("p")).unwrap().problem
    }

    #[test]
    fn permissions_name_sites() {
        let ok = r#"{ "name": "P", "permissions": { "network": ["lrclib.net", "a-b.example.co.uk"], "discord": true } }"#;
        assert_eq!(problem_with(ok), None);
        for host in [
            "https://lrclib.net",
            "lrclib.net/api",
            "LRCLIB.net",
            "localhost",
            "-a.com",
            "a..com",
        ] {
            let json = format!(r#"{{ "name": "P", "permissions": {{ "network": ["{host}"] }} }}"#);
            let problem = problem_with(&json).unwrap_or_else(|| panic!("{host} was accepted"));
            assert!(problem.contains("isn't a site's name"), "{problem}");
        }
    }

    #[test]
    fn settings_are_checked() {
        let with =
            |setting: &str| problem_with(&format!(r#"{{ "name": "P", "settings": [{setting}] }}"#));
        assert_eq!(
            with(r#"{ "key": "on", "type": "boolean", "title": "On" }"#),
            None
        );
        assert_eq!(
            with(
                r#"{ "key": "n", "type": "number", "title": "N", "default": 5, "min": 1, "max": 10 }"#
            ),
            None
        );
        assert_eq!(
            with(
                r#"{ "key": "c", "type": "choice", "title": "C", "default": "b",
                     "choices": [{ "value": "a", "title": "A" }, { "value": "b", "title": "B" }] }"#
            ),
            None
        );
        for (setting, expected) in [
            (
                r#"{ "key": "bad key", "type": "boolean", "title": "T" }"#,
                "letters, digits",
            ),
            (
                r#"{ "key": "k", "type": "boolean", "title": " " }"#,
                "needs a \"title\"",
            ),
            (
                r#"{ "key": "k", "type": "number", "title": "T", "default": 11, "min": 1, "max": 10 }"#,
                "min <= default",
            ),
            (
                r#"{ "key": "k", "type": "number", "title": "T", "default": 1, "min": 1, "max": 10, "step": 0 }"#,
                "step above 0",
            ),
            (
                r#"{ "key": "k", "type": "choice", "title": "T", "default": "a", "choices": [] }"#,
                "different values",
            ),
            (
                r#"{ "key": "k", "type": "choice", "title": "T", "default": "z", "choices": [{ "value": "a", "title": "A" }] }"#,
                "isn't one of",
            ),
            (
                r#"{ "key": "k", "type": "boolean", "title": "T" }, { "key": "k", "type": "boolean", "title": "U" }"#,
                "two settings",
            ),
            (
                r#"{ "key": "k", "type": "colour", "title": "T" }"#,
                "invalid",
            ),
        ] {
            let problem = with(setting).unwrap_or_else(|| panic!("{setting} was accepted"));
            assert!(problem.contains(expected), "{problem}");
        }
    }

    #[test]
    fn setting_values_fall_back_to_defaults() {
        let manifest: PluginManifest = serde_json::from_str(
            r#"{ "name": "P", "settings": [
                { "key": "on", "type": "boolean", "title": "On", "default": true },
                { "key": "n", "type": "number", "title": "N", "default": 5, "min": 1, "max": 10, "step": 0.5 },
                { "key": "c", "type": "choice", "title": "C", "default": "a",
                  "choices": [{ "value": "a", "title": "A" }, { "value": "b", "title": "B" }] }
            ] }"#,
        )
        .unwrap();
        let [on, n, c] = &manifest.settings[..] else {
            panic!("three settings")
        };
        use serde_json::json;
        assert_eq!(on.value_or_default(None), json!(true));
        assert_eq!(on.value_or_default(Some(&json!(false))), json!(false));
        assert_eq!(on.value_or_default(Some(&json!("no"))), json!(true));
        assert_eq!(n.value_or_default(Some(&json!(7.0))), json!(7));
        assert_eq!(n.value_or_default(Some(&json!(2.5))), json!(2.5));
        assert_eq!(n.value_or_default(Some(&json!(99))), json!(5));
        assert_eq!(c.value_or_default(Some(&json!("b"))), json!("b"));
        assert_eq!(c.value_or_default(Some(&json!("z"))), json!("a"));
    }

    #[test]
    fn missing_folder_has_only_bundled_plugins() {
        let root = tempfile::tempdir().unwrap();
        let plugins = discover(&root.path().join("nope"));
        assert_eq!(plugins.len(), bundled::all().len());
    }

    #[test]
    fn ids_must_be_plain_folder_names() {
        assert!(is_valid_id("now-playing"));
        assert!(is_valid_id("my.plugin"));
        for id in ["", ".", "..", "a/b", "../x", "/abs", "a\\b"] {
            assert!(!is_valid_id(id), "{id:?}");
        }
    }
}
