//! Reusable widgets shared by pages and panels.

pub mod balanced_row;
pub mod browse;
pub mod collections;
pub mod cover;
pub mod cover_backdrop;
pub mod discover;
pub mod equaliser;
pub mod format_badge;
pub mod lyrics_view;
pub mod match_width;
pub mod page_header;
pub mod playback;
pub mod resize_handle;
pub mod section_header;
pub mod song_heading;
pub mod track_list;
pub mod visualizer;
pub mod waveform;

pub use balanced_row::BalancedRow;
pub use cover::{CoverImage, CoverSize};
pub use cover_backdrop::CoverBackdrop;
pub use equaliser::equaliser;
pub use format_badge::{FormatBadge, QualityTier};
pub use lyrics_view::LyricsView;
pub use match_width::MatchWidth;
pub use page_header::PageHeader;
pub use playback::{SeekBar, TransportControls, VolumeControls};
pub use resize_handle::{Grows, Resizable, resize_handle};
pub use section_header::{SectionHeader, section_heading};
pub use song_heading::SongHeading;
pub use track_list::{TrackColumn, TrackList};
pub use visualizer::Visualizer;
pub use waveform::Waveform;
