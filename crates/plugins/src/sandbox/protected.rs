//! Calls into plugin code from the Rust functions plugins call: `gsub` replacement
//! functions and tables, modules loaded by `require`, and the `__tostring` metamethods
//! of logged values.
//!
//! mlua's own calls give Lua a message handler that builds a traceback, and Lua runs it
//! for every error, including each pending `__close` handler the time limit stops while
//! unwinding. Close to Lua's limit of 200 nested C calls, calling that handler fails,
//! and the "C stack overflow" error replacing the time-limit error gets a traceback,
//! again for every handler: seconds of work. These calls have no message handler; mlua
//! adds one traceback to the error of the Rust function that made the call.

use std::ffi::c_int;
use std::ptr;

use mlua::{FromLuaMulti, Function, IntoLuaMulti, Lua, LuaString, MultiValue, Table, Value, ffi};

/// Calls `function` with `args`, in protected mode but without a message handler.
pub(crate) fn call<R: FromLuaMulti>(
    lua: &Lua,
    function: &Function,
    args: impl IntoLuaMulti,
) -> mlua::Result<R> {
    let mut values = args.into_lua_multi(lua)?;
    values.push_front(Value::Function(function.clone()));
    let mut status = ffi::LUA_OK;
    // SAFETY: `exec_raw` hands over a stack holding exactly `values`, the function
    // first. A protected call replaces them with its results or its error.
    let mut results: MultiValue = unsafe {
        lua.exec_raw(values, |state| {
            status = ffi::lua_pcall(state, ffi::lua_gettop(state) - 1, ffi::LUA_MULTRET, 0);
        })?
    };
    if status == ffi::LUA_OK {
        R::from_lua_multi(results, lua)
    } else {
        Err(into_error(
            status,
            results.pop_front().unwrap_or(Value::Nil),
        ))
    }
}

/// `table[key]`, running `__index` metamethods.
pub(crate) fn index(lua: &Lua, table: &Table, key: Value) -> mlua::Result<Value> {
    // SAFETY: `get_field` follows Lua's C calling convention.
    let get = unsafe { lua.create_c_function(get_field)? };
    call(lua, &get, (table, key))
}

/// `value` as `tostring` turns it into text, running `__tostring` metamethods.
pub(crate) fn to_string(lua: &Lua, value: Value) -> mlua::Result<LuaString> {
    // SAFETY: `describe_value` follows Lua's C calling convention.
    let describe = unsafe { lua.create_c_function(describe_value)? };
    call(lua, &describe, value)
}

unsafe extern "C-unwind" fn get_field(state: *mut ffi::lua_State) -> c_int {
    // SAFETY: called with the table and the key as its two arguments.
    unsafe { ffi::lua_gettable(state, 1) };
    1
}

unsafe extern "C-unwind" fn describe_value(state: *mut ffi::lua_State) -> c_int {
    // SAFETY: called with the value as its argument.
    unsafe { ffi::luaL_tolstring(state, 1, ptr::null_mut()) };
    1
}

/// The Rust error for the error value of a failed protected call.
fn into_error(status: c_int, value: Value) -> mlua::Error {
    let message = match value {
        Value::Error(error) => return *error,
        Value::String(text) => text.to_string_lossy(),
        other => format!("(error object is a {} value)", other.type_name()),
    };
    if status == ffi::LUA_ERRMEM {
        mlua::Error::MemoryError(message)
    } else {
        mlua::Error::RuntimeError(message)
    }
}
