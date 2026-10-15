//! `mp.settings.get(key)`: the user's choices on the plugin's settings page, which
//! `plugin.json` describes. Changes arrive as `setting_changed` events.

use mlua::{Lua, LuaSerdeExt, Table};
use serde_json::{Map, Value};

use super::ApiContext;
use crate::errors::error_at_caller;
use crate::manifest::SettingSpec;

/// One plugin's settings: what `plugin.json` offers and what the user chose.
pub(crate) struct PluginSettings {
    specs: Vec<SettingSpec>,
    values: Map<String, Value>,
}

impl PluginSettings {
    pub fn new(specs: Vec<SettingSpec>, values: Map<String, Value>) -> Self {
        Self { specs, values }
    }

    /// The value of setting `key`, or `None` if the plugin has no such setting.
    pub fn get(&self, key: &str) -> Option<Value> {
        let spec = self.specs.iter().find(|spec| spec.key == key)?;
        Some(spec.value_or_default(self.values.get(key)))
    }

    /// Sets `key` to `value`, or back to its default for `None`, and returns its new
    /// value. `None` if the plugin has no such setting.
    pub fn set(&mut self, key: &str, value: Option<Value>) -> Option<Value> {
        match value {
            Some(value) => self.values.insert(key.to_owned(), value),
            None => self.values.remove(key),
        };
        self.get(key)
    }
}

pub(super) fn table(lua: &Lua, context: &ApiContext) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    let settings = context.settings.clone();
    let get = lua.create_function(move |lua, key: String| {
        let value = settings.borrow().get(&key).ok_or_else(|| {
            error_at_caller(
                lua,
                format!("there is no setting called '{key}' in plugin.json"),
            )
        })?;
        lua.to_value(&value)
    })?;
    table.set("get", get)?;
    Ok(table)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn values_and_defaults() {
        let specs: Vec<SettingSpec> = serde_json::from_value(json!([
            { "key": "on", "type": "boolean", "title": "On", "default": true },
        ]))
        .unwrap();
        let mut settings = PluginSettings::new(specs, Map::new());
        assert_eq!(settings.get("on"), Some(json!(true)));
        assert_eq!(settings.get("off"), None);
        assert_eq!(settings.set("on", Some(json!(false))), Some(json!(false)));
        assert_eq!(
            settings.set("on", Some(json!("nonsense"))),
            Some(json!(true))
        );
        assert_eq!(settings.set("on", None), Some(json!(true)));
        assert_eq!(settings.set("off", Some(json!(true))), None);
    }
}
