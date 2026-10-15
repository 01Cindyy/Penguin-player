//! Runs Lua plugins through [`mp_plugins::PluginHost`] and connects them to the player
//! and the UI.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use mp_core::app_info::DISCORD_APPLICATION_ID;
use mp_core::{AppPaths, PlayerCommand, PluginPermissions, Track};
use mp_plugins::{
    HostConfig, HostEvent, LyricsSearch, OwnLyrics, PluginHost, PluginInfo, Services,
};
use serde_json::Value;

use crate::context::AppContext;

/// Longest plugin error shown in a toast; the full text is kept for the settings page.
const TOAST_ERROR_CHARS: usize = 120;

mod imp {
    use std::sync::OnceLock;

    use gtk::glib::subclass::Signal;

    use super::*;

    #[derive(Default)]
    pub struct PluginService {
        pub(super) plugins_dir: OnceCell<PathBuf>,
        pub(super) data_dir: OnceCell<PathBuf>,
        pub(super) host: RefCell<Option<PluginHost>>,
        /// What plugins ask of the app. Kept here too so `shutdown` can apply what the
        /// `shutdown` handlers ask after the main loop has stopped.
        pub(super) events: RefCell<Option<async_channel::Receiver<HostEvent>>>,
        pub(super) context: RefCell<Weak<AppContext>>,
        /// The most recent error reported by each plugin, by plugin id.
        pub(super) errors: RefCell<HashMap<String, String>>,
        /// Where the answers to lyrics requests go, by request id.
        pub(super) lyrics_requests: RefCell<HashMap<u64, async_channel::Sender<Option<String>>>>,
        pub(super) last_lyrics_request: Cell<u64>,
        /// The songs running plugins find lyrics for.
        pub(super) lyrics_search: Cell<LyricsSearch>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PluginService {
        const NAME: &'static str = "MpPluginService";
        type Type = super::PluginService;
    }

    impl ObjectImpl for PluginService {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| vec![Signal::builder("changed").build()])
        }
    }
}

glib::wrapper! {
    /// Owns the plugin host. Player events are forwarded to plugins, and plugin
    /// requests come back as player commands and toasts.
    pub struct PluginService(ObjectSubclass<imp::PluginService>);
}

impl PluginService {
    pub fn new(paths: &AppPaths) -> Self {
        let service: Self = glib::Object::new();
        let imp = service.imp();
        imp.plugins_dir
            .set(paths.plugins_dir())
            .expect("a new service has no folders yet");
        imp.data_dir
            .set(paths.plugin_data_dir())
            .expect("a new service has no folders yet");
        service
    }

    /// Starts the host thread with the plugins enabled in settings.
    pub(crate) fn start(&self, ctx: &Rc<AppContext>) {
        let imp = self.imp();
        imp.context.replace(Rc::downgrade(ctx));
        self.switch_off_plugins_wanting_more(ctx);
        let config = {
            let settings = ctx.settings();
            HostConfig {
                plugins_dir: self.plugins_dir().clone(),
                data_dir: self.data_dir().clone(),
                enabled: settings.enabled_plugins.clone(),
                permissions: settings.plugin_permissions.clone().into_iter().collect(),
                settings: settings.plugin_settings.clone().into_iter().collect(),
                services: Services::online(DISCORD_APPLICATION_ID),
            }
        };
        let (host, events) = PluginHost::start(config);
        imp.host.replace(Some(host));
        imp.events.replace(Some(events.clone()));
        self.send_player_state(ctx);

        let weak = Rc::downgrade(ctx);
        ctx.player.connect_player_event(move |event| {
            if let Some(ctx) = weak.upgrade()
                && let Some(host) = ctx.plugins.imp().host.borrow().as_ref()
            {
                host.dispatch(event.clone());
            }
        });

        let weak = Rc::downgrade(ctx);
        glib::spawn_future_local(async move {
            while let Ok(event) = events.recv().await {
                let Some(ctx) = weak.upgrade() else { break };
                ctx.plugins.handle(&ctx, event);
            }
        });
    }

    /// Every plugin folder on disk, for the plugin manager. Reads the plugins
    /// directory, so call it when the list is shown rather than on every redraw.
    pub fn discover(&self) -> Vec<PluginInfo> {
        mp_plugins::discover(self.plugins_dir())
    }

    pub fn plugins_dir(&self) -> &PathBuf {
        self.imp()
            .plugins_dir
            .get()
            .expect("the folders are set when the service is created")
    }

    fn data_dir(&self) -> &PathBuf {
        self.imp()
            .data_dir
            .get()
            .expect("the folders are set when the service is created")
    }

    pub fn is_enabled(&self, id: &str) -> bool {
        self.context()
            .is_some_and(|ctx| ctx.settings().enabled_plugins.iter().any(|p| p == id))
    }

