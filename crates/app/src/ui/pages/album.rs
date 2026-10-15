//! One album: a hero header with its cover, artist, facts and quality, then its
//! tracks in album order, grouped by disc.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use mp_core::AlbumKey;

use crate::context::AppContext;
use crate::model::{AlbumObject, TrackObject};
use crate::ui::icons;
use crate::ui::navigation::Route;
use crate::ui::widgets::browse::{
    self, BrowseItem, DetailPage, GradualStore, ItemKind, QualityBadge, album_facts, disc_count,
    has_guest_artists, quality_badges,
};
use crate::ui::widgets::{CoverImage, CoverSize, PageHeader, TrackColumn, TrackList};
use crate::ui::{disconnect_on_destroy, keep_alive, link_button};

pub fn build(ctx: &Rc<AppContext>, key: &AlbumKey) -> adw::NavigationPage {
    let (page, navigation_page) = AlbumPage::new(ctx, key);
    let page = Rc::new(page);
    page.connect_signals();
    page.refresh();

    navigation_page.add_css_class("mp-album-page");
    let weak = Rc::downgrade(&page);
    browse::install_actions(&navigation_page, ctx, move || {
        weak.upgrade().and_then(|page| page.item())
    });
    let weak = Rc::downgrade(&page);
    let handler = ctx.store.connect_tracks_changed(move |_| {
        if let Some(page) = weak.upgrade() {
            page.refresh();
        }
    });
    disconnect_on_destroy(&navigation_page, &ctx.store, handler);
    keep_alive(&navigation_page, page);
    navigation_page
}

/// How the track list is set up, which depends on the album.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TrackLayout {
    /// Groups tracks under "Disc 1", "Disc 2", … headers.
    discs: bool,
    /// Shows each track's artist (compilations, featured artists).
    artists: bool,
    /// Shows each track's format (albums mixing formats or qualities).
    formats: bool,
}

impl TrackLayout {
    fn of(album: &AlbumObject, tracks: &[TrackObject], badges: &[QualityBadge]) -> Self {
        let tracks = || tracks.iter().map(TrackObject::track);
        Self {
            discs: disc_count(tracks()) > 1,
            artists: has_guest_artists(tracks(), &album.key().artist),
            formats: badges.len() > 1,
        }
    }
}

/// The page's state, owned by its content widget (see [`keep_alive`]).
struct AlbumPage {
    ctx: Rc<AppContext>,
    key: AlbumKey,
    /// The album shown; `None` once it has left the library.
    album: RefCell<Option<AlbumObject>>,
    detail: DetailPage,
    header: PageHeader,
    cover: CoverImage,
    artist_link: gtk::Button,
    artist_name: gtk::Label,
    facts: gtk::Label,
    badges: adw::WrapBox,
    play: gtk::Button,
    shuffle: gtk::Button,
    /// The album's tracks in album order, shown by the current track list.
    tracks: GradualStore,
    tracks_slot: adw::Bin,
    layout: Cell<Option<TrackLayout>>,
}

impl AlbumPage {
    /// The page's state and the navigation page that shows it.
    fn new(ctx: &Rc<AppContext>, key: &AlbumKey) -> (Self, adw::NavigationPage) {
        let header = PageHeader::new("");
        header.set_eyebrow("Album");
        let cover = CoverImage::new(&ctx.covers, CoverSize::Large);
        header.set_leading(Some(&cover));
        header.connect_narrow_notify(glib::clone!(
            #[weak]
            cover,
            move |header| cover.set_pixel_size(CoverSize::header(header.narrow()).pixels())
        ));

        let (artist_link, artist_name) = artist_link();
        let facts = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["mp-page-subtitle", "dim-label"])
            .build();
        let badges = adw::WrapBox::builder()
            .child_spacing(6)
            .line_spacing(6)
            .css_classes(["mp-quality-badges"])
            .build();
        header.add_detail(&artist_link);
        header.add_detail(&facts);
        header.add_detail(&badges);

