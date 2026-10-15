//! [`AppContext`]: the shared handles every part of the UI works with.

use std::cell::{Cell, Ref, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use mp_core::{APP_DIR_NAME, AppPaths, ColorSchemePreference, CornerStyle, Settings, Transparency};
use mp_library::Library;

use crate::display::DisplayOptions;
use crate::model::LibraryStore;
use crate::player::Player;
use crate::services::blur::WindowBlur;
use crate::services::covers::CoverCache;
use crate::services::library::LibraryService;
use crate::services::plugins::PluginService;
use crate::services::theme::ThemeManager;
use crate::ui::navigation::Navigator;

/// Settings changes are written to disk once they have been quiet for this long,
/// so dragging the volume slider doesn't write a file per pixel.
const SETTINGS_SAVE_DELAY: Duration = Duration::from_millis(500);

/// Everything the UI shares, created once at startup and passed around as
/// `Rc<AppContext>`. All of it lives on the GTK main thread.
pub struct AppContext {
    pub app: adw::Application,
    pub paths: AppPaths,
    /// The library as GTK list models.
    pub store: LibraryStore,
    pub player: Player,
    pub covers: CoverCache,
    pub themes: ThemeManager,
    /// Loads, scans and watches the music folder.
    pub library_service: LibraryService,
    pub plugins: PluginService,
    /// Moves the content pane between pages.
    pub navigator: Navigator,
    /// Settings that change what several widgets show; widgets bind to it.
    pub display: DisplayOptions,
    /// Main-thread database connection for quick reads and playlist edits.
    /// `None` if the database could not be opened (the user has been told).
    library: Option<Library>,
    settings: RefCell<Settings>,
    pending_save: RefCell<Option<glib::SourceId>>,
    /// Whether the last save failed, so the user is told once rather than on every change.
    save_failed: Cell<bool>,
    toasts: adw::ToastOverlay,
    window: glib::WeakRef<gtk::Window>,
    /// Blur behind the window, once the window has a surface.
    blur: RefCell<Option<Rc<WindowBlur>>>,
    this: Weak<AppContext>,
}

impl AppContext {
    /// Loads settings, opens the library and creates every service. Starting the
    /// background work (scanning, plugins, media controls) is left to [`Self::start`].
    pub fn new(app: &adw::Application) -> Rc<Self> {
        let paths = resolve_paths();
        let settings = Settings::load(&paths.settings_file());
        let library = open_library(&paths);
        let store = LibraryStore::new();
        let player = Player::new(&store, &settings);
        let themes = ThemeManager::new(&paths);

        let ctx = Rc::new_cyclic(|weak: &Weak<AppContext>| AppContext {
            app: app.clone(),
            covers: CoverCache::new(&paths),
            plugins: PluginService::new(&paths),
            library_service: LibraryService::new(),
            navigator: Navigator::new(weak.clone()),
            display: DisplayOptions::new(&settings),
            paths,
            store,
            player,
            themes,
            library: library.ok(),
            settings: RefCell::new(settings),
            pending_save: RefCell::default(),
            save_failed: Cell::new(false),
            toasts: adw::ToastOverlay::new(),
            window: glib::WeakRef::new(),
            blur: RefCell::default(),
            this: weak.clone(),
        });
        ctx.persist_player_settings();
        if ctx.library.is_none() {
            ctx.toast("Couldn’t open the music library database");
        }
        if let Some(message) = ctx.player.startup_error() {
            ctx.toast(&format!("Audio is unavailable: {message}"));
        }
        let weak = Rc::downgrade(&ctx);
        ctx.player.connect_playback_error(move |_, message| {
            if let Some(ctx) = weak.upgrade() {
                ctx.toast(message);
            }
        });
        ctx
    }

    /// Starts background work: library loading and scanning, plugins and OS media
    /// controls. Call once the main window exists, so the UI appears without waiting.
    pub fn start(self: &Rc<Self>) {
        self.library_service.start(self);
        self.plugins.start(self);
        crate::services::media_controls::start(self);
    }

    /// Stops plugins (running their shutdown handlers), saves settings and stops
    /// playback. Called once when the application quits.
    pub fn shutdown(&self) {
        // Plugins first: their shutdown handlers may still change settings.
        self.plugins.shutdown();
        self.save_settings_now();
        self.player.shutdown();
    }

    pub fn settings(&self) -> Ref<'_, Settings> {
        self.settings.borrow()
    }

    /// Changes settings and schedules a save.
    pub fn update_settings(&self, change: impl FnOnce(&mut Settings)) {
        change(&mut self.settings.borrow_mut());
        self.schedule_settings_save();
    }

    /// Writes settings immediately, e.g. before quitting.
    pub fn save_settings_now(&self) {
        if let Some(source) = self.pending_save.take() {
            source.remove();
        }
        let result = self.settings().save(&self.paths.settings_file());
        if let Err(err) = &result {
            log::error!("Could not save settings: {err}");
        }
        if !self.save_failed.replace(result.is_err()) && result.is_err() {
            self.toast("Couldn’t save your settings");
        }
    }

    fn schedule_settings_save(&self) {
        if self.pending_save.borrow().is_some() {
            return;
        }
        let ctx = self.this.clone();
        let source = glib::timeout_add_local_once(SETTINGS_SAVE_DELAY, move || {
            if let Some(ctx) = ctx.upgrade() {
                ctx.pending_save.take();
                ctx.save_settings_now();
            }
        });
        self.pending_save.replace(Some(source));
    }

    /// Switches to the theme with `id` and remembers the choice. Returns whether the
    /// theme exists; if it doesn't, the default theme is shown and nothing is saved.
    pub fn set_theme(&self, id: &str) -> bool {
        let found = self.themes.apply(id);
        if found {
            self.update_settings(|settings| settings.theme = id.to_owned());
        }
        // Themes have their own window corners, which the blur follows.
        self.apply_appearance();
        found
    }

    /// Sets the light/dark preference and remembers it. Themes made for one scheme
    /// only override it while they are active.
    pub fn set_color_scheme(&self, preference: ColorSchemePreference) {
        self.themes.set_color_scheme(preference);
        self.update_settings(|settings| settings.color_scheme = preference);
    }

    /// Sets how rounded the interface is and remembers it.
    pub fn set_corner_style(&self, corners: CornerStyle) {
        self.update_settings(|settings| settings.corners = corners);
        self.apply_appearance();
    }

    /// Turns window transparency and the blur behind it on or off, and remembers it.
    pub fn set_transparency(&self, transparency: Transparency) {
        self.update_settings(|settings| settings.transparency = transparency);
        self.apply_appearance();
    }

    /// Turns the crisper lines and soft shadows between the bars and the page on or
    /// off, and remembers it.
    pub fn set_defined_edges(&self, enabled: bool) {
        self.update_settings(|settings| settings.defined_edges = enabled);
        self.apply_appearance();
    }

    /// Applies the appearance options from the settings to the active theme and to
    /// the window's blur.
    pub fn apply_appearance(&self) {
        let (corners, transparency, edges) = {
            let settings = self.settings();
            (
                settings.corners,
                settings.transparency,
                settings.defined_edges,
            )
        };
        let blur_wanted = transparency.enabled && transparency.blur;
        let blur = self.blur.borrow();
        self.themes.set_appearance(mp_theme::Appearance {
            corners,
            transparency: transparency.enabled,
            blur: blur_wanted && blur.as_ref().is_some_and(|blur| blur.uses_css()),
            edges,
        });
        if let Some(blur) = blur.as_ref() {
            let radius = self.themes.window_radius(corners);
            blur.set(blur_wanted, radius);
        }
    }

    /// Whether the desktop lets the app blur behind its window. `None` until the
    /// window is shown.
    pub fn blur_supported(&self) -> Option<bool> {
        self.blur.borrow().as_ref().map(|blur| blur.is_supported())
    }

    /// Shows or hides the visualisers above the seek bars and remembers the choice.
    pub fn set_visualizer(&self, enabled: bool) {
        self.update_settings(|settings| settings.visualizer = enabled);
        self.display.set_visualizer(enabled);
    }

    /// Shows the playing song's full quality details, or just its format, and
    /// remembers the choice.
    pub fn set_quality_details(&self, enabled: bool) {
        self.update_settings(|settings| settings.quality_details = enabled);
        self.display.set_quality_details(enabled);
    }

    pub fn set_gapless(&self, enabled: bool) {
        self.player.set_gapless(enabled);
        self.update_settings(|settings| settings.gapless = enabled);
    }

    /// Starts or stops watching the music folder for changes and remembers the choice.
    pub fn set_watch_music_folder(&self, watch: bool) {
        self.update_settings(|settings| settings.watch_music_folder = watch);
        self.library_service.update_watcher();
    }

    /// The main-thread library connection, if the database could be opened.
    pub fn library(&self) -> Option<&Library> {
        self.library.as_ref()
    }

    /// Runs a quick database operation (such as a playlist edit) on the main-thread
    /// connection. Failures are logged and shown as "Couldn’t {action}".
    pub fn edit_library<T>(
        &self,
        action: &str,
        edit: impl FnOnce(&Library) -> mp_library::Result<T>,
    ) -> Option<T> {
        let Some(library) = self.library() else {
            self.toast(&format!("Couldn’t {action}: the library is unavailable"));
            return None;
        };
        match edit(library) {
            Ok(value) => Some(value),
            Err(err) => {
                log::error!("Couldn’t {action}: {err}");
                self.toast(&format!("Couldn’t {action}"));
                None
            }
        }
    }

    /// Re-reads playlists into the [`LibraryStore`] after an edit.
    pub fn refresh_playlists(&self) {
        self.edit_library("load playlists", |library| {
            self.store.refresh_playlists(library)
        });
    }

    /// Shows a short in-app notification. `message` is plain text, so titles and
    /// names with characters such as `&` show as they are.
    pub fn toast(&self, message: &str) {
        self.show_toast(plain_toast(message));
    }

    /// Shows a toast built by the caller, e.g. one with a button. Toast titles are
    /// Pango markup unless built with [`plain_toast`].
    pub fn show_toast(&self, toast: adw::Toast) {
        self.toasts.add_toast(toast);
    }

    /// The overlay toasts appear in; the window makes it its root content.
    pub fn toast_overlay(&self) -> &adw::ToastOverlay {
        &self.toasts
    }

    /// The main window, used as the parent of dialogs.
    pub fn window(&self) -> Option<gtk::Window> {
        self.window.upgrade()
    }

    pub(crate) fn set_window(&self, window: &impl IsA<gtk::Window>) {
        self.window.set(Some(window.upcast_ref()));
        // The compositor's blur attaches to the window's surface, which exists once
        // the window is realized.
        let this = self.this.clone();
        window
            .upcast_ref::<gtk::Window>()
            .connect_realize(move |window| {
                if let Some(ctx) = this.upgrade() {
                    ctx.blur.replace(Some(WindowBlur::new(window)));
                    ctx.apply_appearance();
                }
            });
    }

    fn persist_player_settings(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let persist = move |change: &dyn Fn(&Player, &mut Settings)| {
            if let Some(ctx) = weak.upgrade() {
                ctx.update_settings(|settings| change(&ctx.player, settings));
            }
        };
        let persist = Rc::new(persist);

        let save = persist.clone();
        self.player.connect_volume_notify(move |_| {
            save(&|player, settings| settings.volume = player.volume() as f32);
        });
        let save = persist.clone();
        self.player.connect_muted_notify(move |_| {
            save(&|player, settings| settings.muted = player.muted());
        });
        let save = persist.clone();
        self.player.connect_shuffle_notify(move |_| {
            save(&|player, settings| settings.shuffle = player.shuffle());
        });
        self.player.connect_repeat_notify(move |_| {
            persist(&|player, settings| settings.repeat = player.repeat_mode());
        });
    }
}

/// A toast whose title is shown as plain text rather than parsed as Pango markup.
pub fn plain_toast(title: &str) -> adw::Toast {
    adw::Toast::builder().title(title).use_markup(false).build()
}

/// Platform directories, or a folder in the temp directory if the home directory
/// can't be determined (the app still works, it just won't remember anything).
fn resolve_paths() -> AppPaths {
    let paths = AppPaths::resolve().unwrap_or_else(|err| {
        log::error!("{err}; keeping data in the temporary directory");
        AppPaths::from_root(&std::env::temp_dir().join(APP_DIR_NAME))
    });
    if let Err(err) = paths.ensure_dirs() {
        log::error!("Could not create the app’s directories: {err}");
    }
    paths
}

fn open_library(paths: &AppPaths) -> Result<Library, mp_library::LibraryError> {
    Library::open(&paths.library_db()).inspect_err(|err| {
        log::error!(
            "Could not open the library at {}: {err}",
            paths.library_db().display()
        );
    })
}
