//! Plugins run in the "C" locale, even though GTK switches the app to the user's.
//! Changing the locale affects the whole process, so these tests have their own binary.

#![cfg(any(target_os = "linux", target_os = "macos"))]

mod common;

use std::time::{Duration, Instant};

use common::TestDirs;
use mp_core::PlayerEvent;
use mp_plugins::CALLBACK_TIME_LIMIT;

/// Switches the process to the first installed locale whose string order differs from
/// "C" (where `"B" < "a"`). Returns its name, or `None` if none is installed.
fn use_user_like_locale() -> Option<&'static str> {
    [
        "en_US.UTF-8",
        "en_US.utf8",
        "en_GB.UTF-8",
        "en_GB.utf8",
        "de_DE.UTF-8",
        "fr_FR.UTF-8",
    ]
    .into_iter()
    .find(|name| {
        let name = std::ffi::CString::new(*name).expect("no NUL in locale names");
        // SAFETY: the tests in this binary run one at a time (see `LOCK`), and nothing
        // else uses the locale while it changes.
        unsafe { !libc::setlocale(libc::LC_ALL, name.as_ptr()).is_null() }
    })
}

/// `setlocale` isn't thread-safe, so the tests here never run in parallel.
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Plugins compare strings byte by byte, whatever the app's locale: in many locales
/// `strcoll` is so slow that a loop comparing two long strings would run for seconds
/// past the time limit.
#[test]
fn plugins_use_the_c_locale_whatever_the_app_uses() {
    let _lock = LOCK.lock().unwrap();
    let Some(locale) = use_user_like_locale() else {
        eprintln!("skipped: no locale with a non-bytewise string order is installed");
        return;
    };
    let dirs = TestDirs::new();
    dirs.plugin(
        "locale",
        r#"
        mp.notify("locale", tostring("a" < "B") .. "|" .. tostring(0.5) .. "|" .. os.date("!%A %B", 0))
        local long = string.rep("a", 8e6)
        local other = long:sub(1, -2) .. "b"
        mp.on("seeked", function()
          while true do local _ = long < other end
        end)
        "#,
    );
    let (host, events) = dirs.start(&["locale"]);
    assert_eq!(
        events.notified("locale"),
        "false|0.5|Thursday January",
        "in {locale}"
    );

    let started = Instant::now();
    host.dispatch(PlayerEvent::Seeked {
        position: Duration::ZERO,
    });
    let (_, message) = events.error();
    assert!(message.contains("took longer than 250 ms"), "{message}");
    assert!(
        started.elapsed() < CALLBACK_TIME_LIMIT * 4,
        "stopped after {:?} in {locale}",
        started.elapsed()
    );
}
