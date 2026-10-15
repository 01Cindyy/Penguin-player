use std::cell::RefCell;

use gtk::glib;
use gtk::glib::prelude::*;
use gtk::glib::subclass::prelude::*;
use mp_core::{Playlist, PlaylistEntry, PlaylistId, TrackId};

mod imp {
    use std::marker::PhantomData;

    use super::*;

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::PlaylistObject)]
    pub struct PlaylistObject {
        pub(super) playlist: RefCell<Option<Playlist>>,

        #[property(get = |o: &Self| o.with(|p| p.id.0), type = i64)]
        id: PhantomData<i64>,
        #[property(get = |o: &Self| o.with(|p| p.name.clone()), type = String)]
        name: PhantomData<String>,
        /// Number of entries, including files that are currently missing.
        #[property(get = |o: &Self| o.with(|p| p.entries.len() as u32), type = u32)]
        entry_count: PhantomData<u32>,
        /// Number of entries whose file is in the library.
        #[property(get = |o: &Self| o.with(|p| p.available_tracks().count() as u32), type = u32)]
        track_count: PhantomData<u32>,
    }

    impl PlaylistObject {
        pub(super) fn with<R>(&self, f: impl FnOnce(&Playlist) -> R) -> R {
            let playlist = self.playlist.borrow();
            f(playlist
                .as_ref()
                .expect("PlaylistObject::new always sets the playlist"))
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PlaylistObject {
        const NAME: &'static str = "MpPlaylistObject";
        type Type = super::PlaylistObject;
    }

    #[glib::derived_properties]
    impl ObjectImpl for PlaylistObject {
        fn signals() -> &'static [glib::subclass::Signal] {
            static SIGNALS: std::sync::OnceLock<Vec<glib::subclass::Signal>> =
                std::sync::OnceLock::new();
            SIGNALS.get_or_init(|| vec![glib::subclass::Signal::builder("changed").build()])
        }
    }
}

glib::wrapper! {
    /// A user playlist. Unlike tracks, playlist objects are updated in place when the
    /// playlist changes, so widgets bound to one stay valid.
    pub struct PlaylistObject(ObjectSubclass<imp::PlaylistObject>);
}

impl PlaylistObject {
    pub fn new(playlist: Playlist) -> Self {
        let object: Self = glib::Object::new();
        object.imp().playlist.replace(Some(playlist));
        object
    }

    pub fn playlist_id(&self) -> PlaylistId {
        self.imp().with(|p| p.id)
    }

    /// A copy of the playlist's current data.
    pub fn playlist(&self) -> Playlist {
        self.imp().with(Clone::clone)
    }

    pub fn entries(&self) -> Vec<PlaylistEntry> {
        self.imp().with(|p| p.entries.clone())
    }

    /// Tracks present in the library, in playlist order.
    pub fn available_tracks(&self) -> Vec<TrackId> {
        self.imp().with(|p| p.available_tracks().collect())
    }

    /// Replaces the data and notifies the properties that changed.
    pub(crate) fn update(&self, playlist: Playlist) {
        let old = self.imp().playlist.replace(Some(playlist.clone()));
        let Some(old) = old else { return };
        if old.name != playlist.name {
            self.notify_name();
        }
        if old.entries != playlist.entries {
            self.notify_entry_count();
            self.notify_track_count();
        }
        if old != playlist {
            self.emit_by_name::<()>("changed", &[]);
        }
    }

    /// Called whenever the playlist's name or entries change.
    pub fn connect_changed<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_closure(
            "changed",
            false,
            glib::closure_local!(move |playlist: &Self| f(playlist)),
        )
    }
}
