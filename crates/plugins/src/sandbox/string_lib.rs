//! `string.find`, `match`, `gmatch` and `gsub` on top of [`pattern`], replacing Lua's C
//! versions so the time limit can stop them. Results and errors follow Lua 5.4.
//!
//! [`pattern`]: super::pattern

use mlua::{Function, IntoLuaMulti, Lua, LuaString, MultiValue, Table, Value};

use super::MEMORY_LIMIT;
use super::pattern::{Capture, Matcher, PatternError, is_plain, split_anchor};
use super::protected;
use super::time_limit::{self, TIME_LIMIT_MESSAGE};
use crate::errors::error_at_caller;

/// Replaces the pattern functions in the `string` table (which strings also use for
/// method calls such as `s:find(...)`).
pub(crate) fn install(lua: &Lua, string: &Table) -> mlua::Result<()> {
    string.raw_set(
        "find",
        lua.create_function(|lua, args: FindArgs| find(lua, args, true))?,
    )?;
    string.raw_set(
        "match",
        lua.create_function(|lua, args: FindArgs| find(lua, args, false))?,
    )?;
    string.raw_set("gmatch", lua.create_function(gmatch)?)?;
    string.raw_set("gsub", lua.create_function(gsub)?)?;
    Ok(())
}

type FindArgs = (LuaString, LuaString, Option<Value>, Value);

/// `string.find(s, pattern, init, plain)` and, without `find`, `string.match(s, pattern, init)`.
fn find(
    lua: &Lua,
    (subject, pattern, init, plain): FindArgs,
    find: bool,
) -> mlua::Result<MultiValue> {
    let subject = subject.as_bytes();
    let pattern = pattern.as_bytes();
    let name = if find { "find" } else { "match" };
    let Some(start) = start_offset(lua, init, subject.len(), name)? else {
        return Value::Nil.into_lua_multi(lua);
    };
    if find && (plain.as_boolean().unwrap_or(!plain.is_nil()) || is_plain(&pattern)) {
        return match memchr::memmem::find(&subject[start..], &pattern) {
            Some(offset) => {
                let first = start + offset;
                (position(first + 1), position(first + pattern.len())).into_lua_multi(lua)
            }
            None => Value::Nil.into_lua_multi(lua),
        };
    }
    let (anchored, pattern) = split_anchor(&pattern);
    let out_of_time = time_limit::is_exceeded;
    let mut matcher = Matcher::new(&subject, pattern, &out_of_time);
    let mut start = start;
    loop {
        if let Some(end) = matcher.match_at(start).map_err(|err| lua_error(lua, err))? {
            let captures = matcher
                .captures(start, end, !find)
                .map_err(|err| lua_error(lua, err))?;
            let mut values = Vec::with_capacity(captures.len() + 2);
            if find {
                values.push(Value::Integer(position(start + 1)));
                values.push(Value::Integer(position(end)));
            }
            for capture in captures {
                values.push(capture_value(lua, &subject, capture)?);
            }
            return Ok(MultiValue::from_vec(values));
        }
        if anchored || start >= subject.len() {
            return Value::Nil.into_lua_multi(lua);
        }
        start += 1;
    }
}

/// `string.gmatch(s, pattern, init)`: an iterator over the matches. As in Lua, `^` is
/// not an anchor here.
fn gmatch(
    lua: &Lua,
    (subject, pattern, init): (LuaString, LuaString, Option<Value>),
) -> mlua::Result<Function> {
    let length = subject.as_bytes().len();
    let mut next_start = start_offset(lua, init, length, "gmatch")?.unwrap_or(length + 1);
    let mut last_match = None;
    lua.create_function_mut(move |lua, ()| {
        let subject_bytes = subject.as_bytes();
        let pattern_bytes = pattern.as_bytes();
        let out_of_time = time_limit::is_exceeded;
        let mut matcher = Matcher::new(&subject_bytes, &pattern_bytes, &out_of_time);
        let mut start = next_start;
        while start <= subject_bytes.len() {
            let end = matcher.match_at(start).map_err(|err| lua_error(lua, err))?;
            if let Some(end) = end.filter(|&end| Some(end) != last_match) {
                next_start = end;
                last_match = Some(end);
                let captures = matcher
                    .captures(start, end, true)
                    .map_err(|err| lua_error(lua, err))?;
                return captures
                    .into_iter()
                    .map(|capture| capture_value(lua, &subject_bytes, capture))
                    .collect::<mlua::Result<MultiValue>>();
            }
            start += 1;
        }
        Ok(MultiValue::new())
    })
}

