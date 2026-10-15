//! `mp.timer`: one-shot and repeating timers, run on the host thread.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use mlua::{Function, Lua, Table};

use super::{ApiContext, duration_arg};
use crate::errors::error_at_caller;
use crate::registry::Registry;

/// Repeating timers fire at most this often, so a plugin can't spin the host thread.
pub(crate) const MIN_INTERVAL: Duration = Duration::from_millis(10);

pub(super) fn table(lua: &Lua, context: &ApiContext) -> mlua::Result<Table> {
    let timer = lua.create_table()?;
    timer.set("after", schedule_function(lua, &context.registry, false)?)?;
    timer.set("every", schedule_function(lua, &context.registry, true)?)?;

    let registry = context.registry.clone();
    let cancel = lua.create_function(move |_, id: i64| {
        Ok(u64::try_from(id).is_ok_and(|id| registry.borrow_mut().timers.cancel(id)))
    })?;
    timer.set("cancel", cancel)?;
    Ok(timer)
}

/// `mp.timer.after(ms, fn)` or, when `repeating`, `mp.timer.every(ms, fn)`; both
/// return the timer's id.
fn schedule_function(
    lua: &Lua,
    registry: &Rc<RefCell<Registry<Function>>>,
    repeating: bool,
) -> mlua::Result<Function> {
    let registry = registry.clone();
    lua.create_function(move |lua, (ms, callback): (f64, Function)| {
        let delay = duration_arg(lua, ms)?;
        let (delay, interval) = if repeating {
            let interval = delay.max(MIN_INTERVAL);
            (interval, Some(interval))
        } else {
            (delay, None)
        };
        registry
            .borrow_mut()
            .timers
            .add(Instant::now(), delay, interval, callback)
            .map_err(|err| error_at_caller(lua, err))
    })
}
