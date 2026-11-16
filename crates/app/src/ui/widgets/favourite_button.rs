//! The star that shows whether a song is a favourite and, clicked, makes it one or stops
//! it being one, through [`ACTION`].

use std::cell::{Cell, OnceCell, RefCell};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;

use crate::model::TrackObject;
use crate::ui::shortcuts::primary;
use crate::ui::{icons, set_css_class, set_tooltip};

/// Makes the song whose id (`int64`) it is given a favourite, or stops it being one.
pub const ACTION: &str = "app.toggle-favourite-song";

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct FavouriteButton {
        /// The song shown, and the handler following its `favourite` flag.
        pub(super) bound: RefCell<Option<(TrackObject, glib::SignalHandlerId)>>,
        /// The icon and text of a labelled button.
        pub(super) content: OnceCell<adw::ButtonContent>,
        /// Whether the button is for the playing song, whose shortcut its tooltip names.
        pub(super) playing_song: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FavouriteButton {
        const NAME: &'static str = "MpFavouriteButton";
        type Type = super::FavouriteButton;
        type ParentType = gtk::Button;
    }

    impl ObjectImpl for FavouriteButton {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_valign(gtk::Align::Center);
            obj.set_focus_on_click(false);
            obj.show(false);
        }

        fn dispose(&self) {
            self.obj().unbind();
        }
    }

    impl WidgetImpl for FavouriteButton {}
    impl ButtonImpl for FavouriteButton {}
}

glib::wrapper! {
    /// A song's star, in the accent colour while the song is a favourite
    /// (`mp-favourite`). Hidden, or for a labelled button greyed out, while it has no
    /// song or one from outside the library.
    pub struct FavouriteButton(ObjectSubclass<imp::FavouriteButton>)
        @extends gtk::Button, gtk::Widget,
        @implements gtk::Accessible, gtk::Actionable, gtk::Buildable, gtk::ConstraintTarget;
}

impl FavouriteButton {
    /// A star on its own (`mp-favourite-button`). In a list, `reveal_on_hover` shows a
    /// song's outline star only while its row is hovered or has the keyboard focus
    /// (`mp-reveal-on-hover`).
    pub fn new(reveal_on_hover: bool) -> Self {
        let button: Self = glib::Object::new();
        button.set_css_classes(&["flat", "circular", "mp-favourite-button"]);
        set_css_class(&button, "mp-reveal-on-hover", reveal_on_hover);
        button.set_track(None);
        button
    }

    /// "Favourite" beside the star, looking like the toggle buttons it sits with
    /// (`mp-toggle-labelled`, `mp-active` while the song is a favourite).
    pub fn labelled() -> Self {
        let button: Self = glib::Object::new();
        let content = adw::ButtonContent::builder().label("Favourite").build();
        button.set_child(Some(&content));
        button.set_css_classes(&["flat", "mp-toggle-button", "mp-toggle-labelled"]);
        assert!(
            button.imp().content.set(content).is_ok(),
            "a new button has no content yet"
        );
        button.set_track(None);
        button
    }

    /// Names Ctrl+D in the tooltip, for buttons that show the playing song.
    pub fn show_shortcut(&self) {
        self.imp().playing_song.set(true);
        self.show(self.favourite());
    }

    /// Shows `track`'s star, following it until the next call.
    pub fn set_track(&self, track: Option<&TrackObject>) {
        self.unbind();
        let in_library = track.is_some_and(TrackObject::in_library);
        if self.imp().content.get().is_some() {
            self.set_sensitive(in_library);
        } else {
            self.set_visible(in_library);
        }
        // Without a song there is no target, which the action needs.
        let Some(track) = track.filter(|track| track.in_library()) else {
            self.set_action_name(None);
            self.show(false);
            return;
        };
        self.set_action_target_value(Some(&track.track_id().0.to_variant()));
        self.set_action_name(Some(ACTION));
        let weak = self.downgrade();
        let handler = track.connect_favourite_notify(move |track| {
            if let Some(button) = weak.upgrade() {
                button.show(track.favourite());
            }
        });
        self.show(track.favourite());
        self.imp().bound.replace(Some((track.clone(), handler)));
    }

    fn unbind(&self) {
        if let Some((track, handler)) = self.imp().bound.take() {
            track.disconnect(handler);
        }
    }

    fn favourite(&self) -> bool {
        self.imp()
            .bound
            .borrow()
            .as_ref()
            .is_some_and(|(track, _)| track.favourite())
    }

    fn show(&self, favourite: bool) {
        let imp = self.imp();
        let icon = if favourite {
            icons::STARRED
        } else {
            icons::NON_STARRED
        };
        match imp.content.get() {
            Some(content) => {
                content.set_icon_name(icon);
                set_css_class(self, "mp-active", favourite);
            }
            None => {
                self.set_icon_name(icon);
                set_css_class(self, "mp-favourite", favourite);
            }
        }
        let does = if favourite {
            "Remove from Favourites"
        } else {
            "Add to Favourites"
        };
        let tooltip = if imp.playing_song.get() {
            format!("{does} ({})", primary("D"))
        } else {
            does.to_owned()
        };
        set_tooltip(self, &tooltip);
    }
}
