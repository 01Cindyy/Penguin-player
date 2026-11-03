//! A playlist's artwork: the image chosen for it, a 2×2 mosaic of different album
//! covers, a single cover while there are fewer than four to choose from, or the same
//! placeholder as any cover without art.

use std::cell::{Cell, OnceCell, RefCell};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;
use mp_core::CoverId;

use crate::model::{LibraryStore, PlaylistObject};
use crate::services::covers::CoverCache;
use crate::ui::icons;
use crate::ui::set_css_class;
use crate::ui::widgets::{CoverImage, CoverSize};

/// How many covers fill the mosaic.
pub const MOSAIC_TILES: usize = 4;

/// What shows for `playlist`: the image chosen for it, or else the first covers of
/// different albums, in playlist order.
pub fn playlist_covers(store: &LibraryStore, playlist: &PlaylistObject) -> Vec<CoverId> {
    if let Some(image) = playlist.image() {
        return vec![image];
    }
    let songs = playlist
        .available_tracks()
        .into_iter()
        .filter_map(|id| store.track(id));
    let mut covers: Vec<CoverId> = Vec::with_capacity(MOSAIC_TILES);
    for song in songs {
        if let Some(cover) = &song.track().cover
            && !covers.contains(cover)
        {
            covers.push(cover.clone());
            if covers.len() == MOSAIC_TILES {
                break;
            }
        }
    }
    covers
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct CoverMosaic {
        pub(super) size: Cell<i32>,
        pub(super) covers: RefCell<Vec<CoverId>>,
        pub(super) stack: gtk::Stack,
        pub(super) placeholder: gtk::Image,
        pub(super) single: OnceCell<CoverImage>,
        pub(super) tiles: OnceCell<Vec<CoverImage>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CoverMosaic {
        const NAME: &'static str = "MpCoverMosaic";
        type Type = super::CoverMosaic;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.set_accessible_role(gtk::AccessibleRole::Img);
        }
    }

    impl ObjectImpl for CoverMosaic {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            for class in ["mp-cover", "mp-cover-large", "mp-cover-mosaic"] {
                obj.add_css_class(class);
            }
            obj.set_overflow(gtk::Overflow::Hidden);
            obj.set_halign(gtk::Align::Center);
            obj.set_valign(gtk::Align::Center);
            obj.update_property(&[gtk::accessible::Property::Label("Playlist artwork")]);
            self.placeholder.set_icon_name(Some(icons::SONG));
            self.placeholder.add_css_class("mp-cover-placeholder-icon");
            self.stack
                .set_transition_type(gtk::StackTransitionType::Crossfade);
            self.stack.add_named(&self.placeholder, Some("placeholder"));
            obj.set_child(Some(&self.stack));
        }
    }

    impl WidgetImpl for CoverMosaic {}
    impl BinImpl for CoverMosaic {}
}

