//! The plugin host thread and the handle the app talks to it through.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use mp_core::{PlayerCommand, PlayerEvent, PluginPermissions, Track};
use serde_json::{Map, Value};

use crate::discord::{DiscordConfig, DiscordPresence, NoPresence, Presence, SharedPresence};
use crate::http::{HttpClient, HttpResponse, Offline, WebClient};
use crate::lyrics::{LyricsRequests, LyricsSearch, OwnLyrics};
use crate::manifest;
use crate::plugin::{MAX_CONSECUTIVE_ERRORS, Plugin, PluginEnv};
use crate::sandbox::ThreadLocale;
use crate::snapshot::PlayerSnapshot;

/// How long dropping the [`PluginHost`] waits for `shutdown` handlers to finish.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);
const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(2);

/// Stack size of the host thread. Lua lets a plugin nest about 200 calls that pass
/// through Rust (such as `string.gsub` with a replacement function that calls `gsub`
/// again), and in debug builds each takes more native stack than the default 2 MiB
/// thread stack holds; running out of stack aborts the whole app. Only the pages that
/// are actually used take memory.
const HOST_STACK_SIZE: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct HostConfig {
    pub plugins_dir: PathBuf,
    /// Root for per-plugin persistent storage (see `AppPaths::plugin_data_dir`).
    pub data_dir: PathBuf,
    /// Ids of the plugins to load at start.
    pub enabled: Vec<String>,
    /// What the user allowed each plugin to do, by plugin id.
    pub permissions: HashMap<String, PluginPermissions>,
    /// The user's choices in each plugin's settings, by plugin id.
    pub settings: HashMap<String, Map<String, Value>>,
    pub services: Services,
}

/// What plugins reach outside the app, when the user allows them to.
#[derive(Clone)]
pub struct Services {
    pub http: Arc<dyn HttpClient>,
    pub presence: Arc<dyn Presence>,
}

impl Services {
    /// The internet, and the Discord app on this computer through the Discord
    /// application with `discord_application_id`.
    pub fn online(discord_application_id: &str) -> Self {
        Self {
            http: Arc::new(WebClient::default()),
            presence: Arc::new(DiscordPresence::new(DiscordConfig::new(
                discord_application_id,
            ))),
        }
    }

    /// Nothing outside: web requests fail and Discord shows nothing.
    pub fn offline() -> Self {
        Self {
            http: Arc::new(Offline),
            presence: Arc::new(NoPresence),
        }
    }
}

impl fmt::Debug for Services {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Services")
    }
}

/// Messages from plugins to the app.
#[derive(Debug, Clone, PartialEq)]
pub enum HostEvent {
    /// A plugin asked to control playback.
    Command {
        plugin: String,
        command: PlayerCommand,
    },
    /// A plugin asked to show a notification inside the app.
    Notify {
        plugin: String,
        title: String,
        body: Option<String>,
    },
    /// A plugin failed to load or raised an error; shown in the plugin manager.
    PluginError { plugin: String, message: String },
    /// The answer to [`PluginHost::find_lyrics`]: the first lyrics a plugin found, as
    /// LRC or plain text, or `None` if no plugin found any.
    Lyrics {
        request: u64,
        lyrics: Option<String>,
    },
    /// The songs loaded plugins find lyrics for, all together. Sent when that changes;
    /// it is [`LyricsSearch::Off`] until a plugin finds lyrics.
    LyricsSearch(LyricsSearch),
}

/// Messages to the host thread, from the app and from web requests.
pub(crate) enum Message {
    Dispatch(PlayerEvent),
    SetEnabled {
        id: String,
        enabled: bool,
    },
    SetPermissions {
        id: String,
        permissions: PluginPermissions,
    },
    SetSetting {
        id: String,
        key: String,
        value: Option<Value>,
    },
    FindLyrics {
        request: u64,
        track: Box<Track>,
        own: OwnLyrics,
    },
    Reload,
    Shutdown,
    /// A web request finished.
    Response {
        plugin: String,
        request: u64,
        result: Result<HttpResponse, String>,
    },
}

