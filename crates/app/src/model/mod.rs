//! GObject wrappers around library data, and the [`LibraryStore`] that holds them.

mod album;
mod artist;
mod list_sync;
mod playlist;
mod store;
mod track;

pub use album::AlbumObject;
pub use artist::ArtistObject;
pub use list_sync::sync_list_store;
pub use playlist::PlaylistObject;
pub use store::{LibrarySnapshot, LibraryStore, track_ids_of};
pub use track::TrackObject;

#[cfg(test)]
pub(crate) use track::tests::track as test_track;
