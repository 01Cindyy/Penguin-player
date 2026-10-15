//! Widgets for browsing albums and artists: cards, the grids and shelves that hold
//! them, the actions that play or queue a whole album or artist, and the scrolling
//! layout of the album and artist pages.
//!
//! Every album or artist the app shows is one of these cards, in a [`CardGrid`] or,
//! for a single row such as on the search page, a [`CardShelf`]:
//!
//! ```ignore
//! let grid = CardGrid::builder(ctx, CardKind::Albums(AlbumCardDetail::Artist))
//!     .scrolls(false)
//!     .build(&albums);
//! let shelf = CardShelf::new(ctx, CardKind::Artists);
//! shelf.set_items(&artists);
//! ```

mod actions;
mod album_card;
mod album_summary;
mod artist_card;
mod artist_image;
mod card;
mod card_grid;
mod card_menu;
mod card_shelf;
mod collapsible_items;
mod detail_page;
mod discography;
mod gradual_store;
mod library_status;
mod scaled_square;

pub use actions::{BrowseItem, ItemKind, install_actions, play, set_page_menu, shuffle};
pub use album_card::{AlbumCard, AlbumCardDetail};
pub use album_summary::{QualityBadge, album_facts, disc_count, has_guest_artists, quality_badges};
pub use artist_card::{ArtistCard, artist_stats};
pub use artist_image::ArtistImage;
pub use card::CardKind;
pub use card_grid::CardGrid;
pub use card_shelf::CardShelf;
pub use collapsible_items::CollapsibleItems;
pub use detail_page::DetailPage;
pub use discography::Discography;
pub use gradual_store::GradualStore;
pub use library_status::{LibraryStatus, follow_library_status};
