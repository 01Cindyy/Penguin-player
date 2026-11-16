//! Keyboard shortcuts, and the dialog that lists them.
//!
//! The playback ones are handled by a key controller on the window rather than as
//! application accelerators: GTK runs accelerators before the focused widget, which
//! would make Space or Ctrl+← impossible to type in the search field. Here they are
//! skipped whenever a text field with text in it, a dialog or a popover has the focus.
//! Media keys always work.

use adw::prelude::*;
use gtk::{gdk, glib};

/// The modifier of most shortcuts: Ctrl, like GTK's `<Primary>`.
const PRIMARY: gdk::ModifierType = gdk::ModifierType::CONTROL_MASK;

/// A shortcut with the primary modifier, written for tooltips: "Ctrl+S".
pub fn primary(key: &str) -> String {
    format!("Ctrl+{key}")
}

/// Key, required modifiers and the `app.` action it triggers.
const PLAYBACK_SHORTCUTS: &[(gdk::Key, gdk::ModifierType, &str)] = &[
    (
        gdk::Key::space,
        gdk::ModifierType::empty(),
        "app.play-pause",
    ),
    (gdk::Key::Right, PRIMARY, "app.next"),
    (gdk::Key::Left, PRIMARY, "app.previous"),
    (gdk::Key::Up, PRIMARY, "app.volume-up"),
    (gdk::Key::Down, PRIMARY, "app.volume-down"),
    (gdk::Key::s, PRIMARY, "app.toggle-shuffle"),
    (gdk::Key::r, PRIMARY, "app.cycle-repeat"),
    (gdk::Key::d, PRIMARY, "app.toggle-favourite"),
];

/// Accelerators for actions that never conflict with typing, which the application
/// installs.
pub const ACCELERATORS: &[(&str, &[&str])] = &[
    ("app.search", &["<Primary>f"]),
    ("app.toggle-queue", &["<Primary>l"]),
    ("app.preferences", &["<Primary>comma"]),
    ("app.shortcuts", &["<Primary>question"]),
    ("app.quit", &["<Primary>q"]),
];

/// Shortcuts that widgets handle themselves, for the dialog: a group's title, then
/// what each does and its keys.
const OTHER_SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
    ("General", &[("Back", "Esc / Alt+←"), ("Main Menu", "F10")]),
    (
        "Songs",
        &[
            ("Open a Song’s Menu", "Menu / Shift+F10"),
            ("Move a Song in the Queue or a Playlist", "Alt+↑ / Alt+↓"),
            ("Remove a Song from the Queue or a Playlist", "Delete"),
        ],
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

/// Opens the list of keyboard shortcuts over `parent`. The playback keys and the
/// accelerators are listed from the tables that bind them.
pub fn present(parent: &impl IsA<gtk::Widget>) {
    let playback = adw::PreferencesGroup::builder().title("Playback").build();
    for (key, modifiers, action) in PLAYBACK_SHORTCUTS {
        playback.add(&row(action_title(action), &key_label(*key, *modifiers)));
    }
    let page = adw::PreferencesPage::new();
    page.add(&playback);
    for (title, shortcuts) in OTHER_SHORTCUTS {
        let group = adw::PreferencesGroup::builder().title(*title).build();
        for (does, keys) in *shortcuts {
            group.add(&row(does, keys));
        }
        if *title == "General" {
            for (action, accelerators) in ACCELERATORS {
                let keys: Vec<String> = accelerators
                    .iter()
                    .filter_map(|accelerator| gtk::accelerator_parse(*accelerator))
                    .map(|(key, modifiers)| key_label(key, modifiers))
                    .collect();
                group.add(&row(action_title(action), &keys.join(" / ")));
            }
        }
        page.add(&group);
    }

    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&page));
    adw::Dialog::builder()
        .title("Keyboard Shortcuts")
        .content_width(460)
        .child(&view)
        .build()
        .present(Some(parent));
}

/// One shortcut: what it does, and its keys at the end of the row.
fn row(does: &str, keys: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(does).build();
    if does == "Search" {
        row.set_subtitle("Or just start typing");
    }
    let keys = gtk::Label::builder()
        .label(keys)
        .css_classes(["dim-label"])
        .build();
    row.add_suffix(&keys);
    row
}

/// What the dialog calls an action that has a shortcut.
fn action_title(action: &str) -> &'static str {
    match action {
        "app.play-pause" => "Play or Pause",
        "app.next" => "Next Song",
        "app.previous" => "Previous Song",
        "app.volume-up" => "Volume Up",
        "app.volume-down" => "Volume Down",
        "app.toggle-shuffle" => "Shuffle On or Off",
        "app.cycle-repeat" => "Repeat: Off, All, One",
        "app.toggle-favourite" => "Add to or Remove from Favourites",
        "app.search" => "Search",
        "app.toggle-queue" => "Show or Hide the Queue",
        "app.preferences" => "Preferences",
        "app.shortcuts" => "Keyboard Shortcuts",
        "app.quit" => "Quit",
        _ => "",
    }
}

/// A shortcut as tooltips write it: "Space", "Ctrl+→", "Ctrl+S".
fn key_label(key: gdk::Key, modifiers: gdk::ModifierType) -> String {
    let name = match key {
        gdk::Key::space => "Space".to_owned(),
        gdk::Key::Right => "→".to_owned(),
        gdk::Key::Left => "←".to_owned(),
        gdk::Key::Up => "↑".to_owned(),
        gdk::Key::Down => "↓".to_owned(),
        key => key.to_unicode().map_or_else(
            || key.name().map(String::from).unwrap_or_default(),
            |character| character.to_uppercase().collect(),
        ),
    };
    if modifiers.contains(PRIMARY) {
        primary(&name)
    } else {
        name
    }
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
        let none = gdk::ModifierType::empty();
        assert_eq!(
            action_for(gdk::Key::space, none, || false),
            Some("app.play-pause")
        );
        assert_eq!(action_for(gdk::Key::space, none, || true), None);
        assert_eq!(
            action_for(gdk::Key::Right, PRIMARY, || false),
            Some("app.next")
        );
        assert_eq!(action_for(gdk::Key::Right, none, || false), None);
        assert_eq!(
            action_for(gdk::Key::AudioNext, none, || true),
            Some("app.next")
        );
    }

    #[test]
    fn every_shortcut_is_listed_by_name_and_keys() {
        let playback = PLAYBACK_SHORTCUTS.iter().map(|(_, _, action)| action);
        let accelerators = ACCELERATORS.iter().map(|(action, _)| action);
        for action in playback.chain(accelerators) {
            assert!(!action_title(action).is_empty(), "{action} has no title");
        }
        let labels: Vec<String> = PLAYBACK_SHORTCUTS
            .iter()
            .map(|(key, modifiers, _)| key_label(*key, *modifiers))
            .collect();
        assert_eq!(
            labels,
            [
                "Space", "Ctrl+→", "Ctrl+←", "Ctrl+↑", "Ctrl+↓", "Ctrl+S", "Ctrl+R", "Ctrl+D"
            ]
        );
        assert_eq!(key_label(gdk::Key::comma, PRIMARY), "Ctrl+,");
        assert_eq!(key_label(gdk::Key::question, PRIMARY), "Ctrl+?");
        assert_eq!(key_label(gdk::Key::F10, gdk::ModifierType::empty()), "F10");
    }
}
