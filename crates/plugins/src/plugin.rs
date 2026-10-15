//! One loaded plugin: its sandboxed Lua state and everything its script registered.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use std::time::Instant;

use mlua::chunk::ChunkMode;
use mlua::{Function, IntoLuaMulti, Lua, LuaSerdeExt, MultiValue};
use mp_core::files::read_limited;
use mp_core::{PlayerEvent, PluginPermissions, Track};
use serde_json::{Map, Value};

use crate::api::settings::PluginSettings;
use crate::api::{self, ApiContext, Deliver};
use crate::bundled;
use crate::discord::SharedPresence;
use crate::errors::{self, error_at_caller};
use crate::events::{self, EventKind};
use crate::host::{Emitter, HostEvent, Message};
use crate::http::{HttpClient, HttpResponse};
use crate::lyrics::{LyricsRequests, LyricsSearch, MAX_LYRICS_BYTES, OwnLyrics};
use crate::manifest::{PluginInfo, PluginSource};
use crate::rate_limit::RateLimit;
use crate::registry::Registry;
use crate::sandbox::{self, MAX_SCRIPT_BYTES, Modules, time_limit};
use crate::snapshot::PlayerSnapshot;
use crate::storage::Storage;
use crate::text::truncate;

/// A plugin that fails this many callbacks in a row is unloaded.
pub(crate) const MAX_CONSECUTIVE_ERRORS: u32 = 10;

/// Errors a plugin may report at once, and per second after that. More are only logged.
const ERROR_BURST: u32 = 20;
const ERRORS_PER_SECOND: u32 = 5;

/// Longest error text logged for one failed callback (the message plus Lua's traceback).
const MAX_LOGGED_ERROR_CHARS: usize = 8000;

/// What all plugins share with the host.
pub(crate) struct PluginEnv {
    pub data_dir: PathBuf,
    pub events: async_channel::Sender<HostEvent>,
    pub snapshot: Rc<RefCell<PlayerSnapshot>>,
    pub http: Arc<dyn HttpClient>,
    /// Takes web request results back to the host thread.
    pub inbox: mpsc::Sender<Message>,
    pub request_ids: Rc<Cell<u64>>,
    pub presence: Rc<SharedPresence>,
    pub lyrics: Rc<RefCell<LyricsRequests>>,
}

impl PluginEnv {
    /// Lets go of what plugin `id` left behind once it stopped: the lyrics it was
    /// finding and what it showed on Discord.
    pub fn forget(&self, id: &str) {
        self.lyrics.borrow_mut().forget(id);
        self.presence.set(id, None);
    }
}

pub(crate) struct Plugin {
    id: String,
    lua: Lua,
    registry: Rc<RefCell<Registry<Function>>>,
    storage: Rc<RefCell<Storage>>,
    settings: Rc<RefCell<PluginSettings>>,
    lyrics: Rc<RefCell<LyricsRequests>>,
    emitter: Emitter,
    log_target: Rc<str>,
    error_limit: RateLimit,
    consecutive_errors: u32,
}

