//! Turning Lua errors into short messages for the plugin manager. The full error,
//! with its stack traceback, still goes to the log.

use std::fmt::Display;

use mlua::debug::Debug;
use mlua::{Error, Lua};

use crate::sandbox::MEMORY_LIMIT;
use crate::text::truncate;

/// How far up the Lua stack to look for plugin code.
const MAX_FRAMES: usize = 32;

/// How Lua names the sandbox's own Lua code (see `sandbox::create`). Errors that Lua
/// locates there mean nothing to plugin authors, so they are moved to the plugin's code.
pub(crate) const SANDBOX_SOURCE: &str = "[sandbox]";

/// Longest error message shown in the plugin manager.
const MAX_MESSAGE_CHARS: usize = 1000;

/// A runtime error prefixed with the plugin line that called into Rust
/// (`main.lua:12: ...`), like Lua's own `error()`, so plugin authors can see where
/// they went wrong.
pub(crate) fn error_at_caller(lua: &Lua, message: impl Display) -> Error {
    match caller_location(lua) {
        Some(location) => Error::RuntimeError(format!("{location}: {message}")),
        None => Error::RuntimeError(message.to_string()),
    }
}

/// `source:line` of the innermost plugin code on the Lua stack.
fn caller_location(lua: &Lua) -> Option<String> {
    (1..MAX_FRAMES)
        .map_while(|level| lua.inspect_stack(level, location_of))
        .flatten()
        .next()
}

/// `source:line` of a stack frame, or `None` for C functions and the sandbox's own
/// helpers (whose names start with `[`).
fn location_of(debug: &Debug) -> Option<String> {
    let line = debug.current_line()?;
    let source = debug.source().short_src?;
    (!source.starts_with('[')).then(|| format!("{source}:{line}"))
}

/// One-line description of `err`, including where in the plugin it happened when known.
/// Plugins choose their error messages, so it is cut to a length the app can show.
pub(crate) fn describe(err: &Error) -> String {
    truncate(&message_of(err), MAX_MESSAGE_CHARS)
}

fn message_of(err: &Error) -> String {
    match err {
        Error::CallbackError { traceback, cause } => {
            let message = message_of(cause);
            match location_from_traceback(traceback) {
                Some(location) if !has_location(&message) => format!("{location}: {message}"),
                _ => message,
            }
        }
        Error::RuntimeError(text) | Error::SyntaxError { message: text, .. } => {
            let (message, traceback) = split_traceback(text);
            relocated(message, traceback.and_then(location_from_traceback))
        }
        Error::MemoryError(_) => format!(
            "ran out of memory (plugins may use up to {} MiB)",
            MEMORY_LIMIT / (1024 * 1024)
        ),
        Error::BadArgument { pos, cause, .. } => {
            format!("bad argument #{pos}: {}", message_of(cause))
        }
        Error::WithContext { context, cause } => format!("{context}: {}", message_of(cause)),
        Error::ExternalError(inner) => inner.to_string(),
        other => split_traceback(&other.to_string()).0.to_owned(),
    }
}

/// A message that Lua located in the sandbox's own code (`[sandbox]:76: bad argument
/// ...`, raised by a standard function the sandbox wraps), located at the plugin code
/// on the stack instead. `None` for every other message.
pub(crate) fn relocated_to_caller(lua: &Lua, message: &[u8]) -> Option<String> {
    if !message.starts_with(SANDBOX_SOURCE.as_bytes()) {
        return None;
    }
    let message = String::from_utf8_lossy(message);
    strip_sandbox_location(&message)?;
    Some(relocated(&message, caller_location(lua).as_deref()))
}

/// `message`, with a location in the sandbox's code replaced by `location` (or
/// dropped when the plugin's location is unknown).
fn relocated(message: &str, location: Option<&str>) -> String {
    match (strip_sandbox_location(message), location) {
        (Some(rest), Some(location)) => format!("{location}: {rest}"),
        (Some(rest), None) => rest.to_owned(),
        (None, _) => message.to_owned(),
    }
}

/// The rest of `message` after a leading `[sandbox]:<line>: `.
fn strip_sandbox_location(message: &str) -> Option<&str> {
    let after_source = message.strip_prefix(SANDBOX_SOURCE)?.strip_prefix(':')?;
    let (line, rest) = after_source.split_once(": ")?;
    (!line.is_empty() && line.bytes().all(|b| b.is_ascii_digit())).then_some(rest)
}

