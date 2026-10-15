//! A user playlist: its songs in playlist order, which can be reordered by dragging
//! or with Alt+↑ and Alt+↓ and removed with the context menu or the Delete key, and
//! below them the entries whose file is missing.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::{gio, glib};
use mp_core::{CoverId, PlaylistEntry, PlaylistId, PlaylistItemId, TrackId};

use crate::context::{AppContext, plain_toast};
use crate::model::{TrackObject, sync_list_store};
use crate::ui::icons;
use crate::ui::navigation::Route;
use crate::ui::widgets::collections::{
    CoverMosaic, DistinctRows, MOSAIC_TILES, MissingEntries, playlist_actions, reorder,
};
use crate::ui::widgets::{CoverSize, PageHeader, TrackColumn, TrackList};
use crate::ui::{disconnect_on_destroy, keep_alive, set_css_class, track_actions};
use crate::util::{count_label, song_count, total_length};

/// The page's own actions for the selected songs: `remove`, `move-up` and `move-down`.
const PAGE_ACTIONS: &str = "playlist-page";

type PageAction = fn(&PlaylistPage);

pub fn build(ctx: &Rc<AppContext>, id: PlaylistId) -> adw::NavigationPage {
    let Some(playlist) = ctx.store.playlist(id) else {
        return not_found();
    };
    let page = PlaylistPage::new(ctx, id);
    page.connect_signals();

    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.append(&page.content);
    body.append(&page.missing);
    let navigation_page = super::page_with_header(&playlist.name(), &page.header, &body);
    navigation_page.add_css_class("mp-playlist-page");
    navigation_page.insert_action_group(
        playlist_actions::GROUP,
        Some(&playlist_actions::action_group(ctx)),
    );
    navigation_page.insert_action_group(PAGE_ACTIONS, Some(&page.page_actions()));
    page.navigation_page.set(Some(&navigation_page));
    page.refresh();

    let weak = Rc::downgrade(&page);
    let handler = ctx.store.connect_playlists_changed(move |_| {
        if let Some(page) = weak.upgrade() {
            page.refresh();
        }
    });
    disconnect_on_destroy(&navigation_page, &ctx.store, handler);
    keep_alive(&navigation_page, page);
    navigation_page
}

/// The page's state, owned by its navigation page (see [`keep_alive`]).
struct PlaylistPage {
    ctx: Rc<AppContext>,
    id: PlaylistId,
    header: PageHeader,
    mosaic: CoverMosaic,
    play: gtk::Button,
    shuffle: gtk::Button,
    /// The playlist's songs that are in the library, in playlist order.
    tracks: gio::ListStore,
    /// The playlist entry shown by each row of `tracks`.
    rows: RefCell<Vec<PlaylistItemId>>,
    distinct: RefCell<DistinctRows>,
    /// "songs", or "empty" while no song can be shown.
    content: gtk::Stack,
    list: TrackList,
    browse: gtk::Button,
    missing: MissingEntries,
    navigation_page: glib::WeakRef<adw::NavigationPage>,
}