        let play = header.add_button(icons::PLAY, "_Play", true);
        play.set_tooltip_text(Some("Play this album"));
        let shuffle = header.add_button(icons::SHUFFLE, "_Shuffle", false);
        shuffle.set_tooltip_text(Some("Play this album in random order"));
        browse::set_page_menu(&header.add_menu_button(), ctx, ItemKind::Album);

        let (detail, navigation_page) = DetailPage::new("Album", &header);
        let tracks = GradualStore::new::<TrackObject>(&navigation_page);
        let tracks_slot = adw::Bin::new();
        detail.append(&tracks_slot);

        let page = Self {
            ctx: ctx.clone(),
            key: key.clone(),
            album: RefCell::default(),
            detail,
            header,
            cover,
            artist_link,
            artist_name,
            facts,
            badges,
            play,
            shuffle,
            tracks,
            tracks_slot,
            layout: Cell::default(),
        };
        (page, navigation_page)
    }

    fn connect_signals(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.play.connect_clicked(move |_| {
            if let Some(page) = weak.upgrade()
                && let Some(item) = page.item()
            {
                browse::play(&page.ctx, &item);
            }
        });
        let weak = Rc::downgrade(self);
        self.shuffle.connect_clicked(move |_| {
            if let Some(page) = weak.upgrade()
                && let Some(item) = page.item()
            {
                browse::shuffle(&page.ctx, &item);
            }
        });
        let weak = Rc::downgrade(self);
        self.artist_link.connect_clicked(move |_| {
            if let Some(page) = weak.upgrade() {
                let route = Route::Artist(page.key.artist.clone());
                page.ctx.navigator.navigate(route);
            }
        });
    }

    fn item(&self) -> Option<BrowseItem> {
        self.album.borrow().clone().map(BrowseItem::Album)
    }

    /// Shows the album as the library now has it, or a message if it is gone.
    fn refresh(&self) {
        let Some(album) = self.ctx.store.album(&self.key) else {
            self.album.replace(None);
            self.detail.show_missing(
                icons::ALBUM,
                "Album Unavailable",
                "This album is no longer in your library. Its files may have been moved, \
                 renamed or deleted.",
            );
            return;
        };
        let tracks = self.ctx.store.tracks_for(album.track_ids());
        let badges = quality_badges(tracks.iter().map(TrackObject::track));
        self.show_header(&album, tracks.len(), &badges);
        self.show_tracks(TrackLayout::of(&album, &tracks, &badges), &tracks);
        self.album.replace(Some(album));
        self.detail.show_content();
    }

    fn show_header(&self, album: &AlbumObject, track_count: usize, badges: &[QualityBadge]) {
        let title = album.title();
        self.detail.set_title(&title);
        self.header.set_title(&title);
        self.cover.set_cover(album.cover());
        self.cover
            .update_property(&[gtk::accessible::Property::Label(&format!(
                "Cover of {title}"
            ))]);
        // The label wraps; `Button::set_label` would replace it with one that doesn't.
        self.artist_name.set_label(&album.artist());
        self.facts
            .set_label(&album_facts(album.year(), track_count, album.duration_ms()));

        while let Some(child) = self.badges.first_child() {
            self.badges.remove(&child);
        }
        for badge in badges {
            self.badges.append(&quality_badge(badge));
        }
        self.badges.set_visible(!badges.is_empty());
    }

    /// Shows `tracks`, rebuilding the list only when the album needs another layout.
    fn show_tracks(&self, layout: TrackLayout, tracks: &[TrackObject]) {
        self.tracks.set(tracks);
        if self.layout.replace(Some(layout)) != Some(layout) {
            self.tracks_slot.set_child(Some(&self.track_list(layout)));
        }
    }

    fn track_list(&self, layout: TrackLayout) -> TrackList {
        let mut columns = vec![TrackColumn::Number, TrackColumn::Title];
        if layout.formats {
            columns.push(TrackColumn::Format);
        }
        columns.push(TrackColumn::Duration);
        let mut builder = TrackList::builder(&self.ctx)
            .columns(&columns)
            .artists(layout.artists)
            .sortable(false)
            .scrolls(false);
        if layout.discs {
            builder = builder.sections(&disc_sorter(), &disc_headers());
        }
        let list = builder.build(self.tracks.model());
        list.add_css_class("mp-album-tracks");
        list
    }
}

