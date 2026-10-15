//! A warning across the top of the content pane while the music folder is unavailable,
//! usually because its drive isn't connected.
//!
//! The library is left as it was in that case, so nothing disappears just because a
//! drive was unplugged. If the music really is gone, the banner offers to clear the
//! library (playlists are kept: they refer to files by path and fill up again later).

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use crate::context::AppContext;
use crate::ui::dialogs;

/// The banner for the content pane. It shows and hides itself.
pub fn build(ctx: &Rc<AppContext>) -> adw::Banner {
    let banner = adw::Banner::builder()
        .use_markup(false)
        .css_classes(["mp-folder-banner"])
        .build();
    banner.connect_button_clicked(glib::clone!(
        #[weak]
        ctx,
        move |_| {
            if ctx.store.track_count() > 0 {
                dialogs::clear_library(&ctx, ctx.window().as_ref(), || {});
            } else {
                ctx.app.activate_action("preferences", None);
            }
        }
    ));

    let update = glib::clone!(
        #[weak]
        ctx,
        #[weak]
        banner,
        move || update(&ctx, &banner)
    );
    update();
    let update = Rc::new(update);
    let refresh = update.clone();
    ctx.library_service
        .connect_folder_missing_notify(move |_| refresh());
    ctx.store.connect_track_count_notify(move |_| update());
    banner
}

fn update(ctx: &AppContext, banner: &adw::Banner) {
    let missing = ctx.library_service.folder_missing();
    banner.set_revealed(missing);
    if !missing {
        return;
    }
    let name = ctx
        .settings()
        .music_folder
        .as_deref()
        .and_then(|folder| folder.file_name())
        .map_or_else(
            || "your music folder".to_owned(),
            |name| format!("“{}”", name.to_string_lossy()),
        );
    if ctx.store.track_count() > 0 {
        banner.set_title(&format!(
            "Can’t find your music in {name}. Is its drive connected?"
        ));
        banner.set_button_label(Some("Clear Library…"));
    } else {
        banner.set_title(&format!("Can’t find any music in {name}"));
        banner.set_button_label(Some("Choose Folder…"));
    }
}
