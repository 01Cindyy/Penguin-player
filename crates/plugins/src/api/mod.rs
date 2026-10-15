//! The `mp` table: everything a plugin script can do. See `docs/plugins.md` for the
//! user-facing reference; keep the two in sync.

mod discord;
pub(crate) mod http;
mod json;
mod lyrics;
mod player;
pub(crate) mod settings;
mod storage;
mod timer;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use mlua::{Function, Lua, Table, Value, Variadic};
use mp_core::PluginPermissions;

use crate::API_VERSION;
use crate::discord::SharedPresence;
use crate::errors::error_at_caller;
use crate::events::EventKind;
use crate::host::Emitter;
use crate::http::{HttpClient, HttpResponse};
use crate::rate_limit::RateLimit;
use crate::registry::Registry;
use crate::sandbox::protected;
use crate::snapshot::PlayerSnapshot;
use crate::storage::Storage;
use crate::text::truncate;
use settings::PluginSettings;

/// Hands a web request's result back to the host thread, by request id.
pub(crate) type Deliver = Arc<dyn Fn(u64, Result<HttpResponse, String>) + Send + Sync>;

const MAX_TITLE_CHARS: usize = 200;
const MAX_BODY_CHARS: usize = 2000;
const MAX_LOG_CHARS: usize = 4000;
/// Longest delay a timer or seek accepts (30 days); larger values are clamped.
const MAX_DURATION: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Notifications a plugin may show at once, and per second after that.
const NOTIFICATION_BURST: u32 = 10;
const NOTIFICATIONS_PER_SECOND: u32 = 2;

/// Messages a plugin may log at once, and per second after that, so a plugin logging
/// in a loop can't fill the disk through the system log.
const LOG_BURST: u32 = 100;
const LOGS_PER_SECOND: u32 = 20;

const LOG_LEVELS: [(&str, log::Level); 4] = [
    ("debug", log::Level::Debug),
    ("info", log::Level::Info),
    ("warn", log::Level::Warn),
    ("error", log::Level::Error),
];

/// Everything the `mp` functions of one plugin need.
pub(crate) struct ApiContext {
    pub id: String,
    pub name: String,
    pub version: String,
    /// `plugin::<id>`, the target of everything the plugin logs.
    pub log_target: Rc<str>,
    pub emitter: Emitter,
    pub snapshot: Rc<RefCell<PlayerSnapshot>>,
    pub registry: Rc<RefCell<Registry<Function>>>,
    pub storage: Rc<RefCell<Storage>>,
    pub settings: Rc<RefCell<PluginSettings>>,
    /// What the plugin may do beyond the sandbox: what it asks for and the user allowed.
    pub permissions: PluginPermissions,
    pub http: Arc<dyn HttpClient>,
    pub deliver: Deliver,
    /// The last web request's id, shared by all plugins.
    pub request_ids: Rc<Cell<u64>>,
    pub presence: Rc<SharedPresence>,
}

/// Installs the global `mp` table, and `print` as an alias of `mp.log.info`.
pub(crate) fn install(lua: &Lua, context: &ApiContext) -> mlua::Result<()> {
    let mp = lua.create_table()?;
    mp.set("api_version", API_VERSION)?;
    mp.set("plugin", plugin_table(lua, context)?)?;
    mp.set("on", on_function(lua, context)?)?;
    mp.set("notify", notify_function(lua, context)?)?;
    mp.set("player", player::table(lua, context)?)?;
    mp.set("storage", storage::table(lua, context)?)?;
    mp.set("timer", timer::table(lua, context)?)?;
    mp.set("settings", settings::table(lua, context)?)?;
    mp.set("http", http::table(lua, context)?)?;
    mp.set("json", json::table(lua)?)?;
    mp.set("lyrics", lyrics::table(lua, context)?)?;
    mp.set("discord", discord::table(lua, context)?)?;

    let log = log_table(lua, &context.log_target)?;
    lua.globals()
        .raw_set("print", log.get::<Function>("info")?)?;
    mp.set("log", log)?;

    lua.globals().raw_set("mp", mp)
}

fn plugin_table(lua: &Lua, context: &ApiContext) -> mlua::Result<Table> {
    let plugin = lua.create_table()?;
    plugin.set("id", context.id.as_str())?;
    plugin.set("name", context.name.as_str())?;
    plugin.set("version", context.version.as_str())?;
    Ok(plugin)
}

