//! Pieces shared by album and artist cards.

use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use super::actions::BrowseItem;
use super::scaled_square::ScaledSquare;
use super::{AlbumCard, AlbumCardDetail, ArtistCard};
use crate::context::AppContext;
use crate::model::{AlbumObject, ArtistObject};
use crate::ui::icons;

/// What a [`super::CardGrid`] or [`super::CardShelf`] shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardKind {
    /// [`AlbumObject`]s as [`AlbumCard`]s showing the given detail.
    Albums(AlbumCardDetail),
    /// [`ArtistObject`]s as [`ArtistCard`]s.
    Artists,
}

impl CardKind {
    /// Shown on the empty page unless a spinner replaces it.
    pub(super) fn empty_icon(self) -> &'static str {
        match self {
            CardKind::Albums(_) => icons::ALBUM,
            CardKind::Artists => icons::ARTIST,
        }
    }
}

/// Card art is loaded at this size (in logical pixels) and scaled to the width of the
/// grid's columns, which stay close to it at every width.
pub(super) const CARD_ART_SIZE: i32 = 200;
/// The smallest card art; it decides how many columns fit.
pub(super) const CARD_ART_MIN_SIZE: i32 = 148;

/// Scales `image` (of [`CARD_ART_SIZE`]) to the card's width and stacks the play
/// button over its bottom corner. The button appears while the card is hovered (see
/// browse.css) and stays out of the focus chain: keyboard users play from the card's
/// menu or page.
pub(super) fn card_art(
    card: &impl IsA<gtk::Widget>,
    image: &impl IsA<gtk::Widget>,
    play: &gtk::Button,
) -> gtk::Overlay {
    play.set_icon_name(icons::PLAY);
    play.add_css_class("circular");
    play.add_css_class("mp-card-play");
    play.set_halign(gtk::Align::End);
    play.set_valign(gtk::Align::End);
    play.set_focusable(false);
    play.set_focus_on_click(false);
    // Only clickable while shown, so a tap on a touch screen opens the card instead.
    play.set_can_target(false);
    let button = play.downgrade();
    card.connect_state_flags_changed(move |card, _| {
        if let Some(button) = button.upgrade() {
            button.set_can_target(card.state_flags().contains(gtk::StateFlags::PRELIGHT));
        }
    });

    let overlay = gtk::Overlay::builder()
        .child(&ScaledSquare::new(image, CARD_ART_MIN_SIZE))
        .css_classes(["mp-card-art"])
        .build();
    overlay.add_overlay(play);
    overlay
}

/// Makes `label` a single ellipsized line that takes the card's width rather than
/// widening the card to fit its text.
pub(super) fn card_label(label: &gtk::Label, xalign: f32) -> &gtk::Label {
    label.set_xalign(xalign);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_single_line_mode(true);
    label.set_max_width_chars(1);
    label.set_hexpand(true);
    label
}

/// A card for `kind`, to be [bound](bind_card) to an album or artist.
pub(super) fn new_card(ctx: &Rc<AppContext>, kind: CardKind) -> gtk::Widget {
    match kind {
        CardKind::Albums(detail) => AlbumCard::new(ctx, detail).upcast(),
        CardKind::Artists => ArtistCard::new(ctx).upcast(),
    }
}

/// Shows `object`, an album or an artist, on `card`. Returns the card's accessible
/// label, or `None` if they don't go together.
pub(super) fn bind_card(card: &gtk::Widget, object: &glib::Object) -> Option<String> {
    if let (Some(card), Some(album)) = (
        card.downcast_ref::<AlbumCard>(),
        object.downcast_ref::<AlbumObject>(),
    ) {
        card.bind(album);
        return Some(format!("{} by {}", album.title(), album.artist()));
    }
    let card = card.downcast_ref::<ArtistCard>()?;
    let artist = object.downcast_ref::<ArtistObject>()?;
    card.bind(artist);
    Some(artist.name())
}

/// The album or artist a card shows.
pub(super) fn card_item(card: &gtk::Widget) -> Option<BrowseItem> {
    if let Some(card) = card.downcast_ref::<AlbumCard>() {
        return card.album().map(BrowseItem::Album);
    }
    card.downcast_ref::<ArtistCard>()
        .and_then(ArtistCard::artist)
        .map(BrowseItem::Artist)
}
