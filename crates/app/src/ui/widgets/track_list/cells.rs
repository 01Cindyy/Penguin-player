//! Columns of a [`super::TrackList`] and the widget that renders one cell.

use std::cell::{OnceCell, RefCell};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;
use mp_core::util::format_duration;

use crate::model::TrackObject;
use crate::services::covers::CoverCache;
use crate::ui::icons;
use crate::ui::widgets::{FormatBadge, SongHeading};
use crate::ui::{ellipsized_label, set_css_class};

/// A column a track list can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackColumn {
    /// Track number on its disc.
    Number,
    /// Row number in the list, for playlists and the queue.
    Position,
    /// Title with the artist underneath, and the cover when covers are enabled.
    Title,
    Artist,
    Album,
    Year,
    Duration,
    Format,
    /// Bit depth and sample rate, or the bitrate of lossy formats.
    Quality,
}

impl TrackColumn {
    pub(super) fn header(self) -> &'static str {
        match self {
            TrackColumn::Number | TrackColumn::Position => "#",
            TrackColumn::Title => "Title",
            TrackColumn::Artist => "Artist",
            TrackColumn::Album => "Album",
            TrackColumn::Year => "Year",
            TrackColumn::Duration => "Time",
            TrackColumn::Format => "Format",
            TrackColumn::Quality => "Quality",
        }
    }

    /// Text columns share the spare width; the others have a fixed width.
    pub(super) fn fixed_width(self) -> Option<i32> {
        match self {
            TrackColumn::Number | TrackColumn::Position => Some(56),
            TrackColumn::Year => Some(72),
            TrackColumn::Duration => Some(76),
            TrackColumn::Format => Some(96),
            TrackColumn::Quality => Some(136),
            TrackColumn::Title | TrackColumn::Artist | TrackColumn::Album => None,
        }
    }

    /// Lists narrower than this many sp hide the column, leaving room for the title.
    pub(super) fn hidden_below(self) -> Option<u32> {
        match self {
            TrackColumn::Artist | TrackColumn::Album | TrackColumn::Year => Some(600),
            TrackColumn::Format | TrackColumn::Quality => Some(400),
            _ => None,
        }
    }

    /// How clicking the column header sorts, if it can.
    pub(super) fn sorter(self) -> Option<gtk::Sorter> {
        let property = |name: &str| {
            gtk::PropertyExpression::new(TrackObject::static_type(), None::<&gtk::Expression>, name)
        };
        let text = |name: &str| {
            let sorter = gtk::StringSorter::new(Some(property(name)));
            sorter.set_ignore_case(true);
            sorter.set_collation(gtk::Collation::Unicode);
            sorter.upcast::<gtk::Sorter>()
        };
        let number = |name: &str| gtk::NumericSorter::new(Some(property(name))).upcast();
        match self {
            TrackColumn::Number => Some(number("track-number")),
            TrackColumn::Position => None,
            TrackColumn::Title => Some(text("title")),
            TrackColumn::Artist => Some(text("artist")),
            TrackColumn::Album => Some(text("album")),
            TrackColumn::Year => Some(number("year")),
            TrackColumn::Duration => Some(number("duration-ms")),
            TrackColumn::Format => Some(text("format")),
            TrackColumn::Quality => Some(quality_sorter()),
        }
    }
}

/// The widgets inside a cell, which depend on its column.
enum CellContent {
    /// Track or row number, swapped for a speaker icon while the track plays.
    Number {
        label: gtk::Label,
        playing_icon: gtk::Image,
    },
    Title(SongHeading),
    Text(gtk::Label),
    Format(FormatBadge),
}

mod imp {
    use std::cell::Cell;

    use super::*;

