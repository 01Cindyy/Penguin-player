//! Every album as a grid of cards, sorted by title, artist, year or when it was added.

use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use mp_core::{Album, AlbumKey};

use crate::context::AppContext;
use crate::model::{AlbumObject, LibraryStore};
use crate::ui::icons;
use crate::ui::widgets::PageHeader;
use crate::ui::widgets::browse::{
    AlbumCardDetail, CardGrid, CardKind, LibraryStatus, follow_library_status,
};
use crate::ui::{disconnect_on_destroy, keep_alive};
use crate::util::count_label;

/// How the grid is ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum AlbumSort {
    Title,
    /// The store's own order: album artist, then title.
    #[default]
    Artist,
    /// Newest first; albums without a year last.
    Year,
    /// Albums whose newest track was added most recently first.
    RecentlyAdded,
}

impl AlbumSort {
    const ALL: [AlbumSort; 4] = [
        AlbumSort::Title,
        AlbumSort::Artist,
        AlbumSort::Year,
        AlbumSort::RecentlyAdded,
    ];

    fn id(self) -> &'static str {
        match self {
            AlbumSort::Title => "title",
            AlbumSort::Artist => "artist",
            AlbumSort::Year => "year",
            AlbumSort::RecentlyAdded => "added",
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|sort| sort.id() == id)
    }

    fn label(self) -> &'static str {
        match self {
            AlbumSort::Title => "Title",
            AlbumSort::Artist => "Artist",
            AlbumSort::Year => "Year",
            AlbumSort::RecentlyAdded => "Recently Added",
        }
    }

    /// Orders two albums. Text is compared by the albums' normalised keys, so case
    /// and accents don't matter. `added` tells when an album last gained a track.
    fn compare(self, a: &Album, b: &Album, added: impl Fn(&AlbumKey) -> i64) -> Ordering {
        let by_title = || {
            a.key
                .title
                .cmp(&b.key.title)
                .then_with(|| a.key.artist.cmp(&b.key.artist))
        };
        match self {
            AlbumSort::Title => by_title(),
            AlbumSort::Artist => a.key.cmp(&b.key),
            // `None` sorts below any year, so it ends up last.
            AlbumSort::Year => b.year.cmp(&a.year).then_with(|| a.key.cmp(&b.key)),
            AlbumSort::RecentlyAdded => added(&b.key).cmp(&added(&a.key)).then_with(by_title),
        }
    }
}

thread_local! {
    /// The order chosen this session; every Albums page opens with it.
    static SESSION_SORT: Cell<AlbumSort> = Cell::new(AlbumSort::default());
}

pub fn build(ctx: &Rc<AppContext>) -> adw::NavigationPage {
    let page = Rc::new(AlbumsPage::new(ctx));
    page.set_sort(SESSION_SORT.get());
    page.update_summary();
    page.update_empty_state();

    let navigation_page = super::page_with_header("Albums", &page.header, &page.grid);
    navigation_page.add_css_class("mp-albums-page");
    navigation_page.insert_action_group("albums", Some(&page.sort_actions()));

    let weak = Rc::downgrade(&page);
    let handler = page.sorted.connect_items_changed(move |_, _, _, _| {
        if let Some(page) = weak.upgrade() {
            page.update_summary();
        }
    });
    disconnect_on_destroy(&navigation_page, &page.sorted, handler);
    let weak = Rc::downgrade(&page);
    let handler = ctx.store.connect_tracks_changed(move |_| {
        if let Some(page) = weak.upgrade() {
            page.refresh_added_times();
        }
    });
    disconnect_on_destroy(&navigation_page, &ctx.store, handler);
    let weak = Rc::downgrade(&page);
    follow_library_status(ctx, &navigation_page, move || {
        if let Some(page) = weak.upgrade() {
            page.update_empty_state();
        }
    });
    keep_alive(&navigation_page, page);
    navigation_page
}

/// The page's state, owned by its content widget (see [`keep_alive`]).
struct AlbumsPage {
    ctx: Rc<AppContext>,
    sort: Cell<AlbumSort>,
    /// When each album last gained a track, kept only while sorting by it.
    added: Rc<RefCell<HashMap<AlbumKey, i64>>>,
    sorted: gtk::SortListModel,
    header: PageHeader,
    sort_button: gtk::MenuButton,
    sort_label: adw::ButtonContent,
    grid: CardGrid,
}

impl AlbumsPage {
    fn new(ctx: &Rc<AppContext>) -> Self {
        let header = PageHeader::new("Albums");
        let (sort_button, sort_label) = sort_button();
        header.add_action_widget(&sort_button);

        let sorted = gtk::SortListModel::new(Some(ctx.store.albums()), None::<gtk::Sorter>);
        let grid = CardGrid::builder(ctx, CardKind::Albums(AlbumCardDetail::Artist)).build(&sorted);
        grid.grid_view()
            .update_property(&[gtk::accessible::Property::Label("Albums")]);
        grid.set_vexpand(true);

        Self {
            ctx: ctx.clone(),
            sort: Cell::default(),
            added: Rc::default(),
            sorted,
            header,
            sort_button,
            sort_label,
            grid,
        }
    }

    /// The `albums.sort` action behind the sort menu, holding the current order.
    fn sort_actions(self: &Rc<Self>) -> gio::SimpleActionGroup {
        let action = gio::SimpleAction::new_stateful(
            "sort",
            Some(glib::VariantTy::STRING),
            &SESSION_SORT.get().id().to_variant(),
        );
        let weak = Rc::downgrade(self);
        action.connect_change_state(move |action, value| {
            let sort = value
                .and_then(|value| value.str())
                .and_then(AlbumSort::from_id);
            if let (Some(sort), Some(page)) = (sort, weak.upgrade()) {
                action.set_state(&sort.id().to_variant());
                SESSION_SORT.set(sort);
                page.set_sort(sort);
            }
        });
        let group = gio::SimpleActionGroup::new();
        group.add_action(&action);
        group
    }