/// Handle to the plugin host thread. Dropping it unloads all plugins (running their
/// `shutdown` handlers) and joins the thread. Events still queued at that point are
/// skipped (as are the remaining plugins of one being handled), and a plugin stuck in a
/// long library call is abandoned after a few seconds so it can't hold up the app's exit.
pub struct PluginHost {
    messages: mpsc::Sender<Message>,
    /// Set when the app drops the host, so events still queued are skipped.
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl PluginHost {
    pub fn start(config: HostConfig) -> (Self, async_channel::Receiver<HostEvent>) {
        let (events, receiver) = async_channel::unbounded();
        let (messages, inbox) = mpsc::channel();
        let stopping = Arc::new(AtomicBool::new(false));
        let thread_stopping = stopping.clone();
        let host_messages = messages.clone();
        let spawned = std::thread::Builder::new()
            .name("plugin-host".to_owned())
            .stack_size(HOST_STACK_SIZE)
            .spawn(move || {
                let _locale = ThreadLocale::use_c();
                Host::new(config, events, host_messages, thread_stopping).run(&inbox);
            });
        let thread = spawned
            .inspect_err(|err| log::error!("Could not start the plugin host thread: {err}"))
            .ok();
        let host = Self {
            messages,
            stopping,
            thread,
        };
        (host, receiver)
    }

    /// Forwards a player event to every loaded plugin. Never blocks.
    pub fn dispatch(&self, event: PlayerEvent) {
        self.send(Message::Dispatch(event));
    }

    /// Loads or unloads a plugin at runtime.
    pub fn set_enabled(&self, id: &str, enabled: bool) {
        self.send(Message::SetEnabled {
            id: id.to_owned(),
            enabled,
        });
    }

    /// Sets what plugin `id` may do beyond the sandbox, from its next load on.
    pub fn set_permissions(&self, id: &str, permissions: PluginPermissions) {
        self.send(Message::SetPermissions {
            id: id.to_owned(),
            permissions,
        });
    }

    /// Changes one of plugin `id`'s settings, or puts it back to its default for `None`.
    pub fn set_setting(&self, id: &str, key: &str, value: Option<Value>) {
        self.send(Message::SetSetting {
            id: id.to_owned(),
            key: key.to_owned(),
            value,
        });
    }

    /// Asks the plugins that look up songs with `own` lyrics for `track`'s. The answer
    /// comes back as a [`HostEvent::Lyrics`] for `request`.
    pub fn find_lyrics(&self, request: u64, track: Track, own: OwnLyrics) {
        self.send(Message::FindLyrics {
            request,
            track: Box::new(track),
            own,
        });
    }

    /// Unloads and reloads every enabled plugin from disk.
    pub fn reload(&self) {
        self.send(Message::Reload);
    }

    fn send(&self, message: Message) {
        // Sending only fails once the host thread is gone, and then there is nobody
        // left to tell.
        let _ = self.messages.send(message);
    }
}

impl Drop for PluginHost {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        self.send(Message::Shutdown);
        let Some(thread) = self.thread.take() else {
            return;
        };
        // The time limit can't interrupt a single slow call into C (such as sorting a
        // huge table), so as a last resort never let a plugin stall the app's exit.
        let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
        while !thread.is_finished() && Instant::now() < deadline {
            std::thread::sleep(SHUTDOWN_POLL_INTERVAL);
        }
        if !thread.is_finished() {
            log::error!(
                "Plugins did not stop within {} s; leaving the plugin host thread behind",
                SHUTDOWN_TIMEOUT.as_secs()
            );
        } else if thread.join().is_err() {
            log::error!("The plugin host thread panicked");
        }
    }
}

/// Sends [`HostEvent`]s on behalf of one plugin.
#[derive(Debug, Clone)]
pub(crate) struct Emitter {
    plugin: String,
    events: async_channel::Sender<HostEvent>,
}

impl Emitter {
    pub fn new(plugin: &str, events: async_channel::Sender<HostEvent>) -> Self {
        Self {
            plugin: plugin.to_owned(),
            events,
        }
    }

