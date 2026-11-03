//! The seek bar between the elapsed and total time, kept in step with the player.
//!
//! It follows playback, except while it is being dragged: then it only previews the
//! time, and seeks once released.
//!
//! With the Waveform Seek Bar on, the slider's line is hidden and the song's waveform is
//! drawn in its place. It is still the slider that takes the pointer and the keyboard.

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
use crate::ui::widgets::Waveform;
use crate::ui::{disconnect_on_destroy, set_css_class};

struct Parts {
    elapsed: gtk::Label,
    scale: gtk::Scale,
    waveform: Waveform,
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
            waveform: Waveform::new(),
            duration: time_label(0.0),
        };
        parts.scale.set_range(0.0, 1.0);
        parts.scale.set_increments(5_000.0, 15_000.0);
        parts
            .scale
            .update_property(&[gtk::accessible::Property::Label("Playback position")]);
        bar.attach(&parts.elapsed, 0, 1, 1, 1);
        // In the same cell, under the slider.
        bar.attach(&parts.waveform, 1, 1, 1, 1);
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
        bar.update_levels();
        bar.update_mode();
        bar
    }

    /// Puts `widget` above the slider, as wide as it.
    pub fn set_above(&self, widget: &impl IsA<gtk::Widget>) {
        self.attach(widget, 1, 0, 1, 1);
    }

    /// Takes the slider out, for a layout that shows it elsewhere; it keeps working
    /// there, always as a plain line. [`Self::restore_slider`] puts it back.
    pub fn take_slider(&self) -> gtk::Scale {
        let scale = &self.parts().scale;
        if scale.parent().as_ref() == Some(self.upcast_ref()) {
            self.remove(scale);
        }
        self.update_mode();
        scale.clone()
    }

    /// Puts the slider back, once it has been taken out of wherever it was shown.
    pub fn restore_slider(&self) {
        let scale = &self.parts().scale;
        if scale.parent().is_none() {
            self.attach(scale, 1, 1, 1, 1);
        }
        self.update_mode();
    }

    /// How tall the waveform's bars are at most, in pixels.
    pub fn set_waveform_height(&self, height: i32) {
        self.parts().waveform.set_bar_height(height);
        self.update_mode();
    }

    /// Puts the times right at the row's ends, for a layout that lines the row up with
    /// something else.
    pub fn pin_times_to_ends(&self) {
        let parts = self.parts();
        parts.elapsed.set_xalign(0.0);
        parts.duration.set_xalign(1.0);
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

        let ctx = self.ctx();
        let handler = ctx
            .waveforms
            .connect_changed(follow(self, Self::update_levels));
        disconnect_on_destroy(self, &ctx.waveforms, handler);
        let handler = ctx
            .display
            .connect_waveform_seek_bar_notify(follow(self, Self::update_mode));
        disconnect_on_destroy(self, &ctx.display, handler);
        ctx.display
            .bind_property("bar-radius", &self.parts().waveform, "bar-radius")
            .sync_create()
            .build();

        let adjustment = self.parts().scale.adjustment();
        adjustment.connect_value_changed(follow(self, Self::update_played));
        adjustment.connect_changed(follow(self, Self::update_played));
    }

    /// Shows the waveform or the plain line. Clicks only count on the slider's trough, so
    /// over the waveform the trough is as tall as the bars, and what is drawn on it is
    /// hidden, however a theme styles it.
    fn update_mode(&self) {
        let parts = self.parts();
        let on = self.ctx().display.waveform_seek_bar();
        let here = parts.scale.parent().as_ref() == Some(self.upcast_ref());
        let over_waveform = on && here;
        set_css_class(self, "mp-seek-waveform", on);
        parts.waveform.set_visible(on);
        let Some(trough) = parts.scale.first_child() else {
            return;
        };
        let height = if over_waveform {
            parts.waveform.bar_height()
        } else {
            -1
        };
        trough.set_size_request(-1, height);
        let mut part = trough.first_child();
        while let Some(widget) = part {
            widget.set_opacity(if over_waveform { 0.0 } else { 1.0 });
            part = widget.next_sibling();
        }
    }

    fn update_levels(&self) {
        self.parts()
            .waveform
            .set_levels(self.ctx().waveforms.levels());
    }

    fn update_played(&self) {
        let adjustment = self.parts().scale.adjustment();
        let played = adjustment.value() / adjustment.upper().max(1.0);
        self.parts().waveform.set_played(played);
    }

    fn update_track(&self) {
        let has_track = self.ctx().player.current_track().is_some();
        let parts = self.parts();
        parts.scale.set_sensitive(has_track);
        parts.waveform.set_sensitive(has_track);
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
