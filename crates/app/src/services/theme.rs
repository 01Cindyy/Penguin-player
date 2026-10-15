//! Applying themes: the active theme's CSS sits one priority level above the app's own
//! stylesheet, and the theme decides the colour scheme. The user's appearance options
//! (corners, transparency) sit one level above the theme so they work with every theme.
//! User themes reload live when their files change, so a theme can be edited with
//! instant feedback, and the pywal theme reloads whenever pywal writes a new palette.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::time::Duration;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, gio, glib};
use mp_core::{AppPaths, ColorSchemePreference, CornerStyle};
use mp_theme::{Appearance, Theme, ThemeScheme, ThemeSource};

/// Editors often write a file in several steps; wait for them to settle.
const RELOAD_DELAY: Duration = Duration::from_millis(150);

/// Priority of theme CSS: just above the app's structural stylesheet.
const THEME_STYLE_PRIORITY: u32 = gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1;

/// Priority of the user's appearance options: above the theme they adjust.
const APPEARANCE_STYLE_PRIORITY: u32 = THEME_STYLE_PRIORITY + 1;

mod imp {
    use std::sync::OnceLock;

    use glib::subclass::Signal;

    use super::*;

    #[derive(glib::Properties)]
    #[properties(wrapper_type = super::ThemeManager)]
    pub struct ThemeManager {
        pub(super) provider: gtk::CssProvider,
        pub(super) appearance_provider: gtk::CssProvider,
        pub(super) appearance: Cell<Appearance>,
        pub(super) themes_dir: RefCell<PathBuf>,
        pub(super) themes: RefCell<Vec<Theme>>,
        pub(super) active: RefCell<Option<Theme>>,
        pub(super) color_scheme: Cell<ColorSchemePreference>,
        pub(super) forced_scheme: Cell<Option<adw::ColorScheme>>,
        pub(super) monitor: RefCell<Option<gio::FileMonitor>>,
        pub(super) pending_reload: RefCell<Option<glib::SourceId>>,
        /// The first CSS error of the stylesheet being loaded, if any.
        pub(super) parse_error: RefCell<Option<String>>,

        /// Id of the theme currently applied.
        #[property(get = Self::active_theme_id, type = String)]
        active_theme: std::marker::PhantomData<String>,
    }

    impl ThemeManager {
        fn active_theme_id(&self) -> String {
            self.active
                .borrow()
                .as_ref()
                .map(|theme| theme.id.clone())
                .unwrap_or_default()
        }
    }

    impl Default for ThemeManager {
        fn default() -> Self {
            Self {
                provider: gtk::CssProvider::new(),
                appearance_provider: gtk::CssProvider::new(),
                appearance: Cell::default(),
                themes_dir: RefCell::default(),
                themes: RefCell::default(),
                active: RefCell::default(),
                color_scheme: Cell::default(),
                forced_scheme: Cell::default(),
                monitor: RefCell::default(),
                pending_reload: RefCell::default(),
                parse_error: RefCell::default(),
                active_theme: std::marker::PhantomData,
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ThemeManager {
        const NAME: &'static str = "MpThemeManager";
        type Type = super::ThemeManager;
    }

    #[glib::derived_properties]
    impl ObjectImpl for ThemeManager {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    Signal::builder("error")
                        .param_types([String::static_type()])
                        .build(),
                ]
            })
        }

        fn constructed(&self) {
            self.parent_constructed();
            let manager = self.obj().downgrade();
            self.provider
                .connect_parsing_error(move |_, section, error| {
                    let location = section.to_str();
                    log::warn!("Theme CSS {location}: {error}");
                    if let Some(manager) = manager.upgrade() {
                        let mut first = manager.imp().parse_error.borrow_mut();
                        first.get_or_insert_with(|| format!("{location}: {error}"));
                    }
                });
        }

        fn dispose(&self) {
            if let Some(monitor) = self.monitor.take() {
                monitor.cancel();
            }
            if let Some(source) = self.pending_reload.take() {
                source.remove();
            }
        }
    }
}

glib::wrapper! {
    /// Discovers themes, applies the active one and reloads it when its files change.
    pub struct ThemeManager(ObjectSubclass<imp::ThemeManager>);
}

impl ThemeManager {
    /// Discovers built-in and user themes. Nothing is applied until [`Self::install`].
    pub fn new(paths: &AppPaths) -> Self {
        let manager: Self = glib::Object::new();
        manager.imp().themes_dir.replace(paths.themes_dir());
        manager.rediscover();
        manager
    }