    pub fn command(&self, command: PlayerCommand) {
        self.send(HostEvent::Command {
            plugin: self.plugin.clone(),
            command,
        });
    }

    pub fn notify(&self, title: String, body: Option<String>) {
        self.send(HostEvent::Notify {
            plugin: self.plugin.clone(),
            title,
            body,
        });
    }

    pub fn error(&self, message: String) {
        self.send(HostEvent::PluginError {
            plugin: self.plugin.clone(),
            message,
        });
    }

    fn send(&self, event: HostEvent) {
        // The channel is unbounded, so this only fails after the app dropped the receiver.
        let _ = self.events.try_send(event);
    }
}

/// State owned by the host thread.
struct Host {
    plugins_dir: PathBuf,
    enabled: Vec<String>,
    permissions: HashMap<String, PluginPermissions>,
    settings: HashMap<String, Map<String, Value>>,
    env: PluginEnv,
    plugins: Vec<Plugin>,
    /// What the app was last told about plugins finding lyrics.
    lyrics_search: LyricsSearch,
    stopping: Arc<AtomicBool>,
}

impl Host {
    fn new(
        config: HostConfig,
        events: async_channel::Sender<HostEvent>,
        inbox: mpsc::Sender<Message>,
        stopping: Arc<AtomicBool>,
    ) -> Self {
        let mut enabled = Vec::new();
        for id in config.enabled {
            if !enabled.contains(&id) {
                enabled.push(id);
            }
        }
        Self {
            plugins_dir: config.plugins_dir,
            enabled,
            permissions: config.permissions,
            settings: config.settings,
            env: PluginEnv {
                data_dir: config.data_dir,
                lyrics: Rc::new(RefCell::new(LyricsRequests::new(events.clone()))),
                events,
                snapshot: Rc::new(RefCell::new(PlayerSnapshot::default())),
                http: config.services.http,
                inbox,
                request_ids: Rc::new(Cell::new(0)),
                presence: Rc::new(SharedPresence::new(config.services.presence)),
            },
            plugins: Vec::new(),
            lyrics_search: LyricsSearch::Off,
            stopping,
        }
    }

