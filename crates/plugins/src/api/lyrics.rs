//! `mp.lyrics.provide(finder, [options])`: the plugin finds lyrics for songs that have
//! none of their own, or, with `options.replace`, for songs whose own lyrics it may
//! replace. The app calls `finder(track, done)` when it needs a song's lyrics, and the
//! plugin calls `done(text)` with LRC or plain text, or `done(nil)`, once it knows.

use mlua::{Function, Lua, Table, Value};

use super::ApiContext;
use crate::errors::error_at_caller;
use crate::lyrics::LyricsSearch;

/// The values of `options.replace`, and the songs each has the finder look up.
const REPLACE: [(&str, LyricsSearch); 3] = [
    ("none", LyricsSearch::Missing),
    ("untimed", LyricsSearch::Untimed),
    ("all", LyricsSearch::All),
];

pub(super) fn table(lua: &Lua, context: &ApiContext) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    let registry = context.registry.clone();
    let provide =
        lua.create_function(move |lua, (finder, options): (Function, Option<Table>)| {
            let search = match options {
                Some(options) => read_options(lua, &options)?,
                None => LyricsSearch::Missing,
            };
            registry.borrow_mut().lyrics = Some((finder, search));
            Ok(())
        })?;
    table.set("provide", provide)?;
    Ok(table)
}

fn read_options(lua: &Lua, options: &Table) -> mlua::Result<LyricsSearch> {
    let names = || {
        REPLACE
            .iter()
            .map(|(name, _)| format!("'{name}'"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    for pair in options.pairs::<Value, Value>() {
        let (key, _) = pair?;
        if !matches!(&key, Value::String(key) if key.as_bytes() == b"replace") {
            return Err(error_at_caller(
                lua,
                format!(
                    "unknown lyrics option '{}'; the only option is 'replace'",
                    key.to_string()?
                ),
            ));
        }
    }
    match options.get::<Value>("replace")? {
        Value::Nil => Ok(LyricsSearch::Missing),
        Value::String(replace) => REPLACE
            .iter()
            .find(|(name, _)| name.as_bytes() == replace.as_bytes().as_ref())
            .map(|(_, search)| *search)
            .ok_or_else(|| {
                error_at_caller(
                    lua,
                    format!(
                        "'replace' must be one of {}, not '{}'",
                        names(),
                        replace.to_string_lossy()
                    ),
                )
            }),
        _ => Err(error_at_caller(
            lua,
            format!("'replace' must be one of {}", names()),
        )),
    }
}