impl PlaylistPage {
    fn new(ctx: &Rc<AppContext>, id: PlaylistId) -> Rc<Self> {
        Rc::new_cyclic(|weak: &Weak<Self>| {
            let header = PageHeader::new("");
            header.set_eyebrow("Playlist");
            let mosaic = CoverMosaic::new(&ctx.covers);
            header.set_leading(Some(&mosaic));
            let play = header.add_button(icons::PLAY, "_Play", true);
            play.set_tooltip_text(Some("Play this playlist in order"));
            let shuffle = header.add_button(icons::SHUFFLE, "_Shuffle", false);
            shuffle.set_tooltip_text(Some("Play this playlist in random order"));
            header
                .add_menu_button()
                .set_menu_model(Some(&playlist_actions::menu(id, false)));

            let tracks = gio::ListStore::new::<TrackObject>();
            let list = song_list(ctx, &tracks, weak.clone());
            let browse = gtk::Button::builder()
                .label("_Browse Songs")
                .use_underline(true)
                .halign(gtk::Align::Center)
                .css_classes(["pill"])
                .build();
            let content = gtk::Stack::builder()
                .transition_type(gtk::StackTransitionType::Crossfade)
                .vexpand(true)
                .build();
            content.add_named(&list, Some("songs"));
            content.add_named(&empty_state(&browse), Some("empty"));

            let remove = weak.clone();
            let missing = MissingEntries::new(move |items| {
                if let Some(page) = remove.upgrade() {
                    page.remove_missing(&items);
                }
            });

            Self {
                ctx: ctx.clone(),
                id,
                header,
                mosaic,
                play,
                shuffle,
                tracks,
                rows: RefCell::default(),
                distinct: RefCell::default(),
                content,
                list,
                browse,
                missing,
                navigation_page: glib::WeakRef::new(),
            }
        })
    }

    fn connect_signals(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.play.connect_clicked(move |_| {
            if let Some(page) = weak.upgrade() {
                page.ctx.player.play_all(page.list.track_ids());
            }
        });
        let weak = Rc::downgrade(self);
        self.shuffle.connect_clicked(move |_| {
            if let Some(page) = weak.upgrade() {
                page.ctx.player.shuffle_tracks(page.list.track_ids());
            }
        });
        let weak = Rc::downgrade(self);
        self.browse.connect_clicked(move |_| {
            if let Some(page) = weak.upgrade() {
                page.ctx.navigator.navigate(Route::Songs);
            }
        });
        let weak = Rc::downgrade(self);
        self.header.connect_narrow_notify(move |header| {
            if let Some(page) = weak.upgrade() {
                page.set_narrow(header.narrow());
            }
        });
    }

    /// Narrow pages have smaller artwork, and the missing files' card follows their
    /// narrower gutter.
    fn set_narrow(&self, narrow: bool) {
        self.mosaic
            .set_pixel_size(CoverSize::header(narrow).pixels());
        set_css_class(&self.missing, "mp-narrow", narrow);
    }

    fn page_actions(self: &Rc<Self>) -> gio::SimpleActionGroup {
        let actions: [(&str, PageAction); 3] = [
            ("remove", Self::remove_selected),
            ("move-up", |page| page.move_selected(true)),
            ("move-down", |page| page.move_selected(false)),
        ];
        let group = gio::SimpleActionGroup::new();
        for (name, run) in actions {
            let action = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            action.connect_activate(move |_, _| {
                if let Some(page) = weak.upgrade() {
                    run(&page);
                }
            });
            group.add_action(&action);
        }
        group
    }

    /// Shows the playlist as it is now. Runs after every playlist edit and library
    /// refresh; only rows that changed are touched, so scrolling and selection stay.
    fn refresh(&self) {
        let Some(playlist) = self.ctx.store.playlist(self.id) else {
            self.leave();
            return;
        };
        let playlist = playlist.playlist();
        let mut rows = Vec::with_capacity(playlist.entries.len());
        let mut tracks = Vec::with_capacity(playlist.entries.len());
        let mut missing: Vec<&PlaylistEntry> = Vec::new();
        for entry in &playlist.entries {
            match entry.track.and_then(|id| self.ctx.store.track(id)) {
                Some(track) => {
                    rows.push(entry.item);
                    tracks.push(track);
                }
                None => missing.push(entry),
            }
        }
        self.rows.replace(rows);
        let tracks = self.distinct.borrow_mut().apply(tracks);
        sync_list_store(&self.tracks, &tracks);
        self.missing.set_entries(&missing);

        self.header.set_title(&playlist.name);
        if let Some(page) = self.navigation_page.upgrade() {
            page.set_title(&playlist.name);
        }
        self.header.set_subtitle(&summary(&tracks, missing.len()));
        self.mosaic.set_covers(&mosaic_covers(&tracks));
        self.play.set_sensitive(!tracks.is_empty());
        self.shuffle.set_sensitive(!tracks.is_empty());
        self.update_empty_state(!tracks.is_empty(), !missing.is_empty());
    }

