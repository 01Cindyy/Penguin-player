//! Every song in the library, with quick filters by quality and format.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
use mp_core::{AudioFormat, Track};

use crate::context::AppContext;
use crate::model::TrackObject;
use crate::ui::icons;
use crate::ui::widgets::browse::{LibraryStatus, follow_library_status};
use crate::ui::widgets::page_header::PageHeader;
use crate::ui::widgets::{TrackColumn, TrackList};
use crate::ui::{disconnect_on_destroy, keep_alive};
use crate::util::{song_count, total_length};

/// Which songs the filter chips let through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum SongFilter {
    #[default]
    All,
    Lossless,
    HiRes,
    Format(AudioFormat),
}

impl SongFilter {
    fn matches(self, track: &Track) -> bool {
        match self {
            SongFilter::All => true,
            SongFilter::Lossless => track.format.is_lossless(),
            SongFilter::HiRes => track.is_hi_res(),
            SongFilter::Format(format) => track.format == format,
        }
    }

    fn label(self) -> &'static str {
        match self {
            SongFilter::All => "All",
            SongFilter::Lossless => "Lossless",
            SongFilter::HiRes => "Hi-Res",
            SongFilter::Format(format) => format.label(),
        }
    }
}

pub fn build(ctx: &Rc<AppContext>) -> adw::NavigationPage {
    let page = Rc::new(SongsPage::new(ctx));
    page.connect_signals();
    page.rebuild_chips();
    page.update_summary();
    page.update_empty_state();

    let navigation_page = super::page_with_header("Songs", &page.header, &page.list);
    navigation_page.add_css_class("mp-songs-page");

    let weak = Rc::downgrade(&page);
    let handler = ctx.store.connect_tracks_changed(move |_| {
        if let Some(page) = weak.upgrade() {
            page.rebuild_chips();
        }
    });
    disconnect_on_destroy(&navigation_page, &ctx.store, handler);
    let weak = Rc::downgrade(&page);
    follow_library_status(ctx, &navigation_page, move || {
        if let Some(page) = weak.upgrade() {
            page.update_empty_state();
        }
    });
    keep_alive(&navigation_page, page);
    navigation_page
}

/// The page's state, owned by its content widget (see [`keep_alive`]).
struct SongsPage {
    ctx: Rc<AppContext>,
    /// Shared with the list filter, which reads it for every track.
    filter: Rc<Cell<SongFilter>>,
    custom_filter: gtk::CustomFilter,
    filtered: gtk::FilterListModel,
    header: PageHeader,
    chips: adw::WrapBox,
    play: gtk::Button,
    shuffle: gtk::Button,
    list: TrackList,
}

