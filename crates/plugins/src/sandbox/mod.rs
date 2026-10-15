//! The restricted Lua environment every plugin runs in.
//!
//! Plugins get the pure standard libraries (`table`, `string`, `math`, `utf8`,
//! `coroutine`) and a clock-only `os`. Nothing can touch files, processes or native
//! code: `io`, `debug`, `package`, `dofile`, `loadfile` and `string.dump` are gone,
//! `load` only accepts source text, and `require` only loads `.lua` files from the
//! plugin's own folder.
//!
//! Each state has a memory limit, and plugin code is stopped once it runs past its
//! time limit (see [`time_limit`]). Standard functions that would otherwise escape the
//! time limit are replaced: pattern matching by a Rust port ([`pattern`]), and the rest
//! by the Lua prelude in `prelude.lua`. The plugin thread runs in the "C" locale
//! ([`locale`]), which keeps string comparisons fast and results the same everywhere.

mod locale;
mod pattern;
pub(crate) mod protected;
mod require;
mod string_lib;
pub(crate) mod time_limit;

use mlua::chunk::ChunkMode;
use mlua::{Lua, LuaOptions, StdLib, Table, Value};

use crate::errors::{SANDBOX_SOURCE, describe, relocated_to_caller};

pub(crate) use locale::ThreadLocale;
pub(crate) use require::Modules;
pub use time_limit::CALLBACK_TIME_LIMIT;

/// Most memory one plugin's Lua state may use.
pub const MEMORY_LIMIT: usize = 32 * 1024 * 1024;

/// Largest Lua file (entry script or module) a plugin may load.
pub(crate) const MAX_SCRIPT_BYTES: u64 = 4 * 1024 * 1024;

const PRELUDE: &str = include_str!("prelude.lua");

const REMOVED_GLOBALS: [&str; 3] = ["dofile", "loadfile", "warn"];
const OS_FUNCTIONS: [&str; 4] = ["time", "clock", "date", "difftime"];

/// Registry field holding the loaded libraries, `package.loaded` in stock Lua.
const LOADED_LIBRARIES: &str = "_LOADED";

fn safe_libraries() -> StdLib {
    StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8 | StdLib::COROUTINE | StdLib::OS
}

/// Creates a sandboxed Lua state for a plugin whose modules are `modules`.
pub(crate) fn create(modules: Modules) -> mlua::Result<Lua> {
    let lua = Lua::new_with(safe_libraries(), LuaOptions::default())?;
    let globals = lua.globals();
    for name in REMOVED_GLOBALS {
        globals.raw_set(name, Value::Nil)?;
    }
    let string: Table = globals.get("string")?;
    string.raw_set("dump", Value::Nil)?;
    string_lib::install(&lua, &string)?;
    restrict_os(&lua, &globals)?;
    require::install(&lua, &globals, modules)?;
    run_prelude(&lua)?;
    freeze_function_names(&lua)?;
    time_limit::install(&lua)?;
    lua.set_memory_limit(MEMORY_LIMIT)?;
    Ok(lua)
}

fn restrict_os(lua: &Lua, globals: &Table) -> mlua::Result<()> {
    let full_os: Table = globals.get("os")?;
    let os = lua.create_table()?;
    for name in OS_FUNCTIONS {
        os.raw_set(name, full_os.raw_get::<Value>(name)?)?;
    }
    globals.raw_set("os", os)
}

/// Runs `prelude.lua`, handing it the Rust helpers it needs.
fn run_prelude(lua: &Lua) -> mlua::Result<()> {
    let (check_time, time_is_up) = time_limit::prelude_functions(lua)?;
    // Errors raised by Rust functions reach Lua as error objects; plugins get them
    // as the same short messages the plugin manager shows.
    let readable = lua.create_function(|lua, error: Value| {
        let message = match &error {
            Value::Error(error) => describe(error),
            Value::String(text) => match relocated_to_caller(lua, &text.as_bytes()) {
                Some(message) => message,
                None => return Ok(error),
            },
            _ => return Ok(error),
        };
        lua.create_string(message).map(Value::String)
    })?;
    lua.load(PRELUDE)
        .set_name(format!("={SANDBOX_SOURCE}"))
        .set_mode(ChunkMode::Text)
        .call((check_time, time_is_up, readable))
}

