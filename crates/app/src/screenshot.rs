//! Debug builds only: renders the main window to a PNG once the library has loaded,
//! then quits. Used to check the look of the app without anyone at the screen.
//!
//! | Variable | Effect |
//! |---|---|
//! | `PENGUIN_PLAYER_SCREENSHOT=<file.png>` | Where to save the screenshot (turns this on). |
//! | `PENGUIN_PLAYER_SCREENSHOT_PAGE=<page>` | `songs`, `albums`, `artists`, `formats`, `now-playing`, `queue`, `settings` (or `settings:appearance`, `settings:plugins`), `search:<query>`, or a detail page: `album:<title>`, `artist:<name>`, `playlist:<name>`, `format:<id>`. |
//! | `PENGUIN_PLAYER_SCREENSHOT_SCHEME=light\|dark` | Forces the colour scheme. |
//! | `PENGUIN_PLAYER_SCREENSHOT_PLAY=<n>` | Plays the library from track `n` first. Only with `PENGUIN_PLAYER_NULL_AUDIO=1`, so it never makes sound. |
//! | `PENGUIN_PLAYER_SCREENSHOT_DELAY=<seconds>` | Waits this long after opening the page (default 1), e.g. to capture a later moment of playback. |
//! | `PENGUIN_PLAYER_SCREENSHOT_SCROLL=<0…1>` | Scrolls the page this far down its content before capturing it. |

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};

use crate::context::AppContext;
use crate::player::NULL_AUDIO_ENV;
use crate::ui::navigation::Route;
use crate::ui::window::MainWindow;

const SCREENSHOT_ENV: &str = "PENGUIN_PLAYER_SCREENSHOT";
const PAGE_ENV: &str = "PENGUIN_PLAYER_SCREENSHOT_PAGE";
const SCHEME_ENV: &str = "PENGUIN_PLAYER_SCREENSHOT_SCHEME";
const PLAY_ENV: &str = "PENGUIN_PLAYER_SCREENSHOT_PLAY";
const DELAY_ENV: &str = "PENGUIN_PLAYER_SCREENSHOT_DELAY";
const SCROLL_ENV: &str = "PENGUIN_PLAYER_SCREENSHOT_SCROLL";

/// Give up waiting for the library after this long and take the picture anyway.
const LOAD_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Time for the page to build and covers to load before capturing, unless
/// `PENGUIN_PLAYER_SCREENSHOT_DELAY` asks for longer.
const SETTLE_TIME: Duration = Duration::from_secs(1);

/// Arranges the screenshot if `PENGUIN_PLAYER_SCREENSHOT` is set.
pub fn schedule(ctx: &Rc<AppContext>, window: &MainWindow) {
    let Some(path) = std::env::var_os(SCREENSHOT_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    else {
        return;
    };
    // Headless displays may draw only the first frame, so transitions would never finish.
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_enable_animations(false);
    }
    match std::env::var(SCHEME_ENV).as_deref() {
        Ok("light") => ctx
            .themes
            .force_color_scheme(Some(adw::ColorScheme::ForceLight)),
        Ok("dark") => ctx
            .themes
            .force_color_scheme(Some(adw::ColorScheme::ForceDark)),
        _ => {}
    }
    let page = std::env::var(PAGE_ENV).unwrap_or_default();
    let delay = std::env::var(DELAY_ENV)
        .ok()
        .and_then(|value| value.parse().ok())
        .map_or(SETTLE_TIME, Duration::from_secs_f64);

    let scanned = Rc::new(Cell::new(ctx.settings().music_folder.is_none()));
    let flag = scanned.clone();
    ctx.library_service.connect_scanning_notify(move |service| {
        if !service.scanning() {
            flag.set(true);
        }
    });

    let started = Instant::now();
    let ctx = Rc::downgrade(ctx);
    let window = window.downgrade();
    glib::timeout_add_local(POLL_INTERVAL, move || {
        let (Some(ctx), Some(window)) = (ctx.upgrade(), window.upgrade()) else {
            return glib::ControlFlow::Break;
        };
        let ready = ctx.store.loaded() && scanned.get();
        if !ready && started.elapsed() < LOAD_TIMEOUT {
            return glib::ControlFlow::Continue;
        }
        start_playback(&ctx);
        open_page(&ctx, &window, &page);
        let path = path.clone();
        glib::timeout_add_local_once(delay, move || {
            match capture(&window, &path) {
                Ok(()) => log::info!("Saved screenshot to {}", path.display()),
                Err(err) => log::error!("Screenshot failed: {err}"),
            }
            if let Some(app) = window.application() {
                app.quit();
            }
        });
        glib::ControlFlow::Break
    });
}

fn start_playback(ctx: &AppContext) {
    let Some(start) = std::env::var(PLAY_ENV)
        .ok()
        .and_then(|value| value.parse().ok())
    else {
        return;
    };
    if std::env::var(NULL_AUDIO_ENV).as_deref() != Ok("1") {
        log::warn!("{PLAY_ENV} is ignored without {NULL_AUDIO_ENV}=1");
        return;
    }
    ctx.player.play_tracks(ctx.store.track_ids(), start);
}

fn open_page(ctx: &Rc<AppContext>, window: &MainWindow, page: &str) {
    match page {
        "" | "songs" => ctx.navigator.navigate(Route::Songs),
        "albums" => ctx.navigator.navigate(Route::Albums),
        "artists" => ctx.navigator.navigate(Route::Artists),
        "formats" => ctx.navigator.navigate(Route::Formats),
        "now-playing" => ctx.navigator.navigate(Route::NowPlaying),
        "queue" => window.set_queue_visible(true),
        "settings" => crate::ui::preferences::present(ctx, window),
        other => match other.split_once(':') {
            Some(("search", query)) => window.search_for(query),
            Some(("settings", name)) => {
                crate::ui::preferences::present(ctx, window);
                if let Some(dialog) = window
                    .visible_dialog()
                    .and_downcast::<adw::PreferencesDialog>()
                {
                    dialog.set_visible_page_name(name);
                }
            }
            Some((kind, name)) => match detail_route(ctx, kind, name) {
                Some(route) => ctx.navigator.navigate(route),
                None => log::warn!("No {kind} called “{name}” for the screenshot"),
            },
            None => log::warn!("Unknown screenshot page “{other}”"),
        },
    }
}

