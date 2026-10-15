//! The playlist entries whose file is gone, listed under a playlist's songs.

use std::cell::{Cell, OnceCell, RefCell};
use std::path::Path;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;
use mp_core::{PlaylistEntry, PlaylistItemId};

use super::missing_row::MissingRow;
use crate::ui::icons;
use crate::util::{count_label, display_name};

/// Runs with the entries the user chose to remove.
type RemoveHandler = Box<dyn Fn(Vec<PlaylistItemId>)>;

/// Rows shown before the list starts scrolling.
const VISIBLE_ROWS: i32 = 3;
const ROW_HEIGHT: i32 = 52;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct MissingEntries {
        pub(super) heading: gtk::Label,
        pub(super) toggle: gtk::ToggleButton,
        pub(super) revealer: gtk::Revealer,
        pub(super) scrolled: gtk::ScrolledWindow,
        pub(super) standalone: Cell<bool>,
        /// The missing files' paths, one per row.
        pub(super) paths: gtk::StringList,
        /// The playlist entry of each row.
        pub(super) items: RefCell<Vec<PlaylistItemId>>,
        pub(super) on_remove: OnceCell<RemoveHandler>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MissingEntries {
        const NAME: &'static str = "MpMissingEntries";
        type Type = super::MissingEntries;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for MissingEntries {}
    impl WidgetImpl for MissingEntries {}
    impl BinImpl for MissingEntries {}
}

glib::wrapper! {
    /// A collapsible card (`mp-missing-entries`) saying how many files are missing,
    /// with a button to remove them all and, expanded, one dimmed row per file.
    /// Hidden while nothing is missing. See also [`MissingEntries::set_standalone`].
    pub struct MissingEntries(ObjectSubclass<imp::MissingEntries>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl MissingEntries {
    pub fn new(on_remove: impl Fn(Vec<PlaylistItemId>) + 'static) -> Self {
        let widget: Self = glib::Object::new();
        widget.add_css_class("mp-missing-entries");
        widget.set_visible(false);
        let imp = widget.imp();
        assert!(
            imp.on_remove.set(Box::new(on_remove)).is_ok(),
            "a new widget has no handler yet"
        );

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&widget.build_header());
        imp.revealer.set_child(Some(&widget.build_list()));
        imp.revealer
            .set_transition_type(gtk::RevealerTransitionType::SlideDown);
        content.append(&imp.revealer);
        imp.toggle
            .bind_property("active", &imp.revealer, "reveal-child")
            .sync_create()
            .build();
        widget.set_child(Some(&content));
        widget
    }

    /// Shows `entries`: the playlist entries whose file is missing.
    pub fn set_entries(&self, entries: &[&PlaylistEntry]) {
        let imp = self.imp();
        let items: Vec<PlaylistItemId> = entries.iter().map(|entry| entry.item).collect();
        if *imp.items.borrow() == items {
            return;
        }
        let paths: Vec<String> = entries
            .iter()
            .map(|entry| entry.path.to_string_lossy().into_owned())
            .collect();
        let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
        imp.paths.splice(0, imp.paths.n_items(), &paths);
        imp.heading.set_label(&count_label(
            items.len(),
            "file is missing",
            "files are missing",
        ));
        self.set_visible(!items.is_empty());
        imp.items.replace(items);
    }

    /// Whether the card stands in for the playlist's songs because none of them are
    /// available. It then lists every missing file, without collapsing or a height
    /// limit.
    pub fn set_standalone(&self, standalone: bool) {
        let imp = self.imp();
        if imp.standalone.replace(standalone) == standalone {
            return;
        }
        imp.toggle.set_active(standalone);
        imp.toggle.set_visible(!standalone);
        let max_height = if standalone {
            -1
        } else {
            VISIBLE_ROWS * ROW_HEIGHT
        };
        imp.scrolled.set_max_content_height(max_height);
    }

    fn build_header(&self) -> gtk::Widget {
        let imp = self.imp();
        let icon = gtk::Image::builder()
            .icon_name(icons::UNAVAILABLE)
            .valign(gtk::Align::Center)
            .css_classes(["mp-missing-icon"])
            .build();
        imp.heading.set_xalign(0.0);
        imp.heading.set_wrap(true);
        imp.heading.add_css_class("heading");
        let hint = gtk::Label::builder()
            .label("They may be on a drive that isn’t connected, or were moved or deleted.")
            .xalign(0.0)
            .wrap(true)
            .css_classes(["caption", "dim-label"])
            .build();
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .valign(gtk::Align::Center)
            .spacing(2)
            .hexpand(true)
            .build();
        text.append(&imp.heading);
        text.append(&hint);

        let remove_all = gtk::Button::builder()
            .label("Remove All")
            .valign(gtk::Align::Center)
            .tooltip_text("Remove the missing files from this playlist")
            .css_classes(["mp-missing-remove-all"])
            .build();
        let weak = self.downgrade();
        remove_all.connect_clicked(move |_| {
            if let Some(widget) = weak.upgrade() {
                let items = widget.imp().items.borrow().clone();
                widget.remove(items);
            }
        });

        imp.toggle.set_icon_name(icons::EXPAND);
        imp.toggle.set_valign(gtk::Align::Center);
        imp.toggle
            .set_css_classes(&["flat", "circular", "mp-missing-toggle"]);
        imp.toggle.connect_active_notify(|toggle| {
            let text = if toggle.is_active() {
                "Hide Missing Files"
            } else {
                "Show Missing Files"
            };
            toggle.set_tooltip_text(Some(text));
            toggle.update_property(&[gtk::accessible::Property::Label(text)]);
        });
        imp.toggle.notify("active");

        let header = gtk::Box::builder()
            .spacing(12)
            .css_classes(["mp-missing-header"])
            .build();
        header.append(&icon);
        header.append(&text);
        header.append(&remove_all);
        header.append(&imp.toggle);
        header.upcast()
    }

    /// One recycled row per missing file.
    fn build_list(&self) -> gtk::Widget {
        let factory = gtk::SignalListItemFactory::new();
        let weak = self.downgrade();
        factory.connect_setup(move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            item.set_activatable(false);
            let row = MissingRow::default();
            let list_item = item.downgrade();
            let widget = weak.clone();
            row.remove_button().connect_clicked(move |_| {
                let (Some(widget), Some(item)) = (widget.upgrade(), list_item.upgrade()) else {
                    return;
                };
                let entry = widget
                    .imp()
                    .items
                    .borrow()
                    .get(item.position() as usize)
                    .copied();
                if let Some(entry) = entry {
                    widget.remove(vec![entry]);
                }
            });
            item.set_child(Some(&row));
        });
        factory.connect_bind(|_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let path = item
                .item()
                .and_downcast::<gtk::StringObject>()
                .map(|path| path.string());
            let row = item.child().and_downcast::<MissingRow>();
            if let (Some(row), Some(path)) = (row, path) {
                let path = Path::new(path.as_str());
                row.set_path(path);
                item.set_accessible_label(&format!("{}, file missing", display_name(path)));
            }
        });

        let list = gtk::ListView::builder()
            .model(&gtk::NoSelection::new(Some(self.imp().paths.clone())))
            .factory(&factory)
            .tab_behavior(gtk::ListTabBehavior::Item)
            .css_classes(["mp-missing-list"])
            .build();
        list.update_property(&[gtk::accessible::Property::Label("Missing files")]);
        let scrolled = &self.imp().scrolled;
        scrolled.set_hscrollbar_policy(gtk::PolicyType::Never);
        scrolled.set_propagate_natural_height(true);
        scrolled.set_max_content_height(VISIBLE_ROWS * ROW_HEIGHT);
        scrolled.set_child(Some(&list));
        scrolled.clone().upcast()
    }

    fn remove(&self, items: Vec<PlaylistItemId>) {
        if let Some(handler) = self.imp().on_remove.get()
            && !items.is_empty()
        {
            handler(items);
        }
    }
}
