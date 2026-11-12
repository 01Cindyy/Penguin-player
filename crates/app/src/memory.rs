//! Handing memory the app has freed back to the system.

/// Returns freed memory to the system a few seconds from now, such as after pages close
/// or the library reloads. Calls in quick succession release it once.
pub fn release_soon() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    glibc::release_soon();
}

/// glibc keeps freed memory for reuse, so without this the app would stay as large as
/// it was at its busiest. Other allocators give it back on their own.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
mod glibc {
    use std::cell::RefCell;
    use std::time::Duration;

    use gtk::{gio, glib};

    const DELAY: Duration = Duration::from_secs(3);

    thread_local! {
        static PENDING: RefCell<Option<glib::SourceId>> = const { RefCell::new(None) };
    }

    pub fn release_soon() {
        PENDING.with(|pending| {
            if let Some(source) = pending.take() {
                source.remove();
            }
            let source = glib::timeout_add_local_once(DELAY, || {
                PENDING.with(RefCell::take);
                // Off the main thread, as it goes through the whole heap.
                drop(gio::spawn_blocking(|| {
                    // SAFETY: only takes the allocator's own locks.
                    unsafe { libc::malloc_trim(0) };
                }));
            });
            pending.replace(Some(source));
        });
    }
}
