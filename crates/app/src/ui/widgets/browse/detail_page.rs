//! The layout of the album and artist pages: a hero header that scrolls away with
//! the sections under it, and a message for when what the page shows has left the
//! library.

use adw::prelude::*;

use crate::ui::pages::NARROW_PAGE_WIDTH;
use crate::ui::widgets::{PageHeader, section_heading};
use crate::ui::{mark_narrow_while, max_width_breakpoint};

/// A page whose [`PageHeader`] scrolls with its content (`mp-detail-page`).
///
/// Unlike [`crate::ui::pages::page_with_header`], everything scrolls together, so
/// lists inside must not scroll themselves (see `TrackListBuilder::scrolls` and
/// [`super::CardGridBuilder::scrolls`]). Keep such lists short or collapsible: every
/// row of a non-scrolling list is created, so fill them with a
/// [`super::GradualStore`].
///
/// It holds no reference to its navigation page, so a page's state can keep it (see
/// [`crate::ui::keep_alive`]).
pub struct DetailPage {
    stack: gtk::Stack,
    sections: gtk::Box,
    missing: adw::StatusPage,
    narrow: adw::Breakpoint,
}

impl DetailPage {
    /// Lays out a page with `header` on top. Returns the layout, to add sections to,
    /// and the navigation page that shows it.
    pub fn new(title: &str, header: &PageHeader) -> (Self, adw::NavigationPage) {
        let sections = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["mp-detail-sections"])
            .build();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(header);
        content.append(&sections);

        // A natural scroll policy gives non-scrolling lists their full height.
        let viewport = gtk::Viewport::builder()
            .vscroll_policy(gtk::ScrollablePolicy::Natural)
            .child(&content)
            .build();
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&viewport)
            .build();

        let missing = adw::StatusPage::builder().css_classes(["compact"]).build();
        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();
        stack.add_named(&scrolled, Some("content"));
        stack.add_named(&missing, Some("missing"));

        let bin = adw::BreakpointBin::builder()
            .width_request(300)
            .height_request(200)
            .child(&stack)
            .build();
        let narrow = max_width_breakpoint(NARROW_PAGE_WIDTH);
        narrow.add_setter(header, "narrow", Some(&true.to_value()));
        mark_narrow_while(&narrow, &sections);
        bin.add_breakpoint(narrow.clone());

        let page = adw::NavigationPage::builder()
            .title(title)
            .child(&bin)
            .css_classes(["mp-detail-page"])
            .build();
        let layout = Self {
            stack,
            sections,
            missing,
            narrow,
        };
        (layout, page)
    }

    /// The breakpoint that applies while the page is narrow, e.g. to set the `narrow`
    /// property of grids in its sections. The sections gain `mp-narrow` meanwhile.
    pub fn narrow_breakpoint(&self) -> &adw::Breakpoint {
        &self.narrow
    }

    /// Renames the navigation page, which titles the back button's tooltip.
    pub fn set_title(&self, title: &str) {
        if let Some(page) = self
            .stack
            .ancestor(adw::NavigationPage::static_type())
            .and_downcast::<adw::NavigationPage>()
        {
            page.set_title(title);
        }
    }

    /// Adds `content` under the header with no heading, e.g. an album's tracks.
    pub fn append(&self, content: &impl IsA<gtk::Widget>) {
        self.sections.append(content);
    }

    /// Adds a section headed by `title` (`mp-detail-section`) and returns it, so it
    /// can be hidden while it has nothing to show. `action` sits at the end of the
    /// heading, e.g. a "Show All" button.
    pub fn add_section(
        &self,
        title: &str,
        content: &impl IsA<gtk::Widget>,
        action: Option<&impl IsA<gtk::Widget>>,
    ) -> gtk::Box {
        let section = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["mp-detail-section"])
            .build();
        section.append(&section_heading(title, action));
        section.append(content);
        self.sections.append(&section);
        section
    }

    /// Shows the sections (the default).
    pub fn show_content(&self) {
        self.stack.set_visible_child_name("content");
    }

    /// Replaces the page with a message, e.g. once a rescan removed the album. Both
    /// texts are plain text.
    pub fn show_missing(&self, icon_name: &str, title: &str, description: &str) {
        self.missing.set_icon_name(Some(icon_name));
        self.missing.set_title(title);
        self.missing
            .set_description(Some(&gtk::glib::markup_escape_text(description)));
        self.stack.set_visible_child_name("missing");
    }
}
