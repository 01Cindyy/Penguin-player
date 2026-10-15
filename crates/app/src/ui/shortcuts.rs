//! Playback keyboard shortcuts.
//!
//! These are handled by a key controller on the window rather than as application
//! accelerators: GTK runs accelerators before the focused widget, which would make
//! Space or Ctrl+← impossible to type in the search field. Here they are skipped
//! whenever a text field with text in it, a dialog or a popover has the focus. Media
//! keys always work.

use gtk::prelude::*;
use gtk::{gdk, glib};

/// Key, required modifiers and the `app.` action it triggers.
const PLAYBACK_SHORTCUTS: &[(gdk::Key, gdk::ModifierType, &str)] = &[
    (
        gdk::Key::space,
        gdk::ModifierType::empty(),
        "app.play-pause",
    ),
    (gdk::Key::Right, gdk::ModifierType::CONTROL_MASK, "app.next"),
    (
        gdk::Key::Left,
        gdk::ModifierType::CONTROL_MASK,
        "app.previous",
    ),
    (
        gdk::Key::Up,
        gdk::ModifierType::CONTROL_MASK,
        "app.volume-up",
    ),
    (
        gdk::Key::Down,
        gdk::ModifierType::CONTROL_MASK,
        "app.volume-down",
    ),
    (
        gdk::Key::s,
        gdk::ModifierType::CONTROL_MASK,
        "app.toggle-shuffle",
    ),
    (
        gdk::Key::r,
        gdk::ModifierType::CONTROL_MASK,
        "app.cycle-repeat",
    ),
];

/// Dedicated media keys, for platforms where no media-controls service takes them.
const MEDIA_KEYS: &[(gdk::Key, &str)] = &[
    (gdk::Key::AudioPlay, "app.play-pause"),
    (gdk::Key::AudioPause, "app.play-pause"),
    (gdk::Key::AudioNext, "app.next"),
    (gdk::Key::AudioPrev, "app.previous"),
    (gdk::Key::AudioStop, "app.stop"),
];

/// Adds the playback shortcuts to `window`.
pub fn install(window: &gtk::Window) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed(|controller, key, _, modifiers| {
        let Some(window) = controller.widget().and_downcast::<gtk::Window>() else {
            return glib::Propagation::Proceed;
        };
        let modifiers = modifiers & gtk::accelerator_get_default_mod_mask();
        let Some(action) = action_for(key.to_lower(), modifiers, || focus_wants_keys(&window))
        else {
            return glib::Propagation::Proceed;
        };
        match window.activate_action(action, None) {
            Ok(()) => glib::Propagation::Stop,
            Err(_) => glib::Propagation::Proceed,
        }
    });
    window.add_controller(keys);
}

/// The action for a key press. `focus_wants_keys` is only asked about shortcuts
/// that a text field or dialog could also need.
fn action_for(
    key: gdk::Key,
    modifiers: gdk::ModifierType,
    focus_wants_keys: impl FnOnce() -> bool,
) -> Option<&'static str> {
    if let Some((_, action)) = MEDIA_KEYS.iter().find(|(media, _)| *media == key) {
        return Some(action);
    }
    let (_, _, action) = PLAYBACK_SHORTCUTS
        .iter()
        .find(|(shortcut, required, _)| *shortcut == key && *required == modifiers)?;
    (!focus_wants_keys()).then_some(*action)
}

/// Whether the focused widget needs plain keys for itself: anything inside a dialog
/// or popover, and text fields that hold text.
///
/// An empty field has no use for Space or Ctrl+arrows, and GTK moves the focus into
/// the search field by itself at times (when the sidebar collapses, for instance), so
/// playback shortcuts keep working until something has been typed.
fn focus_wants_keys(window: &gtk::Window) -> bool {
    let Some(focus) = gtk::prelude::GtkWindowExt::focus(window) else {
        return false;
    };
    if focus.ancestor(adw::Dialog::static_type()).is_some()
        || focus.ancestor(gtk::Popover::static_type()).is_some()
    {
        return true;
    }
    let editable = focus
        .clone()
        .downcast::<gtk::Editable>()
        .ok()
        .or_else(|| focus.ancestor(gtk::Editable::static_type()).and_downcast());
    editable.is_some_and(|editable| !editable.text().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_respect_text_fields() {
        let ctrl = gdk::ModifierType::CONTROL_MASK;
        let none = gdk::ModifierType::empty();
        assert_eq!(
            action_for(gdk::Key::space, none, || false),
            Some("app.play-pause")
        );
        assert_eq!(action_for(gdk::Key::space, none, || true), None);
        assert_eq!(
            action_for(gdk::Key::Right, ctrl, || false),
            Some("app.next")
        );
        assert_eq!(action_for(gdk::Key::Right, none, || false), None);
        assert_eq!(
            action_for(gdk::Key::AudioNext, none, || true),
            Some("app.next")
        );
    }
}
