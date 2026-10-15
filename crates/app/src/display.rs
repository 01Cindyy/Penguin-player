//! [`DisplayOptions`]: the settings that change what several widgets show, as
//! properties those widgets bind to, so a change in Preferences reaches them all.

use std::cell::Cell;

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use mp_core::Settings;

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
            .build()
    }
}
