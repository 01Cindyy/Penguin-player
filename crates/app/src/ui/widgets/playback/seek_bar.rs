//! The seek bar between the elapsed and total time, kept in step with the player.
//!
//! It follows playback, except while it is being dragged: then it only previews the
//! time, and seeks once released.

use std::cell::{Cell, OnceCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, glib};
use mp_core::PlayerCommand;
use mp_core::util::format_duration;

use super::follow;
use crate::context::AppContext;
use crate::ui::disconnect_on_destroy;

struct Parts {
    elapsed: gtk::Label,
    scale: gtk::Scale,
    duration: gtk::Label,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct SeekBar {
        pub(super) ctx: OnceCell<Rc<AppContext>>,
        pub(super) parts: OnceCell<Parts>,
        /// True while the user drags the bar, so playback doesn't move it.
        pub(super) dragging: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SeekBar {
        const NAME: &'static str = "MpSeekBar";
        type Type = super::SeekBar;
        type ParentType = gtk::Grid;
    }

    impl ObjectImpl for SeekBar {}
    impl WidgetImpl for SeekBar {}
    impl GridImpl for SeekBar {}
}

glib::wrapper! {
    /// Elapsed time, the seek slider (`mp-seek-bar`) and the track's length
    /// (`mp-seek-row` around them all), with room
    /// above the slider for something as wide as it, such as a visualiser.
    pub struct SeekBar(ObjectSubclass<imp::SeekBar>)
        @extends gtk::Grid, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl SeekBar {
    pub fn new(ctx: &Rc<AppContext>) -> Self {
        let bar: Self = glib::Object::builder()
            .property("column-spacing", 10)
            .build();
        bar.add_css_class("mp-seek-row");
        let time_label = |xalign: f32| {
            gtk::Label::builder()
                .xalign(xalign)
                .width_chars(5)
                .css_classes(["mp-time-label", "numeric", "caption", "dim-label"])
                .build()
        };
        let parts = Parts {
            elapsed: time_label(1.0),
            scale: gtk::Scale::builder()
                .orientation(gtk::Orientation::Horizontal)
                .hexpand(true)
                .draw_value(false)
                .css_classes(["mp-seek-bar"])
                .tooltip_text("Seek")
                .build(),
            duration: time_label(0.0),
        };
        parts.scale.set_range(0.0, 1.0);
        parts.scale.set_increments(5_000.0, 15_000.0);
        parts
            .scale
            .update_property(&[gtk::accessible::Property::Label("Playback position")]);
        bar.attach(&parts.elapsed, 0, 1, 1, 1);
        bar.attach(&parts.scale, 1, 1, 1, 1);
        bar.attach(&parts.duration, 2, 1, 1, 1);
        let imp = bar.imp();
        assert!(imp.ctx.set(ctx.clone()).is_ok(), "set once in new");
        assert!(imp.parts.set(parts).is_ok(), "set once in new");

        bar.connect_dragging();
        bar.follow_player();
        bar.update_track();
        bar.update_duration();
        bar.update_position();
        bar
    }

    /// Puts `widget` above the slider, as wide as it.
    pub fn set_above(&self, widget: &impl IsA<gtk::Widget>) {
        self.attach(widget, 1, 0, 1, 1);
    }

    /// Takes the slider out, for a layout that shows it elsewhere; it keeps working
    /// there. [`Self::restore_slider`] puts it back.
    pub fn take_slider(&self) -> gtk::Scale {
        let scale = &self.parts().scale;
        if scale.parent().as_ref() == Some(self.upcast_ref()) {
            self.remove(scale);
        }
        scale.clone()
    }

    /// Puts the slider back, once it has been taken out of wherever it was shown.
    pub fn restore_slider(&self) {
        let scale = &self.parts().scale;
        if scale.parent().is_none() {
            self.attach(scale, 1, 1, 1, 1);
        }
    }

    fn ctx(&self) -> &Rc<AppContext> {
        self.imp().ctx.get().expect("set in new")
    }

    fn parts(&self) -> &Parts {
        self.imp().parts.get().expect("set in new")
    }

    fn connect_dragging(&self) {
        let scale = &self.parts().scale;
        let pointer = gtk::EventControllerLegacy::new();
        pointer.set_propagation_phase(gtk::PropagationPhase::Capture);
        pointer.connect_event(glib::clone!(
            #[weak(rename_to = bar)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, event| {
                match event.event_type() {
                    gdk::EventType::ButtonPress | gdk::EventType::TouchBegin => {
                        bar.imp().dragging.set(true);
                    }
                    gdk::EventType::ButtonRelease
                    | gdk::EventType::TouchEnd
                    | gdk::EventType::TouchCancel
                        if bar.imp().dragging.replace(false) =>
                    {
                        bar.seek_to(bar.parts().scale.value());
                    }
                    _ => {}
                }
                glib::Propagation::Proceed
            }
        ));
        scale.add_controller(pointer);

        scale.connect_change_value(glib::clone!(
            #[weak(rename_to = bar)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, _, value| {
                if bar.imp().dragging.get() {
                    bar.parts().elapsed.set_label(&format_ms(value as u64));
                } else {
                    bar.seek_to(value);
                }
                glib::Propagation::Proceed
            }
        ));
    }

    fn seek_to(&self, millis: f64) {
        let position = Duration::from_millis(millis.max(0.0) as u64);
        self.ctx().player.execute(PlayerCommand::SeekTo(position));
    }

    fn follow_player(&self) {
        let player = &self.ctx().player;
        for handler in [
            player.connect_current_track_notify(follow(self, Self::update_track)),
            player.connect_duration_notify(follow(self, Self::update_duration)),
            player.connect_position_notify(follow(self, Self::update_position)),
        ] {
            disconnect_on_destroy(self, player, handler);
        }
    }

    fn update_track(&self) {
        let has_track = self.ctx().player.current_track().is_some();
        self.parts().scale.set_sensitive(has_track);
    }

    fn update_duration(&self) {
        let parts = self.parts();
        let duration = self.ctx().player.duration();
        parts.scale.set_range(0.0, duration.max(1) as f64);
        parts.duration.set_label(&format_ms(duration));
    }

    fn update_position(&self) {
        if self.imp().dragging.get() {
            return;
        }
        let parts = self.parts();
        let position = self.ctx().player.position();
        parts.scale.set_value(position as f64);
        parts.elapsed.set_label(&format_ms(position));
    }
}

fn format_ms(millis: u64) -> String {
    format_duration(Duration::from_millis(millis))
}