    /// The host's event loop: handles messages from the app and fires timers
    /// in between, until the app shuts the host down.
    fn run(mut self, inbox: &mpsc::Receiver<Message>) {
        self.load_enabled();
        self.tell_lyrics_search();
        while !self.stopping.load(Ordering::Acquire) {
            let message = match self.next_due() {
                Some(due) => inbox.recv_timeout(due.saturating_duration_since(Instant::now())),
                None => inbox.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match message {
                Ok(Message::Dispatch(event)) => self.dispatch(&event),
                Ok(Message::SetEnabled { id, enabled }) => self.set_enabled(&id, enabled),
                Ok(Message::SetPermissions { id, permissions }) => {
                    self.permissions.insert(id, permissions);
                }
                Ok(Message::SetSetting { id, key, value }) => self.set_setting(&id, &key, value),
                Ok(Message::FindLyrics {
                    request,
                    track,
                    own,
                }) => self.find_lyrics(request, &track, own),
                Ok(Message::Response {
                    plugin,
                    request,
                    result,
                }) => {
                    let mut result = Some(result);
                    self.for_each_plugin(|loaded| {
                        if loaded.id() == plugin
                            && let Some(result) = result.take()
                        {
                            loaded.finish_request(request, result);
                        }
                    });
                }
                Ok(Message::Reload) => self.reload(),
                Ok(Message::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.run_due();
            self.remove_failing();
            self.tell_lyrics_search();
        }
        self.unload_all();
    }

    fn set_setting(&mut self, id: &str, key: &str, value: Option<Value>) {
        let values = self.settings.entry(id.to_owned()).or_default();
        match &value {
            Some(value) => values.insert(key.to_owned(), value.clone()),
            None => values.remove(key),
        };
        self.for_each_plugin(|plugin| {
            if plugin.id() == id {
                plugin.set_setting(key, value.clone());
            }
        });
    }

    fn find_lyrics(&mut self, request: u64, track: &Track, own: OwnLyrics) {
        let finders: Vec<String> = self
            .plugins
            .iter()
            .filter(|plugin| plugin.lyrics_search().covers(own))
            .map(|plugin| plugin.id().to_owned())
            .collect();
        self.env
            .lyrics
            .borrow_mut()
            .start(request, own, finders.clone());
        self.for_each_plugin(|plugin| {
            if finders.iter().any(|id| id == plugin.id()) {
                plugin.find_lyrics(request, track, own);
            }
        });
    }

    /// Tells the app when the songs plugins find lyrics for change.
    fn tell_lyrics_search(&mut self) {
        let search = self
            .plugins
            .iter()
            .map(Plugin::lyrics_search)
            .max()
            .unwrap_or_default();
        if search != self.lyrics_search {
            self.lyrics_search = search;
            let _ = self.env.events.try_send(HostEvent::LyricsSearch(search));
        }
    }

    fn dispatch(&mut self, event: &PlayerEvent) {
        self.env.snapshot.borrow_mut().apply(event);
        self.for_each_plugin(|plugin| plugin.handle_event(event));
    }

    /// Calls `call` on each plugin in turn, skipping the rest once the app starts
    /// shutting down so slow plugins can't hold up its exit. A plugin that panics is
    /// unloaded (see [`survives`]).
    fn for_each_plugin(&mut self, mut call: impl FnMut(&mut Plugin)) {
        let stopping = &self.stopping;
        let env = &self.env;
        self.plugins.retain_mut(|plugin| {
            let keep = stopping.load(Ordering::Acquire) || survives(plugin, &env.events, &mut call);
            if !keep {
                env.forget(plugin.id());
            }
            keep
        });
    }

    fn set_enabled(&mut self, id: &str, enabled: bool) {
        if enabled {
            if !self.enabled.iter().any(|enabled| enabled == id) {
                self.enabled.push(id.to_owned());
            }
            if self.load(id) {
                self.replay_state(|plugin| plugin.id() == id);
            }
        } else {
            self.enabled.retain(|enabled| enabled != id);
            if let Some(index) = self.plugins.iter().position(|plugin| plugin.id() == id) {
                shut_down(self.plugins.remove(index), &self.env);
            }
        }
    }

    fn reload(&mut self) {
        self.unload_all();
        self.load_enabled();
        self.replay_state(|_| true);
    }

    fn load_enabled(&mut self) {
        for id in self.enabled.clone() {
            self.load(&id);
        }
    }

    /// Loads plugin `id` unless it is loaded already. Returns whether it was loaded now.
    fn load(&mut self, id: &str) -> bool {
        if self.plugins.iter().any(|plugin| plugin.id() == id) {
            return false;
        }
        let Some(info) = manifest::find(&self.plugins_dir, id) else {
            log::warn!("Plugin '{id}' is enabled but not installed");
            return false;
        };
        let emitter = Emitter::new(id, self.env.events.clone());
        if let Some(problem) = info.problem {
            emitter.error(problem);
            return false;
        }
        let allowed = self.permissions.get(id).cloned().unwrap_or_default();
        let settings = self.settings.get(id).cloned().unwrap_or_default();
        let loaded = panic::catch_unwind(AssertUnwindSafe(|| {
            Plugin::load(&info, &self.env, &allowed, settings)
        }));
        match loaded {
            Ok(Ok(plugin)) => {
                log::info!("Loaded plugin '{id}'");
                self.plugins.push(plugin);
                return true;
            }
            Ok(Err(message)) => emitter.error(message),
            Err(_) => {
                log::error!("Loading plugin '{id}' panicked");
                emitter.error(CRASH_MESSAGE.to_owned());
            }
        }
        false
    }

    /// Tells the plugins `is_new` picks, which were loaded after the app sent its
    /// state, what the player is doing, so their view of it isn't empty until the next
    /// change. The other plugins have heard it all before.
    fn replay_state(&mut self, is_new: impl Fn(&Plugin) -> bool) {
        let events = self.env.snapshot.borrow().state_events();
        self.for_each_plugin(|plugin| {
            if is_new(plugin) {
                for event in &events {
                    plugin.handle_event(event);
                }
            }
        });
    }

    fn unload_all(&mut self) {
        for plugin in std::mem::take(&mut self.plugins) {
            shut_down(plugin, &self.env);
        }
    }

    fn next_due(&self) -> Option<Instant> {
        self.plugins.iter().filter_map(Plugin::next_due).min()
    }

    fn run_due(&mut self) {
        let now = Instant::now();
        self.for_each_plugin(|plugin| plugin.run_due(now));
    }

    /// Unloads plugins that keep failing, so a broken plugin can't flood the app
    /// with errors. Their `shutdown` handlers are skipped: they are broken.
    fn remove_failing(&mut self) {
        if !self.plugins.iter().any(Plugin::is_failing) {
            return;
        }
        let (failing, healthy): (Vec<Plugin>, Vec<Plugin>) = std::mem::take(&mut self.plugins)
            .into_iter()
            .partition(Plugin::is_failing);
        self.plugins = healthy;
        for plugin in failing {
            log::warn!("Stopping plugin '{}' after repeated errors", plugin.id());
            self.env.forget(plugin.id());
            Emitter::new(plugin.id(), self.env.events.clone()).error(format!(
                "stopped after {MAX_CONSECUTIVE_ERRORS} errors in a row; fix the plugin and reload it"
            ));
        }
    }
}

/// Shown in the plugin manager when the host itself failed while running a plugin.
const CRASH_MESSAGE: &str = "stopped because of an internal error in the plugin host";

/// Calls `call` on `plugin` and returns whether the plugin may stay loaded. A panic
/// is a bug in the host rather than in the plugin, but it must not stop the host thread
/// and every other plugin with it, so the plugin is reported and dropped instead.
fn survives(
    plugin: &mut Plugin,
    events: &async_channel::Sender<HostEvent>,
    call: impl FnOnce(&mut Plugin),
) -> bool {
    if panic::catch_unwind(AssertUnwindSafe(|| call(plugin))).is_ok() {
        return true;
    }
    log::error!("Plugin '{}' made the plugin host panic", plugin.id());
    Emitter::new(plugin.id(), events.clone()).error(CRASH_MESSAGE.to_owned());
    false
}

/// Runs the plugin's `shutdown` handlers, then drops it and what it left behind.
fn shut_down(plugin: Plugin, env: &PluginEnv) {
    let id = plugin.id().to_owned();
    if panic::catch_unwind(AssertUnwindSafe(|| plugin.shutdown())).is_err() {
        log::error!("Plugin '{id}' made the plugin host panic while shutting down");
    }
    env.forget(&id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panicking_plugin_is_dropped_and_reported() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("plugins/fragile");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plugin.json"), r#"{ "name": "Fragile" }"#).unwrap();
        std::fs::write(dir.join("main.lua"), "").unwrap();
        let (events, receiver) = async_channel::unbounded();
        let services = Services::offline();
        let env = PluginEnv {
            data_dir: root.path().join("data"),
            events: events.clone(),
            snapshot: Rc::default(),
            http: services.http,
            inbox: mpsc::channel().0,
            request_ids: Rc::default(),
            presence: Rc::new(SharedPresence::new(services.presence)),
            lyrics: Rc::new(RefCell::new(LyricsRequests::new(events.clone()))),
        };
        let info = manifest::inspect(&dir).unwrap();
        let mut plugin =
            Plugin::load(&info, &env, &PluginPermissions::default(), Map::new()).unwrap();

        assert!(survives(&mut plugin, &events, |_| {}));
        assert!(!survives(&mut plugin, &events, |_| panic!("a bug")));
        assert_eq!(
            receiver.try_recv(),
            Ok(HostEvent::PluginError {
                plugin: "fragile".into(),
                message: CRASH_MESSAGE.into()
            })
        );
    }
}
