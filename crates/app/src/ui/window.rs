//! The main window.
//!
//! ```text
//! ToolbarView ─ content: ToastOverlay ─ Stack ─ "onboarding": welcome screen
//!             │                              └ "library": OverlaySplitView (queue panel at the end)
//!             │                                    └ NavigationSplitView: sidebar | content pane
//!             └ bottom bar: PlayerBar                content pane: header (back, search),
//!                                                  folder banner, pages
//! ```
//!
//! The sidebar, the queue panel and the player bar are resized by dragging their inner
//! edges (see [`resize_handle`]).
//!
//! While the Now Playing page is shown, the window gives it as much room as it can:
//! no player bar (the page has its own controls), no search field, the page reaching
//! up behind the header, and the sidebar showing only icons until another page is
//! chosen.

use std::cell::{Cell, OnceCell};
use std::rc::Rc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib};
use mp_core::APP_NAME;

use crate::context::AppContext;
use crate::ui::icons;
use crate::ui::navigation::Route;
use crate::ui::player_bar::PlayerBar;
use crate::ui::sidebar::Sidebar;
use crate::ui::widgets::{Grows, Resizable, resize_handle};
use crate::ui::{
    folder_banner, max_width_breakpoint, onboarding, queue_panel, set_tooltip, shortcuts,
};

/// Below this width (in sp) the queue panel overlays the content instead of sitting
/// beside it.
const QUEUE_OVERLAY_WIDTH: f64 = 1040.0;
/// Below this width (in sp) the sidebar collapses and the player bar turns compact.
const NARROW_WIDTH: f64 = 720.0;
/// Widths, in pixels, the sidebar and the queue panel can be dragged to. Both at their
/// widest still leave the pages the 300 pixels they need at `QUEUE_OVERLAY_WIDTH`.
const SIDEBAR_MIN_WIDTH: f64 = 180.0;
const SIDEBAR_MAX_WIDTH: f64 = 320.0;
const QUEUE_MIN_WIDTH: f64 = 260.0;
const QUEUE_MAX_WIDTH: f64 = 400.0;
/// Dragging the sidebar narrower than this shows only its icons.
const SIDEBAR_ICONS_ONLY_BELOW: f64 = 130.0;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct MainWindow {
        pub(super) ctx: OnceCell<Rc<AppContext>>,
        pub(super) toolbar: adw::ToolbarView,
        pub(super) stack: gtk::Stack,
        pub(super) queue_split: adw::OverlaySplitView,
        pub(super) nav_split: adw::NavigationSplitView,
        pub(super) search_entry: gtk::SearchEntry,
        pub(super) back_button: gtk::Button,
        pub(super) player_bar: OnceCell<PlayerBar>,
        pub(super) sidebar: OnceCell<Sidebar>,
        pub(super) sidebar_handle: OnceCell<gtk::Widget>,
        /// Strips along the page's edges that the "Defined Edges" option shades, as if
        /// the sidebar and the queue cast a shadow on the page.
        pub(super) sidebar_shade: OnceCell<gtk::Widget>,
        pub(super) queue_shade: OnceCell<gtk::Widget>,
        /// The width the sidebar was dragged to, kept while it shows only icons.
        pub(super) sidebar_width: Cell<f64>,
        pub(super) sidebar_icons_only: Cell<bool>,
        /// The Now Playing page is shown.
        pub(super) now_playing: Cell<bool>,
        /// The content pane's header bar and the rest of it, and the search field in
        /// the header, which Now Playing changes.
        pub(super) content_view: OnceCell<adw::ToolbarView>,
        pub(super) search: OnceCell<adw::Clamp>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MainWindow {
        const NAME: &'static str = "MpMainWindow";
        type Type = super::MainWindow;
        type ParentType = adw::ApplicationWindow;
    }

    impl ObjectImpl for MainWindow {}
    impl WidgetImpl for MainWindow {}

    impl WindowImpl for MainWindow {
        fn close_request(&self) -> glib::Propagation {
            self.obj().save_window_state();
            self.parent_close_request()
        }
    }

    impl ApplicationWindowImpl for MainWindow {}
    impl AdwApplicationWindowImpl for MainWindow {}
}

