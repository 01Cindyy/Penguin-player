//! Moving between pages. A [`Route`] names a page; the [`Navigator`] builds it and
//! shows it in the content pane's `adw::NavigationView`.
//!
//! Top-level routes (the sidebar entries and search) replace the whole stack without
//! animation, like switching tabs. Detail routes (an album, an artist, a format) are
//! pushed on top, so the back button and the back gesture return to where you were.

use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::glib;
use gtk::subclass::prelude::*;
use mp_core::{AlbumKey, AudioFormat, PlaylistId};

use crate::context::AppContext;
use crate::ui::pages;

/// A page of the app.
#[derive(Debug, Clone, PartialEq, Eq, glib::Boxed)]
#[boxed_type(name = "MpRoute")]
pub enum Route {
    Songs,
    Albums,
    Album(AlbumKey),
    Artists,
    /// An album artist, by normalised name (see [`crate::model::ArtistObject::key`]).
    Artist(String),
    Formats,
    Format(AudioFormat),
    /// The playing song, filling the window.
    NowPlaying,
    Playlist(PlaylistId),
    Search(String),
}

impl Route {
    /// Whether this route is a root page rather than a page pushed on top of one.
    pub fn is_top_level(&self) -> bool {
        matches!(
            self,
            Route::Songs
                | Route::Albums
                | Route::Artists
                | Route::Formats
                | Route::NowPlaying
                | Route::Playlist(_)
                | Route::Search(_)
        )
    }

    /// The `adw::NavigationPage` tag of this route's page.
    pub fn tag(&self) -> String {
        match self {
            Route::Songs => "songs".into(),
            Route::Albums => "albums".into(),
            Route::Album(key) => format!("album:{}:{}", key.artist, key.title),
            Route::Artists => "artists".into(),
            Route::Artist(key) => format!("artist:{key}"),
            Route::Formats => "formats".into(),
            Route::Format(format) => format!("format:{}", format.id()),
            Route::NowPlaying => "now-playing".into(),
            Route::Playlist(id) => format!("playlist:{}", id.0),
            Route::Search(_) => "search".into(),
        }
    }

    fn build_page(&self, ctx: &Rc<AppContext>) -> adw::NavigationPage {
        let page = match self {
            Route::Songs => pages::songs::build(ctx),
            Route::Albums => pages::albums::build(ctx),
            Route::Album(key) => pages::album::build(ctx, key),
            Route::Artists => pages::artists::build(ctx),
            Route::Artist(key) => pages::artist::build(ctx, key),
            Route::Formats => pages::formats::build(ctx),
            Route::Format(format) => pages::format::build(ctx, *format),
            Route::NowPlaying => pages::now_playing::build(ctx),
            Route::Playlist(id) => pages::playlist::build(ctx, *id),
            Route::Search(query) => pages::search::build(ctx, query),
        };
        page.set_tag(Some(&self.tag()));
        page
    }
}

mod imp {
    use std::cell::RefCell;
    use std::rc::Weak;
    use std::sync::OnceLock;

    use gtk::glib;
    use gtk::glib::subclass::Signal;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;

    use super::Route;
    use crate::context::AppContext;

    #[derive(Default)]
    pub struct Navigator {
        pub(super) context: RefCell<Weak<AppContext>>,
        pub(super) view: adw::NavigationView,
        pub(super) top_level: RefCell<Option<Route>>,
        /// Where to return when the search is cleared.
        pub(super) before_search: RefCell<Option<Route>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Navigator {
        const NAME: &'static str = "MpNavigator";
        type Type = super::Navigator;
    }

    impl ObjectImpl for Navigator {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    Signal::builder("navigated")
                        .param_types([Route::static_type()])
                        .build(),
                    Signal::builder("search-changed")
                        .param_types([String::static_type()])
                        .build(),
                    Signal::builder("search-activated")
                        .param_types([String::static_type()])
                        .build(),
                ]
            })
        }
    }
}

glib::wrapper! {
    /// Drives the content pane. Owned by [`AppContext`]; reach it as `ctx.navigator`.
    pub struct Navigator(ObjectSubclass<imp::Navigator>);
}

impl Navigator {
    pub(crate) fn new(context: Weak<AppContext>) -> Self {
        let navigator: Self = glib::Object::new();
        navigator.imp().context.replace(context);
        navigator
    }

    /// The navigation view the window places in its content pane.
    pub fn view(&self) -> &adw::NavigationView {
        &self.imp().view
    }