/// Lua names functions in error messages and tracebacks (`bad argument #1 to
/// 'string.rep'`) by searching every loaded library, the global table included, and
/// mlua builds a traceback for every error a Rust function raises. Plugins can grow
/// those tables to millions of entries, which made each error take milliseconds and an
/// error passing through many nested library calls take seconds. Copies of the
/// libraries as plugins first see them name the same functions in a few hundred steps.
fn freeze_function_names(lua: &Lua) -> mlua::Result<()> {
    let loaded: Table = lua.named_registry_value(LOADED_LIBRARIES)?;
    let frozen = lua.create_table()?;
    for entry in loaded.pairs::<Value, Value>() {
        if let (name, Value::Table(library)) = entry? {
            frozen.raw_set(name, functions_of(lua, library)?)?;
        }
    }
    lua.set_named_registry_value(LOADED_LIBRARIES, frozen)
}

/// A new table with the function fields of `table`.
fn functions_of(lua: &Lua, table: Table) -> mlua::Result<Table> {
    let functions = lua.create_table()?;
    for entry in table.pairs::<Value, Value>() {
        let (key, value) = entry?;
        if value.is_function() {
            functions.raw_set(key, value)?;
        }
    }
    Ok(functions)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use super::*;

    struct Sandbox {
        _root: tempfile::TempDir,
        dir: PathBuf,
        lua: Lua,
    }

    fn sandbox() -> Sandbox {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("plugin");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(root.path().join("secret.lua"), "return 'secret'").unwrap();
        let lua = create(Modules::Folder(dir.clone())).unwrap();
        Sandbox {
            _root: root,
            dir,
            lua,
        }
    }

    impl Sandbox {
        /// Runs `code` as one time-limited plugin call.
        fn eval<R: mlua::FromLuaMulti>(&self, code: &str) -> mlua::Result<R> {
            let _limit = time_limit::start();
            self.lua.load(code).set_name("=test.lua").eval()
        }

        /// Runs `code`, which must be stopped by the time limit, and returns how long
        /// that took.
        fn assert_stopped(&self, code: &str) -> Duration {
            let started = Instant::now();
            let err = self.eval::<()>(code).unwrap_err();
            assert!(
                err.to_string().contains("took longer than 250 ms"),
                "{code}: {err}"
            );
            let elapsed = started.elapsed();
            assert!(elapsed < CALLBACK_TIME_LIMIT * 4, "{code} took {elapsed:?}");
            elapsed
        }
    }

    #[test]
    fn dangerous_globals_are_gone() {
        let sandbox = sandbox();
        for global in [
            "io",
            "debug",
            "package",
            "dofile",
            "loadfile",
            "warn",
            "string.dump",
            "os.execute",
            "os.getenv",
            "os.remove",
            "os.rename",
            "os.exit",
            "os.tmpname",
        ] {
            let value: Value = sandbox.eval(&format!("return {global}")).unwrap();
            assert!(value.is_nil(), "{global} should be unavailable");
        }
        let still_there: bool = sandbox
            .eval("return type(os.time()) == 'number' and type(os.clock()) == 'number' and type(os.date('%Y')) == 'string' and string.format('%d', 3) == '3' and utf8.char(72) == 'H' and math.floor(2.5) == 2 and coroutine.wrap(function() return true end)()")
            .unwrap();
        assert!(still_there);
        let no_dump: bool = sandbox
            .eval("return ('').dump == nil and getmetatable('').__index.dump == nil")
            .unwrap();
        assert!(no_dump, "string.dump is gone from string methods too");
    }

    #[test]
    fn binary_chunks_are_refused() {
        let sandbox = sandbox();
        let bytecode = sandbox
            .lua
            .load("return 1")
            .into_function()
            .unwrap()
            .dump(false);
        sandbox
            .lua
            .globals()
            .set("bytecode", sandbox.lua.create_string(&bytecode).unwrap())
            .unwrap();
        let (function, message): (Value, String) =
            sandbox.eval("return load(bytecode, 'x', 'b')").unwrap();
        assert!(function.is_nil());
        assert!(message.contains("binary"), "{message}");
        let text: i64 = sandbox.eval("return load('return 41 + 1')()").unwrap();
        assert_eq!(text, 42);
        let isolated: bool = sandbox
            .eval("local f = load('return x', 'chunk', 't', { x = true }) return f()")
            .unwrap();
        assert!(isolated);
        let no_globals: bool = sandbox
            .eval("return select(2, pcall(load('return print', 'chunk', 't', nil))) ~= nil")
            .unwrap();
        assert!(no_globals, "an explicit nil environment has no globals");
    }

    #[test]
    fn require_stays_inside_the_plugin_folder() {
        let sandbox = sandbox();
        std::fs::create_dir_all(sandbox.dir.join("lib")).unwrap();
        std::fs::write(
            sandbox.dir.join("lib/util.lua"),
            "local name = ... loads = (loads or 0) + 1 return { name = name }",
        )
        .unwrap();
        let name: String = sandbox.eval("return require('lib.util').name").unwrap();
        assert_eq!(name, "lib.util");
        let cached: i64 = sandbox
            .eval("require('lib.util') require('lib.util') return loads")
            .unwrap();
        assert_eq!(cached, 1);

        for name in [
            "..secret",
            "../secret",
            "/etc/passwd",
            "lib..util",
            "",
            "lib/util",
        ] {
            let err = sandbox
                .eval::<Value>(&format!("return require({name:?})"))
                .unwrap_err();
            assert!(
                err.to_string().contains("invalid module name"),
                "{name}: {err}"
            );
        }
        let err = sandbox
            .eval::<Value>("return require('missing')")
            .unwrap_err();
        assert!(err.to_string().contains("not found"), "{err}");
        let err = sandbox.eval::<Value>("return require('lib')").unwrap_err();
        assert!(err.to_string().contains("not found"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn require_refuses_symlinks_out_of_the_folder_and_special_files() {
        let sandbox = sandbox();
        let secret = sandbox.dir.parent().unwrap().join("secret.lua");
        std::os::unix::fs::symlink(secret, sandbox.dir.join("link.lua")).unwrap();
        let err = sandbox.eval::<Value>("return require('link')").unwrap_err();
        assert!(
            err.to_string().contains("outside the plugin folder"),
            "{err}"
        );
        std::fs::create_dir(sandbox.dir.join("folder.lua")).unwrap();
        let err = sandbox
            .eval::<Value>("return require('folder')")
            .unwrap_err();
        assert!(err.to_string().contains("is not a file"), "{err}");
    }

    #[test]
    fn caught_errors_are_readable_strings() {
        let sandbox = sandbox();
        let (kind, message): (String, String) = sandbox
            .eval("local ok, err = pcall(require, 'missing') return type(err), err")
            .unwrap();
        assert_eq!(kind, "string");
        assert!(
            message.starts_with("test.lua:1: module 'missing' not found"),
            "{message}"
        );

        let located: String = sandbox
            .eval("local ok, err = pcall(function()\n  require('missing')\nend) return err")
            .unwrap();
        assert!(located.starts_with("test.lua:2: module"), "{located}");

        let handled: String = sandbox
            .eval("return select(2, xpcall(require, function(e) return type(e) end, 'x'))")
            .unwrap();
        assert_eq!(handled, "string");

        let (resumed, plain): (String, String) = sandbox
            .eval(
                "local co = coroutine.create(function() require('missing') end)
                 local _, err = coroutine.resume(co)
                 return type(err), select(2, pcall(error, 'plain'))",
            )
            .unwrap();
        assert_eq!(resumed, "string");
        assert_eq!(plain, "plain", "Lua's own errors pass through untouched");

        let wrapped: String = sandbox
            .eval(
                "local f = coroutine.wrap(function() error('inside') end)
                 return select(2, pcall(f))",
            )
            .unwrap();
        assert!(wrapped.ends_with("inside"), "{wrapped}");
    }

    /// Errors raised by the standard functions the prelude wraps name the plugin's line,
    /// not the prelude's (`[sandbox]:76: bad argument ...`).
    #[test]
    fn errors_from_wrapped_functions_name_the_plugin_line() {
        let sandbox = sandbox();
        for (code, expected) in [
            (
                "local ok, err = pcall(setmetatable, 1, {}) return err",
                "test.lua:1: bad argument #1 to 'setmetatable' (table expected, got number)",
            ),
            (
                "local ok, err = pcall(coroutine.close, 1) return err",
                "test.lua:1: bad argument #1 to 'close' (thread expected, got number)",
            ),
            (
                "local ok, err = pcall(function()\n  table.sort({ {}, {} })\nend) return err",
                "test.lua:1: attempt to compare two table values",
            ),
            (
                "return select(2, xpcall(string.rep, function(e) return e end))",
                "test.lua:1: bad argument #1 to 'rep' (string expected, got nil)",
            ),
        ] {
            let message: String = sandbox.eval(code).unwrap();
            assert_eq!(message, expected, "{code}");
        }

        let err = sandbox
            .eval::<()>("\n\nlocal list = { {}, {} }\ntable.sort(list)")
            .unwrap_err();
        assert_eq!(
            describe(&err),
            "test.lua:4: attempt to compare two table values"
        );
    }

    #[test]
    fn gc_finalizers_are_refused() {
        let sandbox = sandbox();
        let err = sandbox
            .eval::<Value>("return setmetatable({}, { __gc = function() end })")
            .unwrap_err();
        assert!(err.to_string().contains("__gc"), "{err}");
        let ok: bool = sandbox
            .eval("return getmetatable(setmetatable({}, { __index = {} })) ~= nil")
            .unwrap();
        assert!(ok);
    }

    #[test]
    fn runaway_code_is_stopped_even_inside_pcall() {
        let sandbox = sandbox();
        for code in [
            "while true do end",
            "while true do pcall(function() while true do end end) end",
            "while true do xpcall(function() while true do end end, function(e) return e end) end",
            "while true do coroutine.resume(coroutine.create(function() while true do end end)) end",
            "while true do pcall(coroutine.wrap(function() while true do end end)) end",
            "while true do load(function() while true do end end) end",
            "local function f() return f() end f()",
        ] {
            sandbox.assert_stopped(code);
        }
        let fine: i64 = sandbox.eval("return 1 + 1").unwrap();
        assert_eq!(fine, 2, "the state stays usable");
    }

    /// Lua runs these with hooks disabled after the time-limit error, so the prelude has
    /// to stop them before they start; otherwise they would run forever.
    #[test]
    fn cleanup_code_cannot_outlive_the_time_limit() {
        let sandbox = sandbox();
        let forever = "setmetatable({}, { __close = function() while true do end end })";
        for code in [
            format!("local x <close> = {forever}\nwhile true do end"),
            format!("local function f() local x <close> = {forever} while true do end end f()"),
            format!(
                "local function f() local x <close> = {forever} while true do end end pcall(f)"
            ),
            format!("for _ in function() return true end, nil, nil, {forever} do end"),
            "xpcall(function() while true do end end, function() while true do end end)".to_owned(),
            "xpcall(function() error('x') end, function() while true do end end)".to_owned(),
        ] {
            sandbox.assert_stopped(&code);
        }

        // A coroutine stopped by the time limit is never closed, so its cleanup can't
        // run later either, even with a fresh time limit.
        sandbox
            .lua
            .load(format!(
                "stuck = coroutine.create(function() local x <close> = {forever} while true do end end)"
            ))
            .exec()
            .unwrap();
        sandbox.assert_stopped("coroutine.resume(stuck)");
        let (closed, message): (bool, String) =
            sandbox.eval("return coroutine.close(stuck)").unwrap();
        assert!(!closed);
        assert!(message.contains("stopped by the time limit"), "{message}");
        sandbox.assert_stopped(&format!(
            "coroutine.wrap(function() local x <close> = {forever} while true do end end)()"
        ));
    }

    /// Stopping the plugin builds the time-limit error once and re-raises it for each
    /// pending cleanup handler, so a plugin with many of them (every `__close` handler
    /// Lua runs while unwinding, each itself stopped) is still stopped within budget
    /// rather than rebuilding the error and its traceback every time.
    #[test]
    fn mass_cleanup_is_stopped_in_time() {
        let sandbox = sandbox();
        sandbox.assert_stopped(
            "local closer = setmetatable({}, { __close = function() while true do end end })
             local function recurse(n)
               local a <close> = closer
               local b <close> = closer
               if n > 0 then recurse(n - 1) else while true do end end
             end
             recurse(180)",
        );
    }

    /// Naming a function in an error's traceback doesn't search the global table, which
    /// a plugin can grow to hundreds of thousands of entries: each error a Rust function
    /// raises would then take milliseconds, and a handler catching a batch of them
    /// would overrun the time limit.
    #[test]
    fn errors_do_not_scan_plugin_globals() {
        let sandbox = sandbox();
        sandbox
            .lua
            .load("for i = 1, 2e5 do _G['g' .. i] = i end")
            .exec()
            .unwrap();
        // `string.find` with a malformed pattern raises from Rust, so mlua builds a
        // traceback. Catching 2000 of them must stay in budget.
        let caught: i64 = sandbox
            .eval(
                "local n = 0
                 for _ = 1, 2000 do
                   if not pcall(string.find, 'a', '[') then n = n + 1 end
                 end
                 return n",
            )
            .unwrap();
        assert_eq!(caught, 2000);
    }

    /// The host's message handler describes the time-limit error, a string, with hooks
    /// disabled, so plugins must not be able to give strings a `__tostring` it would run.
    #[test]
    fn strings_cannot_be_given_metamethods() {
        let sandbox = sandbox();
        sandbox.assert_stopped(
            "getmetatable('').__tostring = function() while true do end end
             while true do end",
        );
        let unchanged: bool = sandbox
            .eval(
                "local metatable = getmetatable('')
                 metatable.__index = {} metatable.__add = nil
                 return ('x'):upper() == 'X' and '1' + 1 == 2 and tostring('y') == 'y'
                   and getmetatable('') == metatable",
            )
            .unwrap();
        assert!(unchanged);
    }

    /// These loop in C, out of the time limit's reach, and would hang the plugin host for
    /// hours if the sandbox didn't stop them.
    #[test]
    fn long_library_calls_are_stopped() {
        let sandbox = sandbox();
        for code in [
            "table.move({}, 1, 1e14, 2)",
            "table.move({}, 2, 1e14, 1)",
            "table.insert(setmetatable({}, { __len = function() return 1e14 end }), 1, 0)",
            "table.remove(setmetatable({}, { __len = function() return 1e14 end }), 1)",
            "string.find(string.rep('a', 5000), '.-.-.-.-.-.-.-b')",
            "(string.rep(' ', 3e5) .. 'x'):match('^(.-)%s*$')",
            "string.rep('a', 1e5):gsub('.-b', '')",
            "for _ in string.rep('a', 1e5):gmatch('.-b') do end",
        ] {
            sandbox.assert_stopped(code);
        }
        // A plain search is linear, so it simply finishes.
        let found: Value = sandbox
            .eval("local s = string.rep('a', 2e6) return s:find(string.rep('a', 1e6) .. 'b', 1, true)")
            .unwrap();
        assert!(found.is_nil());
    }

    /// The time is checked between library calls too: a loop fits a few hundred calls,
    /// each taking milliseconds here, into the thousand instructions between Lua's own
    /// checks, which would run for seconds past the limit.
    #[test]
    fn loops_of_slow_library_calls_are_stopped_between_calls() {
        let sandbox = sandbox();
        for code in [
            "local format = string.rep('x', 8e6) while true do string.packsize(format) end",
            "local format = string.rep('%c', 1e5) while true do os.date(format) end",
            "local format = string.rep('%%', 4e6) while true do string.format(format) end",
        ] {
            let elapsed = sandbox.assert_stopped(code);
            assert!(elapsed < CALLBACK_TIME_LIMIT * 2, "{code} took {elapsed:?}");
        }
    }

    #[test]
    fn table_functions_still_work() {
        let sandbox = sandbox();
        let result: String = sandbox
            .eval(
                "local t = { 1, 2, 3 }
                 table.insert(t, 4)
                 table.insert(t, 1, 0)
                 local removed = table.remove(t, 2)
                 local last = table.remove(t)
                 local big = {}
                 for i = 1, 10000 do big[i] = i end
                 table.move(big, 1, 9999, 2)
                 table.move(big, 3, 10000, 1)
                 local copy = table.move(big, 1, 10000, 1, {})
                 table.insert(big, 5000, 'x')
                 return table.concat(t, ',') .. '|' .. removed .. '|' .. last .. '|'
                   .. big[1] .. ',' .. big[9998] .. ',' .. big[5000] .. ',' .. #big .. '|' .. #copy",
            )
            .unwrap();
        assert_eq!(result, "0,2,3|1|4|2,9998,x,10001|10000");
        for (code, message) in [
            ("table.insert(nil, 1)", "table expected, got nil"),
            ("table.insert({}, 5, 1)", "position out of bounds"),
            ("table.insert({}, 1, 2, 3)", "wrong number of arguments"),
            ("table.remove({}, 5)", "position out of bounds"),
            (
                "table.move({}, 1, math.maxinteger, 2)",
                "destination wrap around",
            ),
            (
                "table.move({}, -1, math.maxinteger, 1)",
                "too many elements",
            ),
        ] {
            let err = sandbox.eval::<()>(code).unwrap_err();
            assert!(err.to_string().contains(message), "{code}: {err}");
        }
    }

    #[test]
    fn string_functions_match_lua() {
        let sandbox = sandbox();
        let result: String = sandbox
            .eval(
                r#"local parts = {}
                 local function add(...) for i = 1, select('#', ...) do parts[#parts + 1] = tostring((select(i, ...))) end end
                 add(("hello world"):find("o w"))
                 add(("hello world"):find("o", 6))
                 add(("a.b"):find(".", 1, true))
                 add(("key = value"):match("(%w+)%s*=%s*(%w+)"))
                 add(("abc"):match("()b()"))
                 add(string.match("  trim  ", "^%s*(.-)%s*$"))
                 add(("hello"):gsub("l", "L"))
                 add(("hello world"):gsub("(%w+)", "<%1>"))
                 add(("abc"):gsub("%w", { a = "1", b = false }))
                 add(("abc"):gsub("%w", function(c) if c == "b" then return 2 end end))
                 add(("abc"):gsub("", "-"))
                 add(("x = 1, y = 2"):gsub("(%w+) = (%w+)", "%2 = %1", 1))
                 for k, v in ("a=1, b=2"):gmatch("(%w+)=(%w+)") do add(k, v) end
                 for word in ("one two"):gmatch("%a+", 4) do add(word) end
                 add(("x"):find("y"))
                 add(("abc"):find("b", -1))
                 return table.concat(parts, " ")"#,
            )
            .unwrap();
        assert_eq!(
            result,
            "5 7 8 8 2 2 key value 2 3 trim heLLo 2 <hello> <world> 2 1bc 3 a2c 3 \
             -a-b-c- 4 1 = x, y = 2 1 a 1 b 2 two nil nil"
        );
        for (code, message) in [
            ("string.find('a', '[a')", "malformed pattern (missing ']')"),
            ("string.gsub('a', 'a', '%2')", "invalid capture index %2"),
            (
                "string.gsub('a', 'a', '%x')",
                "invalid use of '%' in replacement string",
            ),
            (
                "string.gsub('a', 'a', {a = {}})",
                "invalid replacement value (a table)",
            ),
            (
                "string.gsub('a', 'a')",
                "string/function/table expected, got nil",
            ),
            (
                "string.find('a', 'a', 1.5)",
                "number has no integer representation",
            ),
        ] {
            let err = sandbox.eval::<()>(code).unwrap_err();
            assert!(err.to_string().contains(message), "{code}: {err}");
            assert!(err.to_string().contains("test.lua:1:"), "{code}: {err}");
        }
    }

    #[test]
    fn memory_is_limited() {
        let sandbox = sandbox();
        for code in [
            "return string.rep('x', 64 * 1024 * 1024)",
            "local s = 'x' while true do s = s .. s end",
            "local t = {} for i = 1, 1e9 do t[i] = i end",
            "return string.rep('x', 1e6):gsub('x', string.rep('y', 100))",
            "return string.rep('x', 1e6):gsub('.+', string.rep('%0', 1000))",
        ] {
            let err = sandbox.eval::<Value>(code).unwrap_err();
            assert!(
                describe(&err).contains("ran out of memory"),
                "{code}: {err}"
            );
        }
        assert!(sandbox.lua.used_memory() < MEMORY_LIMIT);
    }

    /// Runs `code` in the sandbox and in a stock Lua state, whose C string and table
    /// functions the sandbox replaces; both must return the same text.
    fn assert_same_as_stock_lua(code: &str) {
        let expected: String = Lua::new().load(code).set_name("=test.lua").eval().unwrap();
        let actual: String = sandbox().eval(code).unwrap();
        assert_eq!(actual, expected);
    }

    /// Lua code that calls `f(...)` for each argument list in `cases` and returns every
    /// result, or the error without its location, as one line per case.
    fn report_calls(f: &str, cases: &str) -> String {
        format!(
            r#"local out = {{}}
            for _, args in ipairs({{ {cases} }}) do
              local results = table.pack(pcall({f}, table.unpack(args, 1, args.n or #args)))
              if not results[1] then
                local message = tostring(results[2])
                local _, location_end = string.find(message, "^test%.lua:%d+: ")
                -- Stock Lua names functions called through pcall by their global name.
                message = message:sub((location_end or 0) + 1):gsub("'%a+%.", "'")
                results[2] = "error: " .. message:gsub("got no value", "got nil")
              end
              local line = {{}}
              for i = 2, results.n do line[#line + 1] = tostring(results[i]) end
              out[#out + 1] = table.concat(line, ", ")
            end
            return table.concat(out, "\n")"#
        )
    }

    const PATTERN_CASES: &str = r#"
        {"hello world", "o w"}, {"hello", "l+"}, {"hello", "^h(.-)o$"}, {"hello", "l*"},
        {"key = value", "(%w+)%s*=%s*(%w+)"}, {"f(a(b)c)d", "%b()"}, {"f(a(b)c", "%b()"},
        {"THE (quick) fox", "%f[%a]%a+"}, {"abcabc", "(abc)%1"}, {"abc", "()b()"},
        {"a]b", "[]]"}, {"x-y", "[%-]"}, {"a\vb", "%s"}, {"abc", "^b"}, {"", "$"},
        {"", ""}, {"abc", ""}, {"abc", ".-"}, {"aaa", "a-b"}, {"aaab", "a-b"},
        {"[x]", "%[(.)%]"}, {"2024-10-08", "(%d+)-(%d+)-(%d+)"}, {"a.b.c", "%."},
        {"äöü", "[\128-\255]+"}, {"tab\there", "%c"}, {"x=1,y=2", "(%a)=(%d)"},
        {"  trim  ", "^%s*(.-)%s*$"}, {"aXbXc", "[^X]+"}, {"a-z", "[a-]"},
        {"hello", "(h)(e)(l)(l)(o)"}, {"abc", "a?b?c?d?"}, {"abc", "(a)(%1)"},
        {"a", "[a"}, {"a", "%"}, {"a", "%b"}, {"a", "%f"}, {"a", "(a"}, {"a", "a)"},
        {"a", "%1"}, {"a", "(()"}, {"abc", "b", 2}, {"abc", "b", 3}, {"abc", "b", -1},
        {"abc", "b", -10}, {"abc", "b", 10}, {"abc", "b", 1.5}, {"abc", "b", "2"},
        {"a+b", "+", 1, true}, {"a+b", "a+", 1, false}, {"abc", "", 4}, {"abc", "", 5}
    "#;

    #[test]
    fn find_and_match_agree_with_stock_lua() {
        assert_same_as_stock_lua(&report_calls("string.find", PATTERN_CASES));
        assert_same_as_stock_lua(&report_calls("string.match", PATTERN_CASES));
    }

    #[test]
    fn gsub_and_gmatch_agree_with_stock_lua() {
        let gmatch_all = "function(s, p, init) local found = {} for a, b in string.gmatch(s, p, init) do found[#found + 1] = tostring(a) .. \"/\" .. tostring(b) end return table.concat(found, \" \") end";
        assert_same_as_stock_lua(&report_calls(gmatch_all, PATTERN_CASES));
        let gsub_cases = r#"
            {"hello world", "o", "0"}, {"hello world", "(%w+)", "<%1>"}, {"hello", "", "-"},
            {"abc", "%w", "%0%0"}, {"abc", "%w", "%%"}, {"abc", "(b)", "%2"}, {"abc", "b", "%x"},
            {"abc", "%w", { a = 1, b = true }}, {"abc", "%w", { a = "A", b = false }},
            {"abc", "%w", function(c) if c ~= "b" then return c:upper() end end},
            {"abc", "()", "%1"}, {"x = 1, y = 2", "(%w+) = (%w+)", "%2 = %1", 1},
            {"aaa", "a", "b", 0}, {"aaa", "a", "b", -1}, {"aaa", "a", "b", 2.0},
            {"aaa", "^a", "b"}, {"abc", "x*", "-"}, {"abc", ".", 7}, {"abc", "b"},
            {"abc", "b", {}}, {"abc", "b", { b = {} }}, {"abc", "[", "x"}, {"abc", "b", "x", "1"}
        "#;
        assert_same_as_stock_lua(&report_calls("string.gsub", gsub_cases));
    }

    #[test]
    fn table_functions_agree_with_stock_lua() {
        assert_same_as_stock_lua(
            r#"local out = {}
            local function show(t, n) local parts = {} for i = 1, n do parts[i] = tostring(t[i]) end return table.concat(parts, ",") end
            local function try(f, ...) local ok, err = pcall(f, ...) if not ok then out[#out + 1] = "error: " .. tostring(err):gsub("^test%.lua:%d+: ", ""):gsub("'table%.", "'") end end
            for _, size in ipairs({ 0, 1, 5, 4096, 4097, 9000, 20000 }) do
              local t = {}
              for i = 1, size do t[i] = i end
              table.insert(t, "end")
              table.insert(t, 1, "start")
              if size > 2 then table.insert(t, size // 2, "middle") end
              local removed = { table.remove(t), table.remove(t, 1), table.remove(t, math.max(1, #t // 3)) }
              table.move(t, 1, #t, 3)
              table.move(t, 5, #t, 2)
              local copy = table.move(t, 1, #t, 1, {})
              table.move(t, 2, #t // 2, #t // 4 + 1)
              out[#out + 1] = #t .. ":" .. show(removed, 3) .. ":" .. tostring(t[1]) .. "," .. tostring(t[#t // 2]) .. "," .. tostring(t[#t]) .. ":" .. #copy
            end
            try(table.insert, nil, 1) try(table.insert, {}, 5, 1) try(table.insert, {}, 1, 2, 3)
            try(table.insert, {}, 1.5, 1) try(table.insert, {}, "x", 1) try(table.remove, {}, 5)
            try(table.remove, { 1 }, 3) try(table.move, {}, 1, math.maxinteger, 2)
            try(table.move, {}, -1, math.maxinteger, 1) try(table.move, 1, 1, 1, 1)
            local empty = {} table.remove(empty, 0) table.remove(empty, 1) out[#out + 1] = tostring(#empty)
            local one = { "a" } out[#out + 1] = tostring(table.remove(one, 2)) .. tostring(table.remove(one, 1))
            return table.concat(out, "\n")"#,
        );
    }
}
