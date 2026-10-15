//! Every song in one format, with chips that narrow the list to one quality, such as
//! 24/96 or a bitrate band.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use mp_core::{AudioFormat, Track};

use super::formats::stats::{Composition, FormatStats, Quality};
use crate::context::AppContext;
use crate::model::TrackObject;
use crate::ui::icons;
use crate::ui::widgets::discover::codec;
use crate::ui::widgets::{CoverSize, PageHeader, TrackColumn, TrackList};
use crate::ui::{disconnect_on_destroy, keep_alive};
use crate::util::{song_count, total_length};

pub fn build(ctx: &Rc<AppContext>, format: AudioFormat) -> adw::NavigationPage {
    let page = Rc::new(FormatPage::new(ctx, format));
    page.connect_signals();
    page.rebuild_chips();
    page.update_summary();

    let navigation_page = super::page_with_header(format.label(), &page.header, &page.list);
    navigation_page.add_css_class("mp-format-page");

    let weak = Rc::downgrade(&page);
    let handler = ctx.store.connect_tracks_changed(move |_| {
        if let Some(page) = weak.upgrade() {
            page.rebuild_chips();
        }
    });
    disconnect_on_destroy(&navigation_page, &ctx.store, handler);
    keep_alive(&navigation_page, page);
    navigation_page
}

/// The page's state, owned by its content widget (see [`keep_alive`]).
struct FormatPage {
    ctx: Rc<AppContext>,
    format: AudioFormat,
    /// The quality the chips let through, or every quality. Shared with the filter.
    quality: Rc<Cell<Option<Quality>>>,
    custom_filter: gtk::CustomFilter,
    filtered: gtk::FilterListModel,
    header: PageHeader,
    chips: adw::WrapBox,
    play: gtk::Button,
    shuffle: gtk::Button,
    list: TrackList,
}

impl FormatPage {
    fn new(ctx: &Rc<AppContext>, format: AudioFormat) -> Self {
        let header = PageHeader::new(format.label());
        header.set_eyebrow(&format!("{} Format", codec::compression_label(format)));
        header.set_leading(Some(&codec::codec_tile(format, CoverSize::Medium.pixels())));
        let play = header.add_button(icons::PLAY, "_Play", true);
        play.set_tooltip_text(Some("Play these songs in order"));
        let shuffle = header.add_button(icons::SHUFFLE, "_Shuffle", false);
        shuffle.set_tooltip_text(Some("Play these songs in random order"));
        let chips = adw::WrapBox::builder()
            .child_spacing(6)
            .line_spacing(6)
            .css_classes(["mp-filter-chips"])
            .build();
        header.set_extra(&chips);

        let quality = Rc::new(Cell::new(None));
        let wanted = quality.clone();
        let custom_filter = gtk::CustomFilter::new(move |item| {
            item.downcast_ref::<TrackObject>()
                .is_some_and(|track| shows(format, wanted.get(), track.track()))
        });
        let filtered =
            gtk::FilterListModel::new(Some(ctx.store.tracks()), Some(custom_filter.clone()));

        let list = TrackList::builder(ctx)
            .columns(&[
                TrackColumn::Title,
                TrackColumn::Album,
                TrackColumn::Quality,
                TrackColumn::Duration,
            ])
            .covers(true)
            .empty_state(
                &format!("No {} Songs", format.label()),
                "Songs in this format will appear here.",
            )
            .build(&filtered);
        list.set_vexpand(true);

        Self {
            ctx: ctx.clone(),
            format,
            quality,
            custom_filter,
            filtered,
            header,
            chips,
            play,
            shuffle,
            list,
        }
    }

    fn connect_signals(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.play.connect_clicked(move |_| {
            if let Some(page) = weak.upgrade() {
                page.ctx.player.play_all(page.list.track_ids());
            }
        });
        let weak = Rc::downgrade(self);
        self.shuffle.connect_clicked(move |_| {
            if let Some(page) = weak.upgrade() {
                page.ctx.player.shuffle_tracks(page.list.track_ids());
            }
        });
        let weak = Rc::downgrade(self);
        self.filtered.connect_items_changed(move |_, _, _, _| {
            if let Some(page) = weak.upgrade() {
                page.update_summary();
            }
        });
    }

    fn set_quality(&self, quality: Option<Quality>) {
        if self.quality.replace(quality) != quality {
            self.custom_filter.changed(gtk::FilterChange::Different);
        }
    }

    /// "All" and a chip per quality, best first; none when every song is alike.
    fn rebuild_chips(self: &Rc<Self>) {
        let tracks: Vec<TrackObject> = self
            .ctx
            .store
            .tracks()
            .iter::<TrackObject>()
            .filter_map(Result::ok)
            .filter(|track| track.track().format == self.format)
            .collect();
        let composition = Composition::of(tracks.iter().map(TrackObject::track));
        let qualities = composition
            .format(self.format)
            .map(FormatStats::qualities)
            .unwrap_or_default();
        if !qualities
            .iter()
            .any(|&(quality, _)| Some(quality) == self.quality.get())
        {
            self.set_quality(None);
        }

        while let Some(child) = self.chips.first_child() {
            self.chips.remove(&child);
        }
        self.chips.set_visible(qualities.len() > 1);
        if qualities.len() < 2 {
            return;
        }
        let all = self.chip(None, "All", &song_count(composition.tracks));
        for (quality, count) in qualities {
            let tooltip = format!("{} · {}", quality.label(), song_count(count));
            let chip = self.chip(Some(quality), &quality.short_label(), &tooltip);
            chip.set_group(Some(&all));
        }
    }

    fn chip(
        self: &Rc<Self>,
        quality: Option<Quality>,
        label: &str,
        tooltip: &str,
    ) -> gtk::ToggleButton {
        let chip = gtk::ToggleButton::builder()
            .label(label)
            .tooltip_text(tooltip)
            .css_classes(["mp-chip"])
            .active(quality == self.quality.get())
            .build();
        let weak = Rc::downgrade(self);
        chip.connect_toggled(move |chip| {
            if chip.is_active()
                && let Some(page) = weak.upgrade()
            {
                page.set_quality(quality);
            }
        });
        self.chips.append(&chip);
        chip
    }

    /// What the format is, and how many songs the list shows. They are counted in
    /// the store itself: looking up every item of the filter model is many times
    /// slower in a large library.
    fn update_summary(&self) {
        let quality = self.quality.get();
        let (count, duration_ms, bytes) = self
            .ctx
            .store
            .tracks()
            .iter::<TrackObject>()
            .filter_map(Result::ok)
            .filter(|track| shows(self.format, quality, track.track()))
            .fold((0, 0, 0), |(count, duration, bytes), track| {
                let track = track.track();
                (
                    count + 1,
                    duration + track.duration_ms,
                    bytes + track.file_size,
                )
            });
        let mut parts = vec![codec::description(self.format).to_owned()];
        if count > 0 {
            parts.push(song_count(count));
            parts.push(total_length(duration_ms));
            parts.push(glib::format_size(bytes).to_string());
        }
        self.header.set_subtitle(&parts.join(" · "));
        self.play.set_sensitive(count > 0);
        self.shuffle.set_sensitive(count > 0);
    }
}

/// Whether the page lists `track`: it is in `format` and, with a quality chosen, of
/// that quality.
fn shows(format: AudioFormat, quality: Option<Quality>, track: &Track) -> bool {
    track.format == format && quality.is_none_or(|quality| Quality::of(track) == quality)
}