    /// Shows `route`. Choosing the top-level page that is already shown returns to it
    /// instead of rebuilding it, like pressing a tab twice.
    pub fn navigate(&self, route: Route) {
        let Some(ctx) = self.context() else {
            return;
        };
        if let Route::Search(query) = &route {
            self.search(query);
            return;
        }
        let imp = self.imp();
        if route.is_top_level() {
            imp.before_search.replace(None);
            if imp.top_level.borrow().as_ref() == Some(&route) {
                self.pop_to_root();
            } else {
                imp.view.replace(&[route.build_page(&ctx)]);
                imp.top_level.replace(Some(route.clone()));
            }
        } else if let Some(page) = imp.view.find_page(&route.tag()) {
            // A tag can only be in the stack once, so return to the page instead.
            imp.view.pop_to_page(&page);
        } else {
            imp.view.push(&route.build_page(&ctx));
        }
        self.emit_by_name::<()>("navigated", &[&route]);
    }

    /// Shows search results for `query`. While the search page is open, typing only
    /// updates it (see [`Self::connect_search_changed`]); an empty query goes back to
    /// the page that was open before searching.
    pub fn search(&self, query: &str) {
        let imp = self.imp();
        let query = query.trim();
        let searching = matches!(*imp.top_level.borrow(), Some(Route::Search(_)));
        match (searching, query.is_empty()) {
            (false, true) => {}
            (true, true) => {
                let previous = imp.before_search.take().unwrap_or(Route::Songs);
                self.navigate(previous);
            }
            (true, false) => {
                imp.top_level.replace(Some(Route::Search(query.to_owned())));
                self.pop_to_root();
                self.emit_by_name::<()>("search-changed", &[&query]);
            }
            (false, false) => {
                let Some(ctx) = self.context() else {
                    return;
                };
                let previous = imp.top_level.borrow().clone();
                let route = Route::Search(query.to_owned());
                imp.view.replace(&[route.build_page(&ctx)]);
                imp.top_level.replace(Some(route.clone()));
                imp.before_search.replace(previous);
                self.emit_by_name::<()>("navigated", &[&route]);
            }
        }
    }

    /// Enter in the search field: shows results for `query` (which may be newer than
    /// the last search, if typing hasn't paused yet), then tells the search page.
    pub fn activate_search(&self, query: &str) {
        self.search(query);
        self.emit_by_name::<()>("search-activated", &[&query.trim()]);
    }

    /// Goes back one page. Returns `false` at the root.
    pub fn back(&self) -> bool {
        self.imp().view.pop()
    }

    /// Whether there is a page to go back to.
    pub fn can_go_back(&self) -> bool {
        let view = &self.imp().view;
        view.visible_page()
            .is_some_and(|page| view.previous_page(&page).is_some())
    }

    /// The root page currently shown (what the sidebar highlights).
    pub fn top_level_route(&self) -> Option<Route> {
        self.imp().top_level.borrow().clone()
    }

    /// Called after every navigation with the route that was shown.
    pub fn connect_navigated<F: Fn(&Route) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_closure(
            "navigated",
            false,
            glib::closure_local!(move |_: &Self, route: &Route| f(route)),
        )
    }

    /// Called when the query changes while the search page stays open. The search
    /// page uses this to refilter instead of being rebuilt on every keystroke.
    pub fn connect_search_changed<F: Fn(&str) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_closure(
            "search-changed",
            false,
            glib::closure_local!(move |_: &Self, query: &str| f(query)),
        )
    }

    /// Called with the query when Enter is pressed in the search field, after the
    /// search page has been brought up to date with it.
    pub fn connect_search_activated<F: Fn(&str) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_closure(
            "search-activated",
            false,
            glib::closure_local!(move |_: &Self, query: &str| f(query)),
        )
    }

    fn context(&self) -> Option<Rc<AppContext>> {
        self.imp().context.borrow().upgrade()
    }

    fn pop_to_root(&self) {
        let view = &self.imp().view;
        let stack = view.navigation_stack();
        if let Some(root) = stack.item(0).and_downcast::<adw::NavigationPage>() {
            view.pop_to_page(&root);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_level_routes() {
        assert!(Route::Songs.is_top_level());
        assert!(Route::NowPlaying.is_top_level());
        assert!(Route::Playlist(PlaylistId(1)).is_top_level());
        assert!(Route::Search("x".into()).is_top_level());
        assert!(!Route::Artist("a".into()).is_top_level());
        assert!(!Route::Format(AudioFormat::Flac).is_top_level());
        assert_eq!(Route::Format(AudioFormat::Flac).tag(), "format:flac");
    }
}
