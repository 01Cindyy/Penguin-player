//! Plugins run in the "C" locale, whatever locale the app uses.
//!
//! GTK switches the whole app to the user's locale, and Lua follows it when it compares
//! strings (`strcoll`), prints numbers and formats dates. In a UTF-8 locale `strcoll` is
//! about a hundred times slower than in "C": comparing two long strings then takes tens
//! of milliseconds inside a single Lua instruction, where the time limit can't stop it.
//! The "C" locale keeps that fast and gives plugins the same results on every system
//! (`tostring(0.5)` is always `"0.5"`, strings sort byte by byte).
//!
//! Linux and Windows can switch the locale of a single thread. Elsewhere plugins
//! keep the app's locale.

pub(crate) use platform::ThreadLocale;

#[cfg(target_os = "linux")]
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

/// Windows has no `uselocale`, but its C runtime can give a thread a locale of its own
/// and switch that.
#[cfg(windows)]
mod platform {
    use std::ffi::{CStr, CString, c_char, c_int};
    use std::marker::PhantomData;
    use std::ptr;

    const LC_ALL: c_int = 0;
    const ENABLE_PER_THREAD_LOCALE: c_int = 1;

    unsafe extern "C" {
        fn _configthreadlocale(kind: c_int) -> c_int;
        fn setlocale(category: c_int, locale: *const c_char) -> *mut c_char;
    }

    /// The "C" locale, in use on the thread that created it until it is dropped.
    pub(crate) struct ThreadLocale {
        /// The thread's locale before, to switch back to.
        previous: CString,
        previous_mode: c_int,
        /// Locales are per thread, so the guard must stay on its thread.
        _not_send: PhantomData<*const ()>,
    }

    impl ThreadLocale {
        /// Switches the calling thread to the "C" locale; `None` if that failed.
        pub(crate) fn use_c() -> Option<Self> {
            // SAFETY: these C runtime calls only affect the calling thread once it has a
            // locale of its own. The string `setlocale` returns is copied before the
            // next call can overwrite it.
            unsafe {
                let previous_mode = _configthreadlocale(ENABLE_PER_THREAD_LOCALE);
                if previous_mode == -1 {
                    return None;
                }
                let current = setlocale(LC_ALL, ptr::null());
                if !current.is_null() {
                    let previous = CStr::from_ptr(current).to_owned();
                    if !setlocale(LC_ALL, c"C".as_ptr()).is_null() {
                        return Some(Self {
                            previous,
                            previous_mode,
                            _not_send: PhantomData,
                        });
                    }
                }
                _configthreadlocale(previous_mode);
                None
            }
        }
    }

    impl Drop for ThreadLocale {
        fn drop(&mut self) {
            // SAFETY: `previous` is a locale name `setlocale` gave, and this runs on the
            // thread that switched (the guard isn't `Send`).
            unsafe {
                setlocale(LC_ALL, self.previous.as_ptr());
                _configthreadlocale(self.previous_mode);
            }
        }
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod platform {
    /// Stands in for the per-thread locale this platform doesn't have.
    pub(crate) struct ThreadLocale;

    impl ThreadLocale {
        pub(crate) fn use_c() -> Option<Self> {
            None
        }
    }
}