    /// Attaches the theme stylesheet to `display`. libadwaita only switches its own
    /// stylesheet between light and dark, so the theme's CSS is regenerated for the
    /// new scheme whenever libadwaita switches.
    pub fn install(&self, display: &gdk::Display) {
        gtk::style_context_add_provider_for_display(
            display,
            &self.imp().provider,
            THEME_STYLE_PRIORITY,
        );
        gtk::style_context_add_provider_for_display(
            display,
            &self.imp().appearance_provider,
            APPEARANCE_STYLE_PRIORITY,
        );
        let weak = self.downgrade();
        adw::StyleManager::for_display(display).connect_dark_notify(move |_| {
            if let Some(manager) = weak.upgrade() {
                manager.load_stylesheet();
            }
        });
    }

    /// Every available theme: built-ins first, then user themes.
    pub fn themes(&self) -> Vec<Theme> {
        self.imp().themes.borrow().clone()
    }

    /// Looks for themes on disk again, e.g. after the user added one.
    pub fn rediscover(&self) {
        let pywal_dir = mp_theme::pywal_dir();
        let themes = mp_theme::discover(&self.imp().themes_dir.borrow(), pywal_dir.as_deref());
        self.imp().themes.replace(themes);
    }

    /// Applies the theme with `id`, falling back to the default theme if it doesn't
    /// exist (and reporting that through [`Self::connect_error`]). Returns whether `id`
    /// itself was found.
    pub fn apply(&self, id: &str) -> bool {
        let themes = self.imp().themes.borrow();
        let found = themes.iter().find(|theme| theme.id == id).cloned();
        let exists = found.is_some();
        let theme = found.or_else(|| {
            themes
                .iter()
                .find(|theme| theme.id == mp_theme::DEFAULT_THEME_ID)
                .or(themes.first())
                .cloned()
        });
        drop(themes);
        if !exists {
            log::warn!("Theme “{id}” not found; using the default theme");
            self.emit_error(&format!(
                "Theme “{id}” wasn’t found, so the default is used"
            ));
        }
        if let Some(theme) = theme {
            self.activate(theme);
        }
        exists
    }

    /// Applies the user's corner style and transparency on top of the active theme.
    pub fn set_appearance(&self, appearance: Appearance) {
        self.imp().appearance.set(appearance);
        self.load_appearance();
    }

    /// The active theme's window corner radius in pixels with `corners`.
    pub fn window_radius(&self, corners: CornerStyle) -> u32 {
        self.imp()
            .active
            .borrow()
            .as_ref()
            .map_or(0, |theme| theme.window_radius(corners))
    }

    /// Sets the user's light/dark preference; themes that force a scheme override it.
    pub fn set_color_scheme(&self, preference: ColorSchemePreference) {
        self.imp().color_scheme.set(preference);
        self.update_color_scheme();
    }

    /// Overrides every other colour scheme choice (used for screenshots).
    pub fn force_color_scheme(&self, scheme: Option<adw::ColorScheme>) {
        self.imp().forced_scheme.set(scheme);
        self.update_color_scheme();
    }