impl SongsPage {
    fn new(ctx: &Rc<AppContext>) -> Self {
        let header = PageHeader::new("Songs");
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

        let filter = Rc::new(Cell::new(SongFilter::All));
        let current = filter.clone();
        let custom_filter = gtk::CustomFilter::new(move |item| {
            item.downcast_ref::<TrackObject>()
                .is_some_and(|track| current.get().matches(track.track()))
        });
        let filtered =
            gtk::FilterListModel::new(Some(ctx.store.tracks()), Some(custom_filter.clone()));

        let list = TrackList::builder(ctx)
            .columns(&[
                TrackColumn::Title,
                TrackColumn::Album,
                TrackColumn::Format,
                TrackColumn::Duration,
            ])
            .covers(true)
            .build(&filtered);
        list.set_vexpand(true);

        Self {
            ctx: ctx.clone(),
            filter,
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

    fn set_filter(&self, filter: SongFilter) {
        if self.filter.replace(filter) != filter {
            self.custom_filter.changed(gtk::FilterChange::Different);
        }
    }

    /// One chip per filter that would show something; formats most common first.
    fn rebuild_chips(self: &Rc<Self>) {
        let counts = filter_counts(&self.ctx.store.tracks());
        // An empty library has nothing to filter.
        self.chips.set_visible(counts[0].1 > 0);
        let available: Vec<(SongFilter, usize)> = counts
            .into_iter()
            .filter(|&(filter, count)| filter == SongFilter::All || count > 0)
            .collect();
        if !available
            .iter()
            .any(|&(filter, _)| filter == self.filter.get())
        {
            self.set_filter(SongFilter::All);
        }

        while let Some(child) = self.chips.first_child() {
            self.chips.remove(&child);
        }
        let mut group: Option<gtk::ToggleButton> = None;
        let mut previous: Option<SongFilter> = None;
        for (filter, count) in available {
            // Quality chips come first; a divider sets the format chips apart.
            if matches!(filter, SongFilter::Format(_))
                && !matches!(previous, Some(SongFilter::Format(_)))
            {
                self.chips.append(&chip_divider());
            }
            previous = Some(filter);
            let chip = gtk::ToggleButton::builder()
                .label(filter.label())
                .tooltip_text(song_count(count))
                .css_classes(["mp-chip"])
                .active(filter == self.filter.get())
                .build();
            chip.set_group(group.as_ref());
            group.get_or_insert_with(|| chip.clone());
            let weak = Rc::downgrade(self);
            chip.connect_toggled(move |chip| {
                if chip.is_active()
                    && let Some(page) = weak.upgrade()
                {
                    page.set_filter(filter);
                }
            });
            self.chips.append(&chip);
        }
    }

    /// Counts what the filter lets through in the store itself: looking up every item
    /// of the filter model is many times slower in a large library.
    fn update_summary(&self) {
        let filter = self.filter.get();
        let (count, duration_ms) = self
            .ctx
            .store
            .tracks()
            .iter::<TrackObject>()
            .filter_map(Result::ok)
            .filter(|track| filter.matches(track.track()))
            .fold((0, 0), |(count, total), track| {
                (count + 1, total + track.duration_ms())
            });
        let summary = match count {
            0 => String::new(),
            _ => format!("{} · {}", song_count(count), total_length(duration_ms)),
        };
        self.header.set_subtitle(&summary);
        self.play.set_sensitive(count > 0);
        self.shuffle.set_sensitive(count > 0);
    }

    /// Explains an empty library: still scanning, folder unavailable, or no music.
    fn update_empty_state(&self) {
        let status = LibraryStatus::of(&self.ctx.library_service);
        self.list.set_empty_state(
            status.title("No Songs"),
            &status.description("Songs in your music folder will appear here."),
            status.busy(),
        );
    }
}

fn chip_divider() -> gtk::Separator {
    gtk::Separator::builder()
        .orientation(gtk::Orientation::Vertical)
        .valign(gtk::Align::Center)
        .css_classes(["mp-chip-divider"])
        .build()
}

/// How many tracks each chip would show, in chip order.
fn filter_counts(tracks: &gio::ListStore) -> Vec<(SongFilter, usize)> {
    let mut total = 0;
    let mut lossless = 0;
    let mut hi_res = 0;
    let mut formats: HashMap<AudioFormat, usize> = HashMap::new();
    for track in tracks.iter::<TrackObject>().filter_map(Result::ok) {
        let track = track.track();
        total += 1;
        lossless += usize::from(track.format.is_lossless());
        hi_res += usize::from(track.is_hi_res());
        *formats.entry(track.format).or_default() += 1;
    }
    let mut formats: Vec<_> = formats.into_iter().collect();
    formats.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    let mut counts = vec![
        (SongFilter::All, total),
        (SongFilter::Lossless, lossless),
        (SongFilter::HiRes, hi_res),
    ];
    counts.extend(
        formats
            .into_iter()
            .map(|(format, count)| (SongFilter::Format(format), count)),
    );
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::test_track;

    #[test]
    fn filters_and_counts() {
        let mut mp3 = test_track(2, "b", "x", "y");
        mp3.format = AudioFormat::Mp3;
        mp3.bit_depth = None;
        let hires = test_track(1, "a", "x", "y");
        assert!(SongFilter::HiRes.matches(&hires));
        assert!(!SongFilter::Lossless.matches(&mp3));
        assert!(SongFilter::Format(AudioFormat::Mp3).matches(&mp3));

        let store = gio::ListStore::new::<TrackObject>();
        store.append(&TrackObject::new(hires));
        store.append(&TrackObject::new(mp3));
        let counts = filter_counts(&store);
        assert_eq!(counts[0], (SongFilter::All, 2));
        assert_eq!(counts[1], (SongFilter::Lossless, 1));
        assert_eq!(counts[2], (SongFilter::HiRes, 1));
        assert_eq!(counts.len(), 5);
    }
}
