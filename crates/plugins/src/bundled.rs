//! Plugins shipped inside the app. Their files live in `crates/plugins/bundled/<id>/`,
//! laid out like any plugin folder. They are listed with the user's plugins and, like
//! those, stay off until switched on.

pub(crate) struct BundledPlugin {
    pub id: &'static str,
    pub manifest: &'static str,
    /// Every Lua file, by its path inside the plugin folder.
    files: &'static [(&'static str, &'static str)],
}

impl BundledPlugin {
    pub fn file(&self, path: &str) -> Option<&'static str> {
        self.files
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, source)| *source)
    }
}

/// A bundled plugin from `bundled/<id>/`: its `plugin.json` and the Lua files listed.
macro_rules! bundled {
    ($id:literal, [$($file:literal),+ $(,)?]) => {
        BundledPlugin {
            id: $id,
            manifest: include_str!(concat!("../bundled/", $id, "/plugin.json")),
            files: &[$(($file, include_str!(concat!("../bundled/", $id, "/", $file)))),+],
        }
    };
}

const BUNDLED: [BundledPlugin; 2] = [
    bundled!("online-lyrics", ["main.lua", "search.lua"]),
    bundled!("discord-status", ["main.lua"]),
];

pub(crate) fn all() -> &'static [BundledPlugin] {
    &BUNDLED
}

pub(crate) fn find(id: &str) -> Option<&'static BundledPlugin> {
    BUNDLED.iter().find(|plugin| plugin.id == id)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// Every Lua file in a bundled plugin's folder must be listed, or `require` would
    /// fail for it once shipped.
    #[test]
    fn every_file_is_included() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("bundled");
        let mut ids: Vec<String> = std::fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        ids.sort();
        let mut listed: Vec<&str> = BUNDLED.iter().map(|plugin| plugin.id).collect();
        listed.sort_unstable();
        assert_eq!(ids, listed);
        for plugin in &BUNDLED {
            let mut on_disk = Vec::new();
            lua_files(&root.join(plugin.id), Path::new(""), &mut on_disk);
            on_disk.sort();
            let mut included: Vec<&str> = plugin.files.iter().map(|(name, _)| *name).collect();
            included.sort_unstable();
            assert_eq!(on_disk, included, "{}", plugin.id);
        }
    }

    fn lua_files(dir: &Path, relative: &Path, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let name = relative.join(entry.file_name());
            if entry.path().is_dir() {
                lua_files(&entry.path(), &name, found);
            } else if name.extension().is_some_and(|extension| extension == "lua") {
                found.push(name.to_str().unwrap().to_owned());
            }
        }
    }
}