    /// An empty playlist explains how to fill it. When all of a playlist's files are
    /// missing, the list of those files takes the songs' place.
    fn update_empty_state(&self, has_songs: bool, has_missing: bool) {
        let shown = if has_songs { "songs" } else { "empty" };
        self.content.set_visible_child_name(shown);
        self.content.set_visible(has_songs || !has_missing);
        self.missing.set_standalone(!has_songs);
    }

    /// The playlist is gone: show Songs instead, if this page is still the one open.
    fn leave(&self) {
        let ctx = Rc::downgrade(&self.ctx);
        let route = Route::Playlist(self.id);
        // Not while the store is still telling every page about the change.
        glib::idle_add_local_once(move || {
            if let Some(ctx) = ctx.upgrade()
                && ctx.navigator.top_level_route() == Some(route)
            {
                ctx.navigator.navigate(Route::Songs);
            }
        });
    }

    /// Moves the dragged rows to just before row `before` (the row count for the end).
    fn move_rows(&self, dragged: &[u32], before: u32) {
        let Some(playlist) = self.ctx.store.playlist(self.id) else {
            return;
        };
        let order: Vec<PlaylistItemId> = playlist.entries().iter().map(|e| e.item).collect();
        let (moved, new_order) = {
            let rows = self.rows.borrow();
            let moved: Vec<PlaylistItemId> = dragged
                .iter()
                .filter_map(|&position| rows.get(position as usize).copied())
                .collect();
            let new_order = reorder::drop_order(&order, &rows, &moved, before as usize);
            (moved, new_order)
        };
        if new_order == order {
            return;
        }
        let id = self.id;
        let saved = self
            .ctx
            .edit_library("reorder the playlist", |library| {
                library.reorder_playlist(id, &new_order)
            })
            .is_some();
        self.ctx.refresh_playlists();
        if saved {
            self.select_entries(&moved);
        }
    }

    /// Moves the selected songs one row up or down, together.
    fn move_selected(&self, up: bool) {
        let positions = self.list.selected_positions();
        let (Some(&first), Some(&last)) = (positions.first(), positions.last()) else {
            return;
        };
        let before = if up {
            first.checked_sub(1)
        } else {
            Some(last + 2)
        };
        if let Some(before) = before {
            self.move_rows(&positions, before);
        }
    }

    /// Selects the rows showing `entries`, e.g. the rows that were just moved, and
    /// puts the keyboard focus on the first of them.
    fn select_entries(&self, entries: &[PlaylistItemId]) {
        let column_view = self.list.column_view();
        let Some(selection) = column_view.model() else {
            return;
        };
        let wanted: HashSet<&PlaylistItemId> = entries.iter().collect();
        let rows = self.rows.borrow();
        let selected = gtk::Bitset::new_empty();
        for (position, item) in rows.iter().enumerate() {
            if wanted.contains(item) {
                selected.add(position as u32);
            }
        }
        selection.set_selection(&selected, &gtk::Bitset::new_range(0, rows.len() as u32));
        if !selected.is_empty() {
            column_view.scroll_to(selected.minimum(), None, gtk::ListScrollFlags::FOCUS, None);
        }
    }

