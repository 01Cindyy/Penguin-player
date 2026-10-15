//! Application startup: stylesheets, the shared context, actions and shortcuts.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use mp_core::{APP_ID, APP_NAME, PlayerCommand, VERSION};

use crate::context::AppContext;
use crate::player::VOLUME_STEP;
use crate::ui::window::MainWindow;
use crate::ui::{icons, preferences};

/// Accelerators for actions that never conflict with typing. Playback shortcuts
/// (Space, Ctrl+arrows, …) are handled in [`crate::ui::shortcuts`] instead.
const ACCELERATORS: &[(&str, &[&str])] = &[
    ("app.quit", &["<Control>q"]),
    ("app.preferences", &["<Control>comma"]),
    ("app.search", &["<Control>f"]),
    ("app.toggle-queue", &["<Control>l"]),
];

/// Runs the application until it quits.
pub fn run() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    let context: Rc<RefCell<Option<Rc<AppContext>>>> = Rc::default();

    app.connect_startup(|_| {
        let Some(display) = gdk::Display::default() else {
            log::error!("No display to show the app on");
            return;
        };
        load_stylesheet(&display);
        icons::register(&display);
        gtk::Window::set_default_icon_name(APP_ID);
    });
    app.connect_activate(glib::clone!(
        #[strong]
        context,
        move |app| activate(app, &context)
    ));
    app.connect_shutdown(move |_| {
        if let Some(ctx) = context.borrow_mut().take() {
            ctx.shutdown();
        }
    });
    crate::signals::on_termination(quit_running_app);
    app.run()
}

fn activate(app: &adw::Application, context: &RefCell<Option<Rc<AppContext>>>) {
    if let Some(window) = app.active_window() {
        window.present();
        return;
    }

    let ctx = AppContext::new(app);
    apply_theme(&ctx);
    install_actions(&ctx);
    let window = MainWindow::new(&ctx);
    connect_window_requests(&ctx, &window);
    window.present();
    ctx.start();

    #[cfg(debug_assertions)]
    crate::screenshot::schedule(&ctx, &window);

    context.replace(Some(ctx));
}

/// The app's structural styles: the shared base followed by one file per area of the UI.
const STYLESHEET: &str = concat!(
    include_str!("../data/style.css"),
    include_str!("../data/css/browse.css"),
    include_str!("../data/css/collections.css"),
    include_str!("../data/css/discover.css"),
);

/// The app's structural stylesheet, below any theme.
fn load_stylesheet(display: &gdk::Display) {
    let provider = gtk::CssProvider::new();
    provider.connect_parsing_error(|_, section, error| {
        log::warn!("style.css {}: {error}", section.to_str());
    });
    provider.load_from_string(STYLESHEET);
    gtk::style_context_add_provider_for_display(
        display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn apply_theme(ctx: &Rc<AppContext>) {
    let weak = Rc::downgrade(ctx);
    ctx.themes.connect_error(move |_, message| {
        if let Some(ctx) = weak.upgrade() {
            ctx.toast(message);
        }
    });
    if let Some(display) = gdk::Display::default() {
        ctx.themes.install(&display);
    }
    let (theme, scheme) = {
        let settings = ctx.settings();
        (settings.theme.clone(), settings.color_scheme)
    };
    ctx.themes.set_color_scheme(scheme);
    ctx.themes.apply(&theme);
    ctx.apply_appearance();
}

/// The player can ask for the window to be raised or the app to quit (media
/// controls, plugins); the window decides how.
fn connect_window_requests(ctx: &AppContext, window: &MainWindow) {
    ctx.player.connect_raise_requested(glib::clone!(
        #[weak]
        window,
        move |_| window.present()
    ));
    ctx.player.connect_quit_requested(glib::clone!(
        #[weak]
        window,
        move |_| quit(&window)
    ));
}

/// Closes the window first so it can save its size, then quits.
fn quit(window: &MainWindow) {
    let app = window.application();
    window.close();
    if let Some(app) = app {
        app.quit();
    }
}

/// Quits the way Ctrl+Q does, for a termination signal.
fn quit_running_app() {
    let Some(app) = gio::Application::default() else {
        return;
    };
    let window = app
        .downcast_ref::<gtk::Application>()
        .and_then(GtkApplicationExt::active_window)
        .and_downcast::<MainWindow>();
    match window {
        Some(window) => quit(&window),
        None => app.quit(),
    }
}

fn install_actions(ctx: &Rc<AppContext>) {
    let app = &ctx.app;
    let window_action = |name: &str, run: fn(&Rc<AppContext>, &MainWindow)| {
        let weak = Rc::downgrade(ctx);
        gio::ActionEntry::builder(name)
            .activate(move |app: &adw::Application, _, _| {
                let window = app.active_window().and_downcast::<MainWindow>();
                if let (Some(ctx), Some(window)) = (weak.upgrade(), window) {
                    run(&ctx, &window);
                }
            })
            .build()
    };
    let player_action = |name: &str, run: fn(&AppContext)| {
        let weak = Rc::downgrade(ctx);
        gio::ActionEntry::builder(name)
            .activate(move |_: &adw::Application, _, _| {
                if let Some(ctx) = weak.upgrade() {
                    run(&ctx);
                }
            })
            .build()
    };

    app.add_action_entries([
        window_action("quit", |_, window| quit(window)),
        window_action("preferences", |ctx, window| {
            preferences::present(ctx, window);
        }),
        window_action("search", |_, window| window.focus_search()),
        window_action("toggle-queue", |_, window| {
            window.set_queue_visible(!window.is_queue_visible());
        }),
        window_action("about", |_, window| show_about(window)),
        player_action("play-pause", |ctx| {
            ctx.player.execute(PlayerCommand::TogglePlayPause);
        }),
        player_action("stop", |ctx| ctx.player.execute(PlayerCommand::Stop)),
        player_action("next", |ctx| ctx.player.execute(PlayerCommand::Next)),
        player_action("previous", |ctx| {
            ctx.player.execute(PlayerCommand::Previous)
        }),
        player_action("toggle-shuffle", |ctx| {
            let shuffle = !ctx.player.shuffle();
            ctx.player.execute(PlayerCommand::SetShuffle(shuffle));
        }),
        player_action("cycle-repeat", |ctx| ctx.player.cycle_repeat()),
        player_action("volume-up", |ctx| ctx.player.change_volume(VOLUME_STEP)),
        player_action("volume-down", |ctx| ctx.player.change_volume(-VOLUME_STEP)),
        player_action("rescan", |ctx| ctx.library_service.rescan(false)),
    ]);
    for (action, accels) in ACCELERATORS {
        app.set_accels_for_action(action, accels);
    }
}

fn show_about(window: &MainWindow) {
    let about = adw::AboutDialog::builder()
        .application_name(APP_NAME)
        .application_icon(APP_ID)
        .version(VERSION)
        .comments("Your music library, offline and in full quality.")
        .license_type(gtk::License::MitX11)
        .build();
    about.add_legal_section(
        "Icons",
        Some("Adwaita icon theme by the GNOME Project"),
        gtk::License::Lgpl30,
        None,
    );
    about.present(Some(window));
}