glib::wrapper! {
    /// The app's only window. See the [module documentation](self) for its layout.
    pub struct MainWindow(ObjectSubclass<imp::MainWindow>)
        @extends adw::ApplicationWindow, gtk::ApplicationWindow, gtk::Window, gtk::Widget,
        @implements gtk::gio::ActionGroup, gtk::gio::ActionMap, gtk::Accessible, gtk::Buildable,
            gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl MainWindow {
    pub fn new(ctx: &Rc<AppContext>) -> Self {
        let window: Self = glib::Object::builder()
            .property("application", &ctx.app)
            .property("title", APP_NAME)
            .build();
        window.add_css_class("mp-main-window");
        window.set_size_request(360, 480);
        let imp = window.imp();
        assert!(
            imp.ctx.set(ctx.clone()).is_ok(),
            "a new window has no context"
        );
        ctx.set_window(&window);

        window.restore_window_state();
        window.build_layout();
        window.add_breakpoints();
        shortcuts::install(window.upcast_ref());

        if ctx.settings().music_folder.is_some() {
            window.show_library();
        } else {
            window.show_onboarding();
        }
        window
    }

    fn ctx(&self) -> &Rc<AppContext> {
        self.imp().ctx.get().expect("set in MainWindow::new")
    }

    pub fn player_bar(&self) -> &PlayerBar {
        self.imp().player_bar.get().expect("set in MainWindow::new")
    }

    /// Puts the cursor in the search field (Ctrl+F).
    pub fn focus_search(&self) {
        let imp = self.imp();
        if imp.stack.visible_child_name().as_deref() != Some("library") {
            return;
        }
        imp.nav_split.set_show_content(true);
        imp.search_entry.grab_focus();
    }

    /// Types `query` into the search field, which opens the search page.
    pub fn search_for(&self, query: &str) {
        self.imp().search_entry.set_text(query);
    }

    /// Shows or hides the queue panel.
    pub fn set_queue_visible(&self, visible: bool) {
        self.imp().queue_split.set_show_sidebar(visible);
    }

    pub fn is_queue_visible(&self) -> bool {
        self.imp().queue_split.shows_sidebar()
    }

    /// Keeps `toggle` and the queue panel in step: each shows or hides the other.
    pub fn bind_queue_toggle(&self, toggle: &gtk::ToggleButton) {
        self.imp()
            .queue_split
            .bind_property("show-sidebar", toggle, "active")
            .bidirectional()
            .sync_create()
            .build();
    }

    fn show_onboarding(&self) {
        let imp = self.imp();
        imp.stack.set_visible_child_name("onboarding");
        imp.toolbar.set_reveal_bottom_bars(false);
    }

    fn show_library(&self) {
        let imp = self.imp();
        imp.stack.set_visible_child_name("library");
        imp.toolbar.set_reveal_bottom_bars(!imp.now_playing.get());
        if self.ctx().navigator.top_level_route().is_none() {
            self.ctx().navigator.navigate(Route::Songs);
        }
    }

    fn build_layout(&self) {
        let imp = self.imp();
        let ctx = self.ctx();

        imp.nav_split.add_css_class("mp-library-split");
        imp.nav_split.set_sidebar(Some(&self.sidebar_pane()));
        imp.nav_split.set_content(Some(&self.content_pane()));
        imp.nav_split.set_sidebar_width_unit(adw::LengthUnit::Px);

        let queue = gtk::Overlay::builder()
            .child(&queue_panel::build(ctx))
            .build();
        queue.add_overlay(&resize_handle(Grows::Left, self.edge(Pane::Queue)));
        imp.queue_split.add_css_class("mp-queue-split");
        imp.queue_split.set_sidebar_position(gtk::PackType::End);
        imp.queue_split.set_sidebar(Some(&queue));
        imp.queue_split.set_content(Some(&imp.nav_split));
        imp.queue_split.set_show_sidebar(false);
        imp.queue_split.set_sidebar_width_unit(adw::LengthUnit::Px);
        for property in ["show-sidebar", "collapsed"] {
            imp.queue_split.connect_notify_local(
                Some(property),
                glib::clone!(
                    #[weak(rename_to = window)]
                    self,
                    move |_, _| window.update_queue_shade()
                ),
            );
        }
        self.update_queue_shade();
        self.restore_pane_sizes();

        let weak = self.downgrade();
        let onboarding = onboarding::build(ctx, move || {
            if let Some(window) = weak.upgrade() {
                window.show_library();
            }
        });
        imp.stack
            .set_transition_type(gtk::StackTransitionType::Crossfade);
        imp.stack.add_named(&onboarding, Some("onboarding"));
        imp.stack.add_named(&imp.queue_split, Some("library"));

        let player_bar = PlayerBar::new(ctx);
        player_bar
            .bind_property("queue-visible", &imp.queue_split, "show-sidebar")
            .bidirectional()
            .sync_create()
            .build();

        ctx.toast_overlay().set_child(Some(&imp.stack));
        imp.toolbar.set_content(Some(ctx.toast_overlay()));
        imp.toolbar.add_bottom_bar(&player_bar);
        imp.toolbar.set_bottom_bar_style(adw::ToolbarStyle::Flat);
        assert!(imp.player_bar.set(player_bar).is_ok(), "built once");
        self.set_content(Some(&imp.toolbar));
    }

    fn sidebar_pane(&self) -> adw::NavigationPage {
        let sidebar = Sidebar::new(self.ctx());
        let title = adw::WindowTitle::new(APP_NAME, "");
        let menu = primary_menu_button();
        let header = adw::HeaderBar::builder()
            .title_widget(&title)
            .css_classes(["flat"])
            .build();
        header.pack_end(&menu);
        // With only icons there is room for just the menu, in the middle.
        sidebar.connect_icons_only_notify(glib::clone!(
            #[weak]
            header,
            move |sidebar| {
                if sidebar.icons_only() {
                    header.remove(&menu);
                    header.set_title_widget(Some(&menu));
                } else {
                    header.set_title_widget(Some(&title));
                    header.pack_end(&menu);
                }
            }
        ));
        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&sidebar));
        assert!(self.imp().sidebar.set(sidebar).is_ok(), "built once");
        adw::NavigationPage::builder()
            .title(APP_NAME)
            .tag("sidebar")
            .child(&view)
            .build()
    }

    /// The header (back button and search) above the navigator's pages.
    fn content_pane(&self) -> adw::NavigationPage {
        let imp = self.imp();
        let ctx = self.ctx();

        imp.back_button.set_icon_name(icons::BACK);
        imp.back_button.set_visible(false);
        imp.back_button.connect_clicked(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.go_back()
        ));

        imp.search_entry
            .set_placeholder_text(Some("Search songs, albums and artists"));
        imp.search_entry.set_hexpand(true);
        imp.search_entry.add_css_class("mp-search-entry");
        imp.search_entry.set_key_capture_widget(Some(self));
        imp.search_entry.connect_search_changed(glib::clone!(
            #[weak]
            ctx,
            move |entry| ctx.navigator.search(&entry.text())
        ));
        imp.search_entry
            .connect_stop_search(|entry| entry.set_text(""));
        imp.search_entry.connect_activate(glib::clone!(
            #[weak]
            ctx,
            move |entry| ctx.navigator.activate_search(&entry.text())
        ));
        let search = adw::Clamp::builder()
            .maximum_size(460)
            .tightening_threshold(300)
            .child(&imp.search_entry)
            .build();

        let header = adw::HeaderBar::builder()
            .title_widget(&search)
            .show_back_button(false)
            .css_classes(["flat", "mp-content-header"])
            .build();
        header.pack_start(&imp.back_button);

        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.add_top_bar(&folder_banner::build(ctx));
        view.set_content(Some(ctx.navigator.view()));
        assert!(imp.content_view.set(view.clone()).is_ok(), "built once");
        assert!(imp.search.set(search).is_ok(), "built once");

        // The sidebar's edge is dragged from this side, clear of the sidebar's scrollbar.
        let content = gtk::Overlay::builder().child(&view).build();
        let sidebar_shade = edge_shade(gtk::Align::Start);
        let queue_shade = edge_shade(gtk::Align::End);
        let handle = resize_handle(Grows::Right, self.edge(Pane::Sidebar));
        content.add_overlay(&sidebar_shade);
        content.add_overlay(&queue_shade);
        content.add_overlay(&handle);
        assert!(imp.sidebar_handle.set(handle).is_ok(), "built once");
        assert!(imp.sidebar_shade.set(sidebar_shade).is_ok(), "built once");
        assert!(imp.queue_shade.set(queue_shade).is_ok(), "built once");

        self.connect_navigation();
        adw::NavigationPage::builder()
            .title("Library")
            .tag("content")
            .child(&content)
            .build()
    }

    /// Keeps the back button, search field and narrow layout in step with navigation.
    fn connect_navigation(&self) {
        let ctx = self.ctx();
        let weak = self.downgrade();
        ctx.navigator.connect_navigated(move |route| {
            let Some(window) = weak.upgrade() else { return };
            let imp = window.imp();
            imp.nav_split.set_show_content(true);
            // Choosing another section ends the search. Pages opened from the results
            // (an album, an artist) keep it, so going back returns to the results.
            let leaves_search = route.is_top_level() && !matches!(route, Route::Search(_));
            if leaves_search && !imp.search_entry.text().is_empty() {
                imp.search_entry.set_text("");
            }
            window.update_back_button();
        });
        ctx.navigator
            .view()
            .connect_visible_page_notify(glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_| {
                    window.update_back_button();
                    window.update_now_playing();
                }
            ));
        self.imp().nav_split.connect_collapsed_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| {
                window.update_back_button();
                window.update_sidebar_width();
            }
        ));
    }

    /// Switches the window's layout for the Now Playing page as it is shown or left.
    /// Pages opened from it (its album or artist) get the usual layout.
    fn update_now_playing(&self) {
        let imp = self.imp();
        let tag = Route::NowPlaying.tag();
        let shown = self
            .ctx()
            .navigator
            .view()
            .visible_page()
            .and_then(|page| page.tag())
            .is_some_and(|visible| visible == tag);
        if imp.now_playing.replace(shown) == shown {
            return;
        }
        if imp.stack.visible_child_name().as_deref() == Some("library") {
            imp.toolbar.set_reveal_bottom_bars(!shown);
        }
        if let (Some(view), Some(search)) = (imp.content_view.get(), imp.search.get()) {
            view.set_extend_content_to_top_edge(shown);
            search.set_visible(!shown);
        }
        self.update_sidebar_width();
    }

    /// Back goes to the previous page, or from a top-level page to the sidebar when
    /// the sidebar is collapsed.
    fn go_back(&self) {
        if !self.ctx().navigator.back() {
            self.imp().nav_split.set_show_content(false);
        }
    }

    fn update_back_button(&self) {
        let imp = self.imp();
        let can_go_back = self.ctx().navigator.can_go_back();
        let collapsed = imp.nav_split.is_collapsed();
        imp.back_button.set_visible(can_go_back || collapsed);
        let tooltip = if can_go_back { "Back" } else { "Library" };
        imp.back_button.set_tooltip_text(Some(tooltip));
        imp.back_button
            .update_property(&[gtk::accessible::Property::Label(tooltip)]);
    }

    fn add_breakpoints(&self) {
        let imp = self.imp();
        let player_bar = self.player_bar();

        let queue_overlay = max_width_breakpoint(QUEUE_OVERLAY_WIDTH);
        queue_overlay.add_setter(&imp.queue_split, "collapsed", Some(&true.to_value()));
        self.add_breakpoint(queue_overlay);

        let narrow = max_width_breakpoint(NARROW_WIDTH);
        narrow.add_setter(&imp.queue_split, "collapsed", Some(&true.to_value()));
        narrow.add_setter(&imp.nav_split, "collapsed", Some(&true.to_value()));
        narrow.add_setter(player_bar, "compact", Some(&true.to_value()));
        self.add_breakpoint(narrow);
    }

    fn edge(&self, pane: Pane) -> PaneEdge {
        PaneEdge {
            window: self.downgrade(),
            pane,
        }
    }

    fn restore_pane_sizes(&self) {
        let state = self.ctx().settings().window.clone();
        let imp = self.imp();
        imp.sidebar_width
            .set(f64::from(state.sidebar_width).clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH));
        imp.sidebar_icons_only.set(state.sidebar_icons_only);
        self.update_sidebar_width();
        self.set_queue_width(f64::from(state.queue_width));
    }

    /// Sizes the sidebar for a drag that asks for `width`: narrow enough, and it shows
    /// only icons.
    fn resize_sidebar(&self, width: f64) {
        let imp = self.imp();
        let icons_only = width < SIDEBAR_ICONS_ONLY_BELOW;
        imp.sidebar_icons_only.set(icons_only);
        if !icons_only {
            imp.sidebar_width
                .set(width.clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH));
        }
        self.update_sidebar_width();
    }

    /// Gives the sidebar the size it was dragged to, or only its icons while Now
    /// Playing is shown. In the narrow layout it is a page of its own, which shows
    /// everything and isn't resized.
    fn update_sidebar_width(&self) {
        let imp = self.imp();
        let (Some(sidebar), Some(handle)) = (imp.sidebar.get(), imp.sidebar_handle.get()) else {
            return;
        };
        let collapsed = imp.nav_split.is_collapsed();
        let now_playing = imp.now_playing.get();
        sidebar.set_icons_only((imp.sidebar_icons_only.get() || now_playing) && !collapsed);
        handle.set_visible(!collapsed && !now_playing);
        if let Some(shade) = imp.sidebar_shade.get() {
            shade.set_visible(!collapsed);
        }
        let width = match imp.nav_split.sidebar() {
            // As narrow as the icons and the header's buttons allow.
            Some(page) if sidebar.icons_only() => {
                f64::from(page.measure(gtk::Orientation::Horizontal, -1).0)
            }
            _ => imp.sidebar_width.get(),
        };
        imp.nav_split.set_min_sidebar_width(width);
        imp.nav_split.set_max_sidebar_width(width);
    }

    /// The queue casts its shade on the page while it sits beside it, not while it
    /// floats over it (libadwaita shades that itself).
    fn update_queue_shade(&self) {
        let imp = self.imp();
        if let Some(shade) = imp.queue_shade.get() {
            shade.set_visible(imp.queue_split.shows_sidebar() && !imp.queue_split.is_collapsed());
        }
    }

    fn set_queue_width(&self, width: f64) {
        let width = width.clamp(QUEUE_MIN_WIDTH, QUEUE_MAX_WIDTH);
        let split = &self.imp().queue_split;
        split.set_min_sidebar_width(width);
        split.set_max_sidebar_width(width);
    }

    fn save_pane_sizes(&self) {
        let imp = self.imp();
        let sidebar_width = imp.sidebar_width.get().round() as i32;
        let sidebar_icons_only = imp.sidebar_icons_only.get();
        let queue_width = imp.queue_split.max_sidebar_width().round() as i32;
        self.ctx().update_settings(|settings| {
            settings.window.sidebar_width = sidebar_width;
            settings.window.sidebar_icons_only = sidebar_icons_only;
            settings.window.queue_width = queue_width;
        });
    }

    fn restore_window_state(&self) {
        let state = self.ctx().settings().window.clone();
        self.set_default_size(state.width.max(360), state.height.max(480));
        if state.maximized {
            self.maximize();
        }
    }

    fn save_window_state(&self) {
        let (width, height) = self.default_size();
        let maximized = self.is_maximized();
        self.ctx().update_settings(|settings| {
            settings.window.maximized = maximized;
            if !maximized && width > 0 && height > 0 {
                settings.window.width = width;
                settings.window.height = height;
            }
        });
    }
}

