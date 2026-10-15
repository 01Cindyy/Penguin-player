//! Plugins run in the "C" locale, whatever locale the app uses.
//!
//! GTK switches the whole app to the user's locale, and Lua follows it when it compares
//! strings (`strcoll`), prints numbers and formats dates. In a UTF-8 locale `strcoll` is
//! about a hundred times slower than in "C": comparing two long strings then takes tens
//! of milliseconds inside a single Lua instruction, where the time limit can't stop it.
//! The "C" locale keeps that fast and gives plugins the same results on every system
//! (`tostring(0.5)` is always `"0.5"`, strings sort byte by byte).
//!
//! Linux and macOS can switch the locale of a single thread. Elsewhere plugins keep the
//! app's locale.

pub(crate) use platform::ThreadLocale;

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod platform {
    use std::ptr;

    /// The "C" locale, in use on the thread that created it until it is dropped.
    pub(crate) struct ThreadLocale {
        locale: libc::locale_t,
        previous: libc::locale_t,
    }

    impl ThreadLocale {
        /// Switches the calling thread to the "C" locale; `None` if that failed.
        pub(crate) fn use_c() -> Option<Self> {
            const ALL_CATEGORIES: libc::c_int = libc::LC_COLLATE_MASK
                | libc::LC_CTYPE_MASK
                | libc::LC_MESSAGES_MASK
                | libc::LC_MONETARY_MASK
                | libc::LC_NUMERIC_MASK
                | libc::LC_TIME_MASK;
            // SAFETY: "C" is a valid, NUL-terminated locale name and a null base locale
            // asks for a new object, as `newlocale` documents.
            let locale = unsafe { libc::newlocale(ALL_CATEGORIES, c"C".as_ptr(), ptr::null_mut()) };
            if locale.is_null() {
                return None;
            }
            // SAFETY: `locale` is a valid locale object, which stays alive until `drop`
            // has switched the thread back.
            let previous = unsafe { libc::uselocale(locale) };
            if previous.is_null() {
                // SAFETY: the thread never started using `locale`.
                unsafe { libc::freelocale(locale) };
                return None;
            }
            Some(Self { locale, previous })
        }
    }

    impl Drop for ThreadLocale {
        fn drop(&mut self) {
            // SAFETY: `previous` is what `uselocale` returned, so it is valid to switch
            // back to; afterwards nothing uses `locale` any more. The guard is `!Send`
            // (it holds raw pointers), so this runs on the thread that switched.
            unsafe {
                libc::uselocale(self.previous);
                libc::freelocale(self.locale);
            }
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod platform {
    /// Stands in for the per-thread locale this platform doesn't have.
    pub(crate) struct ThreadLocale;

    impl ThreadLocale {
        pub(crate) fn use_c() -> Option<Self> {
            None
        }
    }
}
