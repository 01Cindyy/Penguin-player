//! A song's loudness from start to end as bars, like those of the
//! [visualiser](super::Visualizer) but still, and mirrored around a centre line: the part
//! already played in the CSS `color` of `.mp-waveform`, the rest in a faint version of the
//! text colour. Until the waveform is known the bars lie flat along that line, and they
//! open out when it arrives.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{glib, graphene, gsk};

use super::visualizer::{BAR_WIDTH, add_bar, bar_count};

const BAR_GAP: f32 = 2.0;
/// Height of a bar in silence, or before the waveform is known, in pixels.
const FLAT_BAR: f32 = 2.0;
/// How strongly the part yet to play shows.
const MUTED_ALPHA: f32 = 0.3;
const RISE_MS: u32 = 300;

mod imp {
    use super::*;

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::Waveform)]
    pub struct Waveform {
        pub(super) levels: RefCell<Option<Rc<[u8]>>>,
        /// How far the song has played, from 0 to 1.
        pub(super) played: Cell<f64>,
        pub(super) bar_height: Cell<i32>,
        /// How rounded the bars are, in pixels.
        #[property(get, set = Self::set_bar_radius)]
        pub(super) bar_radius: Cell<f32>,
        /// How far the bars have opened out from flat, from 0 to 1.
        pub(super) rise: Cell<f64>,
        pub(super) rising: OnceCell<adw::TimedAnimation>,
    }

    impl Waveform {
        fn set_bar_radius(&self, radius: f32) {
            self.bar_radius.set(radius);
            self.obj().queue_draw();
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Waveform {
        const NAME: &'static str = "MpWaveform";
        type Type = super::Waveform;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("waveform");
            klass.set_accessible_role(gtk::AccessibleRole::Presentation);
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for Waveform {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.add_css_class("mp-waveform");
            obj.set_hexpand(true);
            obj.set_can_target(false);
        }
    }

    impl WidgetImpl for Waveform {
        /// As tall as its bars, as wide as it is given.
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let size = match orientation {
                gtk::Orientation::Vertical => self.bar_height.get(),
                _ => 0,
            };
            (size, size, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let (width, height) = (obj.width() as f32, obj.height() as f32);
            let count = bar_count(obj.width(), BAR_GAP);
            let start = (width - (count as f32 * (BAR_WIDTH + BAR_GAP) - BAR_GAP)) / 2.0;
            let levels = self.levels.borrow();
            let rise = self.rise.get() as f32;
            let path = gsk::PathBuilder::new();
            for index in 0..count {
                let level = levels
                    .as_deref()
                    .map_or(0.0, |levels| bar_level(levels, index, count));
                // In whole pixels either side of the line, so edges stay sharp.
                let bar = (level * rise * height / 2.0).round() * 2.0;
                let bar = bar.max(FLAT_BAR).min(height);
                let x = start + index as f32 * (BAR_WIDTH + BAR_GAP);
                let top = ((height - bar) / 2.0).floor();
                add_bar(&path, x, top, bar, self.bar_radius.get());
            }
            let path = path.to_path();

            let color = obj.color();
            let text = obj.parent().map_or(color, |parent| parent.color());
            let muted = text.with_alpha(text.alpha() * MUTED_ALPHA);
            let boundary = self.played.get() as f32 * width;
            for (x, part, part_color) in [
                (0.0, boundary, &color),
                (boundary, width - boundary, &muted),
            ] {
                snapshot.push_clip(&graphene::Rect::new(x, 0.0, part, height));
                snapshot.append_fill(&path, gsk::FillRule::Winding, part_color);
                snapshot.pop();
            }
        }
    }
}

glib::wrapper! {
    /// A song's waveform (`mp-waveform`).
    pub struct Waveform(ObjectSubclass<imp::Waveform>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Waveform {
    pub fn new() -> Self {
        let waveform: Self = glib::Object::new();
        let weak = waveform.downgrade();
        let target = adw::CallbackAnimationTarget::new(move |rise| {
            if let Some(waveform) = weak.upgrade() {
                waveform.imp().rise.set(rise);
                waveform.queue_draw();
            }
        });
        let rising = adw::TimedAnimation::builder()
            .widget(&waveform)
            .value_from(0.0)
            .value_to(1.0)
            .duration(RISE_MS)
            .easing(adw::Easing::EaseOutCubic)
            .target(&target)
            .build();
        assert!(waveform.imp().rising.set(rising).is_ok(), "set once in new");
        waveform
    }

    /// How tall the bars are at most, in pixels.
    pub fn set_bar_height(&self, height: i32) {
        if self.imp().bar_height.replace(height.max(0)) != height.max(0) {
            self.queue_resize();
        }
    }

    pub fn bar_height(&self) -> i32 {
        self.imp().bar_height.get()
    }

    /// Shows `levels`, or flat bars for `None`.
    pub fn set_levels(&self, levels: Option<Rc<[u8]>>) {
        let imp = self.imp();
        let known = levels.is_some();
        imp.levels.replace(levels);
        if let Some(rising) = imp.rising.get() {
            if known {
                rising.play();
            } else {
                rising.pause();
                imp.rise.set(0.0);
            }
        }
        self.queue_draw();
    }

    /// Colours the part up to `played`, from 0 to 1.
    pub fn set_played(&self, played: f64) {
        let played = played.clamp(0.0, 1.0);
        if self.imp().played.replace(played) != played {
            self.queue_draw();
        }
    }
}

impl Default for Waveform {
    fn default() -> Self {
        Self::new()
    }
}

/// The height of bar `index` of `count`, from 0 to 1: the loudest of the levels it covers.
fn bar_level(levels: &[u8], index: usize, count: usize) -> f32 {
    let start = index * levels.len() / count;
    let end = ((index + 1) * levels.len() / count)
        .max(start + 1)
        .min(levels.len());
    let loudest = levels.get(start..end).and_then(|part| part.iter().max());
    f32::from(loudest.copied().unwrap_or(0)) / 255.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_show_the_loudest_level_they_cover() {
        let levels = [0, 51, 0, 0, 255, 0];
        assert_eq!(bar_level(&levels, 0, 2), 0.2);
        assert_eq!(bar_level(&levels, 1, 2), 1.0);
        // More bars than levels: each level fills several.
        assert_eq!(bar_level(&levels[..2], 2, 4), 0.2);
        assert_eq!(bar_level(&[], 0, 4), 0.0);
    }
}
