//! One module per page. Each exposes
//! `pub fn build(ctx: &Rc<AppContext>, …route parameters) -> adw::NavigationPage`
//! and is reached through [`crate::ui::navigation::Route`].
//!
//! Pages contain only their content: the content pane's shared header bar provides
//! the back button and the search entry. Large lists must be the direct child of a
//! `gtk::ScrolledWindow` so only visible rows are created.

pub mod album;
pub mod albums;
pub mod artist;
pub mod artists;
pub mod format;
pub mod formats;
pub mod now_playing;
pub mod playlist;
pub mod search;
pub mod songs;

use adw::prelude::*;

use crate::ui::max_width_breakpoint;
use crate::ui::widgets::PageHeader;

/// Pages narrower than this (in sp) switch their header to its narrow layout.
pub const NARROW_PAGE_WIDTH: f64 = 560.0;

/// Builds a page from a [`PageHeader`] above `body`. The header follows the page's
/// width (see [`PageHeader::set_narrow`]); `body` should expand vertically.
pub fn page_with_header(
    title: &str,
    header: &PageHeader,
    body: &impl IsA<gtk::Widget>,
) -> adw::NavigationPage {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(header);
    content.append(body);

    // The breakpoint bin fills the page, so its own minimum size only has to be
    // smaller than the narrowest window.
    let bin = adw::BreakpointBin::builder()
        .width_request(300)
        .height_request(200)
        .child(&content)
        .build();
    let narrow = max_width_breakpoint(NARROW_PAGE_WIDTH);
    narrow.add_setter(header, "narrow", Some(&true.to_value()));
    bin.add_breakpoint(narrow);

    adw::NavigationPage::builder()
        .title(title)
        .child(&bin)
        .build()
}