    /// Called with a user-facing message when a theme can't be found or loaded, or
    /// when its stylesheet has errors.
    pub fn connect_error<F: Fn(&Self, &str) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_closure(
            "error",
            false,
            glib::closure_local!(move |manager: &Self, message: &str| f(manager, message)),
        )
    }

    fn activate(&self, theme: Theme) {
        let watched = match &theme.source {
            ThemeSource::User(dir) => Some((dir.clone(), None)),
            ThemeSource::Pywal(dir) => Some((dir.clone(), Some(mp_theme::PYWAL_COLORS_FILE))),
            ThemeSource::Builtin => None,
        };
        self.report_stylesheet_problem(&theme);
        self.imp().active.replace(Some(theme));
        self.notify_active_theme();
        self.update_color_scheme();
        self.load_stylesheet();
        match watched {
            Some((dir, only)) => self.watch(&dir, only),
            None => self.stop_watching(),
        }
    }

    /// (Re)loads the active theme's CSS for the colour scheme libadwaita is showing.
    fn load_stylesheet(&self) {
        let imp = self.imp();
        let dark = adw::StyleManager::default().is_dark();
        let Some((css, name)) = imp
            .active
            .borrow()
            .as_ref()
            .map(|theme| (theme.to_css_for_scheme(dark), theme.manifest.name.clone()))
        else {
            return;
        };
        imp.parse_error.replace(None);
        imp.provider.load_from_string(&css);
        if let Some(error) = imp.parse_error.take() {
            self.emit_error(&format!("Theme “{name}” has a CSS error: {error}"));
        }
        // The theme may suggest its own opacities for the transparency option.
        self.load_appearance();
    }

    fn load_appearance(&self) {
        let imp = self.imp();
        let css = imp
            .active
            .borrow()
            .as_ref()
            .map(|theme| theme.appearance_css(imp.appearance.get()))
            .unwrap_or_default();
        imp.appearance_provider.load_from_string(&css);
    }

    fn update_color_scheme(&self) {
        let imp = self.imp();
        let theme_scheme = imp
            .active
            .borrow()
            .as_ref()
            .map_or(ThemeScheme::Any, Theme::scheme);
        let scheme = imp
            .forced_scheme
            .get()
            .unwrap_or_else(|| color_scheme_for(theme_scheme, imp.color_scheme.get()));
        adw::StyleManager::default().set_color_scheme(scheme);
    }

    fn stop_watching(&self) {
        if let Some(monitor) = self.imp().monitor.take() {
            monitor.cancel();
        }
    }

    /// Monitors the active theme's folder, or just the file `only` in it.
    fn watch(&self, dir: &Path, only: Option<&'static str>) {
        self.stop_watching();
        let monitor = match gio::File::for_path(dir)
            .monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
        {
            Ok(monitor) => monitor,
            Err(err) => {
                log::warn!("Can’t watch theme folder {}: {err}", dir.display());
                return;
            }
        };
        let weak = self.downgrade();
        monitor.connect_changed(move |_, file, other_file, event| {
            let relevant = only.is_none_or(|name| {
                std::iter::once(file)
                    .chain(other_file)
                    .any(|file| file.basename().is_some_and(|base| base == Path::new(name)))
            });
            if relevant
                && event != gio::FileMonitorEvent::AttributeChanged
                && let Some(manager) = weak.upgrade()
            {
                manager.schedule_reload();
            }
        });
        self.imp().monitor.replace(Some(monitor));
    }

    fn schedule_reload(&self) {
        if let Some(source) = self.imp().pending_reload.take() {
            source.remove();
        }
        let weak = self.downgrade();
        let source = glib::timeout_add_local_once(RELOAD_DELAY, move || {
            if let Some(manager) = weak.upgrade() {
                manager.imp().pending_reload.take();
                manager.reload_active();
            }
        });
        self.imp().pending_reload.replace(Some(source));
    }

    /// Reads the active theme from disk again after its files changed.
    fn reload_active(&self) {
        let source = self
            .imp()
            .active
            .borrow()
            .as_ref()
            .map(|t| t.source.clone());
        let loaded = match source {
            Some(ThemeSource::User(dir)) => mp_theme::load_theme(&dir),
            Some(ThemeSource::Pywal(dir)) => mp_theme::load_pywal_theme(&dir),
            _ => return,
        };
        match loaded {
            Ok(theme) => {
                log::info!("Reloaded theme “{}”", theme.id);
                if let Some(slot) = self
                    .imp()
                    .themes
                    .borrow_mut()
                    .iter_mut()
                    .find(|existing| existing.id == theme.id)
                {
                    *slot = theme.clone();
                }
                self.report_stylesheet_problem(&theme);
                self.imp().active.replace(Some(theme));
                self.update_color_scheme();
                self.load_stylesheet();
            }
            Err(err) => {
                log::warn!("Theme reload failed: {err}");
                self.emit_error(&format!("Theme not reloaded: {err}"));
            }
        }
    }

    fn report_stylesheet_problem(&self, theme: &Theme) {
        if let Some(problem) = &theme.stylesheet_problem {
            let name = &theme.manifest.name;
            self.emit_error(&format!(
                "Theme “{name}” is shown without its stylesheet: {problem}"
            ));
        }
    }

    fn emit_error(&self, message: &str) {
        self.emit_by_name::<()>("error", &[&message]);
    }
}

/// The libadwaita colour scheme for a theme's scheme and the user's preference.
pub fn color_scheme_for(theme: ThemeScheme, preference: ColorSchemePreference) -> adw::ColorScheme {
    match (theme, preference) {
        (ThemeScheme::Light, _) | (ThemeScheme::Any, ColorSchemePreference::Light) => {
            adw::ColorScheme::ForceLight
        }
        (ThemeScheme::Dark, _) | (ThemeScheme::Any, ColorSchemePreference::Dark) => {
            adw::ColorScheme::ForceDark
        }
        (ThemeScheme::Any, ColorSchemePreference::System) => adw::ColorScheme::Default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_scheme_overrides_preference() {
        assert_eq!(
            color_scheme_for(ThemeScheme::Dark, ColorSchemePreference::Light),
            adw::ColorScheme::ForceDark
        );
        assert_eq!(
            color_scheme_for(ThemeScheme::Any, ColorSchemePreference::Light),
            adw::ColorScheme::ForceLight
        );
        assert_eq!(
            color_scheme_for(ThemeScheme::Any, ColorSchemePreference::System),
            adw::ColorScheme::Default
        );
    }
}
