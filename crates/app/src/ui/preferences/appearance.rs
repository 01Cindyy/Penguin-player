//! The Appearance page: a gallery of themes, the colour scheme, corner style, edges,
//! window transparency, and what the player bar shows.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use mp_core::{ColorSchemePreference, CornerStyle, Transparency};
use mp_theme::{Theme, ThemeScheme};

use crate::context::AppContext;
use crate::ui::icons;
use crate::ui::widgets::discover::{PreviewColors, ThemePreview};
use crate::ui::{disconnect_on_destroy, keep_alive, set_css_class};

/// Corner radius of the previews, in pixels.
const PREVIEW_RADIUS: f32 = 10.0;

const SCHEMES: [(ColorSchemePreference, &str, &str); 3] = [
    (ColorSchemePreference::System, "system", "System"),
    (ColorSchemePreference::Light, "light", "Light"),
    (ColorSchemePreference::Dark, "dark", "Dark"),
];

const CORNER_STYLES: [(CornerStyle, &str, &str); 3] = [
    (CornerStyle::Square, "square", "Square"),
    (CornerStyle::Slight, "slight", "Slight"),
    (CornerStyle::Round, "round", "Round"),
];

pub(super) fn page(ctx: &Rc<AppContext>) -> adw::PreferencesPage {
    let appearance = Rc::new(Appearance::new(ctx));
    appearance.rebuild_gallery();
    appearance.update_scheme_row();
    appearance.connect_signals();

    let page = adw::PreferencesPage::builder()
        .name("appearance")
        .title("Appearance")
        .icon_name(icons::APPEARANCE)
        .build();
    page.add(&appearance.theme_group);
    page.add(&appearance.scheme_group);
    page.add(&appearance.transparency_group);
    page.add(&now_playing_group(ctx));
    appearance.follow_style(&page);
    keep_alive(&page, appearance);
    page
}

fn now_playing_group(ctx: &Rc<AppContext>) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title("Now Playing")
        .build();
    group.add(&switch_row(
        ctx,
        "Visualiser",
        "Bars that move with the music above the seek bar, hidden in narrow windows",
        ctx.settings().visualizer,
        AppContext::set_visualizer,
    ));
    group.add(&switch_row(
        ctx,
        "Full Quality Details",
        "The playing song’s bit depth, sample rate and bit rate, not just its format",
        ctx.settings().quality_details,
        AppContext::set_quality_details,
    ));
    group
}

/// A switch that calls `set` on the context whenever it is flipped.
fn switch_row(
    ctx: &Rc<AppContext>,
    title: &str,
    subtitle: &str,
    active: bool,
    set: fn(&AppContext, bool),
) -> adw::SwitchRow {
    let row = adw::SwitchRow::builder()
        .title(title)
        .subtitle(subtitle)
        .active(active)
        .build();
    let weak = Rc::downgrade(ctx);
    row.connect_active_notify(move |row| {
        if let Some(ctx) = weak.upgrade() {
            set(&ctx, row.is_active());
        }
    });
    row
}

/// One theme in the gallery.
struct ThemeCard {
    theme: Theme,
    child: gtk::FlowBoxChild,
    card: gtk::Box,
    preview: ThemePreview,
    check: gtk::Image,
}

/// The page's state, owned by the page (see [`keep_alive`]).
struct Appearance {
    ctx: Rc<AppContext>,
    theme_group: adw::PreferencesGroup,
    gallery: gtk::FlowBox,
    open_folder: gtk::Button,
    reload: gtk::Button,
    cards: RefCell<Vec<ThemeCard>>,
    scheme_group: adw::PreferencesGroup,
    scheme_row: adw::ActionRow,
    scheme: adw::ToggleGroup,
    corners: adw::ToggleGroup,
    transparency_group: adw::PreferencesGroup,
    transparency_row: adw::SwitchRow,
    blur_row: adw::SwitchRow,
}

impl Appearance {
    fn new(ctx: &Rc<AppContext>) -> Self {
        let gallery = gtk::FlowBox::builder()
            .homogeneous(true)
            .min_children_per_line(2)
            .max_children_per_line(4)
            .column_spacing(12)
            .row_spacing(12)
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["mp-theme-gallery"])
            .build();
        gallery.update_property(&[gtk::accessible::Property::Label("Themes")]);

