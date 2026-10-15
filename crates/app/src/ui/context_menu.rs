//! How context menus open, the same everywhere: right-click, a long press on a touch
//! screen, or the Menu key or Shift+F10 for whatever has the keyboard focus.

use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};

/// Calls `open` with `widget` and the press position, in its coordinates, when it is
/// right-clicked or long-pressed.
pub fn connect_press(
    widget: &impl IsA<gtk::Widget>,
    open: impl Fn(&gtk::Widget, f64, f64) + 'static,
) {
    let open = Rc::new(open);
    let pressed = move |gesture: &gtk::Gesture, x: f64, y: f64| {
        if let Some(widget) = gesture.widget() {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            open(&widget, x, y);
        }
    };

    let right_click = gtk::GestureClick::builder()
        .button(gdk::BUTTON_SECONDARY)
        .build();
    let on_click = pressed.clone();
    right_click.connect_pressed(move |gesture, _, x, y| on_click(gesture.upcast_ref(), x, y));
    widget.add_controller(right_click);

    let long_press = gtk::GestureLongPress::builder().touch_only(true).build();
    long_press.connect_pressed(move |gesture, x, y| pressed(gesture.upcast_ref(), x, y));
    widget.add_controller(long_press);
}

/// Calls `open` when the Menu key or Shift+F10 is pressed while the keyboard focus is
/// inside `widget`.
pub fn connect_keys(widget: &impl IsA<gtk::Widget>, open: impl Fn() + 'static) {
    let action = gtk::CallbackAction::new(move |_, _| {
        open();
        glib::Propagation::Stop
    });
    let controller = gtk::ShortcutController::new();
    controller.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("Menu|<Shift>F10"),
        Some(action),
    ));
    widget.add_controller(controller);
}
