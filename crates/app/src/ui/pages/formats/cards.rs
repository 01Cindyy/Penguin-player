//! The Formats page's cards: the library overview and one card per format.

use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use mp_core::AudioFormat;
use mp_core::util::format_total_duration;

use super::stats::{Composition, FormatStats};
use crate::ui::widgets::QualityTier;
use crate::ui::widgets::discover::{CompositionBar, Figure, Segment, codec};
use crate::util::{format_count, song_count};

/// The overview at the top of the page: a bar showing how much of the library each
/// format makes up, its legend, and how the songs split into quality tiers.
pub struct Overview {
    pub root: gtk::Box,
    /// The tiers side by side; the page stacks them when it is narrow.
    pub tiers_box: gtk::Box,
    bar: CompositionBar,
    legend: adw::WrapBox,
    tiers: Vec<(QualityTier, TierTile)>,
}

impl Overview {
    pub fn new() -> Self {
        let title = gtk::Label::builder()
            .label("Library Composition")
            .xalign(0.0)
            .css_classes(["heading"])
            .accessible_role(gtk::AccessibleRole::Heading)
            .build();
        let bar = CompositionBar::new();
        let legend = adw::WrapBox::builder()
            .child_spacing(18)
            .line_spacing(6)
            .css_classes(["mp-composition-legend"])
            .build();
        let composition = gtk::Box::new(gtk::Orientation::Vertical, 12);
        composition.append(&title);
        composition.append(&bar);
        composition.append(&legend);

        let tiers_box = gtk::Box::builder()
            .spacing(12)
            .homogeneous(true)
            .css_classes(["mp-quality-tiers"])
            .build();
        let tiers = QualityTier::BEST_FIRST
            .into_iter()
            .map(|tier| {
                let tile = TierTile::new(tier);
                tiers_box.append(&tile.root);
                (tier, tile)
            })
            .collect();

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(24)
            .css_classes(["mp-format-overview"])
            .build();
        root.append(&composition);
        root.append(&tiers_box);
        Self {
            root,
            tiers_box,
            bar,
            legend,
            tiers,
        }
    }

    pub fn update(&self, composition: &Composition) {
        let segments = composition
            .formats
            .iter()
            .map(|stats| Segment {
                weight: stats.tracks as u64,
                css_class: codec::css_class(stats.format),
                tooltip: format!(
                    "{} · {} ({})",
                    stats.format.label(),
                    song_count(stats.tracks),
                    composition.percent(stats.tracks)
                ),
            })
            .collect();
        let description: Vec<String> = composition
            .formats
            .iter()
            .map(|stats| {
                format!(
                    "{} {}",
                    stats.format.label(),
                    composition.percent(stats.tracks)
                )
            })
            .collect();
        self.bar.set_segments(
            segments,
            &format!("Songs by format: {}", description.join(", ")),
        );

        while let Some(child) = self.legend.first_child() {
            self.legend.remove(&child);
        }
        for stats in &composition.formats {
            self.legend.append(&legend_item(stats, composition));
        }
        for (tier, tile) in &self.tiers {
            tile.update(composition, *tier);
        }
    }
}

/// One quality tier: its name, how many songs, their share and what it means.
struct TierTile {
    root: gtk::Box,
    count: gtk::Label,
    share: gtk::Label,
    meter: gtk::LevelBar,
}

impl TierTile {
    fn new(tier: QualityTier) -> Self {
        let name = gtk::Label::builder()
            .label(tier.label())
            .halign(gtk::Align::Start)
            .css_classes(["mp-format-badge", tier.css_class()])
            .build();
        let count = gtk::Label::builder()
            .xalign(0.0)
            .css_classes(["mp-tier-count", "title-2", "numeric"])
            .build();
        let share = gtk::Label::builder()
            .xalign(1.0)
            .hexpand(true)
            .valign(gtk::Align::Baseline)
            .css_classes(["numeric", "dim-label"])
            .build();
        count.set_valign(gtk::Align::Baseline);
        let numbers = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        numbers.append(&count);
        numbers.append(&share);

        let meter = meter(tier.css_class());
        let description = gtk::Label::builder()
            .label(tier.description())
            .xalign(0.0)
            .wrap(true)
            .css_classes(["caption", "dim-label"])
            .build();

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .css_classes(["mp-quality-tier"])
            .build();
        root.append(&name);
        root.append(&numbers);
        root.append(&meter);
        root.append(&description);
        Self {
            root,
            count,
            share,
            meter,
        }
    }

    fn update(&self, composition: &Composition, tier: QualityTier) {
        let songs = composition.tier(tier);
        self.count.set_label(&song_count(songs));
        self.share.set_label(&composition.percent(songs));
        self.meter.set_value(composition.fraction(songs));
        self.root
            .update_property(&[gtk::accessible::Property::Label(&format!(
                "{}: {}, {} of the library",
                tier.label(),
                song_count(songs),
                composition.percent(songs)
            ))]);
    }
}

/// A coloured dot, the format's name and its share of the library.
fn legend_item(stats: &FormatStats, composition: &Composition) -> gtk::Widget {
    let name = gtk::Label::builder()
        .label(stats.format.label())
        .css_classes(["mp-legend-name"])
        .build();
    let share = gtk::Label::builder()
        .label(composition.percent(stats.tracks))
        .css_classes(["numeric", "dim-label"])
        .build();
    let item = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    item.add_css_class("mp-legend-item");
    item.append(&codec::codec_dot(stats.format));
    item.append(&name);
    item.append(&share);
    item.set_tooltip_text(Some(&song_count(stats.tracks)));
    item.upcast()
}