        let open_folder = super::header_button(icons::OPEN_FOLDER, "Open Themes Folder");
        let reload = super::header_button(icons::REFRESH, "Reload Themes");
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        actions.append(&open_folder);
        actions.append(&reload);
        let theme_group = adw::PreferencesGroup::builder()
            .title("Theme")
            .description(
                "Add themes to the themes folder. The theme in use reloads by itself \
                 while you edit its files.",
            )
            .header_suffix(&actions)
            .build();
        theme_group.add(&gallery);

        let scheme = toggle_group(
            "Colour Scheme",
            SCHEMES.iter().map(|(_, name, label)| (*name, *label)),
        );
        let scheme_row = adw::ActionRow::builder()
            .title("Colour Scheme")
            .use_markup(false)
            .build();
        scheme_row.add_suffix(&scheme);
        let corners = toggle_group(
            "Corners",
            CORNER_STYLES.iter().map(|(_, name, label)| (*name, *label)),
        );
        let corners_row = adw::ActionRow::builder()
            .title("Corners")
            .subtitle("How rounded windows, cards and buttons are")
            .build();
        corners_row.add_suffix(&corners);
        let scheme_group = adw::PreferencesGroup::new();
        scheme_group.add(&scheme_row);
        scheme_group.add(&corners_row);
        scheme_group.add(&switch_row(
            ctx,
            "Defined Edges",
            "Crisper lines and a soft shadow between the bars and the page",
            ctx.settings().defined_edges,
            AppContext::set_defined_edges,
        ));

        let transparency_row = adw::SwitchRow::builder()
            .title("Translucent Window")
            .subtitle("Let your wallpaper show through, with any theme")
            .build();
        let blur_row = adw::SwitchRow::builder()
            .title("Blur Behind Window")
            .build();
        let transparency_group = adw::PreferencesGroup::builder()
            .title("Transparency")
            .build();
        transparency_group.add(&transparency_row);
        transparency_group.add(&blur_row);

