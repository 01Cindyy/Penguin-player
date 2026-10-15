//! A miniature of the app in a theme's colours: sidebar, album art and the player
//! bar, drawn from the theme's palette.

use std::cell::RefCell;

use adw::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};
use mp_theme::{Palette, Theme, ThemeScheme};

/// The preview's size in pixels.
const PREVIEW_WIDTH: f32 = 152.0;
const PREVIEW_HEIGHT: f32 = 97.0;
/// The size the miniature is drawn at before it is scaled to the preview's size.
const WIDTH: f32 = 176.0;
const HEIGHT: f32 = 112.0;

/// libadwaita's own colours, for whatever a theme leaves unset. They can't be read
/// from the CSS variables, which hold the active theme's colours.
struct AdwaitaColors {
    window_bg: &'static str,
    window_fg: &'static str,
    sidebar_bg: &'static str,
    card_bg: &'static str,
    headerbar_bg: &'static str,
}

const ADWAITA_LIGHT: AdwaitaColors = AdwaitaColors {
    window_bg: "#fafafb",
    window_fg: "rgba(0, 0, 6, 0.8)",
    sidebar_bg: "#ebebed",
    card_bg: "#ffffff",
    headerbar_bg: "#ffffff",
};

const ADWAITA_DARK: AdwaitaColors = AdwaitaColors {
    window_bg: "#222226",
    window_fg: "#ffffff",
    sidebar_bg: "#2e2e32",
    card_bg: "rgba(255, 255, 255, 0.08)",
    headerbar_bg: "#2e2e32",
};

/// The colours a preview is drawn with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreviewColors {
    pub window_bg: gdk::RGBA,
    pub window_fg: gdk::RGBA,
    pub sidebar_bg: gdk::RGBA,
    pub card_bg: gdk::RGBA,
    pub player_bar_bg: gdk::RGBA,
    pub accent: gdk::RGBA,
}

impl PreviewColors {
    /// `theme`'s colours in the scheme it would be shown in: its own, or `dark` for
    /// themes that follow the user's preference. `system_accent` stands in for an
    /// unset accent colour, as it does when the theme is applied.
    pub fn of(theme: &Theme, dark: bool, system_accent: gdk::RGBA) -> Self {
        let dark = match theme.scheme() {
            ThemeScheme::Any => dark,
            ThemeScheme::Light => false,
            ThemeScheme::Dark => true,
        };
        let manifest = &theme.manifest;
        let scheme = if dark {
            &manifest.dark
        } else {
            &manifest.light
        };
        let pick = |field: fn(&Palette) -> &Option<String>| {
            field(scheme)
                .as_deref()
                .or(field(&manifest.colors).as_deref())
                .and_then(|value| gdk::RGBA::parse(value).ok())
        };
        let defaults = if dark { &ADWAITA_DARK } else { &ADWAITA_LIGHT };
        let default = |value: &str| gdk::RGBA::parse(value).expect("default colours are valid");

        let headerbar_bg =
            pick(|p| &p.headerbar_bg).unwrap_or_else(|| default(defaults.headerbar_bg));
        Self {
            window_bg: pick(|p| &p.window_bg).unwrap_or_else(|| default(defaults.window_bg)),
            window_fg: pick(|p| &p.window_fg).unwrap_or_else(|| default(defaults.window_fg)),
            sidebar_bg: pick(|p| &p.sidebar_bg).unwrap_or_else(|| default(defaults.sidebar_bg)),
            card_bg: pick(|p| &p.card_bg).unwrap_or_else(|| default(defaults.card_bg)),
            // The player bar follows the header bar unless the theme sets it.
            player_bar_bg: pick(|p| &p.player_bar_bg).unwrap_or(headerbar_bg),
            accent: pick(|p| &p.accent).unwrap_or(system_accent),
        }
    }
}

mod imp {
    use std::cell::Cell;

    use super::*;

    #[derive(Default)]
    pub struct ThemePreview {
        pub(super) colors: RefCell<Option<PreviewColors>>,
        pub(super) radius: Cell<f32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ThemePreview {
        const NAME: &'static str = "MpThemePreview";
        type Type = super::ThemePreview;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("themepreview");
            klass.set_accessible_role(gtk::AccessibleRole::Img);
        }
    }

