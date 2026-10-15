//! Quitting cleanly when the session or a terminal asks the app to stop (SIGTERM,
//! SIGINT, SIGHUP), so the window size, settings and plugins are saved as on Ctrl+Q.
//! Only Unix has these signals; elsewhere this does nothing.

/// Calls `quit` on the main thread whenever a termination signal arrives. Install it
/// once, before the main loop runs.
pub fn on_termination(quit: fn()) {
    #[cfg(unix)]
    for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        glib_unix::unix_signal_add_local(signal, move || {
            quit();
            gtk::glib::ControlFlow::Continue
        });
    }
    #[cfg(not(unix))]
    let _ = quit;
}
