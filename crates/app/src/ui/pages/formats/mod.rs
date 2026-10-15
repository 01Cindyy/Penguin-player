//! The library by format: an overview of what it is made of, then a card per format
//! that opens the format's page.

mod cards;
pub(super) mod stats;

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use mp_core::AudioFormat;

use crate::context::AppContext;
use crate::model::TrackObject;
use crate::ui::icons;
use crate::ui::navigation::Route;
use crate::ui::pages::NARROW_PAGE_WIDTH;
use crate::ui::widgets::browse::{LibraryStatus, follow_library_status};
use crate::ui::widgets::{PageHeader, SectionHeader};
use crate::ui::{
    disconnect_on_destroy, keep_alive, mark_narrow_while, max_width_breakpoint, show_empty_state,
};
use crate::util::{count_label, song_count, total_length};

use cards::Overview;
use stats::Composition;

pub fn build(ctx: &Rc<AppContext>) -> adw::NavigationPage {
    let page = Rc::new(FormatsPage::new(ctx));
    page.connect_signals();
    page.refresh();
    page.update_empty_state();

    let navigation_page = super::page_with_header("Formats", &page.header, &page.body);
    navigation_page.add_css_class("mp-formats-page");

    let weak = Rc::downgrade(&page);
    let handler = ctx.store.connect_tracks_changed(move |_| {
        if let Some(page) = weak.upgrade() {
            page.refresh();
        }
    });
    disconnect_on_destroy(&navigation_page, &ctx.store, handler);
    let weak = Rc::downgrade(&page);
    follow_library_status(ctx, &navigation_page, move || {
        if let Some(page) = weak.upgrade() {
            page.update_empty_state();
        }
    });
    keep_alive(&navigation_page, page);
    navigation_page
}

/// The composition of every track in the store; one pass over the library.
fn library_composition(ctx: &AppContext) -> Composition {
    let tracks: Vec<TrackObject> = ctx.store.tracks().iter().filter_map(Result::ok).collect();
    Composition::of(tracks.iter().map(TrackObject::track))
}

/// The page's state, owned by its content widget (see [`keep_alive`]).
struct FormatsPage {
    ctx: Rc<AppContext>,
    header: PageHeader,
    body: adw::BreakpointBin,
    stack: gtk::Stack,
    content: gtk::Box,
    empty: adw::StatusPage,
    overview: Overview,
    grid: gtk::FlowBox,
    /// The format of each card, in grid order.
    card_formats: RefCell<Vec<AudioFormat>>,
}

impl FormatsPage {
    fn new(ctx: &Rc<AppContext>) -> Self {
        let header = PageHeader::new("Formats");
        let overview = Overview::new();
        let grid = gtk::FlowBox::builder()
            .homogeneous(true)
            .min_children_per_line(1)
            .max_children_per_line(4)
            .column_spacing(12)
            .row_spacing(12)
            .selection_mode(gtk::SelectionMode::None)
            .valign(gtk::Align::Start)
            .css_classes(["mp-format-grid"])
            .build();
        grid.update_property(&[gtk::accessible::Property::Label("Formats")]);
        let formats_section = gtk::Box::new(gtk::Orientation::Vertical, 12);
        formats_section.append(&SectionHeader::new("All Formats", ("format", "formats")).root);
        formats_section.append(&grid);

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(30)
            .css_classes(["mp-formats-content"])
            .build();
        content.append(&overview.root);
        content.append(&formats_section);
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&content)
            .build();

        let empty = adw::StatusPage::builder()
            .icon_name(icons::FORMATS)
            .css_classes(["compact"])
            .build();
        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .vexpand(true)
            .build();
        stack.add_named(&scrolled, Some("content"));
        stack.add_named(&empty, Some("empty"));

        // A breakpoint bin needs a minimum size of its own; the page has room for it.
        let body = adw::BreakpointBin::builder()
            .width_request(240)
            .height_request(160)
            .vexpand(true)
            .child(&stack)
            .build();

        let page = Self {
            ctx: ctx.clone(),
            header,
            body,
            stack,
            content,
            empty,
            overview,
            grid,
            card_formats: RefCell::default(),
        };
        page.add_breakpoint();
        page
    }

    fn connect_signals(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.grid.connect_child_activated(move |_, child| {
            let Some(page) = weak.upgrade() else { return };
            let format = page
                .card_formats
                .borrow()
                .get(child.index() as usize)
                .copied();
            if let Some(format) = format {
                page.ctx.navigator.navigate(Route::Format(format));
            }
        });
    }

    /// Narrow pages stack the quality tiers and use the narrow gutter.
    fn add_breakpoint(&self) {
        let narrow = max_width_breakpoint(NARROW_PAGE_WIDTH);
        narrow.add_setter(
            &self.overview.tiers_box,
            "orientation",
            Some(&gtk::Orientation::Vertical.to_value()),
        );
        mark_narrow_while(&narrow, &self.content);
        self.body.add_breakpoint(narrow);
    }

    fn refresh(&self) {
        let composition = library_composition(&self.ctx);
        self.update_summary(&composition);
        if composition.tracks == 0 {
            self.stack.set_visible_child_name("empty");
            return;
        }
        self.stack.set_visible_child_name("content");
        self.overview.update(&composition);

        self.grid.remove_all();
        let formats = composition.by_popularity();
        for stats in &formats {
            self.grid.append(&cards::format_card(stats, &composition));
        }
        self.card_formats
            .replace(formats.iter().map(|stats| stats.format).collect());
    }

    fn update_empty_state(&self) {
        let status = LibraryStatus::of(&self.ctx.library_service);
        show_empty_state(
            &self.empty,
            status.title("No Formats"),
            &status.description("The formats of the songs in your music folder will appear here."),
            icons::FORMATS,
            status.busy(),
        );
    }

    fn update_summary(&self, composition: &Composition) {
        let summary = if composition.tracks == 0 {
            String::new()
        } else {
            [
                count_label(composition.formats.len(), "format", "formats"),
                song_count(composition.tracks),
                total_length(composition.duration_ms),
                glib::format_size(composition.bytes).to_string(),
            ]
            .join(" · ")
        };
        self.header.set_subtitle(&summary);
    }
}