glib::wrapper! {
    /// Playlist artwork, page-header size by default (`mp-cover mp-cover-large
    /// mp-cover-mosaic`). Each tile loads its cover at its own displayed size.
    pub struct CoverMosaic(ObjectSubclass<imp::CoverMosaic>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl CoverMosaic {
    pub fn new(covers: &CoverCache) -> Self {
        let mosaic: Self = glib::Object::new();
        let imp = mosaic.imp();
        let size = CoverSize::Large.pixels();
        let single = CoverImage::with_pixel_size(covers, size);
        let tiles: Vec<CoverImage> = (0..MOSAIC_TILES)
            .map(|_| CoverImage::with_pixel_size(covers, size / 2))
            .collect();
        let grid = gtk::Grid::builder()
            .row_homogeneous(true)
            .column_homogeneous(true)
            .build();
        for (index, tile) in tiles.iter().enumerate() {
            grid.attach(tile, (index % 2) as i32, (index / 2) as i32, 1, 1);
        }
        imp.stack.add_named(&single, Some("single"));
        imp.stack.add_named(&grid, Some("mosaic"));
        assert!(
            imp.single.set(single).is_ok() && imp.tiles.set(tiles).is_ok(),
            "a new mosaic has no covers yet"
        );
        mosaic.set_pixel_size(size);
        mosaic
    }

    /// A small one with the rounding and shadow of a song row's cover, as beside a
    /// playlist's name in the sidebar.
    pub fn small(covers: &CoverCache, size: i32) -> Self {
        let mosaic = Self::new(covers);
        mosaic.remove_css_class("mp-cover-large");
        mosaic.add_css_class("mp-cover-row");
        mosaic.set_pixel_size(size);
        mosaic
    }

    /// Changes the width and height, e.g. for narrow pages. The covers are loaded
    /// again at the new size.
    pub fn set_pixel_size(&self, size: i32) {
        let imp = self.imp();
        if imp.size.replace(size) == size {
            return;
        }
        imp.placeholder.set_pixel_size((size * 3 / 10).max(16));
        self.single().set_pixel_size(size);
        for tile in self.tiles() {
            tile.set_pixel_size(size / 2);
        }
    }

    /// Shows the first [`MOSAIC_TILES`] of `covers` as a mosaic, or the first cover
    /// alone if there are fewer. Cheap to call again with the same covers.
    pub fn set_covers(&self, covers: &[CoverId]) {
        self.imp().covers.replace(covers.to_vec());
        self.show_covers();
    }

    fn show_covers(&self) {
        let imp = self.imp();
        let covers = imp.covers.borrow();
        let shown = match covers.as_slice() {
            [] => "placeholder",
            [first, ..] if covers.len() < MOSAIC_TILES => {
                self.single().set_cover(Some(first));
                "single"
            }
            _ => {
                for (tile, cover) in self.tiles().iter().zip(covers.iter()) {
                    tile.set_cover(Some(cover));
                }
                "mosaic"
            }
        };
        imp.stack.set_visible_child_name(shown);
        set_css_class(self, "mp-cover-placeholder", covers.is_empty());
    }

    fn single(&self) -> &CoverImage {
        self.imp().single.get().expect("set in new")
    }

    fn tiles(&self) -> &[CoverImage] {
        self.imp().tiles.get().expect("set in new")
    }
}

#[cfg(test)]
mod tests {
    use mp_core::{Playlist, PlaylistEntry, PlaylistId, PlaylistItemId, TrackId};

    use super::*;
    use crate::model::test_track;

    #[test]
    fn playlists_show_their_image_or_else_different_covers_in_order() {
        let covers = [
            Some("a"),
            Some("a"),
            None,
            Some("b"),
            Some("c"),
            Some("d"),
            Some("e"),
        ];
        let tracks = covers
            .iter()
            .enumerate()
            .map(|(index, cover)| {
                let mut track = test_track(index as i64 + 1, "t", "a", "b");
                track.cover = cover.map(|cover| CoverId(cover.to_owned()));
                track
            })
            .collect();
        let store = LibraryStore::new();
        store.replace_tracks(tracks);
        let playlist = |songs: i64, image: Option<&str>| {
            PlaylistObject::new(Playlist {
                id: PlaylistId(1),
                name: "Mix".to_owned(),
                created: 0,
                modified: 0,
                image: image.map(|image| CoverId(image.to_owned())),
                entries: (1..=songs)
                    .map(|id| PlaylistEntry {
                        item: PlaylistItemId(id),
                        path: format!("/music/{id}.flac").into(),
                        track: Some(TrackId(id)),
                    })
                    .collect(),
            })
        };
        let shown = |songs, image| -> Vec<String> {
            playlist_covers(&store, &playlist(songs, image))
                .into_iter()
                .map(|cover| cover.0)
                .collect()
        };
        assert_eq!(shown(7, None), ["a", "b", "c", "d"]);
        assert_eq!(shown(2, None), ["a"]);
        assert_eq!(shown(7, Some("mine")), ["mine"]);
    }
}
