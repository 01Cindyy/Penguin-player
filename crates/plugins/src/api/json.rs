//! `mp.json.decode(text)`: JSON, such as a web API's answer, as Lua values.

use mlua::{Lua, LuaSerdeExt, Table};

use super::storage::to_lua_options;
use crate::errors::error_at_caller;
use crate::http::MAX_BODY_BYTES;

pub(super) fn table(lua: &Lua) -> mlua::Result<Table> {
    let json = lua.create_table()?;
    let decode = lua.create_function(|lua, text: mlua::LuaString| {
        let bytes = text.as_bytes();
        if bytes.len() as u64 > MAX_BODY_BYTES {
            return Err(error_at_caller(
                lua,
                format!(
                    "JSON texts are limited to {} MiB",
                    MAX_BODY_BYTES / 1024 / 1024
                ),
            ));
        }
        let value: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|err| error_at_caller(lua, format!("invalid JSON: {err}")))?;
        lua.to_value_with(&value, to_lua_options())
    })?;
    json.set("decode", decode)?;
    Ok(json)
}
