//! The General page: the music folder, the library and playback.

use std::path::Path;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use crate::context::AppContext;
use crate::model::TrackObject;
use crate::ui::icons;
use crate::ui::widgets::browse::{LibraryStatus, follow_library_status};
use crate::ui::widgets::discover::Figure;
use crate::ui::{dialogs, disconnect_on_destroy};
use crate::util::{format_count, total_length};

pub(super) fn page(ctx: &Rc<AppContext>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .name("general")
        .title("General")
        .icon_name(icons::GENERAL)
        .build();
    page.add(&folder_group(ctx));
    page.add(&library_group(ctx));
    page.add(&playback_group(ctx));
    page.add(&clear_group(ctx));
    page
}

/// The music folder, a warning while it is unavailable, and watching it.
fn folder_group(ctx: &Rc<AppContext>) -> adw::PreferencesGroup {
    // The library is never emptied because the folder went missing (often an unplugged
    // drive); the user decides whether the music is gone.
    let unavailable = adw::ActionRow::builder()
        .title("Folder Unavailable")
        .subtitle(
            "The library stays as it is until the folder is back. If the music is gone \
             for good, clear the library below.",
        )
        .css_classes(["mp-folder-unavailable"])
        .build();
    unavailable.add_prefix(&super::alert_icon("warning"));

    let watch = adw::SwitchRow::builder()
        .title("Watch for Changes")
        .subtitle("Update the library when files are added, changed or removed")
        .active(ctx.settings().watch_music_folder)
        .build();
    watch.connect_active_notify(glib::clone!(
        #[weak]
        ctx,
        move |row| ctx.set_watch_music_folder(row.is_active())
    ));

    let group = adw::PreferencesGroup::builder()
        .title("Music Folder")
        .description("Music is read from this folder and its subfolders.")
        .build();
    group.add(&music_folder_row(ctx));
    group.add(&unavailable);
    group.add(&watch);

    let service = &ctx.library_service;
    unavailable.set_visible(service.folder_missing());
    let handler = service.connect_folder_missing_notify(glib::clone!(
        #[weak]
        unavailable,
        move |service| unavailable.set_visible(service.folder_missing())
    ));
    disconnect_on_destroy(&group, service, handler);
    group
}

/// The chosen folder, with a button to choose another.
fn music_folder_row(ctx: &Rc<AppContext>) -> adw::ActionRow {
    let folder_row = adw::ActionRow::builder()
        .title("Music Folder")
        .subtitle(folder_label(ctx.settings().music_folder.as_deref()))
        .subtitle_selectable(true)
        .use_markup(false)
        .build();
    folder_row.add_prefix(&gtk::Image::from_icon_name(icons::MUSIC_FOLDER));
    let change = gtk::Button::builder()
        .label("_Change…")
        .use_underline(true)
        .valign(gtk::Align::Center)
        .build();
    change.connect_clicked(glib::clone!(
        #[weak]
        ctx,
        #[weak]
        folder_row,
        move |button| choose_folder(&ctx, button, &folder_row)
    ));
    folder_row.add_suffix(&change);
    folder_row.set_activatable_widget(Some(&change));
    folder_row
}

/// What "Rescan Library" does, shown under it while no scan runs.
const RESCAN_DESCRIPTION: &str = "Look for new, changed and removed files";

/// Library totals, and rescanning with its progress.
fn library_group(ctx: &Rc<AppContext>) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder().title("Library").build();
    group.add(&stats_row(ctx, &group));
    let (rescan, reread) = rescan_rows(ctx, &group);
    group.add(&rescan);
    group.add(&reread);
    group
}

/// The library's totals, kept current. The total length goes in `group`'s
/// description.
fn stats_row(ctx: &Rc<AppContext>, group: &adw::PreferencesGroup) -> adw::PreferencesRow {
    let stats = LibraryStats::new();
    let row = adw::PreferencesRow::builder()
        .activatable(false)
        .child(&stats.root)
        .css_classes(["mp-library-stats-row"])
        .build();
    let update = glib::clone!(
        #[weak]
        ctx,
        #[weak]
        group,
        #[weak]
        row,
        move || {
            let summary = stats.update(&ctx);
            row.upcast_ref::<gtk::Widget>()
                .update_property(&[gtk::accessible::Property::Label(&summary)]);
            let duration = total_length(ctx.store.total_duration_ms());
            group.set_description(Some(&format!("{duration} of music")));
        }
    );
    update();
    let handler = ctx.store.connect_tracks_changed(move |_| update());
    disconnect_on_destroy(group, &ctx.store, handler);
    row
}

/// "Rescan Library", which shows a scan's progress while one runs, and "Re-Read All
/// Tags".
fn rescan_rows(
    ctx: &Rc<AppContext>,
    group: &adw::PreferencesGroup,
) -> (adw::ActionRow, adw::ActionRow) {
    let (rescan, rescan_button) = action_row("Rescan Library", RESCAN_DESCRIPTION, "_Rescan");
    let spinner = adw::Spinner::builder().visible(false).build();
    rescan.add_suffix(&spinner);
    let (reread, reread_button) = action_row(
        "Re-Read All Tags",
        "Read every file again, e.g. after editing tags in another app",
        "Re-_Read",
    );
    rescan_button.connect_clicked(glib::clone!(
        #[weak]
        ctx,
        move |_| ctx.library_service.rescan(false)
    ));
    reread_button.connect_clicked(glib::clone!(
        #[weak]
        ctx,
        move |_| ctx.library_service.rescan(true)
    ));

    let update = glib::clone!(
        #[weak]
        ctx,
        #[weak]
        rescan,
        move || {
            let status = LibraryStatus::of(&ctx.library_service);
            let scanning = status.busy();
            spinner.set_visible(scanning);
            rescan_button.set_visible(!scanning);
            reread_button.set_sensitive(!scanning);
            rescan.set_subtitle(&if scanning {
                status.description("")
            } else {
                RESCAN_DESCRIPTION.to_owned()
            });
        }
    );
    update();
    follow_library_status(ctx, group, update);
    (rescan, reread)
}

