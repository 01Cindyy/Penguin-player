//! The start of every song row: a small cover beside the title and the artist.

use adw::prelude::*;

use crate::model::TrackObject;
use crate::services::covers::CoverCache;
use crate::ui::ellipsized_label;
use crate::ui::widgets::{CoverImage, CoverSize};

/// A row-sized cover (if any), and the title (`mp-track-title`) over the artist
/// (`mp-track-artist`). Rows are recycled, so [`Self::set_track`] only updates them.
pub struct SongHeading {
    pub root: gtk::Box,
    cover: Option<CoverImage>,
    title: gtk::Label,
    artist: gtk::Label,
}

impl SongHeading {
    /// With covers from `covers`, or without art for `None`.
    pub fn new(covers: Option<&CoverCache>) -> Self {
        let title = ellipsized_label(&["mp-track-title"]);
        let artist = ellipsized_label(&["mp-track-artist", "caption", "dim-label"]);
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .valign(gtk::Align::Center)
            .hexpand(true)
            .spacing(2)
            .build();
        text.append(&title);
        text.append(&artist);

        let root = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let cover = covers.map(|covers| CoverImage::new(covers, CoverSize::Row));
        if let Some(cover) = &cover {
            root.append(cover);
        }
        root.append(&text);
        Self {
            root,
            cover,
            title,
            artist,
        }
    }

    pub fn set_track(&self, track: &TrackObject) {
        self.title.set_label(&track.title());
        self.artist.set_label(&track.artist());
        if let Some(cover) = &self.cover {
            cover.set_cover(track.track().cover.as_ref());
        }
    }

    /// Shows the now-playing indicator over the cover.
    pub fn set_playing(&self, playing: bool) {
        if let Some(cover) = &self.cover {
            cover.set_playing(playing);
        }
    }

    /// Leaves the artist out, e.g. where every song is by the same one.
    pub fn hide_artist(&self) {
        self.artist.set_visible(false);
    }
}
