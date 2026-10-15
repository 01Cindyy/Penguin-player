//! The preferences dialog. Every change applies immediately.

mod appearance;
mod general;
mod plugins;

use std::path::Path;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;

use crate::context::AppContext;
use crate::ui::icons;

/// Opens the preferences dialog over `parent`.
pub fn present(ctx: &Rc<AppContext>, parent: &impl IsA<gtk::Widget>) {
    let dialog = adw::PreferencesDialog::builder()
        .title("Preferences")
        .search_enabled(false)
        .build();
    dialog.add_css_class("mp-preferences");
    dialog.add(&general::page(ctx));
    dialog.add(&appearance::page(ctx));
    dialog.add(&plugins::page(ctx));
    dialog.present(Some(parent));
}

/// A small flat icon button for a group's header, e.g. "Open Folder".
fn header_button(icon_name: &str, tooltip: &str) -> gtk::Button {
    super::icon_button(icon_name, tooltip, &["flat", "circular"])
}

/// A warning sign at the start of a row, tinted with `style`: `warning` or `error`.
fn alert_icon(style: &str) -> gtk::Image {
    gtk::Image::builder()
        .icon_name(icons::WARNING)
        .css_classes([style])
        .accessible_role(gtk::AccessibleRole::Presentation)
        .build()
}

/// Opens `dir` in the file manager, creating it first so there is something to open.
fn open_folder(dir: &Path, widget: &impl IsA<gtk::Widget>) {
    if let Err(err) = std::fs::create_dir_all(dir) {
        log::warn!("Could not create {}: {err}", dir.display());
        toast(widget, "Couldn’t create the folder");
        return;
    }
    let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(dir)));
    let window = widget.root().and_downcast::<gtk::Window>();
    let widget = widget.as_ref().downgrade();
    launcher.launch(window.as_ref(), gio::Cancellable::NONE, move |result| {
        if let Err(err) = result
            && !err.matches(gtk::DialogError::Dismissed)
        {
            log::warn!("Could not open the folder: {err}");
            if let Some(widget) = widget.upgrade() {
                toast(&widget, "Couldn’t open the folder");
            }
        }
    });
}

/// Shows a toast in the preferences dialog around `widget`.
fn toast(widget: &impl IsA<gtk::Widget>, message: &str) {
    let dialog = widget
        .ancestor(adw::PreferencesDialog::static_type())
        .and_downcast::<adw::PreferencesDialog>();
    if let Some(dialog) = dialog {
        dialog.add_toast(crate::context::plain_toast(message));
    }
}
