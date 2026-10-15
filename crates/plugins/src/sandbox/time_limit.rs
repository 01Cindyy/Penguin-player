//! The time limit on plugin code.
//!
//! The host starts a time limit each time it calls into a plugin (see [`start`]). A Lua
//! hook checks the deadline every thousand VM instructions and before every function
//! call, and raises an error once it has passed. Checking before calls matters for loops
//! of slow library calls (such as `string.upper` on a long string): only a few of them
//! would fit between two instruction checks, but each can take milliseconds. Long-running
//! Rust code checks the deadline itself with [`is_exceeded`].
//!
//! The hook is installed through Lua's C API instead of mlua's `set_global_hook`. When an
//! mlua hook returns an error, mlua resets the interrupted function's stack while still
//! inside the hook, and Lua then runs that function's `__close` handlers with hooks
//! disabled, where no time limit can stop them. Raising the error directly lets Lua
//! unwind normally, with hooks enabled again.
//!
//! Unwinding still calls plugin code: every pending `__close` handler (a plugin can have
//! close to a million) and `xpcall` message handlers. The hook stops each one as it
//! starts, and every stop after the first raises the very same error (see [`raise`]), so
//! that a million stops take a fraction of a second.
//!
//! Lua itself still runs two kinds of plugin code with hooks disabled after a hook
//! error: `xpcall` message handlers, and the `__close` handlers of a coroutine killed by
//! the hook when it is closed. The sandbox prelude skips both once time is up.

use std::cell::Cell;
use std::ffi::{CStr, c_int};
use std::time::{Duration, Instant};

use mlua::{Function, Lua, Value, ffi};

use crate::errors::error_at_caller;

/// Longest a plugin may run each time the host calls it: loading its script, handling
/// one event (all of its handlers together) or running its due timers.
pub const CALLBACK_TIME_LIMIT: Duration = Duration::from_millis(250);

/// The error message, after the location where the plugin was stopped.
pub(crate) const TIME_LIMIT_MESSAGE: &str = "took longer than 250 ms and was stopped";

/// How many Lua instructions run between deadline checks. Having a hook at all is what
/// slows Lua down; checking every thousand instructions costs no more than every ten
/// thousand, and keeps the overshoot small when single instructions are slow (such as
/// concatenating two long strings).
const INSTRUCTIONS_PER_CHECK: c_int = 1_000;

/// When the hook runs: every [`INSTRUCTIONS_PER_CHECK`] instructions, and on every call.
const HOOK_EVENTS: c_int = ffi::LUA_MASKCOUNT | ffi::LUA_MASKCALL;

/// Registry fields of each Lua state, used by [`raise`]: the function that makes a
/// located time-limit error, an unlocated one for when that fails, and the error kept
/// for the time limit numbered in the last field.
const MAKE_ERROR_FIELD: &CStr = c"mp_plugins.time_limit.make_error";
const GENERIC_ERROR_FIELD: &CStr = c"mp_plugins.time_limit.generic_error";
const KEPT_ERROR_FIELD: &CStr = c"mp_plugins.time_limit.kept_error";
const KEPT_LIMIT_FIELD: &CStr = c"mp_plugins.time_limit.kept_limit";

thread_local! {
    /// When the plugin code running on this thread has to stop. All plugins run on the
    /// host thread, one call at a time, so one deadline per thread is enough.
    static DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };

    /// Numbers the time limits started on this thread, from 1.
    static LIMIT_NUMBER: Cell<ffi::lua_Integer> = const { Cell::new(0) };
}

/// Ends the time limit started by [`start`] when dropped.
#[must_use = "the time limit ends when this guard is dropped"]
pub(crate) struct TimeLimit {
    previous: Option<Instant>,
}

impl Drop for TimeLimit {
    fn drop(&mut self) {
        DEADLINE.set(self.previous);
    }
}

/// Gives the plugin code that runs until the guard is dropped [`CALLBACK_TIME_LIMIT`]
/// to finish. A limit started while another is running shares its deadline.
pub(crate) fn start() -> TimeLimit {
    let previous = DEADLINE.get();
    if previous.is_none() {
        DEADLINE.set(Some(Instant::now() + CALLBACK_TIME_LIMIT));
        LIMIT_NUMBER.set(LIMIT_NUMBER.get() + 1);
    }
    TimeLimit { previous }
}

/// Whether the running plugin is out of time.
pub(crate) fn is_exceeded() -> bool {
    DEADLINE
        .get()
        .is_some_and(|deadline| Instant::now() >= deadline)
}

