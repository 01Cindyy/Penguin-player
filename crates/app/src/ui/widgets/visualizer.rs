//! A cava-style spectrum visualiser: bars that move with the music, drawn in the CSS
//! `color` of `.mp-visualizer`, at most [`Visualizer::set_bar_height`] tall.
//!
//! The bar height is its own setting rather than `height-request`, which GTK applies to
//! the whole widget, margins included: the CSS margins may be negative to let the bars
//! overlap what is below.
//!
//! It only animates while something plays and it is on screen; then it asks the audio
//! engine for a copy of the audio, which costs nothing otherwise.

use std::cell::{Cell, RefCell};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{glib, graphene, gsk};
use mp_audio::{Scope, ScopeWatch, Spectrum};

const BAR_WIDTH: f32 = 4.0;
const BAR_GAP: f32 = 3.0;
const MAX_BARS: usize = 96;
/// Longest step the bars take at once, so they don't jump after the window was hidden.
const MAX_STEP_SECONDS: f32 = 0.1;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Visualizer {
        pub(super) scope: RefCell<Option<Scope>>,
        /// Held while audio is wanted from the scope.
        pub(super) watch: RefCell<Option<ScopeWatch>>,
        pub(super) spectrum: RefCell<Spectrum>,
        pub(super) samples: RefCell<Vec<f32>>,
        pub(super) levels: RefCell<Vec<f32>>,
        pub(super) playing: Cell<bool>,
        pub(super) tick: RefCell<Option<gtk::TickCallbackId>>,
        /// Frame time of the last update, in microseconds.
        pub(super) last_frame: Cell<Option<i64>>,
        pub(super) bar_height: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Visualizer {
        const NAME: &'static str = "MpVisualizer";
        type Type = super::Visualizer;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("visualizer");
            klass.set_accessible_role(gtk::AccessibleRole::Presentation);
        }
    }

    impl ObjectImpl for Visualizer {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.add_css_class("mp-visualizer");
            obj.set_hexpand(true);
            obj.set_can_target(false);
        }

        fn dispose(&self) {
            self.obj().stop();
        }
    }

    impl WidgetImpl for Visualizer {
        /// As tall as its bars, as wide as it is given.
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let size = match orientation {
                gtk::Orientation::Vertical => self.bar_height.get(),
                _ => 0,
            };
            (size, size, -1, -1)
        }

        fn map(&self) {
            self.parent_map();
            self.obj().update_running();
        }

        fn unmap(&self) {
            self.parent_unmap();
            self.obj().stop();
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let levels = self.levels.borrow();
            let height = obj.height() as f32;
            let count = levels.len() as f32;
            let start = (obj.width() as f32 - (count * (BAR_WIDTH + BAR_GAP) - BAR_GAP)) / 2.0;
            let path = gsk::PathBuilder::new();
            let mut empty = true;
            for (index, level) in levels.iter().enumerate() {
                let bar = (level * height).round();
                if bar < 1.0 {
                    continue;
                }
                let x = start + index as f32 * (BAR_WIDTH + BAR_GAP);
                let rect = graphene::Rect::new(x, height - bar, BAR_WIDTH, bar);
                let radius = (BAR_WIDTH / 2.0).min(bar / 2.0);
                let corner = graphene::Size::new(radius, radius);
                path.add_rounded_rect(&gsk::RoundedRect::new(rect, corner, corner, corner, corner));
                empty = false;
            }
            if !empty {
                snapshot.append_fill(&path.to_path(), gsk::FillRule::Winding, &obj.color());
            }
        }
    }
}

glib::wrapper! {
    /// The spectrum visualiser (`mp-visualizer`).
    pub struct Visualizer(ObjectSubclass<imp::Visualizer>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Visualizer {
    /// Shows the audio from `scope`; without one (no audio engine) it stays empty.
    pub fn new(scope: Option<Scope>) -> Self {
        let visualizer: Self = glib::Object::new();
        visualizer.imp().scope.replace(scope);
        visualizer
    }

    /// How tall the bars are at most, in pixels.
    pub fn set_bar_height(&self, height: i32) {
        if self.imp().bar_height.replace(height.max(0)) != height.max(0) {
            self.queue_resize();
        }
    }

    /// Animates while `playing`; otherwise the bars fall and come to rest.
    pub fn set_playing(&self, playing: bool) {
        if self.imp().playing.replace(playing) != playing {
            self.update_running();
        }
    }

    fn update_running(&self) {
        let imp = self.imp();
        let watching = self.is_mapped() && imp.scope.borrow().is_some();
        let wanted = watching && imp.playing.get();
        if wanted != imp.watch.borrow().is_some() {
            let watch = imp
                .scope
                .borrow()
                .as_ref()
                .filter(|_| wanted)
                .map(Scope::watch);
            imp.watch.replace(watch);
        }
        let moving = imp.playing.get() || !imp.spectrum.borrow().is_silent();
        if watching && moving && imp.tick.borrow().is_none() {
            let id = self.add_tick_callback(|visualizer, clock| visualizer.on_frame(clock));
            imp.tick.replace(Some(id));
        }
    }

    /// Stops animating and copying audio, e.g. while hidden.
    fn stop(&self) {
        let imp = self.imp();
        if let Some(id) = imp.tick.take() {
            id.remove();
        }
        imp.last_frame.set(None);
        imp.watch.take();
    }

    fn on_frame(&self, clock: &gtk::gdk::FrameClock) -> glib::ControlFlow {
        let imp = self.imp();
        let now = clock.frame_time();
        let elapsed = imp
            .last_frame
            .replace(Some(now))
            .map_or(0.0, |last| (now - last) as f32 / 1_000_000.0)
            .clamp(0.0, MAX_STEP_SECONDS);

        let mut samples = imp.samples.borrow_mut();
        let audio = match imp.scope.borrow().as_ref() {
            Some(scope) if imp.playing.get() => {
                let rate = scope.sample_rate();
                samples.resize(Spectrum::frames_wanted(rate), 0.0);
                scope.latest(&mut samples).then_some(rate)
            }
            _ => None,
        };
        let mut spectrum = imp.spectrum.borrow_mut();
        let levels = spectrum.update(
            audio.map(|rate| (samples.as_slice(), rate)),
            bar_count(self.width()),
            elapsed,
        );
        let mut shown = imp.levels.borrow_mut();
        shown.clear();
        shown.extend_from_slice(levels);
        drop(shown);
        self.queue_draw();

        if imp.playing.get() || !spectrum.is_silent() {
            return glib::ControlFlow::Continue;
        }
        imp.tick.take();
        imp.last_frame.set(None);
        glib::ControlFlow::Break
    }
}

/// How many bars fit across `width` pixels.
fn bar_count(width: i32) -> usize {
    let fitting = (width as f32 + BAR_GAP) / (BAR_WIDTH + BAR_GAP);
    (fitting.max(0.0) as usize).min(MAX_BARS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_fill_the_width_up_to_a_limit() {
        assert_eq!(bar_count(0), 0);
        assert_eq!(bar_count(4), 1);
        assert_eq!(bar_count(10), 1);
        assert_eq!(bar_count(11), 2);
        assert_eq!(bar_count(5_000), MAX_BARS);
    }
}