/// Finds a detail page by what it shows: an album title, artist or playlist name
/// (case-insensitive) or a format id such as `flac`.
fn detail_route(ctx: &AppContext, kind: &str, name: &str) -> Option<Route> {
    let named = |model: gtk::gio::ListStore, title: &dyn Fn(&glib::Object) -> String| {
        (0..model.n_items())
            .filter_map(|index| model.item(index))
            .find(|item| title(item).eq_ignore_ascii_case(name))
    };
    match kind {
        "album" => named(ctx.store.albums(), &|item| item.property::<String>("title"))
            .and_downcast::<crate::model::AlbumObject>()
            .map(|album| Route::Album(album.key().clone())),
        "artist" => named(ctx.store.artists(), &|item| item.property::<String>("name"))
            .and_downcast::<crate::model::ArtistObject>()
            .map(|artist| Route::Artist(artist.key().to_owned())),
        "playlist" => named(ctx.store.playlists(), &|item| {
            item.property::<String>("name")
        })
        .and_downcast::<crate::model::PlaylistObject>()
        .map(|playlist| Route::Playlist(playlist.playlist_id())),
        "format" => Some(Route::Format(mp_core::AudioFormat::from_id(name))),
        _ => None,
    }
}

/// Scrolls the scrolled window with the most to scroll as
/// `PENGUIN_PLAYER_SCREENSHOT_SCROLL` asks, so content below the fold can be checked.
/// Returns whether it scrolled.
fn scroll_page(window: &MainWindow) -> bool {
    let Some(fraction) = std::env::var(SCROLL_ENV)
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
    else {
        return false;
    };
    // An open dialog (such as Preferences) is what the screenshot is about.
    let root: gtk::Widget = window
        .visible_dialog()
        .map_or_else(|| window.clone().upcast(), |dialog| dialog.upcast());
    let mut scrolled = Vec::new();
    find_scrolled_windows(&root, &mut scrolled);
    let range = |adjustment: &gtk::Adjustment| adjustment.upper() - adjustment.page_size();
    let Some(adjustment) = scrolled
        .iter()
        .map(gtk::ScrolledWindow::vadjustment)
        .max_by(|a, b| range(a).total_cmp(&range(b)))
    else {
        log::warn!("Nothing to scroll for the screenshot");
        return false;
    };
    adjustment.set_value(range(&adjustment).max(0.0) * fraction.clamp(0.0, 1.0));
    true
}

fn find_scrolled_windows(widget: &gtk::Widget, found: &mut Vec<gtk::ScrolledWindow>) {
    if let Some(scrolled) = widget.downcast_ref::<gtk::ScrolledWindow>() {
        found.push(scrolled.clone());
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        find_scrolled_windows(&current, found);
        child = current.next_sibling();
    }
}

/// Draws the window here rather than reusing its last frame, because a headless
/// display (Broadway without a browser) stops producing frames after the first one.
/// Running the frame clock's update and layout phases by hand finishes animations
/// (such as a dialog opening) and brings sizes up to date, and every widget is
/// redrawn because cached drawings would show styles from the first frame.
/// Rendering uses the software renderer, which works on every backend.
fn capture(window: &MainWindow, path: &Path) -> Result<(), String> {
    update_layout(window);
    // Only now do the page's scrolled windows know how far they can scroll.
    if scroll_page(window) {
        update_layout(window);
    }
    queue_draw_all(window.upcast_ref());
    let display = WidgetExt::display(window);
    let (width, height) = (window.width(), window.height());
    let viewport = graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
    let snapshot = gtk::Snapshot::new();
    snapshot.append_color(&window_background(&display), &viewport);
    window.imp().parent_snapshot(&snapshot);
    let node = snapshot.to_node().ok_or("the window rendered nothing")?;

    let renderer = gsk::CairoRenderer::new();
    renderer
        .realize_for_display(&display)
        .map_err(|err| err.to_string())?;
    let texture = renderer.render_texture(&node, Some(&viewport));
    renderer.unrealize();
    texture.save_to_png(path).map_err(|err| err.to_string())
}

/// GTK paints a widget's CSS background before calling its snapshot function, so
/// `parent_snapshot` leaves the window's own background out. A probe label reads it.
fn window_background(display: &gdk::Display) -> gdk::RGBA {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(".mp-screenshot-probe { color: var(--mp-window-bg-color); }");
    gtk::style_context_add_provider_for_display(
        display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_USER,
    );
    let probe = gtk::Label::builder()
        .css_classes(["mp-screenshot-probe"])
        .build();
    let color = probe.color();
    gtk::style_context_remove_provider_for_display(display, &provider);
    color
}

fn update_layout(window: &MainWindow) {
    window.queue_resize();
    if let Some(clock) = window.frame_clock() {
        // A few rounds, as finished animations can trigger new layout work.
        for _ in 0..3 {
            clock.emit_by_name::<()>("update", &[]);
            clock.emit_by_name::<()>("layout", &[]);
        }
    }
}

fn queue_draw_all(widget: &gtk::Widget) {
    widget.queue_draw();
    let mut child = widget.first_child();
    while let Some(current) = child {
        queue_draw_all(&current);
        child = current.next_sibling();
    }
}
