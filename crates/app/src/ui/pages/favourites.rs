//! Favourites: the songs the user starred, newest first, which their star, the context
//! menu or the Delete key removes, and below them the favourites whose file is missing.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
use mp_core::{CoverId, Favourite, TrackId};

use crate::context::AppContext;
use crate::model::{TrackObject, sync_list_store};
use crate::ui::dialogs::{self, ImageOwner};
use crate::ui::icons;
use crate::ui::navigation::Route;
use crate::ui::widgets::collections::{CoverMosaic, MissingEntries};
use crate::ui::widgets::{CoverSize, PageHeader, TrackColumn, TrackList};
use crate::ui::{disconnect_on_destroy, keep_alive, set_css_class, track_actions};
use crate::util::count_label;

/// The page's actions: `play-next`, `enqueue`, `choose-image` and `remove-image`, and
/// `remove` for the selected songs.
const PAGE_ACTIONS: &str = "favourites-page";

type PageAction = fn(&FavouritesPage);

pub fn build(ctx: &Rc<AppContext>) -> adw::NavigationPage {
    let page = Rc::new(FavouritesPage::new(ctx));
    page.connect_signals();

    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.append(&page.content);
    body.append(&page.missing);
    let navigation_page = super::page_with_header("Favourites", &page.header, &body);
    navigation_page.add_css_class("mp-favourites-page");
    navigation_page.insert_action_group(PAGE_ACTIONS, Some(&page.page_actions()));
    page.refresh();

    let weak = Rc::downgrade(&page);
    let handler = ctx.store.connect_favourites_changed(move |_| {
        if let Some(page) = weak.upgrade() {
            page.refresh();
        }
    });
    disconnect_on_destroy(&navigation_page, &ctx.store, handler);
    keep_alive(&navigation_page, page);
    navigation_page
}

/// The page's state, owned by its navigation page (see [`keep_alive`]).
struct FavouritesPage {
    ctx: Rc<AppContext>,
    header: PageHeader,
    mosaic: CoverMosaic,
    menu_button: gtk::MenuButton,
    play: gtk::Button,
    shuffle: gtk::Button,
    /// The favourites that are in the library, newest first.
    tracks: gio::ListStore,
    /// "songs", or "empty" while no song can be shown.
    content: gtk::Stack,
    list: TrackList,
    browse: gtk::Button,
    missing: MissingEntries,
    /// The favourite shown by each row of `missing`.
    missing_entries: Rc<RefCell<Vec<Favourite>>>,
}