/// The first plugin source location in a Lua traceback, skipping C functions and
/// the sandbox's own helpers (whose names start with `[`).
fn location_from_traceback(traceback: &str) -> Option<&str> {
    traceback
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('['))
        .find_map(|line| line.split_once(": in ").map(|(location, _)| location))
}

/// Whether `message` already starts with a `source:line: ` location.
fn has_location(message: &str) -> bool {
    let Some((head, _)) = message.split_once(": ") else {
        return false;
    };
    let Some((source, line)) = head.rsplit_once(':') else {
        return false;
    };
    !source.is_empty()
        && !source.contains(' ')
        && !line.is_empty()
        && line.bytes().all(|b| b.is_ascii_digit())
}

/// Splits Lua's `message\nstack traceback:\n...` into the message and the traceback.
fn split_traceback(text: &str) -> (&str, Option<&str>) {
    match text.split_once("\nstack traceback:") {
        Some((message, traceback)) => (message.trim(), Some(traceback)),
        None => (text.trim(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(lua: &Lua, code: &str) -> Error {
        lua.load(code).set_name("=main.lua").exec().unwrap_err()
    }

    #[test]
    fn runtime_errors_lose_their_traceback() {
        let err = Error::RuntimeError("main.lua:3: boom\nstack traceback:\n\t[C]: in ?".into());
        assert_eq!(describe(&err), "main.lua:3: boom");
        let lua = Lua::new();
        assert_eq!(
            describe(&run(&lua, "local x = nil\nreturn x.field")),
            "main.lua:2: attempt to index a nil value (local 'x')"
        );
    }

    #[test]
    fn callback_errors_gain_a_location() {
        let lua = Lua::new();
        let fail = lua
            .create_function(|_, ()| -> mlua::Result<()> {
                Err(Error::external("the app said no"))
            })
            .unwrap();
        lua.globals().set("fail", fail).unwrap();
        assert_eq!(
            describe(&run(&lua, "local x = 1\nfail()")),
            "main.lua:2: the app said no"
        );
    }

    #[test]
    fn located_callback_errors_keep_their_location() {
        let lua = Lua::new();
        let fail = lua
            .create_function(|lua, ()| -> mlua::Result<()> { Err(error_at_caller(lua, "nope")) })
            .unwrap();
        lua.globals().set("fail", fail).unwrap();
        assert_eq!(describe(&run(&lua, "\n\nfail()")), "main.lua:3: nope");
    }

    #[test]
    fn locations_are_recognised() {
        assert!(has_location("main.lua:3: boom"));
        assert!(has_location("lib/util.lua:12: took longer than 250 ms"));
        assert!(!has_location("bad argument #1: nope"));
        assert!(!has_location("unknown event 'x'; expected one of: a, b"));
        assert!(!has_location("ran out of memory"));
        assert!(!has_location(
            "my plugin:3: spaces mean this isn't a location"
        ));
    }

    #[test]
    fn sandbox_locations_are_replaced_by_the_plugins() {
        let traceback = "[sandbox]:76: bad argument #1 to 'setmetatable'\nstack traceback:\n\t\
                         [C]: in function 'setmetatable'\n\t[sandbox]:76: in function \
                         'setmetatable'\n\tmain.lua:9: in main chunk";
        assert_eq!(
            describe(&Error::RuntimeError(traceback.into())),
            "main.lua:9: bad argument #1 to 'setmetatable'"
        );
        assert_eq!(
            describe(&Error::RuntimeError("[sandbox]:3: no plugin frame".into())),
            "no plugin frame"
        );
        for untouched in [
            "[sandbox]: no line",
            "[sandbox]:x: not a line",
            "main.lua:1: ok",
        ] {
            assert_eq!(relocated(untouched, Some("main.lua:2")), untouched);
        }
    }

    #[test]
    fn memory_errors_mention_the_limit() {
        let err = Error::MemoryError("not enough memory".into());
        assert_eq!(
            describe(&err),
            "ran out of memory (plugins may use up to 32 MiB)"
        );
    }
}