    impl ObjectImpl for ThemePreview {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().add_css_class("mp-theme-preview");
        }
    }

    impl WidgetImpl for ThemePreview {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let size = match orientation {
                gtk::Orientation::Horizontal => PREVIEW_WIDTH,
                _ => PREVIEW_HEIGHT,
            } as i32;
            (size, size, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            if let Some(colors) = *self.colors.borrow() {
                let scale = PREVIEW_WIDTH / WIDTH;
                snapshot.save();
                snapshot.scale(scale, PREVIEW_HEIGHT / HEIGHT);
                draw(snapshot, &colors, self.radius.get() / scale);
                snapshot.restore();
            }
        }
    }
}

glib::wrapper! {
    /// See the [module documentation](self). Styled as `mp-theme-preview`.
    pub struct ThemePreview(ObjectSubclass<imp::ThemePreview>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ThemePreview {
    /// A preview with corners of `radius` pixels, matching the card around it.
    pub fn new(colors: PreviewColors, radius: f32) -> Self {
        let preview: Self = glib::Object::new();
        preview.imp().radius.set(radius);
        preview.set_colors(colors);
        preview
    }

    pub fn set_colors(&self, colors: PreviewColors) {
        if self.imp().colors.replace(Some(colors)) != Some(colors) {
            self.queue_draw();
        }
    }
}

fn draw(snapshot: &gtk::Snapshot, colors: &PreviewColors, radius: f32) {
    let window = gsk::RoundedRect::from_rect(rect(0.0, 0.0, WIDTH, HEIGHT), radius);
    snapshot.push_rounded_clip(&window);
    snapshot.append_color(&colors.window_bg, window.bounds());
    let painter = Painter { snapshot, colors };
    painter.sidebar();
    painter.albums();
    painter.player_bar();
    snapshot.pop();

    // A hairline edge keeps light previews apart from light backgrounds.
    let edge = painter.ink(0.12);
    snapshot.append_border(&window, &[1.0; 4], &[edge; 4]);
}

const SIDEBAR_WIDTH: f32 = WIDTH * 0.3;
const PLAYER_BAR_HEIGHT: f32 = HEIGHT * 0.22;

fn rect(x: f32, y: f32, width: f32, height: f32) -> graphene::Rect {
    graphene::Rect::new(x, y, width, height)
}

/// Draws the parts of the miniature window in a theme's colours.
struct Painter<'a> {
    snapshot: &'a gtk::Snapshot,
    colors: &'a PreviewColors,
}

impl Painter<'_> {
    fn fill(&self, bounds: graphene::Rect, radius: f32, color: gdk::RGBA) {
        self.snapshot
            .push_rounded_clip(&gsk::RoundedRect::from_rect(bounds, radius));
        self.snapshot.append_color(&color, &bounds);
        self.snapshot.pop();
    }

    /// The text colour at `alpha` of its strength, for text and lines.
    fn ink(&self, alpha: f32) -> gdk::RGBA {
        let foreground = self.colors.window_fg;
        foreground.with_alpha(foreground.alpha() * alpha)
    }

    /// The sidebar with three entries, the first one selected.
    fn sidebar(&self) {
        self.snapshot.append_color(
            &self.colors.sidebar_bg,
            &rect(0.0, 0.0, SIDEBAR_WIDTH, HEIGHT - PLAYER_BAR_HEIGHT),
        );
        self.fill(
            rect(6.0, 10.0, SIDEBAR_WIDTH - 12.0, 10.0),
            4.0,
            self.ink(0.1),
        );
        self.fill(rect(10.0, 13.5, 3.0, 3.0), 1.5, self.colors.accent);
        for (index, width) in [0.55, 0.45, 0.6].into_iter().enumerate() {
            let y = 14.0 + index as f32 * 13.0;
            let x = if index == 0 { 17.0 } else { 10.0 };
            let entry = rect(x, y - 0.5, (SIDEBAR_WIDTH - 20.0) * width, 3.0);
            self.fill(entry, 1.5, self.ink(0.35));
        }
    }

    /// A heading and a row of album art: the accent at decreasing strength.
    fn albums(&self) {
        let x = SIDEBAR_WIDTH + 10.0;
        self.fill(rect(x, 12.0, 46.0, 5.0), 2.5, self.ink(0.7));
        self.fill(rect(x, 21.0, 28.0, 3.0), 1.5, self.ink(0.3));
        let art = 32.0;
        for (index, strength) in [1.0, 0.65, 0.35].into_iter().enumerate() {
            let art_x = x + index as f32 * (art + 7.0);
            let bounds = rect(art_x, 32.0, art, art);
            self.fill(bounds, 5.0, self.colors.card_bg);
            self.fill(bounds, 5.0, self.colors.accent.with_alpha(strength));
            let title = rect(art_x, 32.0 + art + 5.0, art * 0.8, 3.0);
            self.fill(title, 1.5, self.ink(0.35));
        }
    }

    /// The player bar: a separating line, the cover, the play button and the progress.
    fn player_bar(&self) {
        let top = HEIGHT - PLAYER_BAR_HEIGHT;
        let bar = rect(0.0, top, WIDTH, PLAYER_BAR_HEIGHT);
        self.snapshot.append_color(&self.colors.player_bar_bg, &bar);
        self.snapshot
            .append_color(&self.ink(0.1), &rect(0.0, top, WIDTH, 1.0));

        let button = PLAYER_BAR_HEIGHT * 0.56;
        let button_y = top + (PLAYER_BAR_HEIGHT - button) / 2.0;
        self.fill(rect(10.0, button_y, button, button), 3.0, self.ink(0.15));
        let play = rect(WIDTH / 2.0 - button / 2.0, button_y, button, button);
        self.fill(play, button / 2.0, self.colors.accent);

        let track_x = WIDTH / 2.0 + button / 2.0 + 8.0;
        let track_y = top + PLAYER_BAR_HEIGHT / 2.0 - 1.5;
        let track_width = WIDTH - track_x - 12.0;
        self.fill(
            rect(track_x, track_y, track_width, 3.0),
            1.5,
            self.ink(0.15),
        );
        let progress = rect(track_x, track_y, track_width * 0.4, 3.0);
        self.fill(progress, 1.5, self.colors.accent);
    }
}

