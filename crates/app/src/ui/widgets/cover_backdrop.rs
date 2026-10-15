//! The album art blurred and stretched to fill an area, fading from one song's art to
//! the next. The blur is worked out once per cover, on a small copy of the art, so
//! drawing it costs no more than drawing an image.

use std::cell::{Cell, OnceCell, RefCell};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};
use mp_core::CoverId;

use crate::services::covers::CoverCache;

/// The art is blurred at this size, then scaled up.
const SOURCE_SIZE: u32 = 64;
/// Box blur radius in source pixels; three passes come close to a Gaussian blur.
const BLUR_RADIUS: usize = 5;
const BLUR_PASSES: usize = 3;
const FADE_MS: u32 = 700;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct CoverBackdrop {
        pub(super) cache: OnceCell<CoverCache>,
        pub(super) cover: RefCell<Option<CoverId>>,
        pub(super) current: RefCell<Option<gdk::Texture>>,
        /// The art being faded out.
        pub(super) previous: RefCell<Option<gdk::Texture>>,
        /// How far `current` has faded in over `previous`, from 0 to 1.
        pub(super) fade: Cell<f64>,
        pub(super) animation: OnceCell<adw::TimedAnimation>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CoverBackdrop {
        const NAME: &'static str = "MpCoverBackdrop";
        type Type = super::CoverBackdrop;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("cover-backdrop");
            klass.set_accessible_role(gtk::AccessibleRole::Presentation);
        }
    }

    impl ObjectImpl for CoverBackdrop {}

    impl WidgetImpl for CoverBackdrop {
        /// It fills whatever room it is given.
        fn measure(&self, _orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            (0, 0, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let bounds = graphene::Rect::new(0.0, 0.0, obj.width() as f32, obj.height() as f32);
            snapshot.push_clip(&bounds);
            let fade = self.fade.get();
            if let Some(previous) = self.previous.borrow().as_ref()
                && fade < 1.0
            {
                draw_filling(snapshot, previous, &bounds);
            }
            if let Some(current) = self.current.borrow().as_ref() {
                snapshot.push_opacity(fade);
                draw_filling(snapshot, current, &bounds);
                snapshot.pop();
            }
            snapshot.pop();
        }
    }
}

glib::wrapper! {
    /// Blurred album art (`cover-backdrop`), drawn behind other widgets.
    pub struct CoverBackdrop(ObjectSubclass<imp::CoverBackdrop>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl CoverBackdrop {
    pub fn new(covers: &CoverCache) -> Self {
        let backdrop: Self = glib::Object::new();
        let imp = backdrop.imp();
        assert!(imp.cache.set(covers.clone()).is_ok(), "set once in new");
        let target = adw::CallbackAnimationTarget::new(glib::clone!(
            #[weak]
            backdrop,
            move |fade| {
                backdrop.imp().fade.set(fade);
                backdrop.queue_draw();
            }
        ));
        let animation = adw::TimedAnimation::builder()
            .widget(&backdrop)
            .value_from(0.0)
            .value_to(1.0)
            .duration(FADE_MS)
            .easing(adw::Easing::EaseInOutCubic)
            .target(&target)
            .build();
        assert!(imp.animation.set(animation).is_ok(), "set once in new");
        backdrop
    }

    /// Fades to `cover`'s art, or to nothing for `None` (or art that can't be loaded).
    pub fn set_cover(&self, cover: Option<&CoverId>) {
        let imp = self.imp();
        if imp.cover.borrow().as_ref() == cover {
            return;
        }
        imp.cover.replace(cover.cloned());
        let Some(cover) = cover.cloned() else {
            self.fade_to(None);
            return;
        };
        let cache = imp.cache.get().expect("set in new").clone();
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let texture = cache.load(&cover, SOURCE_SIZE).await;
            let Some(backdrop) = weak.upgrade() else {
                return;
            };
            if backdrop.imp().cover.borrow().as_ref() == Some(&cover) {
                backdrop.fade_to(texture.as_ref().map(blurred));
            }
        });
    }

    fn fade_to(&self, texture: Option<gdk::Texture>) {
        let imp = self.imp();
        let shown = imp.current.replace(texture);
        // Halfway through a fade the new art shows more than the old, so it stays.
        if imp.fade.get() >= 0.5 || imp.previous.borrow().is_none() {
            imp.previous.replace(shown);
        }
        imp.fade.set(0.0);
        if let Some(animation) = imp.animation.get() {
            animation.play();
        }
        self.queue_draw();
    }
}