/// Song, album and artist counts and the space the music takes.
struct LibraryStats {
    root: gtk::Box,
    songs: Figure,
    albums: Figure,
    artists: Figure,
    size: Figure,
}

impl LibraryStats {
    fn new() -> Self {
        let root = gtk::Box::builder()
            .homogeneous(true)
            .spacing(12)
            .css_classes(["mp-library-stats"])
            .build();
        let stats = Self {
            songs: Figure::new("Songs"),
            albums: Figure::new("Albums"),
            artists: Figure::new("Artists"),
            size: Figure::new("On Disk"),
            root,
        };
        for figure in [&stats.songs, &stats.albums, &stats.artists, &stats.size] {
            stats.root.append(&figure.root);
        }
        stats
    }

    /// Shows the library's current totals and returns them as one sentence for
    /// screen readers.
    fn update(&self, ctx: &AppContext) -> String {
        let store = &ctx.store;
        let bytes: u64 = store
            .tracks()
            .iter::<TrackObject>()
            .filter_map(Result::ok)
            .map(|track| track.track().file_size)
            .sum();
        let songs = format_count(store.track_count() as usize);
        let albums = format_count(store.albums().n_items() as usize);
        let artists = format_count(store.artists().n_items() as usize);
        let size = glib::format_size(bytes).to_string();
        self.songs.set_value(&songs);
        self.albums.set_value(&albums);
        self.artists.set_value(&artists);
        self.size.set_value(&size);
        format!("{songs} songs, {albums} albums, {artists} artists, {size} on disk")
    }
}

fn playback_group(ctx: &Rc<AppContext>) -> adw::PreferencesGroup {
    let gapless = adw::SwitchRow::builder()
        .title("Gapless Playback")
        .subtitle("Play albums without silence between songs")
        .active(ctx.settings().gapless)
        .build();
    gapless.connect_active_notify(glib::clone!(
        #[weak]
        ctx,
        move |row| ctx.set_gapless(row.is_active())
    ));
    let group = adw::PreferencesGroup::builder().title("Playback").build();
    group.add(&gapless);

    if let Some(error) = ctx.player.startup_error() {
        let unavailable = adw::ActionRow::builder()
            .title("Audio Unavailable")
            .subtitle(&error)
            .subtitle_selectable(true)
            .use_markup(false)
            .build();
        unavailable.add_prefix(&super::alert_icon("warning"));
        group.add(&unavailable);
    }
    group
}

fn clear_group(ctx: &Rc<AppContext>) -> adw::PreferencesGroup {
    let clear = adw::ButtonRow::builder()
        .title("Clear Library…")
        .start_icon_name(icons::TRASH)
        .build();
    // Added afterwards: the builder's `css_classes` would replace the row's own
    // `button` class, and with it all of its button-row styling.
    clear.add_css_class("destructive-action");
    clear.connect_activated(glib::clone!(
        #[weak]
        ctx,
        move |row| confirm_clear(&ctx, row)
    ));
    let group = adw::PreferencesGroup::builder()
        .description(
            "Clearing removes every song from the library. Your music files and \
             playlists are kept.",
        )
        .build();
    group.add(&clear);
    group
}

fn confirm_clear(ctx: &Rc<AppContext>, row: &adw::ButtonRow) {
    let row_weak = row.downgrade();
    dialogs::clear_library(ctx, Some(row), move || {
        if let Some(row) = row_weak.upgrade() {
            super::toast(&row, "Library cleared");
        }
    });
}

/// A row with a button at its end that the whole row activates.
fn action_row(title: &str, subtitle: &str, button_label: &str) -> (adw::ActionRow, gtk::Button) {
    let button = gtk::Button::builder()
        .label(button_label)
        .use_underline(true)
        .valign(gtk::Align::Center)
        .build();
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(subtitle)
        .build();
    row.add_suffix(&button);
    row.set_activatable_widget(Some(&button));
    (row, button)
}

fn choose_folder(ctx: &Rc<AppContext>, parent: &gtk::Button, row: &adw::ActionRow) {
    let window = parent.root().and_downcast::<gtk::Window>();
    let current = ctx.settings().music_folder.clone();
    let weak = Rc::downgrade(ctx);
    let row = row.downgrade();
    dialogs::choose_music_folder(window.as_ref(), current.as_deref(), move |folder| {
        let Some(ctx) = weak.upgrade() else { return };
        let row = row.upgrade();
        match folder {
            Some(folder) => {
                if let Some(row) = row {
                    row.set_subtitle(&folder_label(Some(&folder)));
                }
                ctx.library_service.set_music_folder(folder);
            }
            None => match row {
                Some(row) => super::toast(&row, dialogs::FOLDER_NOT_LOCAL),
                None => ctx.toast(dialogs::FOLDER_NOT_LOCAL),
            },
        }
    });
}

fn folder_label(folder: Option<&Path>) -> String {
    folder.map_or_else(
        || "Not chosen".to_owned(),
        |path| path.display().to_string(),
    )
}