/// The album artist under the title, opening their page, and the label that names
/// them.
fn artist_link() -> (gtk::Button, gtk::Label) {
    let name = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .build();
    let button = link_button(&name);
    button.set_tooltip_text(Some("Go to Artist"));
    button.add_css_class("mp-artist-link");
    (button, name)
}

/// "FLAC · 24-bit / 96 kHz", styled like the format badges of track lists.
fn quality_badge(badge: &QualityBadge) -> gtk::Label {
    gtk::Label::builder()
        .label(badge.label())
        .tooltip_text(badge.tier.label())
        .css_classes([
            "mp-format-badge",
            "mp-quality-badge",
            badge.tier.css_class(),
        ])
        .build()
}

fn disc_sorter() -> gtk::NumericSorter {
    let disc = gtk::PropertyExpression::new(
        TrackObject::static_type(),
        None::<&gtk::Expression>,
        "disc-number",
    );
    gtk::NumericSorter::new(Some(disc))
}

/// "Disc 1", "Disc 2", … above each disc's tracks.
fn disc_headers() -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let Some(header) = item.downcast_ref::<gtk::ListHeader>() else {
            return;
        };
        let row = gtk::Box::builder()
            .spacing(9)
            .css_classes(["mp-disc-header"])
            .build();
        row.append(&gtk::Image::from_icon_name(icons::DISC));
        row.append(
            &gtk::Label::builder()
                .xalign(0.0)
                .css_classes(["heading"])
                .accessible_role(gtk::AccessibleRole::Heading)
                .build(),
        );
        header.set_child(Some(&row));
    });
    factory.connect_bind(|_, item| {
        let Some(header) = item.downcast_ref::<gtk::ListHeader>() else {
            return;
        };
        let label = header
            .child()
            .and_then(|row| row.last_child())
            .and_downcast::<gtk::Label>();
        let track = header.item().and_downcast::<TrackObject>();
        if let (Some(label), Some(track)) = (label, track) {
            label.set_label(&format!("Disc {}", track.disc_number()));
        }
    });
    factory
}

#[cfg(test)]
mod tests {
    use mp_core::{AudioFormat, Track};

    use super::*;
    use crate::model::{LibraryStore, test_track};

    fn layout_of(tracks: Vec<Track>) -> TrackLayout {
        let store = LibraryStore::new();
        store.replace_tracks(tracks);
        let album = store
            .albums()
            .item(0)
            .and_downcast::<AlbumObject>()
            .unwrap();
        let tracks = store.tracks_for(album.track_ids());
        let badges = quality_badges(tracks.iter().map(TrackObject::track));
        TrackLayout::of(&album, &tracks, &badges)
    }

    #[test]
    fn plain_albums_need_only_numbers_and_titles() {
        let tracks = vec![
            test_track(1, "One", "Band", "Record"),
            test_track(2, "Two", "Band", "Record"),
        ];
        let plain = TrackLayout {
            discs: false,
            artists: false,
            formats: false,
        };
        assert_eq!(layout_of(tracks), plain);
    }

    #[test]
    fn layout_follows_discs_artists_and_formats() {
        let mut first = test_track(1, "One", "Singer", "Hits");
        let mut second = test_track(2, "Two", "Other Singer", "Hits");
        for track in [&mut first, &mut second] {
            track.album_artist = "Various Artists".into();
        }
        second.disc_number = Some(2);
        second.format = AudioFormat::Mp3;
        second.bit_depth = None;
        let layout = layout_of(vec![first, second]);
        assert!(layout.discs && layout.artists && layout.formats);
    }
}
