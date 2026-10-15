//! What an empty library page says: still scanning, folder unavailable, or no music.

use std::rc::Rc;

use gtk::prelude::*;

use crate::context::AppContext;
use crate::services::library::LibraryService;
use crate::ui::disconnect_on_destroy;
use crate::util::format_count;

/// Why a library page has nothing to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryStatus {
    /// The music folder is being scanned; `total` is 0 until the files are counted.
    Scanning { processed: u32, total: u32 },
    /// The music folder can't be reached.
    FolderMissing,
    /// The library is loaded and has nothing of this kind.
    Empty,
}

impl LibraryStatus {
    pub fn of(service: &LibraryService) -> Self {
        if service.scanning() {
            LibraryStatus::Scanning {
                processed: service.scan_processed(),
                total: service.scan_total(),
            }
        } else if service.folder_missing() {
            LibraryStatus::FolderMissing
        } else {
            LibraryStatus::Empty
        }
    }

    /// The empty state's title; `empty` names what the page lists, e.g. "No Albums".
    pub fn title<'a>(&self, empty: &'a str) -> &'a str {
        match self {
            LibraryStatus::Scanning { .. } => "Scanning Your Music",
            LibraryStatus::FolderMissing => "Music Folder Unavailable",
            LibraryStatus::Empty => empty,
        }
    }

    /// The empty state's description; `empty` explains where things will appear.
    pub fn description(&self, empty: &str) -> String {
        match *self {
            LibraryStatus::Scanning { total: 0, .. } => "Looking for music files…".to_owned(),
            LibraryStatus::Scanning { processed, total } => format!(
                "{} of {} files",
                format_count(processed as usize),
                format_count(total as usize)
            ),
            LibraryStatus::FolderMissing => {
                "Connect the drive it is on, or choose another folder in Preferences.".to_owned()
            }
            LibraryStatus::Empty => empty.to_owned(),
        }
    }

    /// Whether to show a spinner instead of an icon.
    pub fn busy(&self) -> bool {
        matches!(self, LibraryStatus::Scanning { .. })
    }
}

/// Calls `update` whenever the library's scanning state changes, until `widget` is
/// destroyed.
pub fn follow_library_status(
    ctx: &Rc<AppContext>,
    widget: &impl IsA<gtk::Widget>,
    update: impl Fn() + 'static,
) {
    let service = &ctx.library_service;
    let update = Rc::new(update);
    let notify = move |_: &LibraryService| update();
    for handler in [
        service.connect_scanning_notify(notify.clone()),
        service.connect_scan_processed_notify(notify.clone()),
        service.connect_scan_total_notify(notify.clone()),
        service.connect_folder_missing_notify(notify),
    ] {
        disconnect_on_destroy(widget, service, handler);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_each_status() {
        let counting = LibraryStatus::Scanning {
            processed: 0,
            total: 0,
        };
        assert_eq!(counting.title("No Albums"), "Scanning Your Music");
        assert_eq!(counting.description(""), "Looking for music files…");
        assert!(counting.busy());

        let scanning = LibraryStatus::Scanning {
            processed: 1200,
            total: 5000,
        };
        assert_eq!(scanning.description(""), "1,200 of 5,000 files");

        assert_eq!(LibraryStatus::Empty.title("No Albums"), "No Albums");
        assert_eq!(LibraryStatus::Empty.description("Soon."), "Soon.");
        assert!(!LibraryStatus::FolderMissing.busy());
    }
}
