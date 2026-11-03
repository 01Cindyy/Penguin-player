//! Small dialogs shared by several parts of the UI.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use mp_core::{PlaylistId, TrackId};

use crate::context::AppContext;
use crate::model::PlaylistObject;
use crate::services::covers;

/// The response that confirms a playlist name.
const ACCEPT: &str = "accept";

/// Says that the folder chosen in [`choose_music_folder`] can't be used.
pub const FOLDER_NOT_LOCAL: &str = "That folder isn’t on this computer";

/// Asks for the music folder over `parent`, starting in `current`. `chosen` runs with
/// the folder, or with `None` if it has no local path, such as a network location;
/// cancelling does nothing.
pub fn choose_music_folder(
    parent: Option<&gtk::Window>,
    current: Option<&Path>,
    chosen: impl FnOnce(Option<PathBuf>) + 'static,
) {
    let dialog = gtk::FileDialog::builder()
        .title("Choose Music Folder")
        .accept_label("_Select")
        .modal(true)
        .build();
    if let Some(current) = current {
        dialog.set_initial_folder(Some(&gio::File::for_path(current)));
    }
    dialog.select_folder(parent, gio::Cancellable::NONE, move |result| match result {
        Ok(folder) => chosen(folder.path()),
        Err(err) if err.matches(gtk::DialogError::Dismissed) => {}
        Err(err) => log::warn!("Folder chooser failed: {err}"),
    });
}

/// Asks for a playlist name, creates the playlist with `tracks` in it and refreshes
/// the playlists. `created` runs with the new playlist's id and name.
pub fn new_playlist(
    ctx: &Rc<AppContext>,
    tracks: Vec<TrackId>,
    created: impl FnOnce(PlaylistId, String) + 'static,
) {
    let (dialog, entry) = playlist_name_dialog("New Playlist", "", "C_reate");
    let weak = Rc::downgrade(ctx);
    let parent = ctx.window();
    dialog.choose(parent.as_ref(), gio::Cancellable::NONE, move |response| {
        let Some(ctx) = weak.upgrade() else { return };
        let name = entry.text().trim().to_owned();
        if response != ACCEPT || name.is_empty() {
            return;
        }
        let id = ctx.edit_library("create the playlist", |library| {
            let id = library.create_playlist(&name)?;
            if !tracks.is_empty() {
                library.add_to_playlist(id, &tracks)?;
            }
            Ok(id)
        });
        // Also after a failure: the playlist may exist without its songs.
        ctx.refresh_playlists();
        if let Some(id) = id {
            created(id, name);
        }
    });
}

/// Asks for a new name for `playlist` and renames it.
pub fn rename_playlist(ctx: &Rc<AppContext>, playlist: &PlaylistObject) {
    let old_name = playlist.name();
    let (dialog, entry) = playlist_name_dialog("Rename Playlist", &old_name, "_Rename");
    let id = playlist.playlist_id();
    let weak = Rc::downgrade(ctx);
    let parent = ctx.window();
    dialog.choose(parent.as_ref(), gio::Cancellable::NONE, move |response| {
        let Some(ctx) = weak.upgrade() else { return };
        let name = entry.text().trim().to_owned();
        if response != ACCEPT || name.is_empty() || name == old_name {
            return;
        }
        let renamed = ctx.edit_library("rename the playlist", |library| {
            library.rename_playlist(id, &name)
        });
        if renamed.is_some() {
            ctx.refresh_playlists();
        }
    });
}

/// Asks before deleting `playlist`. The songs themselves are not affected.
pub fn delete_playlist(ctx: &Rc<AppContext>, playlist: &PlaylistObject) {
    let name = playlist.name();
    let dialog = adw::AlertDialog::builder()
        .heading("Delete Playlist?")
        .body(format!(
            "“{name}” will be deleted. The songs in it stay in your library."
        ))
        .close_response("cancel")
        .default_response("cancel")
        .build();
    dialog.add_responses(&[("cancel", "_Cancel"), ("delete", "_Delete")]);
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);

    let id = playlist.playlist_id();
    let weak = Rc::downgrade(ctx);
    let parent = ctx.window();
    dialog.choose(parent.as_ref(), gio::Cancellable::NONE, move |response| {
        let Some(ctx) = weak.upgrade() else { return };
        if response != "delete" {
            return;
        }
        let image = ctx.store.playlist(id).and_then(|playlist| playlist.image());
        let deleted =
            ctx.edit_library("delete the playlist", |library| library.delete_playlist(id));
        if deleted.is_some() {
            if let Some(image) = image {
                covers::remove_playlist_image(&ctx.paths.playlist_images_dir(), &image);
            }
            ctx.refresh_playlists();
            ctx.toast(&format!("Deleted “{name}”"));
        }
    });
}

