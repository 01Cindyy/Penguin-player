//! Square album art, loaded asynchronously. Until it has loaded the cover is a plain
//! tinted tile; without art the tile shows a music note, the same everywhere.

use std::cell::{Cell, OnceCell, RefCell};

use adw::prelude::*;
use gtk::glib;
use gtk::subclass::prelude::*;
use mp_core::CoverId;

use crate::services::covers::CoverCache;
use crate::ui::icons;

/// The smallest a cover that can shrink gets.
const SHRUNK_SIZE: i32 = 96;

/// Preset sizes with matching CSS classes (`mp-cover-small`/`-medium`/`-large`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverSize {
    /// Track rows.
    Row,
    /// The player bar and the queue's now-playing card.
    Small,
    /// Page headers on narrow pages, and the format page's tile.
    Medium,
    /// Page headers.
    Large,
}

impl CoverSize {
    /// The artwork of a page header. Narrow pages stack it above the title, so it is
    /// smaller there to leave room for the content.
    pub fn header(narrow: bool) -> Self {
        if narrow {
            CoverSize::Medium
        } else {
            CoverSize::Large
        }
    }

    pub fn pixels(self) -> i32 {
        match self {
            CoverSize::Row => 36,
            CoverSize::Small => 56,
            CoverSize::Medium => 168,
            CoverSize::Large => 232,
        }
    }

    fn css_class(self) -> &'static str {
        match self {
            CoverSize::Row => "mp-cover-row",
            CoverSize::Small => "mp-cover-small",
            CoverSize::Medium => "mp-cover-medium",
            CoverSize::Large => "mp-cover-large",
        }
    }
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct CoverImage {
        pub(super) cache: OnceCell<CoverCache>,
        pub(super) size: Cell<i32>,
        /// It may be shown smaller than `size`, down to [`SHRUNK_SIZE`].
        pub(super) can_shrink: Cell<bool>,
        pub(super) cover: RefCell<Option<CoverId>>,
        pub(super) picture: gtk::Picture,
        pub(super) placeholder: gtk::Image,
        /// Drawn over the art while its track is playing.
        pub(super) playing_icon: gtk::Image,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CoverImage {
        const NAME: &'static str = "MpCoverImage";
        type Type = super::CoverImage;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("cover");
            klass.set_accessible_role(gtk::AccessibleRole::Img);
        }
    }

    impl ObjectImpl for CoverImage {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.add_css_class("mp-cover");
            obj.set_overflow(gtk::Overflow::Hidden);
            obj.set_halign(gtk::Align::Center);
            obj.set_valign(gtk::Align::Center);

            self.placeholder.set_icon_name(Some(icons::SONG));
            self.placeholder.add_css_class("mp-cover-placeholder-icon");
            self.placeholder.set_parent(&*obj);

            self.picture.set_content_fit(gtk::ContentFit::Cover);
            self.picture.set_can_shrink(true);
            self.picture.set_visible(false);
            self.picture.set_parent(&*obj);

            self.playing_icon.set_icon_name(Some(icons::NOW_PLAYING));
            self.playing_icon.add_css_class("mp-cover-playing-icon");
            self.playing_icon.set_visible(false);
            self.playing_icon.set_parent(&*obj);

            obj.connect_scale_factor_notify(|cover| cover.reload(true));
        }

        fn dispose(&self) {
            self.placeholder.unparent();
            self.picture.unparent();
            self.playing_icon.unparent();
        }
    }

    impl WidgetImpl for CoverImage {
        /// Exactly `size` × `size` (or smaller, if it can shrink), whatever the image's
        /// own dimensions.
        fn measure(&self, _orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let size = self.size.get();
            let minimum = if self.can_shrink.get() {
                size.min(SHRUNK_SIZE)
            } else {
                size
            };
            (minimum, size, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.picture.allocate(width, height, baseline, None);
            for icon in [&self.placeholder, &self.playing_icon] {
                // A shrunk cover may be smaller than its icons, which stay centred.
                let (icon_width, ..) = icon.measure(gtk::Orientation::Horizontal, -1);
                let (icon_height, ..) = icon.measure(gtk::Orientation::Vertical, -1);
                let (icon_width, icon_height) = (width.max(icon_width), height.max(icon_height));
                let offset = gtk::graphene::Point::new(
                    ((width - icon_width) / 2) as f32,
                    ((height - icon_height) / 2) as f32,
                );
                icon.allocate(
                    icon_width,
                    icon_height,
                    -1,
                    Some(gtk::gsk::Transform::new().translate(&offset)),
                );
            }
        }
    }
}

