//! First run: ask where the music lives.

use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use mp_core::APP_NAME;

use crate::context::AppContext;
use crate::ui::{dialogs, icons};
use crate::util::display_name;

/// The welcome screen shown until a music folder is chosen. `done` runs once the
/// folder is saved and scanning has started.
pub fn build(ctx: &Rc<AppContext>, done: impl Fn() + 'static) -> gtk::Widget {
    let done: Rc<dyn Fn()> = Rc::new(done);

    let choose = gtk::Button::builder()
        .label("_Choose Music Folder…")
        .use_underline(true)
        .css_classes(["pill", "suggested-action"])
        .build();
    choose.connect_clicked(glib::clone!(
        #[weak]
        ctx,
        #[strong]
        done,
        move |_| pick_folder(&ctx, done.clone())
    ));

    // Centred as a column, so both buttons take the width of the wider one.
    let buttons = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .halign(gtk::Align::Center)
        .build();
    buttons.append(&choose);
    if let Some(music) = glib::user_special_dir(glib::UserDirectory::Music) {
        buttons.append(&use_folder_button(ctx, music, done));
    }

    let status = adw::StatusPage::builder()
        .icon_name(icons::MUSIC_FOLDER)
        .title(format!("Welcome to {APP_NAME}"))
        .description(
            "Choose the folder that holds your music. Everything stays on this computer, \
             and you can change the folder at any time in Preferences.",
        )
        .child(&buttons)
        .vexpand(true)
        .build();

    let header = adw::HeaderBar::builder()
        .show_title(false)
        .css_classes(["flat"])
        .build();
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&status));
    view.add_css_class("mp-onboarding");
    view.upcast()
}

/// "Use Music" — a one-click choice for the platform's standard music folder.
fn use_folder_button(ctx: &Rc<AppContext>, folder: PathBuf, done: Rc<dyn Fn()>) -> gtk::Button {
    let name = display_name(&folder);
    let button = gtk::Button::builder()
        .label(format!("Use “{name}”"))
        .tooltip_text(folder.display().to_string())
        .css_classes(["pill"])
        .build();
    button.connect_clicked(glib::clone!(
        #[weak]
        ctx,
        move |_| use_folder(&ctx, folder.clone(), done.as_ref())
    ));
    button
}

fn pick_folder(ctx: &Rc<AppContext>, done: Rc<dyn Fn()>) {
    let weak = Rc::downgrade(ctx);
    dialogs::choose_music_folder(ctx.window().as_ref(), None, move |folder| {
        let Some(ctx) = weak.upgrade() else { return };
        match folder {
            Some(folder) => use_folder(&ctx, folder, done.as_ref()),
            None => ctx.toast(dialogs::FOLDER_NOT_LOCAL),
        }
    });
}

fn use_folder(ctx: &AppContext, folder: PathBuf, done: &dyn Fn()) {
    ctx.library_service.set_music_folder(folder);
    done();
}