    /// Removes the selected songs, offering to undo it, and keeps the keyboard on the
    /// row that moves up into the gap.
    fn remove_selected(&self) {
        let Some(playlist) = self.ctx.store.playlist(self.id) else {
            return;
        };
        let positions = self.list.selected_positions();
        let selected: HashSet<PlaylistItemId> = {
            let rows = self.rows.borrow();
            positions
                .iter()
                .filter_map(|&position| rows.get(position as usize).copied())
                .collect()
        };
        // Old index and track of each removed song, in playlist order.
        let removed: Vec<(usize, PlaylistEntry)> = playlist
            .entries()
            .into_iter()
            .enumerate()
            .filter(|(_, entry)| selected.contains(&entry.item))
            .collect();
        let items: Vec<PlaylistItemId> = removed.iter().map(|(_, entry)| entry.item).collect();
        let what = track_actions::describe(&self.list.selected_tracks());
        if !self.remove_entries(&items) {
            return;
        }
        let songs = removed
            .into_iter()
            .filter_map(|(index, entry)| Some((index, entry.track?)))
            .collect();
        self.announce_removal(&what, songs);

        let remaining = self.tracks.n_items();
        if let Some(&first) = positions.first()
            && remaining > 0
        {
            let flags = gtk::ListScrollFlags::FOCUS | gtk::ListScrollFlags::SELECT;
            self.list
                .column_view()
                .scroll_to(first.min(remaining - 1), None, flags, None);
        }
    }

    fn remove_missing(&self, entries: &[PlaylistItemId]) {
        if self.remove_entries(entries) {
            let what = count_label(entries.len(), "missing file", "missing files");
            self.announce_removal(&what, Vec::new());
        }
    }

    /// Removes `entries` from the playlist. Returns whether it worked.
    fn remove_entries(&self, entries: &[PlaylistItemId]) -> bool {
        if entries.is_empty() {
            return false;
        }
        let id = self.id;
        let removed = self
            .ctx
            .edit_library("remove from the playlist", |library| {
                library.remove_from_playlist(id, entries)
            });
        self.ctx.refresh_playlists();
        removed.is_some()
    }

    /// "Removed `what` from “Playlist”", with Undo when `songs` (their old indices
    /// and tracks, in playlist order) can be put back.
    fn announce_removal(&self, what: &str, songs: Vec<(usize, TrackId)>) {
        let name = self
            .ctx
            .store
            .playlist(self.id)
            .map(|playlist| playlist.name())
            .unwrap_or_default();
        let toast = plain_toast(&format!("Removed {what} from “{name}”"));
        if !songs.is_empty() {
            toast.set_button_label(Some("_Undo"));
            let ctx = Rc::downgrade(&self.ctx);
            let id = self.id;
            toast.connect_button_clicked(move |_| {
                if let Some(ctx) = ctx.upgrade() {
                    restore_songs(&ctx, id, &songs);
                }
            });
        }
        self.ctx.show_toast(toast);
    }
}

/// Puts removed songs back where they were: appends them, then restores the order.
fn restore_songs(ctx: &AppContext, id: PlaylistId, songs: &[(usize, TrackId)]) {
    let (indices, tracks): (Vec<usize>, Vec<TrackId>) = songs.iter().copied().unzip();
    ctx.edit_library("put the songs back", |library| {
        library.add_to_playlist(id, &tracks)?;
        let order: Vec<PlaylistItemId> = library
            .playlist(id)?
            .map(|playlist| playlist.entries.iter().map(|entry| entry.item).collect())
            .unwrap_or_default();
        let added = &order[order.len().saturating_sub(tracks.len())..];
        library.reorder_playlist(id, &reorder::restored_order(&order, added, &indices))
    });
    ctx.refresh_playlists();
}

/// The playlist's songs in playlist order, reordered by dragging them.
fn song_list(ctx: &Rc<AppContext>, tracks: &gio::ListStore, page: Weak<PlaylistPage>) -> TrackList {
    let list = TrackList::builder(ctx)
        .columns(&[
            TrackColumn::Position,
            TrackColumn::Title,
            TrackColumn::Album,
            TrackColumn::Format,
            TrackColumn::Duration,
        ])
        .covers(true)
        .sortable(false)
        .menu_section(&entry_menu())
        .on_reorder(move |_, dragged, before| {
            if let Some(page) = page.upgrade() {
                page.move_rows(dragged, before);
            }
        })
        .build(tracks);
    list.set_vexpand(true);
    list.add_controller(keyboard_shortcuts());
    list
}