/// `mp.on(event, handler)`
fn on_function(lua: &Lua, context: &ApiContext) -> mlua::Result<Function> {
    let registry = context.registry.clone();
    lua.create_function(move |lua, (event, handler): (String, Function)| {
        let kind = EventKind::from_name(&event).ok_or_else(|| {
            let known: Vec<&str> = EventKind::ALL.iter().map(|kind| kind.name()).collect();
            error_at_caller(
                lua,
                format!(
                    "unknown event '{event}'; expected one of: {}",
                    known.join(", ")
                ),
            )
        })?;
        registry
            .borrow_mut()
            .on(kind, handler)
            .map_err(|err| error_at_caller(lua, err))
    })
}

/// `mp.notify(title, body?)`. Notifications beyond the rate limit are dropped: nobody
/// wants a wall of them, even from a plugin reacting to rapid track changes.
fn notify_function(lua: &Lua, context: &ApiContext) -> mlua::Result<Function> {
    let emitter = context.emitter.clone();
    let log_target = context.log_target.clone();
    let limit = RefCell::new(RateLimit::new(
        "notifications",
        NOTIFICATION_BURST,
        NOTIFICATIONS_PER_SECOND,
    ));
    lua.create_function(move |lua, (title, body): (String, Option<String>)| {
        if title.trim().is_empty() {
            return Err(error_at_caller(lua, "mp.notify needs a title"));
        }
        if limit.borrow_mut().allow(&log_target) {
            let body = body
                .filter(|body| !body.trim().is_empty())
                .map(|body| truncate(&body, MAX_BODY_CHARS));
            emitter.notify(truncate(&title, MAX_TITLE_CHARS), body);
        }
        Ok(())
    })
}

/// `mp.log.debug/info/warn/error(...)`, logged with target `plugin::<id>`. The levels
/// share one rate limit.
fn log_table(lua: &Lua, log_target: &Rc<str>) -> mlua::Result<Table> {
    let log = lua.create_table()?;
    let limit = Rc::new(RefCell::new(RateLimit::new(
        "log messages",
        LOG_BURST,
        LOGS_PER_SECOND,
    )));
    for (name, level) in LOG_LEVELS {
        let target = log_target.clone();
        let limit = limit.clone();
        let log_function = lua.create_function(move |lua, values: Variadic<Value>| {
            if log::log_enabled!(target: &target, level) && limit.borrow_mut().allow(&target) {
                let message = join_values(lua, &values)?;
                log::log!(target: &target, level, "{}", truncate(&message, MAX_LOG_CHARS));
            }
            Ok(())
        })?;
        log.set(name, log_function)?;
    }
    Ok(log)
}

/// Joins values like `print` does (strings as-is, other values via `tostring`),
/// separated by spaces. Strings are cut before they are copied: the message is
/// shortened to [`MAX_LOG_CHARS`] anyway, and a plugin may log a 30 MB string.
fn join_values(lua: &Lua, values: &[Value]) -> mlua::Result<String> {
    // A character takes at most four bytes, so this many bytes always suffice.
    const MAX_LOG_BYTES: usize = MAX_LOG_CHARS * 4 + 4;
    let parts = values
        .iter()
        .map(|value| match value {
            Value::String(text) => {
                let bytes = text.as_bytes();
                let shown = &bytes[..bytes.len().min(MAX_LOG_BYTES)];
                Ok(String::from_utf8_lossy(shown).into_owned())
            }
            other => protected::to_string(lua, other.clone()).map(|text| text.to_string_lossy()),
        })
        .collect::<mlua::Result<Vec<_>>>()?;
    Ok(parts.join(" "))
}

/// A finite number argument, or an error naming the problem.
fn finite(lua: &Lua, value: f64) -> mlua::Result<f64> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(error_at_caller(
            lua,
            format!("expected a finite number, got {value}"),
        ))
    }
}

/// A non-negative number of milliseconds as a duration, clamped to [`MAX_DURATION`].
fn duration_arg(lua: &Lua, ms: f64) -> mlua::Result<Duration> {
    let ms = finite(lua, ms)?;
    if ms < 0.0 {
        return Err(error_at_caller(
            lua,
            format!("expected milliseconds >= 0, got {ms}"),
        ));
    }
    // Clamped before converting: a value such as 1e300 doesn't fit in a `Duration`.
    let seconds = (ms / 1000.0).min(MAX_DURATION.as_secs_f64());
    Ok(Duration::try_from_secs_f64(seconds).unwrap_or(Duration::ZERO))
}
