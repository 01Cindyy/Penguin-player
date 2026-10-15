//! `mp.storage`: small persistent key/value storage, saved as JSON.

use mlua::serde::{DeserializeOptions, SerializeOptions};
use mlua::{Lua, LuaSerdeExt, Table, Value};

use super::ApiContext;
use crate::errors::error_at_caller;
use crate::storage::{MAX_DEPTH, MAX_STORAGE_BYTES, StorageError};

pub(super) fn table(lua: &Lua, context: &ApiContext) -> mlua::Result<Table> {
    let storage = lua.create_table()?;

    let store = context.storage.clone();
    let get = lua.create_function(move |lua, key: String| match store.borrow().get(&key) {
        Some(value) => lua.to_value_with(value, to_lua_options()),
        None => Ok(Value::Nil),
    })?;
    storage.set("get", get)?;

    let store = context.storage.clone();
    let set = lua.create_function(move |lua, (key, value): (String, Value)| {
        if key.is_empty() {
            return Err(error_at_caller(lua, "storage keys must not be empty"));
        }
        let json = match value {
            Value::Nil => None,
            value => Some(to_json(lua, &key, value)?),
        };
        store
            .borrow_mut()
            .set(&key, json)
            .map_err(|err| error_at_caller(lua, err))
    })?;
    storage.set("set", set)?;

    Ok(storage)
}

/// `value` as JSON, or an error telling the plugin why it can't be stored.
fn to_json(lua: &Lua, key: &str, value: Value) -> mlua::Result<serde_json::Value> {
    let mut room = MAX_STORAGE_BYTES;
    match measure(&value, 0, &mut room) {
        Ok(()) => {}
        Err(Unstorable::TooLarge) => return Err(error_at_caller(lua, StorageError::TooLarge)),
        Err(Unstorable::NotFinite) => {
            return Err(error_at_caller(
                lua,
                format!("can't store '{key}': JSON has no NaN or infinite numbers"),
            ));
        }
    }
    lua.from_value_with(value, from_lua_options())
        .map_err(|err| {
            let reason = match err {
                mlua::Error::DeserializeError(reason) => reason,
                other => other.to_string(),
            };
            error_at_caller(
                lua,
                format!(
                    "can't store '{key}': only booleans, numbers, strings and tables of \
                     those (lists or string keys, nested at most {MAX_DEPTH} deep) can be \
                     stored ({reason})"
                ),
            )
        })
}

/// Why a value can't be stored, found before converting it.
enum Unstorable {
    TooLarge,
    NotFinite,
}

/// Takes the size `value` will have as JSON out of `room` (a lower bound: escapes and
/// the digits of numbers aren't counted) and refuses numbers JSON can't hold.
///
/// This runs before the conversion, which copies a string every time a table refers
/// to it: a table holding one long string thousands of times, or a few tables nested
/// in each other many times over, takes a few kilobytes in Lua but gigabytes (and
/// minutes) as JSON. Everything the conversion refuses anyway, such as tables nested
/// deeper than [`MAX_DEPTH`], is left to it.
fn measure(value: &Value, depth: usize, room: &mut usize) -> Result<(), Unstorable> {
    let size = match value {
        Value::Number(number) if !number.is_finite() => return Err(Unstorable::NotFinite),
        Value::String(text) => text.as_bytes().len() + 2,
        Value::Table(table) if depth < MAX_DEPTH => {
            // Raw iteration, like the conversion: no metamethods run.
            for pair in table.pairs::<Value, Value>() {
                let Ok((key, item)) = pair else {
                    break;
                };
                measure(&key, depth + 1, room)?;
                measure(&item, depth + 1, room)?;
            }
            2
        }
        _ => 1,
    };
    *room = room.checked_sub(size).ok_or(Unstorable::TooLarge)?;
    Ok(())
}

/// JSON `null` becomes `nil` and arrays become plain tables.
pub(super) fn to_lua_options() -> SerializeOptions {
    SerializeOptions::new()
        .set_array_metatable(false)
        .serialize_none_to_null(false)
        .serialize_unit_to_null(false)
}

/// Functions and other non-data values are refused, and tables that mix list
/// items with keys are rejected instead of silently losing the keys.
fn from_lua_options() -> DeserializeOptions {
    DeserializeOptions::new()
        .deny_unsupported_types(true)
        .deny_recursive_tables(true)
        .detect_mixed_tables(true)
        .recursion_limit(MAX_DEPTH)
}