glib::wrapper! {
    /// Album art (`mp-cover`). Shows a tinted tile (`mp-cover-placeholder`) until the
    /// image has loaded, and a music note on it when there is no art.
    pub struct CoverImage(ObjectSubclass<imp::CoverImage>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl CoverImage {
    pub fn new(covers: &CoverCache, size: CoverSize) -> Self {
        let cover = Self::with_pixel_size(covers, size.pixels());
        cover.add_css_class(size.css_class());
        cover
    }

    /// A cover of any square size, for layouts the presets don't fit.
    pub fn with_pixel_size(covers: &CoverCache, size: i32) -> Self {
        let cover: Self = glib::Object::new();
        assert!(
            cover.imp().cache.set(covers.clone()).is_ok(),
            "a new cover has no cache yet"
        );
        cover.set_pixel_size(size);
        let weak = cover.downgrade();
        let handler = covers.connect_covers_changed(move |_| {
            if let Some(cover) = weak.upgrade() {
                cover.retry_without_art();
            }
        });
        crate::ui::disconnect_on_destroy(&cover, covers, handler);
        cover
    }

    /// Changes the width and height, e.g. for narrow pages. The art is loaded again
    /// at the new size, and the art already shown stays until then.
    pub fn set_pixel_size(&self, size: i32) {
        let imp = self.imp();
        if imp.size.replace(size) == size {
            return;
        }
        imp.placeholder.set_pixel_size((size * 3 / 10).max(16));
        imp.playing_icon
            .set_pixel_size((size * 4 / 9).clamp(16, 48));
        self.queue_resize();
        self.reload(true);
    }

    /// Lets the cover be shown smaller than its size when there isn't room, as in a
    /// square [`gtk::AspectFrame`]. The art is loaded at the full size either way.
    pub fn set_can_shrink(&self, can_shrink: bool) {
        self.imp().can_shrink.set(can_shrink);
        self.queue_resize();
    }

    /// Shows `cover`, or the placeholder for `None`. Cheap to call repeatedly, e.g.
    /// when list rows are recycled: images already in memory appear immediately.
    pub fn set_cover(&self, cover: Option<&CoverId>) {
        let imp = self.imp();
        if imp.cover.borrow().as_ref() == cover {
            return;
        }
        imp.cover.replace(cover.cloned());
        self.reload(false);
    }

    /// Shows a now-playing indicator over the art, as track lists do for the current track.
    pub fn set_playing(&self, playing: bool) {
        self.imp().playing_icon.set_visible(playing);
    }

    /// Shows the cover at the current size: straight away if it is in memory,
    /// otherwise once it has loaded. Until then the plain tile shows or, with
    /// `keep_shown` (the same cover at another size), the art already on screen.
    fn reload(&self, keep_shown: bool) {
        let Some(cover) = self.imp().cover.borrow().clone() else {
            self.show_placeholder();
            return;
        };
        let cache = self.cache();
        let pixels = self.physical_size();
        if let Some(texture) = cache.cached(&cover, pixels) {
            self.show_texture(&texture);
            return;
        }
        if !(keep_shown && self.imp().picture.is_visible()) {
            self.show_tile(false);
        }
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let texture = cache.load(&cover, pixels).await;
            let Some(this) = weak.upgrade() else { return };
            // The widget may have been rebound to another cover, or resized, meanwhile.
            if this.imp().cover.borrow().as_ref() != Some(&cover) || this.physical_size() != pixels
            {
                return;
            }
            match texture {
                Some(texture) => this.show_texture(&texture),
                None => this.show_placeholder(),
            }
        });
    }

    /// Loads the art again if there was none, as its image may have appeared since.
    fn retry_without_art(&self) {
        let imp = self.imp();
        if imp.cover.borrow().is_some() && !imp.picture.is_visible() {
            self.reload(false);
        }
    }

    fn show_texture(&self, texture: &gtk::gdk::Texture) {
        let imp = self.imp();
        imp.picture.set_paintable(Some(texture));
        imp.picture.set_visible(true);
        imp.placeholder.set_visible(false);
        self.remove_css_class("mp-cover-placeholder");
    }

    fn show_placeholder(&self) {
        self.show_tile(true);
    }

    /// The tinted tile instead of art, with the music note or, while art is still
    /// loading, without it.
    fn show_tile(&self, with_note: bool) {
        let imp = self.imp();
        imp.picture.set_paintable(None::<&gtk::gdk::Paintable>);
        imp.picture.set_visible(false);
        imp.placeholder.set_visible(with_note);
        self.add_css_class("mp-cover-placeholder");
    }

    fn physical_size(&self) -> u32 {
        (self.imp().size.get() * self.scale_factor().max(1)) as u32
    }

    fn cache(&self) -> CoverCache {
        self.imp()
            .cache
            .get()
            .expect("covers are created with a cache")
            .clone()
    }
}
