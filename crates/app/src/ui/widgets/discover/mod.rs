//! Widgets of the discovery pages: search results, the format overview and the
//! preferences dialog's theme gallery.

pub mod codec;
pub mod composition_bar;
pub mod figure;
pub mod song_list;
pub mod theme_preview;
pub mod top_result;

pub use composition_bar::{CompositionBar, Segment};
pub use figure::Figure;
pub use song_list::SongList;
pub use theme_preview::{PreviewColors, ThemePreview};
pub use top_result::{TopResult, TopResultCard};