/// What an empty playlist shows: how to fill it, and `browse` to go and find songs.
fn empty_state(browse: &gtk::Button) -> adw::StatusPage {
    adw::StatusPage::builder()
        .title("No Songs Yet")
        .description(
            "Add songs from any song’s menu, or drag them onto this playlist in the sidebar.",
        )
        .child(browse)
        .css_classes(["compact"])
        .build()
}

/// "Remove from Playlist", appended to the track list's context menu.
fn entry_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    menu.append(
        Some("Remove from Playlist"),
        Some(&format!("{PAGE_ACTIONS}.remove")),
    );
    menu
}

/// While the track list has the focus, Delete removes the selected songs and Alt+↑
/// and Alt+↓ move them.
fn keyboard_shortcuts() -> gtk::ShortcutController {
    let controller = gtk::ShortcutController::new();
    for (trigger, action) in [
        ("Delete|KP_Delete", "remove"),
        ("<Alt>Up", "move-up"),
        ("<Alt>Down", "move-down"),
    ] {
        controller.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string(trigger),
            Some(gtk::NamedAction::new(&format!("{PAGE_ACTIONS}.{action}"))),
        ));
    }
    controller
}

fn not_found() -> adw::NavigationPage {
    let status = adw::StatusPage::builder()
        .icon_name(icons::UNAVAILABLE)
        .title("Playlist Not Found")
        .description("It may have been deleted.")
        .build();
    adw::NavigationPage::builder()
        .title("Playlist")
        .child(&status)
        .build()
}

/// "12 songs · 48 min", plus how many files are missing.
fn summary(tracks: &[TrackObject], missing: usize) -> String {
    let duration_ms: u64 = tracks.iter().map(TrackObject::duration_ms).sum();
    let mut parts = Vec::new();
    if !tracks.is_empty() {
        parts.push(song_count(tracks.len()));
        parts.push(total_length(duration_ms));
    }
    if missing > 0 {
        parts.push(count_label(missing, "file missing", "files missing"));
    }
    parts.join(" · ")
}

/// The first covers of different albums, in playlist order, for the mosaic.
fn mosaic_covers(tracks: &[TrackObject]) -> Vec<CoverId> {
    let mut covers: Vec<CoverId> = Vec::with_capacity(MOSAIC_TILES);
    for cover in tracks
        .iter()
        .filter_map(|track| track.track().cover.as_ref())
    {
        if !covers.contains(cover) {
            covers.push(cover.clone());
            if covers.len() == MOSAIC_TILES {
                break;
            }
        }
    }
    covers
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::test_track;

    #[test]
    fn mosaic_uses_distinct_covers_in_order() {
        let track = |id: i64, cover: Option<&str>| {
            let mut track = test_track(id, "t", "a", "b");
            track.cover = cover.map(|c| CoverId(c.to_owned()));
            TrackObject::new(track)
        };
        let tracks = vec![
            track(1, Some("a")),
            track(2, Some("a")),
            track(3, None),
            track(4, Some("b")),
            track(5, Some("c")),
            track(6, Some("d")),
            track(7, Some("e")),
        ];
        let covers: Vec<String> = mosaic_covers(&tracks).into_iter().map(|c| c.0).collect();
        assert_eq!(covers, ["a", "b", "c", "d"]);
        assert_eq!(mosaic_covers(&tracks[..2]).len(), 1);
    }

    #[test]
    fn summary_mentions_missing_files() {
        let tracks: Vec<TrackObject> = (1..=2)
            .map(|id| TrackObject::new(test_track(id, "t", "a", "b")))
            .collect();
        assert_eq!(summary(&tracks, 0), "2 songs · 7 min");
        assert_eq!(summary(&tracks, 3), "2 songs · 7 min · 3 files missing");
        assert_eq!(summary(&[], 1), "1 file missing");
        assert_eq!(summary(&[], 0), "");
    }
}
