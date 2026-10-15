//! A format's identity: what its name stands for and a colour, so a format looks the
//! same in the composition bar, on its card and on its page.
//!
//! Each format has a CSS class (`mp-codec-flac`, `mp-codec-mp3`, …) that sets
//! `--mp-codec-color`; themes can override it per format.

use adw::prelude::*;
use mp_core::AudioFormat;

/// The CSS class that sets `--mp-codec-color` for `format`.
pub fn css_class(format: AudioFormat) -> &'static str {
    match format {
        AudioFormat::Flac => "mp-codec-flac",
        AudioFormat::Alac => "mp-codec-alac",
        AudioFormat::Wav => "mp-codec-wav",
        AudioFormat::Aiff => "mp-codec-aiff",
        AudioFormat::Mp3 => "mp-codec-mp3",
        AudioFormat::Aac => "mp-codec-aac",
        AudioFormat::Opus => "mp-codec-opus",
        AudioFormat::Vorbis => "mp-codec-vorbis",
        AudioFormat::Unknown => "mp-codec-unknown",
    }
}

/// What the format's name stands for, e.g. "Free Lossless Audio Codec".
pub fn description(format: AudioFormat) -> &'static str {
    match format {
        AudioFormat::Flac => "Free Lossless Audio Codec",
        AudioFormat::Alac => "Apple Lossless Audio Codec",
        AudioFormat::Wav => "Waveform Audio File Format",
        AudioFormat::Aiff => "Audio Interchange File Format",
        AudioFormat::Mp3 => "MPEG-1 Audio Layer III",
        AudioFormat::Aac => "Advanced Audio Coding",
        AudioFormat::Opus => "Opus Interactive Audio Codec",
        AudioFormat::Vorbis => "Ogg Vorbis",
        AudioFormat::Unknown => "Unrecognised Format",
    }
}

/// "Lossless" or "Lossy" ("Unknown" when the format itself is unknown).
pub fn compression_label(format: AudioFormat) -> &'static str {
    match format {
        AudioFormat::Unknown => "Unknown",
        format if format.is_lossless() => "Lossless",
        _ => "Lossy",
    }
}

/// A rounded square in the format's colour with its name on it, used like album art
/// for a format (`mp-codec-tile`). `size` is in pixels.
pub fn codec_tile(format: AudioFormat, size: i32) -> gtk::Widget {
    let label = gtk::Label::builder()
        .label(format.label())
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .css_classes(["mp-codec-tile-label"])
        .build();
    let tile: gtk::Widget = adw::Bin::builder()
        .child(&label)
        .width_request(size)
        .height_request(size)
        .halign(gtk::Align::Start)
        .valign(gtk::Align::Start)
        .overflow(gtk::Overflow::Hidden)
        .css_classes(["mp-codec-tile", css_class(format)])
        .build()
        .upcast();
    tile.update_property(&[gtk::accessible::Property::Label(format.label())]);
    tile
}

/// A small dot in the format's colour (`mp-codec-dot`), for legends.
pub fn codec_dot(format: AudioFormat) -> gtk::Widget {
    adw::Bin::builder()
        .valign(gtk::Align::Center)
        .accessible_role(gtk::AccessibleRole::Presentation)
        .css_classes(["mp-codec-dot", css_class(format)])
        .build()
        .upcast()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_format_has_an_identity() {
        let mut classes: Vec<&str> = AudioFormat::ALL.into_iter().map(css_class).collect();
        classes.sort_unstable();
        classes.dedup();
        assert_eq!(classes.len(), AudioFormat::ALL.len());
        assert!(
            AudioFormat::ALL
                .into_iter()
                .all(|f| !description(f).is_empty())
        );
        assert_eq!(css_class(AudioFormat::Flac), "mp-codec-flac");
        assert_eq!(compression_label(AudioFormat::Alac), "Lossless");
        assert_eq!(compression_label(AudioFormat::Opus), "Lossy");
        assert_eq!(compression_label(AudioFormat::Unknown), "Unknown");
    }
}