/// `string.gsub(s, pattern, replacement, max)`: returns the new string and the number
/// of substitutions.
fn gsub(
    lua: &Lua,
    (subject, pattern, replacement, max): (LuaString, LuaString, Value, Option<Value>),
) -> mlua::Result<(Value, i64)> {
    let source = subject.as_bytes();
    let pattern_bytes = pattern.as_bytes();
    let replacement = Replacement::new(lua, replacement)?;
    let max_replacements = match max {
        Some(value) if !value.is_nil() => integer_argument(lua, value, 4, "gsub")?,
        _ => position(source.len() + 1),
    };
    let (anchored, pattern) = split_anchor(&pattern_bytes);
    let out_of_time = time_limit::is_exceeded;
    let mut matcher = Matcher::new(&source, pattern, &out_of_time);
    let mut result = Output::new(MEMORY_LIMIT.saturating_sub(lua.used_memory()));
    let mut changed = false;
    let mut replacements = 0;
    let mut s = 0;
    // The subject up to here is in `result`; the bytes after it until `s` are unchanged.
    let mut copied = 0;
    let mut last_match = None;
    while replacements < max_replacements {
        let end = matcher.match_at(s).map_err(|err| lua_error(lua, err))?;
        match end.filter(|&end| Some(end) != last_match) {
            Some(end) => {
                replacements += 1;
                result.push(&source[copied..s])?;
                changed |= replacement.append(lua, &matcher, &source, s..end, &mut result)?;
                s = end;
                copied = end;
                last_match = Some(end);
            }
            None if s < source.len() => s += 1,
            None => break,
        }
        if anchored {
            break;
        }
    }
    if !changed {
        return Ok((Value::String(subject.clone()), replacements));
    }
    result.push(&source[copied..])?;
    Ok((
        Value::String(lua.create_string(&result.bytes)?),
        replacements,
    ))
}

/// The string `gsub` builds. Lua would build it in its own memory, so it may only grow
/// as large as the plugin's remaining memory: a single match repeated by a template
/// such as `%0%0%0...` could otherwise take gigabytes outside the memory limit.
struct Output {
    bytes: Vec<u8>,
    room: usize,
}

impl Output {
    fn new(room: usize) -> Self {
        Self {
            bytes: Vec::new(),
            room,
        }
    }

