//! `mp.player`: playback commands and getters.
//!
//! Commands are sent to the app as [`HostEvent::Command`]; getters answer from the
//! host's [`PlayerSnapshot`], so they never wait for the UI thread.
//!
//! [`HostEvent::Command`]: crate::HostEvent::Command

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use mlua::{FromLuaMulti, IntoLuaMulti, Lua, Table};
use mp_core::{PlayerCommand, RepeatMode};

use super::{ApiContext, duration_arg, finite};
use crate::errors::error_at_caller;
use crate::events::{duration_ms, track_table};
use crate::host::Emitter;
use crate::rate_limit::RateLimit;
use crate::snapshot::PlayerSnapshot;

/// Commands a plugin may send at once, and per second after that. A volume fade
/// needs a few dozen; a plugin sending more is stuck in a loop.
const COMMAND_BURST: u32 = 50;
const COMMANDS_PER_SECOND: u32 = 20;

pub(super) fn table(lua: &Lua, context: &ApiContext) -> mlua::Result<Table> {
    let player = lua.create_table()?;
    let commands = Commands {
        emitter: context.emitter.clone(),
        log_target: context.log_target.clone(),
        limit: Rc::new(RefCell::new(RateLimit::new(
            "player commands",
            COMMAND_BURST,
            COMMANDS_PER_SECOND,
        ))),
    };
    add_commands(lua, &player, &commands)?;
    add_getters(lua, &player, &context.snapshot)?;
    Ok(player)
}

/// Sends a plugin's commands to the app, dropping those beyond the rate limit.
#[derive(Clone)]
struct Commands {
    emitter: Emitter,
    log_target: Rc<str>,
    limit: Rc<RefCell<RateLimit>>,
}

impl Commands {
    fn send(&self, command: PlayerCommand) {
        if self.limit.borrow_mut().allow(&self.log_target) {
            self.emitter.command(command);
        }
    }
}

fn add_commands(lua: &Lua, player: &Table, commands: &Commands) -> mlua::Result<()> {
    let without_arguments = [
        ("play", PlayerCommand::Play),
        ("pause", PlayerCommand::Pause),
        ("toggle", PlayerCommand::TogglePlayPause),
        ("stop", PlayerCommand::Stop),
        ("next", PlayerCommand::Next),
        ("previous", PlayerCommand::Previous),
    ];
    for (name, command) in without_arguments {
        add_command(
            lua,
            player,
            commands,
            name,
            move |_, ()| Ok(command.clone()),
        )?;
    }

    add_command(lua, player, commands, "seek", |lua, ms: f64| {
        Ok(PlayerCommand::SeekTo(duration_arg(lua, ms)?))
    })?;
    add_command(lua, player, commands, "seek_by", |lua, ms: f64| {
        Ok(PlayerCommand::SeekBy(finite(lua, ms)?.round() as i64))
    })?;
    add_command(lua, player, commands, "set_volume", |lua, volume: f64| {
        Ok(PlayerCommand::SetVolume {
            volume: finite(lua, volume)?.clamp(0.0, 1.0) as f32,
            unmute: false,
        })
    })?;
    add_command(lua, player, commands, "set_shuffle", |_, on: bool| {
        Ok(PlayerCommand::SetShuffle(on))
    })?;
    add_command(lua, player, commands, "set_repeat", |lua, mode: String| {
        RepeatMode::from_id(&mode)
            .map(PlayerCommand::SetRepeat)
            .ok_or_else(|| {
                let message =
                    format!("unknown repeat mode '{mode}' (use \"off\", \"all\" or \"one\")");
                error_at_caller(lua, message)
            })
    })?;
    Ok(())
}

/// Adds `mp.player.<name>(...)`, which turns its arguments into a command for the app.
fn add_command<A: FromLuaMulti>(
    lua: &Lua,
    player: &Table,
    commands: &Commands,
    name: &str,
    build: impl Fn(&Lua, A) -> mlua::Result<PlayerCommand> + 'static,
) -> mlua::Result<()> {
    let commands = commands.clone();
    let command = lua.create_function(move |lua, args: A| {
        commands.send(build(lua, args)?);
        Ok(())
    })?;
    player.set(name, command)
}

fn add_getters(
    lua: &Lua,
    player: &Table,
    snapshot: &Rc<RefCell<PlayerSnapshot>>,
) -> mlua::Result<()> {
    add_getter(lua, player, snapshot, "state", |_, s| Ok(s.state.id()))?;
    add_getter(lua, player, snapshot, "current_track", |lua, s| {
        s.track
            .as_deref()
            .map(|track| track_table(lua, track))
            .transpose()
    })?;
    add_getter(lua, player, snapshot, "volume", |_, s| {
        Ok(f64::from(s.volume))
    })?;
    add_getter(lua, player, snapshot, "muted", |_, s| Ok(s.muted))?;
    add_getter(lua, player, snapshot, "shuffle", |_, s| Ok(s.shuffle))?;
    // `repeat` is a Lua keyword, so `mp.player.repeat()` doesn't parse; plugins can
    // write `mp.player["repeat"]()` or use the `repeat_mode` alias.
    add_getter(lua, player, snapshot, "repeat", |_, s| Ok(s.repeat.id()))?;
    add_getter(lua, player, snapshot, "repeat_mode", |_, s| {
        Ok(s.repeat.id())
    })?;
    add_getter(lua, player, snapshot, "position_ms", |_, s| {
        let position: Duration = s.position_at(Instant::now());
        Ok(duration_ms(position.as_millis()))
    })
}

/// Adds `mp.player.<name>()`, answered from the snapshot.
fn add_getter<R: IntoLuaMulti>(
    lua: &Lua,
    player: &Table,
    snapshot: &Rc<RefCell<PlayerSnapshot>>,
    name: &str,
    get: impl Fn(&Lua, &PlayerSnapshot) -> mlua::Result<R> + 'static,
) -> mlua::Result<()> {
    let snapshot = snapshot.clone();
    let getter = lua.create_function(move |lua, ()| get(lua, &snapshot.borrow()))?;
    player.set(name, getter)
}
