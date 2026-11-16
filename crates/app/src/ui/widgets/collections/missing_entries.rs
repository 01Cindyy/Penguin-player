//! The entries of a playlist, or the favourites, whose file is gone, listed under the
//! songs.

use std::cell::{Cell, OnceCell, RefCell};
use std::path::{Path, PathBuf};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;

use super::missing_row::MissingRow;
use crate::ui::icons;
use crate::util::{count_label, display_name};

/// Runs with the positions, among the entries last set, of those the user chose to
/// remove.
type RemoveHandler = Box<dyn Fn(Vec<usize>)>;

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
        /// The same, to tell whether new entries change anything.
        pub(super) shown: RefCell<Vec<PathBuf>>,
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
    /// `remove_all` is the tooltip of the button that removes every entry.
    pub fn new(remove_all: &str, on_remove: impl Fn(Vec<usize>) + 'static) -> Self {
        let widget: Self = glib::Object::new();
        widget.add_css_class("mp-missing-entries");
        widget.set_visible(false);
        let imp = widget.imp();
        assert!(
            imp.on_remove.set(Box::new(on_remove)).is_ok(),
            "a new widget has no handler yet"
        );

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&widget.build_header(remove_all));
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

    /// Shows the entries whose file is missing, by their paths.
    pub fn set_entries(&self, paths: &[&Path]) {
        let imp = self.imp();
        if imp.shown.borrow().iter().eq(paths.iter().copied()) {
            return;
        }
        let texts: Vec<String> = paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
        imp.paths.splice(0, imp.paths.n_items(), &texts);
        imp.heading.set_label(&count_label(
            paths.len(),
            "file is missing",
            "files are missing",
        ));
        self.set_visible(!paths.is_empty());
        imp.shown
            .replace(paths.iter().map(|path| path.to_path_buf()).collect());
    }

    /// Whether the card stands in for the songs because none of them are available. It then lists every missing file, without collapsing or a height
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

    fn build_header(&self, remove_all_tooltip: &str) -> gtk::Widget {
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
            .tooltip_text(remove_all_tooltip)
            .css_classes(["mp-missing-remove-all"])
            .build();
        let weak = self.downgrade();
        remove_all.connect_clicked(move |_| {
            if let Some(widget) = weak.upgrade() {
                let count = widget.imp().shown.borrow().len();
                widget.remove((0..count).collect());
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
                let position = item.position() as usize;
                if position < widget.imp().shown.borrow().len() {
                    widget.remove(vec![position]);
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

    fn remove(&self, positions: Vec<usize>) {
        if let Some(handler) = self.imp().on_remove.get()
            && !positions.is_empty()
        {
            handler(positions);
        }
    }
}
