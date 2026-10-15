//! Every album artist as a grid of round images, sorted by name.

use std::rc::Rc;

use adw::prelude::*;

use crate::context::AppContext;
use crate::ui::widgets::PageHeader;
use crate::ui::widgets::browse::{CardGrid, CardKind, LibraryStatus, follow_library_status};
use crate::ui::{disconnect_on_destroy, keep_alive};
use crate::util::count_label;

pub fn build(ctx: &Rc<AppContext>) -> adw::NavigationPage {
    let page = Rc::new(ArtistsPage::new(ctx));
    page.update_summary();
    page.update_empty_state();

    let navigation_page = super::page_with_header("Artists", &page.header, &page.grid);
    navigation_page.add_css_class("mp-artists-page");

    let artists = ctx.store.artists();
    let weak = Rc::downgrade(&page);
    let handler = artists.connect_items_changed(move |_, _, _, _| {
        if let Some(page) = weak.upgrade() {
            page.update_summary();
        }
    });
    disconnect_on_destroy(&navigation_page, &artists, handler);
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
struct ArtistsPage {
    ctx: Rc<AppContext>,
    header: PageHeader,
    grid: CardGrid,
}

impl ArtistsPage {
    fn new(ctx: &Rc<AppContext>) -> Self {
        let header = PageHeader::new("Artists");
        // The store keeps artists sorted by name.
        let grid = CardGrid::builder(ctx, CardKind::Artists).build(&ctx.store.artists());
        grid.grid_view()
            .update_property(&[gtk::accessible::Property::Label("Artists")]);
        grid.set_vexpand(true);
        Self {
            ctx: ctx.clone(),
            header,
            grid,
        }
    }

    fn update_summary(&self) {
        let count = self.ctx.store.artists().n_items() as usize;
        let summary = match count {
            0 => String::new(),
            _ => count_label(count, "artist", "artists"),
        };
        self.header.set_subtitle(&summary);
    }

    fn update_empty_state(&self) {
        let status = LibraryStatus::of(&self.ctx.library_service);
        self.grid.set_empty_state(
            status.title("No Artists"),
            &status.description("Artists in your music folder will appear here."),
            status.busy(),
        );
    }
}