impl Plugin {
    /// Creates the plugin's sandbox, installs the `mp` API and runs its entry script.
    /// The plugin may do what it asks for in `plugin.json` and the user `allowed`, and
    /// `settings` are the user's choices on its settings page. The error is a message
    /// for the user.
    pub fn load(
        info: &PluginInfo,
        env: &PluginEnv,
        allowed: &PluginPermissions,
        settings: Map<String, Value>,
    ) -> Result<Self, String> {
        let log_target: Rc<str> = format!("plugin::{}", info.id).into();
        let (source, modules) = entry_and_modules(info)?;
        let lua = sandbox::create(modules)
            .map_err(|err| format!("could not create the Lua sandbox: {err}"))?;
        let registry = Rc::new(RefCell::new(Registry::default()));
        let emitter = Emitter::new(&info.id, env.events.clone());
        let storage = Rc::new(RefCell::new(Storage::open(
            env.data_dir.join(format!("{}.json", info.id)),
        )));
        let settings = Rc::new(RefCell::new(PluginSettings::new(
            info.manifest.settings.clone(),
            settings,
        )));
        let inbox = env.inbox.clone();
        let id = info.id.clone();
        let deliver: Deliver = Arc::new(move |request, result| {
            let _ = inbox.send(Message::Response {
                plugin: id.clone(),
                request,
                result,
            });
        });
        let context = ApiContext {
            id: info.id.clone(),
            name: info.manifest.name.clone(),
            version: info.manifest.version.clone(),
            log_target: log_target.clone(),
            emitter: emitter.clone(),
            snapshot: env.snapshot.clone(),
            registry: registry.clone(),
            storage: storage.clone(),
            settings: settings.clone(),
            permissions: info.manifest.permissions.intersection(allowed),
            http: env.http.clone(),
            deliver,
            request_ids: env.request_ids.clone(),
            presence: env.presence.clone(),
        };
        api::install(&lua, &context)
            .map_err(|err| format!("could not set up the plugin API: {err}"))?;

        let entry = info.manifest.entry.as_str();
        let loaded = {
            let _limit = time_limit::start();
            lua.load(source)
                .set_name(format!("={entry}"))
                .set_mode(ChunkMode::Text)
                .exec()
        };
        loaded.map_err(|err| {
            log::warn!(target: &log_target, "Failed to load: {}", loggable(&err));
            format!("failed to load: {}", errors::describe(&err))
        })?;

        Ok(Self {
            id: info.id.clone(),
            lua,
            registry,
            storage,
            settings,
            lyrics: env.lyrics.clone(),
            emitter,
            log_target,
            error_limit: RateLimit::new("errors", ERROR_BURST, ERRORS_PER_SECOND),
            consecutive_errors: 0,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// Runs the plugin's handlers for `event`.
    pub fn handle_event(&mut self, event: &PlayerEvent) {
        self.run_handlers(EventKind::of(event), |lua| events::event_args(lua, event));
    }

    /// Changes setting `key`, or puts it back to its default for `None`, and tells the
    /// plugin.
    pub fn set_setting(&mut self, key: &str, value: Option<Value>) {
        let Some(value) = self.settings.borrow_mut().set(key, value) else {
            return;
        };
        self.run_handlers(EventKind::SettingChanged, |lua| {
            (key, lua.to_value(&value)?).into_lua_multi(lua)
        });
    }

    /// The songs the plugin finds lyrics for.
    pub fn lyrics_search(&self) -> LyricsSearch {
        self.registry
            .borrow()
            .lyrics
            .as_ref()
            .map_or(LyricsSearch::Off, |(_, search)| *search)
    }

    /// Asks the plugin's lyrics finder for the lyrics of `track`, whose own are `own`,
    /// to answer `request`.
    pub fn find_lyrics(&mut self, request: u64, track: &Track, own: OwnLyrics) {
        let finder = self
            .registry
            .borrow()
            .lyrics
            .as_ref()
            .map(|(finder, _)| finder.clone());
        let started = finder.is_some_and(|finder| {
            let requests = self.lyrics.clone();
            let id = self.id.clone();
            self.run("lyrics finder", |lua| {
                let answered = Cell::new(false);
                let done = lua.create_function(move |lua, lyrics: Option<mlua::LuaString>| {
                    if answered.replace(true) {
                        return Ok(());
                    }
                    if lyrics
                        .as_ref()
                        .is_some_and(|text| text.as_bytes().len() > MAX_LYRICS_BYTES)
                    {
                        requests.borrow_mut().answer(request, &id, None);
                        return Err(error_at_caller(
                            lua,
                            format!("lyrics are limited to {} KiB", MAX_LYRICS_BYTES / 1024),
                        ));
                    }
                    let lyrics = lyrics.map(|text| text.to_string_lossy());
                    requests.borrow_mut().answer(request, &id, lyrics);
                    Ok(())
                })?;
                let track = events::track_table(lua, track)?;
                track.set("lyrics", own.name())?;
                finder.call::<()>((track, done))
            })
        });
        if !started {
            self.lyrics.borrow_mut().answer(request, &self.id, None);
        }
    }

    /// Hands web request `request`'s result to the callback waiting for it.
    pub fn finish_request(&mut self, request: u64, result: Result<HttpResponse, String>) {
        let callback = self.registry.borrow_mut().requests.remove(&request);
        if let Some(callback) = callback {
            self.run("web request callback", |lua| {
                callback.call::<()>(api::http::callback_args(lua, result)?)
            });
        }
    }

    /// Runs the plugin's handlers for `kind` with the arguments `args` makes. They
    /// share one time limit, so a plugin can't stall the host by registering thousands
    /// of slow handlers.
    fn run_handlers(&mut self, kind: EventKind, args: impl Fn(&Lua) -> mlua::Result<MultiValue>) {
        let handlers = self.registry.borrow().handlers(kind);
        if handlers.is_empty() {
            return;
        }
        let what = format!("'{}' handler", kind.name());
        let _limit = time_limit::start();
        for (index, handler) in handlers.iter().enumerate() {
            if self.is_failing() {
                break;
            }
            if time_limit::is_exceeded() {
                self.report_error(&format!(
                    "'{}' handlers took longer than {} ms together; {} of them were skipped",
                    kind.name(),
                    sandbox::CALLBACK_TIME_LIMIT.as_millis(),
                    handlers.len() - index
                ));
                break;
            }
            // Arguments are built inside the time limit: creating tables can run
            // the garbage collector.
            let succeeded = self.run(&what, |lua| handler.call::<()>(args(lua)?));
            if !succeeded && time_limit::is_exceeded() {
                break;
            }
        }
    }

    /// When the plugin next needs the host: a timer, or storage changes to save.
    pub fn next_due(&self) -> Option<Instant> {
        let timer = self.registry.borrow().timers.next_due();
        let save = self.storage.borrow().save_due();
        timer.into_iter().chain(save).min()
    }

    /// Saves storage changes and runs the timers due at `now`, sharing one time limit.
    /// Timers still due when it runs out stay due and run on the host's next round.
    pub fn run_due(&mut self, now: Instant) {
        self.storage.borrow_mut().save_if_due(now);
        let _limit = time_limit::start();
        while !self.is_failing() && !time_limit::is_exceeded() {
            let Some(callback) = self.registry.borrow_mut().timers.pop_due(now) else {
                break;
            };
            self.run("timer", |_| callback.call::<()>(()));
        }
    }

    pub fn is_failing(&self) -> bool {
        self.consecutive_errors >= MAX_CONSECUTIVE_ERRORS
    }

    /// Runs the plugin's `shutdown` handlers, sharing one time limit; the plugin is
    /// dropped afterwards.
    pub fn shutdown(mut self) {
        let handlers = self.registry.borrow().handlers(EventKind::Shutdown);
        let _limit = time_limit::start();
        for handler in handlers {
            if time_limit::is_exceeded() {
                break;
            }
            self.run("'shutdown' handler", |_| handler.call::<()>(()));
        }
    }

    /// Runs one callback within the current time limit (or a new one) and reports its
    /// error, if any. Returns whether it succeeded.
    fn run(&mut self, what: &str, callback: impl FnOnce(&Lua) -> mlua::Result<()>) -> bool {
        let result = {
            let _limit = time_limit::start();
            callback(&self.lua)
        };
        match result {
            Ok(()) => {
                self.consecutive_errors = 0;
                true
            }
            Err(err) => {
                log::warn!(target: &self.log_target, "Error in {what}: {}", loggable(&err));
                self.report_error(&format!("error in {what}: {}", errors::describe(&err)));
                false
            }
        }
    }

    /// Counts an error towards unloading the plugin and tells the app, unless the
    /// plugin is reporting errors faster than anyone could read them.
    fn report_error(&mut self, message: &str) {
        self.consecutive_errors += 1;
        if self.error_limit.allow(&self.log_target) {
            self.emitter.error(message.to_owned());
        }
    }
}

/// The entry script's source, and where `require` finds the plugin's modules.
fn entry_and_modules(info: &PluginInfo) -> Result<(Vec<u8>, Modules), String> {
    let entry = info.manifest.entry.as_str();
    match &info.source {
        PluginSource::Folder(dir) => {
            let source = read_limited(&dir.join(entry), MAX_SCRIPT_BYTES)
                .map_err(|err| format!("could not read {entry}: {err}"))?;
            Ok((source, Modules::Folder(dir.clone())))
        }
        PluginSource::Bundled => {
            let plugin =
                bundled::find(&info.id).ok_or("isn't included in this version of the app")?;
            let source = plugin
                .file(entry)
                .ok_or_else(|| format!("could not read {entry}"))?;
            Ok((source.as_bytes().to_vec(), Modules::Bundled(plugin)))
        }
    }
}

/// The full error with its traceback, cut to a size that keeps the log readable.
fn loggable(err: &mlua::Error) -> String {
    truncate(&err.to_string(), MAX_LOGGED_ERROR_CHARS)
}