impl FavouritesPage {
    fn new(ctx: &Rc<AppContext>) -> Self {
        let header = PageHeader::new("Favourites");
        let mosaic = CoverMosaic::new(&ctx.covers);
        mosaic.set_placeholder(icons::STARRED, "Favourites artwork");
        header.set_leading(Some(&mosaic));
        let play = header.add_button(icons::PLAY, "_Play", true);
        play.set_tooltip_text(Some("Play your favourites, newest first"));
        let shuffle = header.add_button(icons::SHUFFLE, "_Shuffle", false);
        shuffle.set_tooltip_text(Some("Play your favourites in random order"));
        let menu_button = header.add_menu_button();

        let tracks = gio::ListStore::new::<TrackObject>();
        let list = song_list(ctx, &tracks);
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

        let missing_entries: Rc<RefCell<Vec<Favourite>>> = Rc::default();
        let missing = {
            let ctx = Rc::downgrade(ctx);
            let entries = missing_entries.clone();
            MissingEntries::new(
                "Remove the missing files from Favourites",
                move |positions| {
                    let Some(ctx) = ctx.upgrade() else { return };
                    let removed: Vec<Favourite> = {
                        let entries = entries.borrow();
                        positions
                            .iter()
                            .filter_map(|&position| entries.get(position).cloned())
                            .collect()
                    };
                    let what = count_label(removed.len(), "missing file", "missing files");
                    track_actions::remove_favourites(&ctx, removed, &what);
                },
            )
        };

        Self {
            ctx: ctx.clone(),
            header,
            mosaic,
            menu_button,
            play,
            shuffle,
            tracks,
            content,
            list,
            browse,
            missing,
            missing_entries,
        }
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
        let actions: [(&str, PageAction); 5] = [
            ("play-next", Self::play_next),
            ("enqueue", Self::enqueue),
            ("choose-image", |page| {
                dialogs::choose_image(&page.ctx, ImageOwner::Favourites);
            }),
            ("remove-image", |page| {
                dialogs::remove_image(&page.ctx, ImageOwner::Favourites);
            }),
            ("remove", Self::remove_selected),
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

    /// Shows the favourites as they are now. Runs after every change to them and every
    /// library refresh; only rows that changed are touched, so scrolling and selection
    /// stay.
    fn refresh(&self) {
        let favourites = self.ctx.store.favourites();
        let image: Vec<CoverId> = favourites.image.iter().cloned().collect();
        self.mosaic.set_covers(&image);
        self.menu_button
            .set_menu_model(Some(&page_menu(favourites.image.is_some())));

        let mut tracks = Vec::with_capacity(favourites.entries.len());
        let mut missing = Vec::new();
        for entry in favourites.entries {
            match entry.track.and_then(|id| self.ctx.store.track(id)) {
                Some(track) => tracks.push(track),
                None => missing.push(entry),
            }
        }
        sync_list_store(&self.tracks, &tracks);
        let paths: Vec<&Path> = missing.iter().map(|entry| entry.path.as_path()).collect();
        self.missing.set_entries(&paths);

        self.header
            .set_subtitle(&super::playlist::summary(&tracks, missing.len()));
        self.play.set_sensitive(!tracks.is_empty());
        self.shuffle.set_sensitive(!tracks.is_empty());
        self.update_empty_state(!tracks.is_empty(), !missing.is_empty());
        self.missing_entries.replace(missing);
    }

    /// Without favourites the page explains how to add some. When all of their files are
    /// missing, the list of those files takes the songs' place.
    fn update_empty_state(&self, has_songs: bool, has_missing: bool) {
        let shown = if has_songs { "songs" } else { "empty" };
        self.content.set_visible_child_name(shown);
        self.content.set_visible(has_songs || !has_missing);
        self.missing.set_standalone(!has_songs);
    }

    fn play_next(&self) {
        if let Some(tracks) = self.playable_tracks() {
            self.ctx.player.play_next(&tracks);
            self.ctx.toast("Favourites will play next");
        }
    }

    fn enqueue(&self) {
        if let Some(tracks) = self.playable_tracks() {
            self.ctx.player.enqueue(&tracks);
            self.ctx.toast("Added Favourites to the queue");
        }
    }

    /// The favourites in the library, newest first, or `None` (and a toast saying so).
    fn playable_tracks(&self) -> Option<Vec<TrackId>> {
        let tracks = self.list.track_ids();
        if tracks.is_empty() {
            self.ctx.toast("Favourites has no songs to play");
            return None;
        }
        Some(tracks)
    }

    /// Removes the selected songs, offering to undo it, and keeps the keyboard on the
    /// row that moves up into the gap.
    fn remove_selected(&self) {
        let positions = self.list.selected_positions();
        track_actions::remove_from_favourites(&self.ctx, &self.list.selected_tracks());
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
}

/// The favourites, newest first.
fn song_list(ctx: &Rc<AppContext>, tracks: &gio::ListStore) -> TrackList {
    let list = TrackList::builder(ctx)
        .columns(&[
            TrackColumn::Position,
            TrackColumn::Title,
            TrackColumn::Album,
            TrackColumn::Format,
            TrackColumn::Favourite,
            TrackColumn::Duration,
        ])
        .covers(true)
        .sortable(false)
        .build(tracks);
    list.set_vexpand(true);
    list.add_controller(delete_shortcut());
    list
}

/// What the page shows without favourites: how to add some, and `browse` to go and find
/// songs.
fn empty_state(browse: &gtk::Button) -> adw::StatusPage {
    adw::StatusPage::builder()
        .title("No Favourites Yet")
        .description(
            "Add songs with their star or from any song’s menu, or drag them onto \
             Favourites in the sidebar.",
        )
        .child(browse)
        .css_classes(["compact"])
        .build()
}

/// The page's "more" menu: playback, then its image.
fn page_menu(has_image: bool) -> gio::Menu {
    let playback = gio::Menu::new();
    playback.append(
        Some("Play Next"),
        Some(&format!("{PAGE_ACTIONS}.play-next")),
    );
    playback.append(
        Some("Add to Queue"),
        Some(&format!("{PAGE_ACTIONS}.enqueue")),
    );
    let image = gio::Menu::new();
    image.append(
        Some("Choose Image…"),
        Some(&format!("{PAGE_ACTIONS}.choose-image")),
    );
    if has_image {
        image.append(
            Some("Remove Image"),
            Some(&format!("{PAGE_ACTIONS}.remove-image")),
        );
    }
    let menu = gio::Menu::new();
    menu.append_section(None, &playback);
    menu.append_section(None, &image);
    menu
}

/// While the list has the focus, Delete removes the selected songs.
fn delete_shortcut() -> gtk::ShortcutController {
    let controller = gtk::ShortcutController::new();
    controller.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("Delete|KP_Delete"),
        Some(gtk::NamedAction::new(&format!("{PAGE_ACTIONS}.remove"))),
    ));
    controller
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_image_is_offered_only_with_an_image() {
        let labels = |menu: &gio::Menu| -> Vec<String> {
            let mut labels = Vec::new();
            for section in 0..menu.n_items() {
                let Some(items) = menu.item_link(section, gio::MENU_LINK_SECTION) else {
                    continue;
                };
                for item in 0..items.n_items() {
                    let label = items.item_attribute_value(item, "label", None);
                    labels.extend(label.and_then(|label| label.get::<String>()));
                }
            }
            labels
        };
        assert_eq!(
            labels(&page_menu(false)),
            ["Play Next", "Add to Queue", "Choose Image…"]
        );
        assert_eq!(labels(&page_menu(true)).last().unwrap(), "Remove Image");
    }
}
