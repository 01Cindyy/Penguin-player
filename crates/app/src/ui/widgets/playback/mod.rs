//! Playback controls shared by the player bar and the Now Playing page. Each keeps
//! itself in step with the player for as long as it exists.

mod seek_bar;
mod transport;
mod volume;

pub use seek_bar::SeekBar;
pub use transport::TransportControls;
pub use volume::VolumeControls;

use crate::ui::follow;