/// A strip along the page's `halign` edge that the edges option shades (`mp-edge-shade`).
fn edge_shade(halign: gtk::Align) -> gtk::Widget {
    let side = if halign == gtk::Align::Start {
        "mp-start"
    } else {
        "mp-end"
    };
    gtk::Box::builder()
        .halign(halign)
        .can_target(false)
        .css_classes(["mp-edge-shade", side])
        .build()
        .upcast()
}

#[derive(Clone, Copy)]
enum Pane {
    Sidebar,
    Queue,
}

/// The inner edge of the sidebar or the queue panel, which resizes it.
struct PaneEdge {
    window: glib::WeakRef<MainWindow>,
    pane: Pane,
}

impl Resizable for PaneEdge {
    fn size(&self) -> f64 {
        let Some(window) = self.window.upgrade() else {
            return 0.0;
        };
        let imp = window.imp();
        match self.pane {
            Pane::Sidebar => imp.nav_split.max_sidebar_width(),
            Pane::Queue => imp.queue_split.max_sidebar_width(),
        }
    }

    fn resize(&self, size: f64) {
        if let Some(window) = self.window.upgrade() {
            match self.pane {
                Pane::Sidebar => window.resize_sidebar(size),
                Pane::Queue => window.set_queue_width(size),
            }
        }
    }

    fn resized(&self) {
        if let Some(window) = self.window.upgrade() {
            window.save_pane_sizes();
        }
    }
}

/// The app's main menu at the end of the sidebar's header bar (also opened with F10).
fn primary_menu_button() -> gtk::MenuButton {
    let library = gio::Menu::new();
    library.append(Some("_Preferences"), Some("app.preferences"));
    library.append(Some("_Rescan Library"), Some("app.rescan"));
    let menu = gio::Menu::new();
    menu.append_section(None, &library);
    menu.append(Some(&format!("_About {APP_NAME}")), Some("app.about"));

    let button = gtk::MenuButton::builder()
        .icon_name(icons::MENU)
        .menu_model(&menu)
        .primary(true)
        .build();
    set_tooltip(&button, "Main Menu");
    button
}
