//! `mp.http.get(url, [options], callback)`: web requests to the sites in the plugin's
//! `permissions`. The callback runs later on the host thread, with the response or
//! `nil` and an error message.

use std::cell::RefCell;

use mlua::{Lua, Table, Value};

use super::ApiContext;
use crate::errors::error_at_caller;
use crate::http::{self, HttpRequest};
use crate::rate_limit::RateLimit;

/// Web requests a plugin may wait for at once.
pub(crate) const MAX_REQUESTS: usize = 4;
/// Requests a plugin may start at once, and per second after that.
const REQUEST_BURST: u32 = 10;
const REQUESTS_PER_SECOND: u32 = 1;
const MAX_HEADERS: usize = 32;

const USAGE: &str = "use mp.http.get(url, callback) or mp.http.get(url, options, callback)";

pub(super) fn table(lua: &Lua, context: &ApiContext) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    let allowed = context.permissions.network.clone();
    let registry = context.registry.clone();
    let client = context.http.clone();
    let deliver = context.deliver.clone();
    let request_ids = context.request_ids.clone();
    let log_target = context.log_target.clone();
    let limit = RefCell::new(RateLimit::new(
        "web requests",
        REQUEST_BURST,
        REQUESTS_PER_SECOND,
    ));
    let get = lua.create_function(move |lua, (url, second, third): (String, Value, Value)| {
        let (options, callback) = match (second, third) {
            (Value::Function(callback), Value::Nil) | (Value::Nil, Value::Function(callback)) => {
                (None, callback)
            }
            (Value::Table(options), Value::Function(callback)) => (Some(options), callback),
            _ => return Err(error_at_caller(lua, USAGE)),
        };
        if allowed.is_empty() {
            return Err(error_at_caller(
                lua,
                "this plugin may not use the internet: name the sites it needs in \
                 \"permissions\": { \"network\": [...] } in plugin.json",
            ));
        }
        let (query, headers) = match &options {
            Some(options) => read_options(lua, options)?,
            None => (Vec::new(), Vec::new()),
        };
        let url = http::with_query(&url, &query);
        http::check_url(&url, &allowed).map_err(|message| error_at_caller(lua, message))?;
        if registry.borrow().requests.len() >= MAX_REQUESTS {
            return Err(error_at_caller(
                lua,
                format!("a plugin can wait for at most {MAX_REQUESTS} web requests at once"),
            ));
        }
        if !limit.borrow_mut().allow(&log_target) {
            return Err(error_at_caller(
                lua,
                "too many web requests: wait a moment before the next one",
            ));
        }
        let id = request_ids.get() + 1;
        request_ids.set(id);
        registry.borrow_mut().requests.insert(id, callback);
        let request = HttpRequest { url, headers };
        let client = client.clone();
        let deliver = deliver.clone();
        let spawned = std::thread::Builder::new()
            .name("plugin-http".to_owned())
            .spawn(move || deliver(id, client.get(&request)));
        if let Err(err) = spawned {
            registry.borrow_mut().requests.remove(&id);
            return Err(error_at_caller(
                lua,
                format!("could not start the request: {err}"),
            ));
        }
        Ok(())
    })?;
    table.set("get", get)?;
    Ok(table)
}

type Pairs = Vec<(String, String)>;

/// `query` and `headers` from a request's options, as name and value pairs.
fn read_options(lua: &Lua, options: &Table) -> mlua::Result<(Pairs, Pairs)> {
    let mut query = Vec::new();
    let mut headers = Vec::new();
    for pair in options.pairs::<Value, Value>() {
        let (key, value) = pair?;
        let key = match &key {
            Value::String(key) => key.to_string_lossy(),
            _ => String::new(),
        };
        let Value::Table(value) = value else {
            return Err(error_at_caller(
                lua,
                format!(
                    "request option '{key}' must be a table; options are 'query' and 'headers'"
                ),
            ));
        };
        match key.as_str() {
            "query" => query = text_pairs(lua, &value, "query")?,
            "headers" => headers = text_pairs(lua, &value, "headers")?,
            _ => {
                return Err(error_at_caller(
                    lua,
                    format!("unknown request option '{key}'; options are 'query' and 'headers'"),
                ));
            }
        }
    }
    if headers.len() > MAX_HEADERS {
        return Err(error_at_caller(
            lua,
            format!("a request can have at most {MAX_HEADERS} headers"),
        ));
    }
    if let Some((name, _)) = headers
        .iter()
        .find(|(name, _)| http::is_reserved_header(name))
    {
        return Err(error_at_caller(
            lua,
            format!("the '{name}' header is set by the app"),
        ));
    }
    if headers
        .iter()
        .any(|(name, value)| [name, value].iter().any(|text| text.contains(['\r', '\n'])))
    {
        return Err(error_at_caller(lua, "headers can't contain line breaks"));
    }
    Ok((query, headers))
}

/// A table of names and texts (or numbers), sorted by name so requests are repeatable.
fn text_pairs(lua: &Lua, table: &Table, what: &str) -> mlua::Result<Pairs> {
    let mut pairs = Vec::new();
    for pair in table.pairs::<Value, Value>() {
        let (name, value) = pair?;
        let (Value::String(name), value) = (name, value) else {
            return Err(error_at_caller(lua, format!("'{what}' names must be text")));
        };
        let value = match value {
            Value::String(text) => text.to_string_lossy(),
            Value::Integer(number) => number.to_string(),
            Value::Number(number) => number.to_string(),
            _ => {
                return Err(error_at_caller(
                    lua,
                    format!("'{what}' values must be text or numbers"),
                ));
            }
        };
        pairs.push((name.to_string_lossy(), value));
    }
    pairs.sort();
    Ok(pairs)
}

/// The arguments a request's callback receives: a response table, or `nil` and an
/// error message.
pub(crate) fn callback_args(
    lua: &Lua,
    result: Result<http::HttpResponse, String>,
) -> mlua::Result<(Value, Option<String>)> {
    match result {
        Ok(response) => {
            let table = lua.create_table_with_capacity(0, 3)?;
            table.set("status", response.status)?;
            table.set("ok", (200..300).contains(&response.status))?;
            table.set("body", lua.create_string(&response.body)?)?;
            Ok((Value::Table(table), None))
        }
        Err(message) => Ok((Value::Nil, Some(message))),
    }
}