    /// Loads or unloads a plugin and remembers the choice. Does nothing if the plugin
    /// already is in that state, e.g. while the plugin manager shows its switches.
    pub fn set_enabled(&self, id: &str, enabled: bool) {
        let Some(ctx) = self.context() else { return };
        if self.is_enabled(id) == enabled {
            return;
        }
        ctx.update_settings(|settings| {
            settings.enabled_plugins.retain(|existing| existing != id);
            if enabled {
                settings.enabled_plugins.push(id.to_owned());
            }
        });
        if !enabled {
            self.imp().errors.borrow_mut().remove(id);
        }
        if let Some(host) = self.imp().host.borrow().as_ref() {
            host.set_enabled(id, enabled);
        }
        self.emit_changed();
    }

    /// What the user allowed plugin `id` to do beyond the sandbox.
    pub fn permissions(&self, id: &str) -> PluginPermissions {
        self.context()
            .and_then(|ctx| ctx.settings().plugin_permissions.get(id).cloned())
            .unwrap_or_default()
    }

    /// Allows plugin `id` to do what `permissions` say, from its next load on, such as
    /// before switching it on.
    pub fn allow(&self, id: &str, permissions: &PluginPermissions) {
        let Some(ctx) = self.context() else { return };
        ctx.update_settings(|settings| {
            settings
                .plugin_permissions
                .insert(id.to_owned(), permissions.clone());
        });
        if let Some(host) = self.imp().host.borrow().as_ref() {
            host.set_permissions(id, permissions.clone());
        }
    }

    /// The user's choice for plugin `id`'s setting `key`; `None` while it is the default.
    pub fn setting(&self, id: &str, key: &str) -> Option<Value> {
        let ctx = self.context()?;
        let settings = ctx.settings();
        settings.plugin_settings.get(id)?.get(key).cloned()
    }

    /// Changes plugin `id`'s setting `key`, or puts it back to its default for `None`.
    pub fn set_setting(&self, id: &str, key: &str, value: Option<Value>) {
        let Some(ctx) = self.context() else { return };
        ctx.update_settings(|settings| {
            let values = settings.plugin_settings.entry(id.to_owned()).or_default();
            match &value {
                Some(value) => values.insert(key.to_owned(), value.clone()),
                None => values.remove(key),
            };
            if values.is_empty() {
                settings.plugin_settings.remove(id);
            }
        });
        if let Some(host) = self.imp().host.borrow().as_ref() {
            host.set_setting(id, key, value);
        }
    }

    /// The songs running plugins find lyrics for.
    pub fn lyrics_search(&self) -> LyricsSearch {
        self.imp().lyrics_search.get()
    }

    /// Asks the plugins that look up songs with `own` lyrics for `track`'s: LRC or plain
    /// text, or `None` if they found none.
    pub async fn find_lyrics(&self, track: Track, own: OwnLyrics) -> Option<String> {
        let imp = self.imp();
        let request = imp.last_lyrics_request.get() + 1;
        imp.last_lyrics_request.set(request);
        let (sender, answer) = async_channel::bounded(1);
        {
            let host = imp.host.borrow();
            let host = host.as_ref()?;
            imp.lyrics_requests.borrow_mut().insert(request, sender);
            host.find_lyrics(request, track, own);
        }
        answer.recv().await.ok().flatten()
    }

    /// Plugins switched on before they asked for more than the user allowed, such as
    /// after an update, are switched off until the user allows it.
    fn switch_off_plugins_wanting_more(&self, ctx: &AppContext) {
        let plugins = self.discover();
        let wanting: Vec<String> = {
            let settings = ctx.settings();
            settings
                .enabled_plugins
                .iter()
                .filter(|id| {
                    let allowed = settings
                        .plugin_permissions
                        .get(*id)
                        .cloned()
                        .unwrap_or_default();
                    plugins
                        .iter()
                        .find(|plugin| plugin.id == **id)
                        .is_some_and(|plugin| !allowed.covers(&plugin.manifest.permissions))
                })
                .cloned()
                .collect()
        };
        if wanting.is_empty() {
            return;
        }
        ctx.update_settings(|settings| {
            settings.enabled_plugins.retain(|id| !wanting.contains(id));
        });
        let mut errors = self.imp().errors.borrow_mut();
        for id in wanting {
            log::warn!("Plugin “{id}” asks for more than it was allowed; switching it off");
            errors.insert(
                id,
                "switched off because it now asks for more access; switch it on again to \
                 see what it asks for"
                    .to_owned(),
            );
        }
    }

    /// Unloads every plugin, running their `shutdown` handlers, and stops the host
    /// thread. Settings the handlers change (such as a sleep timer putting the volume
    /// back) are applied, so they can be saved; anything else is moot as the app quits.
    pub(crate) fn shutdown(&self) {
        // Dropping the host waits for the handlers.
        self.imp().host.replace(None);
        let (Some(events), Some(ctx)) = (self.imp().events.take(), self.context()) else {
            return;
        };
        for command in settings_commands(&events) {
            ctx.player.execute(command);
        }
    }