    #[derive(Default)]
    pub struct TrackCell {
        pub(super) column: Cell<Option<TrackColumn>>,
        pub(super) content: OnceCell<CellContent>,
        /// The bound track and the handler watching its `playing` flag.
        pub(super) bound: RefCell<Option<(TrackObject, glib::SignalHandlerId)>>,
        /// The row's number column marks the playing song instead of the cover.
        pub(super) number_marks_playing: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TrackCell {
        const NAME: &'static str = "MpTrackCell";
        type Type = super::TrackCell;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for TrackCell {
        fn dispose(&self) {
            self.obj().unbind();
        }
    }

    impl WidgetImpl for TrackCell {}
    impl BinImpl for TrackCell {}
}

glib::wrapper! {
    /// One cell of a track list row. Created once per visible cell and rebound as
    /// the list scrolls, so binding only updates labels.
    pub struct TrackCell(ObjectSubclass<imp::TrackCell>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl TrackCell {
    pub fn new(column: TrackColumn, covers: Option<&CoverCache>) -> Self {
        let cell: Self = glib::Object::new();
        cell.imp().column.set(Some(column));
        let (child, content) = build_content(column, covers);
        cell.set_child(Some(&child));
        assert!(
            cell.imp().content.set(content).is_ok(),
            "a new cell has no content yet"
        );
        cell
    }

    pub fn bind(&self, track: &TrackObject, position: u32) {
        self.unbind();
        match self.content() {
            CellContent::Number { label, .. } => {
                let text = match self.column() {
                    TrackColumn::Position => (position + 1).to_string(),
                    _ => match track.track_number() {
                        0 => String::new(),
                        number => number.to_string(),
                    },
                };
                label.set_label(&text);
            }
            CellContent::Title(heading) => heading.set_track(track),
            CellContent::Text(label) => {
                label.set_label(&cell_text(self.column(), track));
            }
            CellContent::Format(badge) => badge.set_track(Some(track.track())),
        }

        let weak = self.downgrade();
        let handler = track.connect_playing_notify(move |track| {
            if let Some(cell) = weak.upgrade() {
                cell.show_playing(track.playing());
            }
        });
        self.show_playing(track.playing());
        self.imp().bound.replace(Some((track.clone(), handler)));
    }

    pub fn unbind(&self) {
        if let Some((track, handler)) = self.imp().bound.take() {
            track.disconnect(handler);
        }
    }

    /// Hides the artist under the title (see `TrackListBuilder::artists`).
    pub fn hide_artist(&self) {
        if let CellContent::Title(heading) = self.content() {
            heading.hide_artist();
        }
    }

    /// Leaves the now-playing icon to the row's number column, so the cover in the
    /// title doesn't show a second one.
    pub fn leave_playing_to_number(&self) {
        self.imp().number_marks_playing.set(true);
    }

    /// Keeps the Position column right when rows move without being rebound. Cells
    /// whose row was removed report `INVALID_LIST_POSITION`, which is ignored.
    pub fn set_position(&self, position: u32) {
        if position == gtk::INVALID_LIST_POSITION {
            return;
        }
        if let (TrackColumn::Position, CellContent::Number { label, .. }) =
            (self.column(), self.content())
        {
            label.set_label(&(position + 1).to_string());
        }
    }

    fn show_playing(&self, playing: bool) {
        set_css_class(self, "mp-playing", playing);
        match self.content() {
            CellContent::Number {
                label,
                playing_icon,
            } => {
                label.set_visible(!playing);
                playing_icon.set_visible(playing);
            }
            CellContent::Title(heading) if !self.imp().number_marks_playing.get() => {
                heading.set_playing(playing);
            }
            _ => {}
        }
    }

    fn column(&self) -> TrackColumn {
        self.imp()
            .column
            .get()
            .expect("cells are created with a column")
    }

    fn content(&self) -> &CellContent {
        self.imp()
            .content
            .get()
            .expect("cells are created with content")
    }
}

fn cell_text(column: TrackColumn, track: &TrackObject) -> String {
    match column {
        TrackColumn::Artist => track.artist(),
        TrackColumn::Album => track.album(),
        TrackColumn::Year => match track.year() {
            0 => String::new(),
            year => year.to_string(),
        },
        TrackColumn::Duration => format_duration(track.track().duration()),
        TrackColumn::Quality => track.quality(),
        _ => String::new(),
    }
}

fn build_content(column: TrackColumn, covers: Option<&CoverCache>) -> (gtk::Widget, CellContent) {
    let text = |classes: &[&str]| {
        let label = ellipsized_label(classes);
        (label.clone().upcast(), CellContent::Text(label))
    };
    match column {
        TrackColumn::Number | TrackColumn::Position => number_cell(),
        TrackColumn::Title => {
            let heading = SongHeading::new(covers);
            (heading.root.clone().upcast(), CellContent::Title(heading))
        }
        // Left-aligned like the column headers, which GTK always starts at the left.
        TrackColumn::Duration | TrackColumn::Year => {
            text(&["mp-track-duration", "numeric", "dim-label"])
        }
        TrackColumn::Quality => text(&["mp-track-quality", "numeric", "dim-label"]),
        TrackColumn::Artist => text(&["mp-track-artist", "dim-label"]),
        TrackColumn::Album => text(&["mp-track-album", "dim-label"]),
        TrackColumn::Format => {
            let badge = FormatBadge::new();
            (badge.clone().upcast(), CellContent::Format(badge))
        }
    }
}

/// The track or row number, with the now-playing icon that replaces it.
fn number_cell() -> (gtk::Widget, CellContent) {
    let label = gtk::Label::builder()
        .xalign(1.0)
        .css_classes(["mp-track-number", "numeric", "dim-label"])
        .build();
    let playing_icon = gtk::Image::builder()
        .icon_name(icons::NOW_PLAYING)
        .halign(gtk::Align::End)
        .visible(false)
        .css_classes(["mp-playing-icon"])
        .tooltip_text("Now playing")
        .build();
    let row = gtk::Box::builder()
        .halign(gtk::Align::End)
        .margin_end(6)
        .build();
    row.append(&label);
    row.append(&playing_icon);
    (
        row.upcast(),
        CellContent::Number {
            label,
            playing_icon,
        },
    )
}

/// Sorts lossless above lossy, then by bit depth, sample rate and bitrate.
fn quality_sorter() -> gtk::Sorter {
    let rank = |item: &glib::Object| {
        item.downcast_ref::<TrackObject>().map(|track| {
            let track = track.track();
            (
                track.format.is_lossless(),
                track.bit_depth,
                track.sample_rate,
                track.bitrate_kbps,
            )
        })
    };
    gtk::CustomSorter::new(move |a, b| rank(a).cmp(&rank(b)).into()).upcast()
}
