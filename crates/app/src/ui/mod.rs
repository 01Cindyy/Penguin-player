//! Everything the user sees: the main window, its panels, pages and shared widgets.

pub mod context_menu;
pub mod dialogs;
pub mod folder_banner;
pub mod icons;
pub mod navigation;
pub mod onboarding;
pub mod pages;
pub mod player_bar;
pub mod preferences;
pub mod queue_panel;
pub mod shortcuts;
pub mod sidebar;
pub mod track_actions;
pub mod widgets;
pub mod window;

use gtk::prelude::*;

/// A breakpoint that applies while its window or `adw::BreakpointBin` is at most
/// `width` sp wide.
pub fn max_width_breakpoint(width: f64) -> adw::Breakpoint {
    adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        width,
        adw::LengthUnit::Sp,
    ))
}

/// Gives `widget` the `mp-narrow` CSS class while `breakpoint` applies, for styles
/// such as narrower gutters.
pub fn mark_narrow_while(breakpoint: &adw::Breakpoint, widget: &impl IsA<gtk::Widget>) {
    let applied = widget.as_ref().downgrade();
    breakpoint.connect_apply(move |_| {
        if let Some(widget) = applied.upgrade() {
            widget.add_css_class("mp-narrow");
        }
    });
    let unapplied = widget.as_ref().downgrade();
    breakpoint.connect_unapply(move |_| {
        if let Some(widget) = unapplied.upgrade() {
            widget.remove_css_class("mp-narrow");
        }
    });
}

/// Sets the tooltip and the accessible label, which icon-only buttons need. A shortcut
/// hint at the end of the tooltip ("Next (Ctrl+→)") is left out of the label.
pub fn set_tooltip(widget: &impl IsA<gtk::Widget>, text: &str) {
    let widget = widget.upcast_ref::<gtk::Widget>();
    widget.set_tooltip_text(Some(text));
    let label = text.split(" (").next().unwrap_or(text);
    widget.update_property(&[gtk::accessible::Property::Label(label)]);
}

/// A vertically centred icon-only button with a tooltip (see [`set_tooltip`]).
pub fn icon_button(icon_name: &str, tooltip: &str, classes: &[&str]) -> gtk::Button {
    let button = gtk::Button::builder()
        .icon_name(icon_name)
        .valign(gtk::Align::Center)
        .css_classes(classes)
        .build();
    set_tooltip(&button, tooltip);
    button
}

/// A vertically centred icon-only toggle button with a tooltip (see [`set_tooltip`]).
pub fn icon_toggle(icon_name: &str, tooltip: &str, classes: &[&str]) -> gtk::ToggleButton {
    let button = gtk::ToggleButton::builder()
        .icon_name(icon_name)
        .valign(gtk::Align::Center)
        .css_classes(classes)
        .build();
    set_tooltip(&button, tooltip);
    button
}

/// A single line of text, cut short with an ellipsis when it doesn't fit.
pub fn ellipsized_label(classes: &[&str]) -> gtk::Label {
    gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .single_line_mode(true)
        .css_classes(classes)
        .build()
}

/// A flat button showing `label` as a link to a page (`mp-link`), such as an artist's
/// name that opens the artist.
pub fn link_button(label: &gtk::Label) -> gtk::Button {
    gtk::Button::builder()
        .child(label)
        .halign(gtk::Align::Start)
        .css_classes(["flat", "mp-link"])
        .build()
}

/// Fills in an empty state: plain-text `title` and `description` under `icon_name`,
/// or under a spinner while `busy`.
pub fn show_empty_state(
    page: &adw::StatusPage,
    title: &str,
    description: &str,
    icon_name: &str,
    busy: bool,
) {
    page.set_title(title);
    page.set_description(Some(&gtk::glib::markup_escape_text(description)));
    let spinning = page
        .paintable()
        .is_some_and(|paintable| paintable.is::<adw::SpinnerPaintable>());
    if busy && !spinning {
        page.set_paintable(Some(&adw::SpinnerPaintable::new(Some(page))));
    } else if !busy && spinning {
        page.set_icon_name(Some(icon_name));
    }
}

/// Adds or removes a CSS class depending on `enabled`.
pub fn set_css_class(widget: &impl IsA<gtk::Widget>, class: &str, enabled: bool) {
    if enabled {
        widget.add_css_class(class);
    } else {
        widget.remove_css_class(class);
    }
}

/// A signal handler that calls `update` on `widget` for as long as the widget exists.
pub fn follow<W: IsA<gtk::glib::Object>, T>(widget: &W, update: fn(&W)) -> impl Fn(&T) + 'static {
    let widget = widget.downgrade();
    move |_| {
        if let Some(widget) = widget.upgrade() {
            update(&widget);
        }
    }
}

/// Disconnects `handler` from `object` when `widget` is destroyed. Pages use this for
/// signals of long-lived objects (the store, the player) so that opening a page many
/// times doesn't pile up handlers.
pub fn disconnect_on_destroy(
    widget: &impl IsA<gtk::Widget>,
    object: &impl IsA<gtk::glib::Object>,
    handler: gtk::glib::SignalHandlerId,
) {
    let object = object.as_ref().downgrade();
    let handler = std::cell::Cell::new(Some(handler));
    widget.connect_destroy(move |_| {
        if let (Some(object), Some(handler)) = (object.upgrade(), handler.take()) {
            object.disconnect(handler);
        }
    });
}

/// Ties `state` to `widget`: it is dropped when the widget is destroyed. Pages keep
/// their Rust state this way and reach it from signal handlers through `Weak`
/// references, so handlers never keep a closed page alive. `state` must not hold a
/// strong reference to `widget` itself.
pub fn keep_alive<T: 'static>(widget: &impl IsA<gtk::Widget>, state: T) {
    let state = std::cell::RefCell::new(Some(state));
    widget.connect_destroy(move |_| {
        state.take();
    });
}