    fn push(&mut self, bytes: &[u8]) -> mlua::Result<()> {
        if bytes.len() > self.room - self.bytes.len() {
            return Err(mlua::Error::MemoryError("not enough memory".to_owned()));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
}

/// The third argument of `gsub`.
enum Replacement {
    /// A string (or number) where `%0`-`%9` stand for captures.
    Template(LuaString),
    Function(Function),
    Table(Table),
}

impl Replacement {
    fn new(lua: &Lua, value: Value) -> mlua::Result<Self> {
        match value {
            Value::Function(function) => Ok(Self::Function(function)),
            Value::Table(table) => Ok(Self::Table(table)),
            Value::String(_) | Value::Integer(_) | Value::Number(_) => {
                let text = lua
                    .coerce_string(value)?
                    .expect("strings and numbers convert");
                Ok(Self::Template(text))
            }
            other => Err(error_at_caller(
                lua,
                format!(
                    "bad argument #3 to 'gsub' (string/function/table expected, got {})",
                    other.type_name()
                ),
            )),
        }
    }

    /// Appends the replacement for the match `whole` to `result`; returns whether that
    /// changed anything.
    fn append(
        &self,
        lua: &Lua,
        matcher: &Matcher,
        source: &[u8],
        whole: std::ops::Range<usize>,
        result: &mut Output,
    ) -> mlua::Result<bool> {
        let value = match self {
            Self::Template(template) => {
                append_template(lua, matcher, source, whole, &template.as_bytes(), result)?;
                return Ok(true);
            }
            Self::Function(function) => {
                let captures = matcher
                    .captures(whole.start, whole.end, true)
                    .map_err(|err| lua_error(lua, err))?;
                let arguments = captures
                    .into_iter()
                    .map(|capture| capture_value(lua, source, capture))
                    .collect::<mlua::Result<MultiValue>>()?;
                protected::call(lua, function, arguments)?
            }
            Self::Table(table) => {
                let key = matcher
                    .capture(0, whole.start, whole.end)
                    .map_err(|err| lua_error(lua, err))?;
                protected::index(lua, table, capture_value(lua, source, key)?)?
            }
        };
        match value {
            Value::Nil | Value::Boolean(false) => {
                result.push(&source[whole])?;
                Ok(false)
            }
            Value::String(text) => {
                result.push(&text.as_bytes())?;
                Ok(true)
            }
            number @ (Value::Integer(_) | Value::Number(_)) => {
                let text = lua.coerce_string(number)?.expect("numbers convert");
                result.push(&text.as_bytes())?;
                Ok(true)
            }
            other => Err(error_at_caller(
                lua,
                format!("invalid replacement value (a {})", other.type_name()),
            )),
        }
    }
}

/// Expands a replacement string: `%0` is the whole match, `%1`-`%9` captures, `%%` a `%`.
fn append_template(
    lua: &Lua,
    matcher: &Matcher,
    source: &[u8],
    whole: std::ops::Range<usize>,
    template: &[u8],
    result: &mut Output,
) -> mlua::Result<()> {
    let mut rest = template;
    while let Some(percent) = memchr::memchr(b'%', rest) {
        result.push(&rest[..percent])?;
        let item = rest.get(percent + 1);
        rest = rest.get(percent + 2..).unwrap_or_default();
        match item {
            Some(b'%') => result.push(b"%")?,
            Some(b'0') => result.push(&source[whole.clone()])?,
            Some(&digit @ b'1'..=b'9') => {
                let index = usize::from(digit - b'1');
                match matcher
                    .capture(index, whole.start, whole.end)
                    .map_err(|err| lua_error(lua, err))?
                {
                    Capture::Text(range) => result.push(&source[range])?,
                    Capture::Position(position) => {
                        result.push(position.to_string().as_bytes())?;
                    }
                }
            }
            _ => {
                return Err(error_at_caller(
                    lua,
                    "invalid use of '%' in replacement string",
                ));
            }
        }
    }
    result.push(rest)
}

/// The 0-based offset where a search starts, from Lua's 1-based (or negative,
/// counting from the end) `init`; `None` when it is past the end of the subject.
fn start_offset(
    lua: &Lua,
    init: Option<Value>,
    length: usize,
    function: &str,
) -> mlua::Result<Option<usize>> {
    let init = match init {
        Some(value) if !value.is_nil() => integer_argument(lua, value, 3, function)?,
        _ => 1,
    };
    let length_i64 = position(length);
    let one_based = if init > 0 {
        init
    } else if init == 0 || init < -length_i64 {
        1
    } else {
        length_i64 + init + 1
    };
    let offset = usize::try_from(one_based - 1).unwrap_or(usize::MAX);
    Ok((offset <= length).then_some(offset))
}

fn integer_argument(lua: &Lua, value: Value, number: usize, function: &str) -> mlua::Result<i64> {
    let type_name = value.type_name();
    let is_number = matches!(value, Value::Number(_));
    lua.coerce_integer(value)?.ok_or_else(|| {
        let problem = if is_number {
            "number has no integer representation".to_owned()
        } else {
            format!("number expected, got {type_name}")
        };
        error_at_caller(
            lua,
            format!("bad argument #{number} to '{function}' ({problem})"),
        )
    })
}

/// A byte offset or length as a Lua integer.
fn position(offset: usize) -> i64 {
    i64::try_from(offset).unwrap_or(i64::MAX)
}

fn capture_value(lua: &Lua, source: &[u8], capture: Capture) -> mlua::Result<Value> {
    Ok(match capture {
        Capture::Text(range) => Value::String(lua.create_string(&source[range])?),
        Capture::Position(offset) => Value::Integer(position(offset)),
    })
}

fn lua_error(lua: &Lua, err: PatternError) -> mlua::Error {
    match err {
        PatternError::Invalid(message) => error_at_caller(lua, message),
        PatternError::OutOfTime => error_at_caller(lua, TIME_LIMIT_MESSAGE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A match expanded by a template is checked against the room left before it is
    /// appended; otherwise `("x"):rep(1e7):gsub(".+", ("%0"):rep(1000))` takes 10 GB.
    #[test]
    fn output_never_grows_past_its_room() {
        let mut output = Output::new(10);
        output.push(b"12345").unwrap();
        let too_much = output.push(b"678901");
        assert!(matches!(too_much, Err(mlua::Error::MemoryError(_))));
        output.push(b"67890").unwrap();
        assert!(output.push(b"x").is_err());
        assert_eq!(output.bytes, b"1234567890");
    }
}