#[cfg(test)]
mod tests {
    use mp_theme::builtin_themes;

    use super::*;

    #[test]
    fn unset_colours_fall_back_to_adwaita_and_the_system_accent() {
        let accent = gdk::RGBA::new(1.0, 0.0, 0.0, 1.0);
        let adwaita = &builtin_themes()[0];
        let light = PreviewColors::of(adwaita, false, accent);
        assert_eq!(light.accent, accent);
        assert_eq!(light.window_bg, gdk::RGBA::parse("#fafafb").unwrap());
        let dark = PreviewColors::of(adwaita, true, accent);
        assert_eq!(dark.window_bg, gdk::RGBA::parse("#222226").unwrap());
        assert_eq!(dark.player_bar_bg, dark.sidebar_bg);
    }

    #[test]
    fn scheme_palettes_override_shared_colours() {
        let mut theme = builtin_themes()[0].clone();
        theme.manifest.colors.accent = Some("#00ff00".into());
        theme.manifest.dark.accent = Some("#0000ff".into());
        theme.manifest.light.window_bg = Some("not a colour".into());
        let system = gdk::RGBA::new(1.0, 0.0, 0.0, 1.0);
        let light = PreviewColors::of(&theme, false, system);
        assert_eq!(light.accent, gdk::RGBA::parse("#00ff00").unwrap());
        assert_eq!(light.window_bg, gdk::RGBA::parse("#fafafb").unwrap());
        assert_eq!(
            PreviewColors::of(&theme, true, system).accent,
            gdk::RGBA::parse("#0000ff").unwrap()
        );

        theme.manifest.scheme = ThemeScheme::Dark;
        let forced = PreviewColors::of(&theme, false, system);
        assert_eq!(forced.accent, gdk::RGBA::parse("#0000ff").unwrap());
    }
}