    fn set_sort(&self, sort: AlbumSort) {
        self.sort.set(sort);
        self.sort_label.set_label(sort.label());
        let accessible_label = format!("Sort by {}", sort.label());
        self.sort_button
            .update_property(&[gtk::accessible::Property::Label(&accessible_label)]);
        self.refresh_added_times();
        // The store is already in artist order.
        let sorter = (sort != AlbumSort::Artist).then(|| album_sorter(sort, self.added.clone()));
        self.sorted.set_sorter(sorter.as_ref());
    }

    /// Recomputes when albums were added if the grid is sorted by it.
    fn refresh_added_times(&self) {
        if self.sort.get() != AlbumSort::RecentlyAdded {
            self.added.borrow_mut().clear();
            return;
        }
        self.added.replace(added_times(&self.ctx.store));
        if let Some(sorter) = self.sorted.sorter() {
            sorter.changed(gtk::SorterChange::Different);
        }
    }

    fn update_summary(&self) {
        let count = self.sorted.n_items() as usize;
        let summary = match count {
            0 => String::new(),
            _ => count_label(count, "album", "albums"),
        };
        self.header.set_subtitle(&summary);
        // Nothing to sort in an empty library.
        self.sort_button.set_visible(count > 0);
    }

    fn update_empty_state(&self) {
        let status = LibraryStatus::of(&self.ctx.library_service);
        self.grid.set_empty_state(
            status.title("No Albums"),
            &status.description("Albums in your music folder will appear here."),
            status.busy(),
        );
    }
}

/// The header's sort menu, and the label showing the current order.
fn sort_button() -> (gtk::MenuButton, adw::ButtonContent) {
    let orders = gio::Menu::new();
    for sort in AlbumSort::ALL {
        let item = gio::MenuItem::new(Some(sort.label()), None);
        item.set_action_and_target_value(Some("albums.sort"), Some(&sort.id().to_variant()));
        orders.append_item(&item);
    }
    let menu = gio::Menu::new();
    menu.append_section(Some("Sort By"), &orders);

    let label = adw::ButtonContent::builder()
        .icon_name(icons::SORT_DESCENDING)
        .build();
    let button = gtk::MenuButton::builder()
        .child(&label)
        .menu_model(&menu)
        .always_show_arrow(true)
        .tooltip_text("Sort Albums")
        .valign(gtk::Align::Center)
        .css_classes(["flat", "mp-sort-button"])
        .build();
    (button, label)
}

fn album_sorter(sort: AlbumSort, added: Rc<RefCell<HashMap<AlbumKey, i64>>>) -> gtk::Sorter {
    gtk::CustomSorter::new(move |a, b| {
        let (Some(a), Some(b)) = (
            a.downcast_ref::<AlbumObject>(),
            b.downcast_ref::<AlbumObject>(),
        ) else {
            return gtk::Ordering::Equal;
        };
        let added = added.borrow();
        sort.compare(a.album(), b.album(), |key| {
            added.get(key).copied().unwrap_or_default()
        })
        .into()
    })
    .upcast()
}

/// When each album last gained a track (Unix seconds).
fn added_times(store: &LibraryStore) -> HashMap<AlbumKey, i64> {
    store
        .albums()
        .iter::<AlbumObject>()
        .filter_map(Result::ok)
        .map(|album| {
            let added = album
                .track_ids()
                .iter()
                .filter_map(|id| store.track(*id))
                .map(|track| track.track().added)
                .max()
                .unwrap_or_default();
            (album.key().clone(), added)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::test_track;

    fn library() -> LibraryStore {
        let mut tracks = vec![
            test_track(1, "a", "Zed", "Beta"),
            test_track(2, "b", "Abba", "Gamma"),
            test_track(3, "c", "Mira", "Alpha"),
            test_track(4, "d", "Mira", "Alpha"),
        ];
        tracks[0].year = Some(1999);
        tracks[1].year = Some(2020);
        tracks[0].added = 300;
        tracks[1].added = 100;
        tracks[2].added = 50;
        tracks[3].added = 400;
        let store = LibraryStore::new();
        store.replace_tracks(tracks);
        store
    }

    fn titles(store: &LibraryStore, sort: AlbumSort) -> Vec<String> {
        let added = added_times(store);
        let mut albums: Vec<AlbumObject> = store
            .albums()
            .iter::<AlbumObject>()
            .filter_map(Result::ok)
            .collect();
        albums.sort_by(|a, b| sort.compare(a.album(), b.album(), |key| added[key]));
        albums.iter().map(AlbumObject::title).collect()
    }

    #[test]
    fn sorts_albums() {
        let store = library();
        assert_eq!(
            titles(&store, AlbumSort::Artist),
            ["Gamma", "Alpha", "Beta"]
        );
        assert_eq!(titles(&store, AlbumSort::Title), ["Alpha", "Beta", "Gamma"]);
        assert_eq!(titles(&store, AlbumSort::Year), ["Gamma", "Beta", "Alpha"]);
        assert_eq!(
            titles(&store, AlbumSort::RecentlyAdded),
            ["Alpha", "Beta", "Gamma"]
        );
    }

    #[test]
    fn sort_ids_round_trip() {
        for sort in AlbumSort::ALL {
            assert_eq!(AlbumSort::from_id(sort.id()), Some(sort));
        }
        assert_eq!(AlbumSort::from_id("nope"), None);
    }
}