        Self {
            ctx: ctx.clone(),
            theme_group,
            gallery,
            open_folder,
            reload,
            cards: RefCell::default(),
            scheme_group,
            scheme_row,
            scheme,
            corners,
            transparency_group,
            transparency_row,
            blur_row,
        }
    }

    fn connect_signals(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.gallery.connect_child_activated(move |_, child| {
            let Some(appearance) = weak.upgrade() else {
                return;
            };
            let id = appearance
                .cards
                .borrow()
                .get(child.index() as usize)
                .map(|card| card.theme.id.clone());
            if let Some(id) = id {
                appearance.ctx.set_theme(&id);
            }
        });

        let weak = Rc::downgrade(self);
        self.open_folder.connect_clicked(move |button| {
            if let Some(appearance) = weak.upgrade() {
                super::open_folder(&appearance.ctx.paths.themes_dir(), button);
            }
        });
        let weak = Rc::downgrade(self);
        self.reload.connect_clicked(move |button| {
            if let Some(appearance) = weak.upgrade() {
                appearance.reload_themes();
                super::toast(button, "Themes reloaded");
            }
        });

        let selected = SCHEMES
            .iter()
            .find(|(preference, ..)| *preference == self.ctx.settings().color_scheme)
            .map_or("system", |(_, name, _)| name);
        self.scheme.set_active_name(Some(selected));
        let weak = Rc::downgrade(self);
        self.scheme.connect_active_name_notify(move |toggles| {
            let Some(appearance) = weak.upgrade() else {
                return;
            };
            let active = toggles.active_name();
            let preference = SCHEMES
                .iter()
                .find(|(_, name, _)| active.as_deref() == Some(*name))
                .map(|(preference, ..)| *preference);
            if let Some(preference) = preference {
                appearance.ctx.set_color_scheme(preference);
            }
        });

        self.connect_corners();
        self.connect_transparency();
    }

    fn connect_corners(self: &Rc<Self>) {
        let current = self.ctx.settings().corners;
        let selected = CORNER_STYLES
            .iter()
            .find(|(style, ..)| *style == current)
            .map_or("round", |(_, name, _)| name);
        self.corners.set_active_name(Some(selected));
        let weak = Rc::downgrade(self);
        self.corners.connect_active_name_notify(move |toggles| {
            let Some(appearance) = weak.upgrade() else {
                return;
            };
            let active = toggles.active_name();
            let style = CORNER_STYLES
                .iter()
                .find(|(_, name, _)| active.as_deref() == Some(*name))
                .map(|(style, ..)| *style);
            if let Some(style) = style {
                appearance.ctx.set_corner_style(style);
            }
        });
    }

    fn connect_transparency(self: &Rc<Self>) {
        let current = self.ctx.settings().transparency;
        self.transparency_row.set_active(current.enabled);
        self.blur_row.set_active(current.blur);
        // Only now that the rows show the saved settings.
        self.update_blur_row();
        for row in [&self.transparency_row, &self.blur_row] {
            let weak = Rc::downgrade(self);
            row.connect_active_notify(move |_| {
                if let Some(appearance) = weak.upgrade() {
                    appearance.ctx.set_transparency(Transparency {
                        enabled: appearance.transparency_row.is_active(),
                        blur: appearance.blur_row.is_active(),
                    });
                    appearance.update_blur_row();
                }
            });
        }
    }

    /// Keeps previews and marks current while the page is open: the colour scheme and
    /// system accent change previews, and the active theme can change from elsewhere.
    fn follow_style(self: &Rc<Self>, page: &adw::PreferencesPage) {
        let style = adw::StyleManager::default();
        let weak = Rc::downgrade(self);
        let recolour = move |_: &adw::StyleManager| {
            if let Some(appearance) = weak.upgrade() {
                appearance.recolour_previews();
            }
        };
        let handlers = [
            style.connect_dark_notify(recolour.clone()),
            style.connect_accent_color_rgba_notify(recolour),
        ];
        for handler in handlers {
            disconnect_on_destroy(page, &style, handler);
        }

        let weak = Rc::downgrade(self);
        let handler = self.ctx.themes.connect_active_theme_notify(move |_| {
            if let Some(appearance) = weak.upgrade() {
                appearance.mark_active_theme();
                appearance.update_scheme_row();
            }
        });
        disconnect_on_destroy(page, &self.ctx.themes, handler);
    }

    /// Looks for new or changed themes, and applies the current one again so edits
    /// to it show.
    fn reload_themes(&self) {
        self.ctx.themes.rediscover();
        let current = self.ctx.settings().theme.clone();
        self.ctx.set_theme(&current);
        self.rebuild_gallery();
        self.update_scheme_row();
    }

    fn rebuild_gallery(&self) {
        self.gallery.remove_all();
        let colors = preview_context();
        let cards: Vec<ThemeCard> = self
            .ctx
            .themes
            .themes()
            .into_iter()
            .map(|theme| theme_card(theme, colors))
            .collect();
        for card in &cards {
            self.gallery.append(&card.child);
        }
        self.cards.replace(cards);
        self.mark_active_theme();
    }

    fn recolour_previews(&self) {
        let (dark, accent) = preview_context();
        for card in self.cards.borrow().iter() {
            card.preview
                .set_colors(PreviewColors::of(&card.theme, dark, accent));
        }
    }

    fn mark_active_theme(&self) {
        let active = self.ctx.themes.active_theme();
        for card in self.cards.borrow().iter() {
            let is_active = card.theme.id == active;
            set_css_class(&card.card, "mp-active", is_active);
            card.check.set_visible(is_active);
            let mut label = format!("{} theme", card.theme.manifest.name);
            if is_active {
                label.push_str(", in use");
            }
            card.child
                .update_property(&[gtk::accessible::Property::Label(&label)]);
        }
    }

    /// Blur only applies to a translucent window, and only some desktops offer it.
    fn update_blur_row(&self) {
        let translucent = self.transparency_row.is_active();
        let subtitle = match self.ctx.blur_supported() {
            Some(false) => {
                "Your desktop doesn’t let apps ask for blur. On Hyprland, blur follows \
                 your Hyprland settings."
            }
            _ if !translucent => "Turn on the translucent window to use blur",
            _ => "Soften what shows through the window",
        };
        self.blur_row
            .set_sensitive(translucent && self.ctx.blur_supported() != Some(false));
        self.blur_row.set_subtitle(subtitle);
    }

    /// Themes made for one scheme decide it themselves while they are in use.
    fn update_scheme_row(&self) {
        let active = self.ctx.themes.active_theme();
        let forced = self
            .cards
            .borrow()
            .iter()
            .find(|card| card.theme.id == active)
            .and_then(|card| {
                let scheme = match card.theme.scheme() {
                    ThemeScheme::Any => return None,
                    ThemeScheme::Light => "light",
                    ThemeScheme::Dark => "dark",
                };
                Some(format!(
                    "The “{}” theme is always {scheme}",
                    card.theme.manifest.name
                ))
            });
        self.scheme.set_sensitive(forced.is_none());
        let subtitle =
            forced.unwrap_or_else(|| "Follow the system, or always use light or dark".to_owned());
        self.scheme_row.set_subtitle(&subtitle);
    }
}