/// A card (`mp-format-card`) describing one format. It sits in a `gtk::FlowBox`,
/// which makes it focusable and activatable.
pub fn format_card(stats: &FormatStats, composition: &Composition) -> gtk::FlowBoxChild {
    let format = stats.format;
    let description = gtk::Label::builder()
        .label(codec::description(format))
        .xalign(0.0)
        .max_width_chars(1)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["mp-format-description", "caption", "dim-label"])
        .build();
    let meter = meter(codec::css_class(format));
    meter.set_value(composition.fraction(stats.tracks));

    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .css_classes(["mp-format-card", codec::css_class(format)])
        .build();
    card.append(&card_title(format));
    card.append(&description);
    card.append(&card_figures(stats));
    card.append(&card_footer(stats, composition));
    card.append(&meter);

    let child = gtk::FlowBoxChild::builder()
        .child(&card)
        .tooltip_text(format!("Show {} songs", format.label()))
        .build();
    child.update_property(&[gtk::accessible::Property::Label(&format!(
        "{}, {}, {}, {}, {} of the library",
        format.label(),
        codec::compression_label(format).to_lowercase(),
        song_count(stats.tracks),
        stats.typical_quality(),
        composition.percent(stats.tracks),
    ))]);
    child
}

/// The format's colour dot and name, and whether it is lossless or compressed.
fn card_title(format: AudioFormat) -> gtk::Box {
    let name = gtk::Label::builder()
        .label(format.label())
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["mp-format-name", "title-3"])
        .build();
    let badge = gtk::Label::builder()
        .label(codec::compression_label(format))
        .valign(gtk::Align::Center)
        .css_classes(["mp-format-badge"])
        .build();
    if format.is_lossless() {
        badge.add_css_class("mp-lossless");
    }
    let title = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    title.append(&codec::codec_dot(format));
    title.append(&name);
    title.append(&badge);
    title
}

/// How many songs, how long and how big.
fn card_figures(stats: &FormatStats) -> gtk::Box {
    let songs = Figure::new(songs_caption(stats.tracks));
    songs.set_value(&format_count(stats.tracks));
    let duration = Duration::from_millis(stats.duration_ms);
    let time = Figure::new("Time");
    time.set_value(&compact_duration(duration));
    time.set_detail(&format_total_duration(duration));
    let size = Figure::new("Size");
    size.set_value(&glib::format_size(stats.bytes));

    let figures = gtk::Box::builder()
        .spacing(12)
        .homogeneous(true)
        .css_classes(["mp-format-figures"])
        .build();
    for figure in [&songs, &time, &size] {
        figures.append(&figure.root);
    }
    figures
}

/// The typical quality, and the format's share of the library.
fn card_footer(stats: &FormatStats, composition: &Composition) -> gtk::Box {
    let quality = gtk::Label::builder()
        .label(stats.typical_quality())
        .xalign(0.0)
        .hexpand(true)
        .max_width_chars(1)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["mp-format-quality", "caption"])
        .tooltip_text(if stats.format.is_lossless() {
            "Typical resolution"
        } else {
            "Bitrate range"
        })
        .build();
    let share = gtk::Label::builder()
        .label(composition.percent(stats.tracks))
        .css_classes(["caption", "numeric", "dim-label"])
        .build();
    let footer = gtk::Box::builder()
        .spacing(6)
        .css_classes(["mp-format-footer"])
        .build();
    footer.append(&quality);
    footer.append(&share);
    footer
}

/// A duration short enough for a card's figure: "42 min", "3.5 hr", "1,204 hr".
fn compact_duration(duration: Duration) -> String {
    let hours = duration.as_secs_f64() / 3600.0;
    if hours < 1.0 {
        return format_total_duration(duration);
    }
    if hours >= 10.0 {
        return format!("{} hr", format_count(hours.round() as usize));
    }
    let tenths = (hours * 10.0).round() as u64;
    match tenths % 10 {
        0 => format!("{} hr", tenths / 10),
        tenth => format!("{}.{tenth} hr", tenths / 10),
    }
}

fn songs_caption(count: usize) -> &'static str {
    if count == 1 { "Song" } else { "Songs" }
}

/// A thin bar showing a share (`mp-meter`), coloured by `css_class`.
fn meter(css_class: &str) -> gtk::LevelBar {
    let meter = gtk::LevelBar::builder()
        .min_value(0.0)
        .max_value(1.0)
        .css_classes(["mp-meter", css_class])
        .build();
    // The default offsets recolour the bar as it fills; a share has no good or bad level.
    for offset in [
        gtk::LEVEL_BAR_OFFSET_LOW,
        gtk::LEVEL_BAR_OFFSET_HIGH,
        gtk::LEVEL_BAR_OFFSET_FULL,
    ] {
        meter.remove_offset_value(Some(offset.as_str()));
    }
    meter
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_durations() {
        let hours = |h: f64| compact_duration(Duration::from_secs_f64(h * 3600.0));
        assert_eq!(hours(0.7), "42 min");
        assert_eq!(hours(1.0), "1 hr");
        assert_eq!(hours(3.46), "3.5 hr");
        assert_eq!(hours(9.97), "10 hr");
        assert_eq!(hours(167.22), "167 hr");
        assert_eq!(hours(1204.4), "1,204 hr");
    }
}