/// Installs the deadline check on `lua`. Coroutines inherit it when they are created.
pub(crate) fn install(lua: &Lua) -> mlua::Result<()> {
    let make_error = lua.create_function(|lua, ()| -> mlua::Result<()> {
        Err(error_at_caller(lua, TIME_LIMIT_MESSAGE))
    })?;
    let generic_error = Value::Error(Box::new(mlua::Error::runtime(TIME_LIMIT_MESSAGE)));
    // SAFETY: `exec_raw` hands over the state's main thread with both values on its
    // stack. Setting registry fields and a hook is allowed at any time.
    unsafe {
        lua.exec_raw::<()>((make_error, generic_error), |state| {
            // Every field exists from here on, so keeping an error never allocates.
            ffi::lua_pushvalue(state, -1);
            ffi::lua_setfield(state, ffi::LUA_REGISTRYINDEX, KEPT_ERROR_FIELD.as_ptr());
            ffi::lua_setfield(state, ffi::LUA_REGISTRYINDEX, GENERIC_ERROR_FIELD.as_ptr());
            ffi::lua_setfield(state, ffi::LUA_REGISTRYINDEX, MAKE_ERROR_FIELD.as_ptr());
            ffi::lua_pushinteger(state, 0);
            ffi::lua_setfield(state, ffi::LUA_REGISTRYINDEX, KEPT_LIMIT_FIELD.as_ptr());
            ffi::lua_sethook(state, Some(hook), HOOK_EVENTS, INSTRUCTIONS_PER_CHECK);
        })
    }
}

/// `check_time()` and `time_is_up()` for the sandbox prelude: the first raises the hook's
/// error once the plugin is out of time, the second tells whether it is. Plain C
/// functions, because the prelude calls them for every pending `__close` handler and
/// message handler while Lua unwinds.
pub(crate) fn prelude_functions(lua: &Lua) -> mlua::Result<(Function, Function)> {
    // SAFETY: both functions follow Lua's C calling convention, and `check_time` only
    // raises errors the way C functions may.
    unsafe {
        Ok((
            lua.create_c_function(check_time)?,
            lua.create_c_function(time_is_up)?,
        ))
    }
}

unsafe extern "C-unwind" fn hook(state: *mut ffi::lua_State, _: *mut ffi::lua_Debug) {
    if is_exceeded() {
        // SAFETY: `state` is the running thread, which Lua passes to its hooks, and hooks
        // may raise errors.
        unsafe { raise(state) }
    }
}

unsafe extern "C-unwind" fn check_time(state: *mut ffi::lua_State) -> c_int {
    if is_exceeded() {
        // SAFETY: `state` is the thread that called this C function, which may raise.
        unsafe { raise(state) }
    }
    0
}

unsafe extern "C-unwind" fn time_is_up(state: *mut ffi::lua_State) -> c_int {
    // SAFETY: a C function always has room to push one result.
    unsafe { ffi::lua_pushboolean(state, c_int::from(is_exceeded())) };
    1
}

/// Raises the time-limit error.
///
/// The first stop in a time limit makes the error, named after the plugin line that ran
/// out of time, and the state keeps it; later stops in the same time limit raise the kept
/// error again. Re-raising allocates nothing, and mlua's message handler passes mlua error
/// objects on without building a traceback, which would otherwise take milliseconds for
/// each of up to a million pending `__close` handlers.
///
/// `lua_error` leaves this function with a `longjmp`, so nothing here may need dropping.
unsafe fn raise(state: *mut ffi::lua_State) -> ! {
    let limit = LIMIT_NUMBER.get();
    // SAFETY: Lua guarantees hooks and C functions room for at least `LUA_MINSTACK` (20)
    // values, more than used here. The registry has no metatable, so reading and
    // overwriting its fields runs no code and allocates nothing.
    unsafe {
        if !push_kept_error(state, limit) {
            push_new_error(state);
            ffi::lua_pushvalue(state, -1);
            ffi::lua_setfield(state, ffi::LUA_REGISTRYINDEX, KEPT_ERROR_FIELD.as_ptr());
            ffi::lua_pushinteger(state, limit);
            ffi::lua_setfield(state, ffi::LUA_REGISTRYINDEX, KEPT_LIMIT_FIELD.as_ptr());
        }
        ffi::lua_error(state)
    }
}

/// Pushes the error kept for time limit number `limit`; returns `false`, pushing
/// nothing, if the kept error is from an earlier time limit.
unsafe fn push_kept_error(state: *mut ffi::lua_State, limit: ffi::lua_Integer) -> bool {
    // SAFETY: see `raise`.
    unsafe {
        ffi::lua_getfield(state, ffi::LUA_REGISTRYINDEX, KEPT_LIMIT_FIELD.as_ptr());
        let kept = ffi::lua_tointeger(state, -1) == limit;
        ffi::lua_pop(state, 1);
        if kept {
            ffi::lua_getfield(state, ffi::LUA_REGISTRYINDEX, KEPT_ERROR_FIELD.as_ptr());
        }
        kept
    }
}

/// Pushes a new time-limit error made by `make_error`, or the generic one when Lua is
/// too short of memory or C stack to run it.
unsafe fn push_new_error(state: *mut ffi::lua_State) {
    // SAFETY: see `raise`; a protected call always returns here. `make_error` always
    // fails, and only an error it raised itself is an mlua error object (a userdata).
    unsafe {
        ffi::lua_getfield(state, ffi::LUA_REGISTRYINDEX, MAKE_ERROR_FIELD.as_ptr());
        let made = ffi::lua_pcall(state, 0, 0, 0) == ffi::LUA_ERRRUN
            && ffi::lua_type(state, -1) == ffi::LUA_TUSERDATA;
        if !made {
            ffi::lua_pop(state, 1);
            ffi::lua_getfield(state, ffi::LUA_REGISTRYINDEX, GENERIC_ERROR_FIELD.as_ptr());
        }
    }
}