    /// Reloads every enabled plugin from disk, e.g. after editing one.
    pub fn reload(&self) {
        self.imp().errors.borrow_mut().clear();
        if let Some(host) = self.imp().host.borrow().as_ref() {
            host.reload();
        }
        self.emit_changed();
    }

    /// Tells the host what the player is doing, so the plugins' view of it isn't
    /// empty until the next change. Events from the player follow from here on, and
    /// the host passes the state on to plugins loaded later.
    fn send_player_state(&self, ctx: &AppContext) {
        if let Some(host) = self.imp().host.borrow().as_ref() {
            for event in ctx.player.state_events() {
                host.dispatch(event);
            }
        }
    }

    /// The last error a plugin reported since it was (re)loaded.
    pub fn last_error(&self, id: &str) -> Option<String> {
        self.imp().errors.borrow().get(id).cloned()
    }

    /// Called whenever plugin state shown in the UI changes (enabled set, errors, or
    /// the songs plugins find lyrics for).
    pub fn connect_changed<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_closure(
            "changed",
            false,
            glib::closure_local!(move |service: &Self| f(service)),
        )
    }

    fn handle(&self, ctx: &AppContext, event: HostEvent) {
        match event {
            HostEvent::Command { plugin, command } => {
                log::debug!("Plugin “{plugin}” requested {command:?}");
                ctx.player.execute(command);
            }
            HostEvent::Notify { title, body, .. } => match body {
                Some(body) if !body.is_empty() => ctx.toast(&format!("{title} — {body}")),
                _ => ctx.toast(&title),
            },
            HostEvent::PluginError { plugin, message } => {
                log::warn!("Plugin “{plugin}”: {message}");
                ctx.toast(&format!(
                    "Plugin “{plugin}”: {}",
                    first_line_truncated(&message, TOAST_ERROR_CHARS)
                ));
                self.imp().errors.borrow_mut().insert(plugin, message);
                self.emit_changed();
            }
            HostEvent::Lyrics { request, lyrics } => {
                if let Some(answer) = self.imp().lyrics_requests.borrow_mut().remove(&request) {
                    let _ = answer.try_send(lyrics);
                }
            }
            HostEvent::LyricsSearch(search) => {
                self.imp().lyrics_search.set(search);
                self.emit_changed();
            }
        }
    }

    fn emit_changed(&self) {
        self.emit_by_name::<()>("changed", &[]);
    }

    fn context(&self) -> Option<Rc<AppContext>> {
        self.imp().context.borrow().upgrade()
    }
}

/// Takes every event waiting in `events` and returns the commands among them that
/// change a saved setting.
fn settings_commands(events: &async_channel::Receiver<HostEvent>) -> Vec<PlayerCommand> {
    std::iter::from_fn(|| events.try_recv().ok())
        .filter_map(|event| match event {
            HostEvent::Command {
                command:
                    command @ (PlayerCommand::SetVolume { .. }
                    | PlayerCommand::SetShuffle(_)
                    | PlayerCommand::SetRepeat(_)),
                ..
            } => Some(command),
            _ => None,
        })
        .collect()
}

fn first_line_truncated(text: &str, max_chars: usize) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    match line.char_indices().nth(max_chars) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_settings_are_applied_at_shutdown() {
        let (sender, events) = async_channel::unbounded();
        let command = |command| HostEvent::Command {
            plugin: "sleep-timer".into(),
            command,
        };
        for event in [
            command(PlayerCommand::SetVolume {
                volume: 0.8,
                unmute: false,
            }),
            HostEvent::Notify {
                plugin: "sleep-timer".into(),
                title: "Bye".into(),
                body: None,
            },
            command(PlayerCommand::Quit),
            command(PlayerCommand::Next),
            command(PlayerCommand::SetRepeat(mp_core::RepeatMode::All)),
            command(PlayerCommand::SetShuffle(true)),
        ] {
            sender.try_send(event).unwrap();
        }
        assert_eq!(
            settings_commands(&events),
            [
                PlayerCommand::SetVolume {
                    volume: 0.8,
                    unmute: false,
                },
                PlayerCommand::SetRepeat(mp_core::RepeatMode::All),
                PlayerCommand::SetShuffle(true),
            ]
        );
        assert!(events.is_empty());
    }

    #[test]
    fn error_toasts_are_short() {
        assert_eq!(first_line_truncated("bad\ntraceback", 10), "bad");
        assert_eq!(first_line_truncated("abcdef", 3), "abc…");
        assert_eq!(first_line_truncated("", 3), "");
    }
}
