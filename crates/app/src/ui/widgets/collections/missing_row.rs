//! One row of [`super::MissingEntries`]: a playlist entry whose file is gone.

use std::path::Path;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;

use crate::ui::{icons, set_tooltip};
use crate::util::display_name;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct MissingRow {
        pub(super) name: gtk::Label,
        pub(super) folder: gtk::Label,
        pub(super) remove: gtk::Button,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MissingRow {
        const NAME: &'static str = "MpMissingRow";
        type Type = super::MissingRow;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for MissingRow {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.add_css_class("mp-missing-row");
            for (label, classes) in [
                (&self.name, &["mp-missing-name"][..]),
                (&self.folder, &["caption", "dim-label"]),
            ] {
                label.set_xalign(0.0);
                label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                label.set_single_line_mode(true);
                label.set_css_classes(classes);
            }
            self.remove.set_icon_name(icons::REMOVE);
            self.remove.set_valign(gtk::Align::Center);
            self.remove.set_css_classes(&["flat", "circular"]);
            set_tooltip(&self.remove, "Remove from Playlist");

            let text = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .valign(gtk::Align::Center)
                .spacing(2)
                .hexpand(true)
                .build();
            text.append(&self.name);
            text.append(&self.folder);
            let layout = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            layout.append(&text);
            layout.append(&self.remove);
            obj.set_child(Some(&layout));
        }
    }

    impl WidgetImpl for MissingRow {}
    impl BinImpl for MissingRow {}
}

glib::wrapper! {
    /// The file name over "File missing · folder", and a remove button.
    pub struct MissingRow(ObjectSubclass<imp::MissingRow>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for MissingRow {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl MissingRow {
    pub fn set_path(&self, path: &Path) {
        let imp = self.imp();
        imp.name.set_label(&display_name(path));
        let folder = path
            .parent()
            .map_or_else(String::new, |parent| parent.display().to_string());
        imp.folder.set_label(&format!("File missing · {folder}"));
        self.set_tooltip_text(Some(&path.display().to_string()));
    }

    pub fn remove_button(&self) -> &gtk::Button {
        &self.imp().remove
    }
}
