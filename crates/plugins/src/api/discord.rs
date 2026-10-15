//! `mp.discord.set_activity(activity)` and `mp.discord.clear()`: what the user is
//! listening to, on their Discord profile. Only for plugins with `"discord": true` in
//! their `permissions`.

use std::time::Duration;

use mlua::{Lua, Table, Value};

use super::{ApiContext, duration_arg};
use crate::discord::{Activity, ActivityKind, MAX_TEXT_CHARS, StatusDisplay};
use crate::errors::error_at_caller;
use crate::text::truncate;

const FIELDS: [&str; 10] = [
    "type",
    "details",
    "state",
    "large_image",
    "large_text",
    "small_image",
    "small_text",
    "elapsed_ms",
    "duration_ms",
    "status_display",
];

/// Image names and addresses longer than this are refused by Discord.
const MAX_IMAGE_CHARS: usize = 256;

pub(super) fn table(lua: &Lua, context: &ApiContext) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    let allowed = context.permissions.discord;
    let refused = || "this plugin may not use Discord: add \"discord\": true to \"permissions\" in plugin.json";

    let presence = context.presence.clone();
    let id = context.id.clone();
    let set_activity = lua.create_function(move |lua, activity: Table| {
        if !allowed {
            return Err(error_at_caller(lua, refused()));
        }
        let activity = read_activity(lua, &activity)?;
        presence.set(&id, Some(activity));
        Ok(())
    })?;
    table.set("set_activity", set_activity)?;

    let presence = context.presence.clone();
    let id = context.id.clone();
    let clear = lua.create_function(move |lua, ()| {
        if !allowed {
            return Err(error_at_caller(lua, refused()));
        }
        presence.set(&id, None);
        Ok(())
    })?;
    table.set("clear", clear)?;
    Ok(table)
}

fn read_activity(lua: &Lua, table: &Table) -> mlua::Result<Activity> {
    let mut activity = Activity::default();
    let mut elapsed = None;
    let mut duration = None;
    for pair in table.pairs::<Value, Value>() {
        let (key, value) = pair?;
        let key = match &key {
            Value::String(key) => key.to_string_lossy(),
            _ => String::new(),
        };
        match key.as_str() {
            "type" => {
                let name = text(lua, &key, &value)?;
                activity.kind = ActivityKind::ALL
                    .into_iter()
                    .find(|kind| kind.name() == name)
                    .ok_or_else(|| {
                        unknown_name(lua, &key, &name, ActivityKind::ALL.map(ActivityKind::name))
                    })?;
            }
            "status_display" => {
                let name = text(lua, &key, &value)?;
                activity.status_display = StatusDisplay::ALL
                    .into_iter()
                    .find(|display| display.name() == name)
                    .ok_or_else(|| {
                        unknown_name(
                            lua,
                            &key,
                            &name,
                            StatusDisplay::ALL.map(StatusDisplay::name),
                        )
                    })?;
            }
            "details" | "state" | "large_text" | "small_text" => {
                let shown = Some(truncate(&text(lua, &key, &value)?, MAX_TEXT_CHARS))
                    .filter(|text| !text.trim().is_empty());
                match key.as_str() {
                    "details" => activity.details = shown,
                    "state" => activity.state = shown,
                    "large_text" => activity.large_text = shown,
                    _ => activity.small_text = shown,
                }
            }
            "large_image" | "small_image" => {
                let image = text(lua, &key, &value)?;
                if image.chars().count() > MAX_IMAGE_CHARS {
                    return Err(error_at_caller(
                        lua,
                        format!("'{key}' is longer than {MAX_IMAGE_CHARS} characters"),
                    ));
                }
                let image = Some(image).filter(|image| !image.is_empty());
                if key == "large_image" {
                    activity.large_image = image;
                } else {
                    activity.small_image = image;
                }
            }
            "elapsed_ms" => elapsed = Some(milliseconds(lua, &key, &value)?),
            "duration_ms" => duration = Some(milliseconds(lua, &key, &value)?),
            _ => {
                return Err(error_at_caller(
                    lua,
                    format!(
                        "unknown activity field '{key}'; expected one of: {}",
                        FIELDS.join(", ")
                    ),
                ));
            }
        }
    }
    if let Some(elapsed) = elapsed {
        activity.set_progress(elapsed, duration);
    }
    Ok(activity)
}

fn text(lua: &Lua, key: &str, value: &Value) -> mlua::Result<String> {
    match value {
        Value::String(text) => Ok(text.to_string_lossy()),
        _ => Err(error_at_caller(lua, format!("'{key}' must be text"))),
    }
}

fn milliseconds(lua: &Lua, key: &str, value: &Value) -> mlua::Result<Duration> {
    let ms = match value {
        Value::Integer(ms) => *ms as f64,
        Value::Number(ms) => *ms,
        _ => return Err(error_at_caller(lua, format!("'{key}' must be a number"))),
    };
    duration_arg(lua, ms)
}

fn unknown_name<const N: usize>(lua: &Lua, key: &str, name: &str, known: [&str; N]) -> mlua::Error {
    error_at_caller(
        lua,
        format!(
            "unknown {key} '{name}'; expected one of: {}",
            known.join(", ")
        ),
    )
}
