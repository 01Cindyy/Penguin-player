//! [`DisplayOptions`]: the settings that change what several widgets show, as
//! properties those widgets bind to, so a change in Preferences reaches them all.

use std::cell::Cell;

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use mp_core::{CornerStyle, Settings};

mod imp {
    use super::*;

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::DisplayOptions)]
    pub struct DisplayOptions {
        /// Show the spectrum visualiser above the seek bars.
        #[property(get, set)]
        visualizer: Cell<bool>,
        /// Show the playing song's bit depth, sample rate and bit rate, not just its
        /// format.
        #[property(get, set)]
        quality_details: Cell<bool>,
        /// Use the song's waveform as the seek bar instead of a plain line.
        #[property(get, set)]
        waveform_seek_bar: Cell<bool>,
        /// How rounded the visualiser's and waveform's bars are, in pixels.
        #[property(get, set)]
        bar_radius: Cell<f32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DisplayOptions {
        const NAME: &'static str = "MpDisplayOptions";
        type Type = super::DisplayOptions;
    }

    #[glib::derived_properties]
    impl ObjectImpl for DisplayOptions {}
}

glib::wrapper! {
    /// Owned by [`crate::AppContext`]; reach it as `ctx.display`, and change it
    /// through the context so the choice is saved.
    pub struct DisplayOptions(ObjectSubclass<imp::DisplayOptions>);
}

impl DisplayOptions {
    pub fn new(settings: &Settings) -> Self {
        glib::Object::builder()
            .property("visualizer", settings.visualizer)
            .property("quality-details", settings.quality_details)
            .property("waveform-seek-bar", settings.waveform_seek_bar)
            .property("bar-radius", bar_radius(settings.corners))
            .build()
    }
}

/// The corner radius of bars with `corners`. They are 4 pixels wide, so 2 makes their
/// ends round.
pub fn bar_radius(corners: CornerStyle) -> f32 {
    match corners {
        CornerStyle::Round => 2.0,
        CornerStyle::Slight => 1.0,
        CornerStyle::Square => 0.0,
    }
}