/// A row of mutually exclusive toggles, given as `(name, label)` pairs.
fn toggle_group<'a>(
    accessible_label: &str,
    toggles: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> adw::ToggleGroup {
    let group = adw::ToggleGroup::builder()
        .valign(gtk::Align::Center)
        .homogeneous(true)
        .build();
    for (name, label) in toggles {
        group.add(adw::Toggle::builder().name(name).label(label).build());
    }
    group
        .upcast_ref::<gtk::Widget>()
        .update_property(&[gtk::accessible::Property::Label(accessible_label)]);
    group
}

/// The current scheme and system accent, which decide how previews look.
fn preview_context() -> (bool, gtk::gdk::RGBA) {
    let style = adw::StyleManager::default();
    (style.is_dark(), style.accent_color_rgba())
}

/// A gallery entry: the preview with a check mark while in use, the name and author.
fn theme_card(theme: Theme, (dark, accent): (bool, gtk::gdk::RGBA)) -> ThemeCard {
    let manifest = &theme.manifest;
    let preview = ThemePreview::new(PreviewColors::of(&theme, dark, accent), PREVIEW_RADIUS);
    let check = gtk::Image::builder()
        .icon_name(icons::CHECK)
        .halign(gtk::Align::End)
        .valign(gtk::Align::Start)
        .visible(false)
        .css_classes(["mp-theme-check"])
        .accessible_role(gtk::AccessibleRole::Presentation)
        .build();
    let overlay = gtk::Overlay::builder()
        .child(&preview)
        .halign(gtk::Align::Center)
        .build();
    overlay.add_overlay(&check);

    let name = gtk::Label::builder()
        .label(&manifest.name)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(1)
        .css_classes(["mp-theme-name"])
        .build();
    let details = gtk::Label::builder()
        .label(scheme_label(theme.scheme()))
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(1)
        .css_classes(["caption", "dim-label"])
        .build();

    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .css_classes(["mp-theme-card"])
        .build();
    card.append(&overlay);
    card.append(&name);
    card.append(&details);

    let byline = (!manifest.author.is_empty()).then(|| format!("By {}", manifest.author));
    let tooltip: Vec<&str> = [Some(manifest.description.as_str()), byline.as_deref()]
        .into_iter()
        .flatten()
        .filter(|line| !line.is_empty())
        .collect();
    let child = gtk::FlowBoxChild::builder()
        .child(&card)
        .tooltip_text(tooltip.join("\n"))
        .build();
    if !manifest.description.is_empty() {
        child.update_property(&[gtk::accessible::Property::Description(
            &manifest.description,
        )]);
    }
    ThemeCard {
        theme,
        child,
        card,
        preview,
        check,
    }
}

fn scheme_label(scheme: ThemeScheme) -> &'static str {
    match scheme {
        ThemeScheme::Any => "Light and dark",
        ThemeScheme::Light => "Light only",
        ThemeScheme::Dark => "Dark only",
    }
}