/// Draws `texture` over all of `bounds`, keeping its proportions; what doesn't fit is
/// cut off.
fn draw_filling(snapshot: &gtk::Snapshot, texture: &gdk::Texture, bounds: &graphene::Rect) {
    let (width, height) = (texture.width() as f32, texture.height() as f32);
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    let scale = (bounds.width() / width).max(bounds.height() / height);
    let (scaled_width, scaled_height) = (width * scale, height * scale);
    let rect = graphene::Rect::new(
        (bounds.width() - scaled_width) / 2.0,
        (bounds.height() - scaled_height) / 2.0,
        scaled_width,
        scaled_height,
    );
    snapshot.append_scaled_texture(texture, gsk::ScalingFilter::Linear, &rect);
}

/// A blurred copy of `texture`.
fn blurred(texture: &gdk::Texture) -> gdk::Texture {
    let format = gdk::MemoryFormat::R8g8b8a8Premultiplied;
    let mut downloader = gdk::TextureDownloader::new(texture);
    downloader.set_format(format);
    let (bytes, stride) = downloader.download_bytes();
    let (width, height) = (texture.width() as usize, texture.height() as usize);
    let mut pixels: Vec<u8> = bytes
        .chunks(stride)
        .take(height)
        .flat_map(|row| &row[..width * 4])
        .copied()
        .collect();
    for _ in 0..BLUR_PASSES {
        box_blur(&mut pixels, width, height, BLUR_RADIUS);
    }
    gdk::MemoryTexture::new(
        width as i32,
        height as i32,
        format,
        &glib::Bytes::from_owned(pixels),
        width * 4,
    )
    .upcast()
}

/// Blurs `pixels` (`width` × `height`, four bytes each) with a box reaching `radius`
/// pixels either side, across then down. Beyond the edges, the edge pixels repeat.
fn box_blur(pixels: &mut [u8], width: usize, height: usize, radius: usize) {
    let mut line = Vec::new();
    for y in 0..height {
        blur_run(pixels, y * width * 4, 4, width, radius, &mut line);
    }
    for x in 0..width {
        blur_run(pixels, x * 4, width * 4, height, radius, &mut line);
    }
}

/// Blurs the `count` pixels starting at byte `start`, each `step` bytes after the last.
fn blur_run(
    pixels: &mut [u8],
    start: usize,
    step: usize,
    count: usize,
    radius: usize,
    line: &mut Vec<[u32; 4]>,
) {
    if count == 0 {
        return;
    }
    line.clear();
    line.extend((0..count).map(|index| {
        let at = start + index * step;
        std::array::from_fn(|channel| u32::from(pixels[at + channel]))
    }));
    let pixel = |index: isize| line[index.clamp(0, count as isize - 1) as usize];
    let radius = radius as isize;
    let window = (2 * radius + 1) as u32;
    let mut sums = [0u32; 4];
    for index in -radius..=radius {
        for (sum, value) in sums.iter_mut().zip(pixel(index)) {
            *sum += value;
        }
    }
    for index in 0..count {
        let at = start + index * step;
        for (channel, sum) in sums.iter().enumerate() {
            pixels[at + channel] = ((sum + window / 2) / window) as u8;
        }
        let (entering, leaving) = (
            pixel(index as isize + radius + 1),
            pixel(index as isize - radius),
        );
        for ((sum, add), remove) in sums.iter_mut().zip(entering).zip(leaving) {
            *sum = *sum + add - remove;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_colour_stays_the_same() {
        let mut pixels = [10, 20, 30, 255].repeat(6 * 4);
        let original = pixels.clone();
        box_blur(&mut pixels, 6, 4, 2);
        assert_eq!(pixels, original);
    }

    #[test]
    fn a_bright_spot_spreads_out_evenly() {
        let (width, height) = (9, 9);
        let mut pixels = vec![0u8; width * height * 4];
        let centre = (4 * width + 4) * 4;
        pixels[centre..centre + 4].copy_from_slice(&[225, 225, 225, 225]);
        box_blur(&mut pixels, width, height, 1);
        let at = |x: usize, y: usize| pixels[(y * width + x) * 4];
        // A 3×3 box: the light is shared by the nine pixels around the spot.
        assert_eq!(at(4, 4), 25);
        assert_eq!(at(3, 3), 25);
        assert_eq!(at(5, 4), 25);
        assert_eq!(at(2, 4), 0);
        let total: u32 = pixels
            .iter()
            .step_by(4)
            .map(|&value| u32::from(value))
            .sum();
        assert_eq!(total, 225);
    }

    #[test]
    fn edges_repeat_rather_than_darken() {
        let mut pixels = [200, 100, 50, 255].repeat(3);
        box_blur(&mut pixels, 3, 1, 4);
        assert_eq!(&pixels[..4], &[200, 100, 50, 255]);
    }
}
