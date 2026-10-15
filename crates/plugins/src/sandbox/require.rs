//! `require(name)` for plugins: loads `<plugin folder>/<name with dots as folders>.lua`
//! and nothing else, cached per plugin like Lua's own `require`.

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use mlua::chunk::ChunkMode;
use mlua::{Error, Lua, Table, Value};
use mp_core::files::read_limited;

use super::MAX_SCRIPT_BYTES;
use super::protected;
use crate::bundled::BundledPlugin;
use crate::errors::error_at_caller;

const MAX_MODULE_NAME_LEN: usize = 200;

/// Where a plugin's modules are.
pub(crate) enum Modules {
    Folder(PathBuf),
    Bundled(&'static BundledPlugin),
}

impl Modules {
    /// The source of module `name` and its path inside the plugin folder.
    fn find(&self, name: &str) -> Result<(Vec<u8>, String), String> {
        let valid_segment = |segment: &str| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        };
        if name.len() > MAX_MODULE_NAME_LEN || !name.split('.').all(valid_segment) {
            return Err(format!(
                "invalid module name '{name}': use letters, digits, '_' and '-', with '.' between folders"
            ));
        }
        let relative = format!("{}.lua", name.replace('.', "/"));
        let source = match self {
            Modules::Folder(root) => {
                let path = module_path(root, name, &relative)?;
                read_limited(&path, MAX_SCRIPT_BYTES)
                    .map_err(|err| format!("could not read module '{name}': {err}"))?
            }
            Modules::Bundled(plugin) => plugin
                .file(&relative)
                .ok_or_else(|| format!("module '{name}' not found (looked for {relative})"))?
                .as_bytes()
                .to_vec(),
        };
        Ok((source, relative))
    }
}

pub(crate) fn install(lua: &Lua, globals: &Table, modules: Modules) -> mlua::Result<()> {
    let modules = match modules {
        Modules::Folder(dir) => Modules::Folder(dir.canonicalize().map_err(Error::external)?),
        bundled => bundled,
    };
    let cache = lua.create_table()?;
    let loading = RefCell::new(HashSet::new());
    let require = lua.create_function(move |lua, name: String| {
        let cached: Value = cache.raw_get(name.as_str())?;
        if !cached.is_nil() {
            return Ok(cached);
        }
        let (source, relative) = modules
            .find(&name)
            .map_err(|message| error_at_caller(lua, message))?;
        if !loading.borrow_mut().insert(name.clone()) {
            return Err(error_at_caller(
                lua,
                format!("module '{name}' requires itself"),
            ));
        }
        let result = load_module(lua, source, &relative, &name);
        loading.borrow_mut().remove(&name);
        let module = match result? {
            Value::Nil => Value::Boolean(true),
            module => module,
        };
        cache.raw_set(name.as_str(), module.clone())?;
        Ok(module)
    })?;
    globals.raw_set("require", require)
}

fn load_module(lua: &Lua, source: Vec<u8>, relative: &str, name: &str) -> mlua::Result<Value> {
    let module = lua
        .load(source)
        .set_name(format!("={relative}"))
        .set_mode(ChunkMode::Text)
        .into_function()?;
    protected::call(lua, &module, (name, relative))
}

/// The canonical path of module `name` (`relative` inside the plugin folder `root`).
fn module_path(root: &Path, name: &str, relative: &str) -> Result<PathBuf, String> {
    let path = root
        .join(relative)
        .canonicalize()
        .map_err(|_| format!("module '{name}' not found (looked for {relative})"))?;
    if !path.starts_with(root) {
        return Err(format!("module '{name}' is outside the plugin folder"));
    }
    if !path.is_file() {
        return Err(format!("module '{name}' is not a file ({relative})"));
    }
    Ok(path)
}