/// Asks for an image to show for `playlist` in place of its songs' covers. It is copied
/// to the app's own folder, so moving or deleting the original doesn't matter.
pub fn choose_playlist_image(ctx: &Rc<AppContext>, playlist: &PlaylistObject) {
    let images = gtk::FileFilter::new();
    images.set_name(Some("Images"));
    images.add_pixbuf_formats();
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&images);
    let dialog = gtk::FileDialog::builder()
        .title("Choose Playlist Image")
        .modal(true)
        .filters(&filters)
        .default_filter(&images)
        .build();
    let weak = Rc::downgrade(ctx);
    let id = playlist.playlist_id();
    let parent = ctx.window();
    dialog.open(parent.as_ref(), gio::Cancellable::NONE, move |result| {
        let Some(ctx) = weak.upgrade() else { return };
        match result {
            Ok(file) => match file.path() {
                Some(path) => use_playlist_image(&ctx, id, path),
                None => ctx.toast("Only images on this computer can be used"),
            },
            Err(err) if err.matches(gtk::DialogError::Dismissed) => {}
            Err(err) => log::warn!("Image chooser failed: {err}"),
        }
    });
}

/// Copies the image at `path` to the app's folder and shows it for playlist `id`.
fn use_playlist_image(ctx: &Rc<AppContext>, id: PlaylistId, path: PathBuf) {
    let weak = Rc::downgrade(ctx);
    let dir = ctx.paths.playlist_images_dir();
    glib::spawn_future_local(async move {
        let imported = {
            let dir = dir.clone();
            gio::spawn_blocking(move || covers::import_playlist_image(&path, id, &dir)).await
        };
        let Some(ctx) = weak.upgrade() else { return };
        let image = match imported {
            Ok(Ok(image)) => image,
            Ok(Err(err)) => {
                log::warn!("Could not use the playlist image: {err}");
                ctx.toast("Couldn’t use that image");
                return;
            }
            Err(_) => {
                log::error!("Copying the playlist image panicked");
                ctx.toast("Couldn’t use that image");
                return;
            }
        };
        let previous = ctx.store.playlist(id).and_then(|playlist| playlist.image());
        let saved = ctx.edit_library("save the playlist’s image", |library| {
            library.set_playlist_image(id, Some(&image))
        });
        let replaced = if saved.is_some() {
            previous
        } else {
            Some(image.clone())
        };
        if let Some(unused) = replaced.filter(|unused| saved.is_none() || *unused != image) {
            covers::remove_playlist_image(&dir, &unused);
        }
        ctx.refresh_playlists();
    });
}

/// Goes back to showing the covers of `playlist`'s songs.
pub fn remove_playlist_image(ctx: &Rc<AppContext>, playlist: &PlaylistObject) {
    let Some(image) = playlist.image() else {
        return;
    };
    let id = playlist.playlist_id();
    let saved = ctx.edit_library("remove the playlist’s image", |library| {
        library.set_playlist_image(id, None)
    });
    if saved.is_some() {
        covers::remove_playlist_image(&ctx.paths.playlist_images_dir(), &image);
        ctx.refresh_playlists();
    }
}

/// Asks before removing every song from the library; the music files and playlists
/// are kept. `cleared` runs once the library is cleared.
pub fn clear_library(
    ctx: &Rc<AppContext>,
    parent: Option<&impl IsA<gtk::Widget>>,
    cleared: impl FnOnce() + 'static,
) {
    let dialog = adw::AlertDialog::builder()
        .heading("Clear Library?")
        .body(
            "Every song is removed from the library. Your music files and playlists are \
             kept, and the songs come back when the music folder is scanned again.",
        )
        .close_response("cancel")
        .default_response("cancel")
        .build();
    dialog.add_responses(&[("cancel", "_Cancel"), ("clear", "C_lear Library")]);
    dialog.set_response_appearance("clear", adw::ResponseAppearance::Destructive);

    let weak = Rc::downgrade(ctx);
    dialog.choose(parent, gio::Cancellable::NONE, move |response| {
        if response == "clear"
            && let Some(ctx) = weak.upgrade()
        {
            ctx.library_service.clear_tracks();
            cleared();
        }
    });
}

/// An alert with a name field, starting with `name`, whose suggested response
/// (labelled `accept_label`) is only enabled while the name isn't blank.
fn playlist_name_dialog(
    heading: &str,
    name: &str,
    accept_label: &str,
) -> (adw::AlertDialog, gtk::Entry) {
    let entry = gtk::Entry::builder()
        .placeholder_text("Playlist name")
        .text(name)
        .activates_default(true)
        .build();
    entry.update_property(&[gtk::accessible::Property::Label("Playlist name")]);
    let dialog = adw::AlertDialog::builder()
        .heading(heading)
        .close_response("cancel")
        .default_response(ACCEPT)
        .extra_child(&entry)
        .focus_widget(&entry)
        .build();
    dialog.add_responses(&[("cancel", "_Cancel"), (ACCEPT, accept_label)]);
    dialog.set_response_appearance(ACCEPT, adw::ResponseAppearance::Suggested);
    dialog.set_response_enabled(ACCEPT, !name.trim().is_empty());

    let weak_dialog = dialog.downgrade();
    entry.connect_changed(move |entry| {
        if let Some(dialog) = weak_dialog.upgrade() {
            dialog.set_response_enabled(ACCEPT, !entry.text().trim().is_empty());
        }
    });
    (dialog, entry)
}
