//! The header at the top of a page: an optional cover, a large title, a subtitle
//! and action buttons such as Play and Shuffle.

use std::cell::Cell;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;

use crate::ui::{icons, set_css_class};

mod imp {
    use super::*;

    #[derive(glib::Properties)]
    #[properties(wrapper_type = super::PageHeader)]
    pub struct PageHeader {
        /// Leading image, text column and (in wide compact headers) the actions.
        pub(super) layout: gtk::Box,
        pub(super) leading: adw::Bin,
        pub(super) text: gtk::Box,
        pub(super) eyebrow: gtk::Label,
        pub(super) title: gtk::Label,
        pub(super) subtitle: gtk::Label,
        /// Extra rows under the subtitle (see [`super::PageHeader::add_detail`]).
        pub(super) details: gtk::Box,
        pub(super) actions: gtk::Box,
        pub(super) extra: gtk::Box,
        /// Stacks the contents for narrow pages: actions under the title, the leading
        /// image above it. [`crate::ui::pages::page_with_header`] sets this.
        #[property(get, set = Self::set_narrow, explicit_notify)]
        pub(super) narrow: Cell<bool>,
    }

    impl PageHeader {
        fn set_narrow(&self, narrow: bool) {
            if self.narrow.replace(narrow) != narrow {
                let obj = self.obj();
                set_css_class(&*obj, "mp-narrow", narrow);
                obj.update_layout();
                obj.notify_narrow();
            }
        }
    }

    impl Default for PageHeader {
        fn default() -> Self {
            let label = |classes: &[&str]| {
                gtk::Label::builder()
                    .xalign(0.0)
                    .wrap(true)
                    .wrap_mode(gtk::pango::WrapMode::WordChar)
                    .css_classes(classes)
                    .build()
            };
            Self {
                layout: gtk::Box::new(gtk::Orientation::Horizontal, 24),
                leading: adw::Bin::new(),
                text: gtk::Box::new(gtk::Orientation::Vertical, 6),
                eyebrow: label(&["mp-page-eyebrow", "caption-heading", "dim-label"]),
                title: label(&["mp-page-title", "title-1"]),
                subtitle: label(&["mp-page-subtitle", "dim-label"]),
                details: gtk::Box::new(gtk::Orientation::Vertical, 6),
                actions: gtk::Box::new(gtk::Orientation::Horizontal, 12),
                extra: gtk::Box::new(gtk::Orientation::Vertical, 0),
                narrow: Cell::new(false),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PageHeader {
        const NAME: &'static str = "MpPageHeader";
        type Type = super::PageHeader;
        type ParentType = adw::Bin;
    }

    #[glib::derived_properties]
    impl ObjectImpl for PageHeader {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.add_css_class("mp-page-header");

            self.leading.set_visible(false);
            self.eyebrow.set_visible(false);
            self.subtitle.set_visible(false);
            self.details.set_visible(false);
            self.actions.add_css_class("mp-page-actions");
            self.actions.set_visible(false);

            self.text.set_valign(gtk::Align::End);
            self.text.set_hexpand(true);
            self.text.append(&self.eyebrow);
            self.text.append(&self.title);
            self.text.append(&self.subtitle);
            self.text.append(&self.details);

            self.layout.append(&self.leading);
            self.layout.append(&self.text);

            let column = gtk::Box::new(gtk::Orientation::Vertical, 18);
            column.append(&self.layout);
            column.append(&self.extra);
            self.extra.set_visible(false);
            obj.set_child(Some(&column));
            obj.update_layout();
        }
    }

    impl WidgetImpl for PageHeader {}
    impl BinImpl for PageHeader {}
}

glib::wrapper! {
    /// Page header (`mp-page-header`, with `mp-page-title` and `mp-page-subtitle`).
    ///
    /// Without a leading image the header is compact and its action buttons sit at the
    /// end of the title row. With one (album, artist and playlist pages) it becomes a
    /// hero header with the buttons under the title. Narrow headers (see the `narrow`
    /// property) put the buttons under the title and the image above it, and gain the
    /// `mp-narrow` class.
    pub struct PageHeader(ObjectSubclass<imp::PageHeader>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl PageHeader {
    pub fn new(title: &str) -> Self {
        let header: Self = glib::Object::new();
        header.set_title(title);
        header
    }

    pub fn set_title(&self, title: &str) {
        self.imp().title.set_label(title);
    }

    /// A small label above the title, e.g. "Album" or "Playlist".
    pub fn set_eyebrow(&self, text: &str) {
        let label = &self.imp().eyebrow;
        label.set_label(text);
        label.set_visible(!text.is_empty());
    }

    /// Secondary line under the title, e.g. "12 songs · 48 min". Empty hides it.
    pub fn set_subtitle(&self, subtitle: &str) {
        let label = &self.imp().subtitle;
        label.set_label(subtitle);
        label.set_visible(!subtitle.is_empty());
    }

    /// A widget left of the title, usually a large cover or artist image. Setting one
    /// turns the header into a hero header (see the type documentation).
    pub fn set_leading(&self, widget: Option<&impl IsA<gtk::Widget>>) {
        let leading = &self.imp().leading;
        leading.set_child(widget);
        leading.set_visible(widget.is_some());
        self.update_layout();
    }

    /// Adds a pill button. `primary` marks the main action (Play).
    pub fn add_button(&self, icon_name: &str, label: &str, primary: bool) -> gtk::Button {
        let content = adw::ButtonContent::builder()
            .icon_name(icon_name)
            .label(label)
            .use_underline(true)
            .build();
        let button = gtk::Button::builder()
            .child(&content)
            .css_classes(["pill", "mp-page-action"])
            .build();
        if primary {
            button.add_css_class("suggested-action");
        }
        let actions = &self.imp().actions;
        actions.append(&button);
        actions.set_visible(true);
        button
    }

    /// Adds the round "More Options" button (`mp-page-menu-button`) after the action
    /// buttons. The caller gives it its menu.
    pub fn add_menu_button(&self) -> gtk::MenuButton {
        let button = gtk::MenuButton::builder()
            .icon_name(icons::MORE)
            .tooltip_text("More Options")
            .valign(gtk::Align::Center)
            .css_classes(["mp-page-menu-button"])
            .build();
        button.update_property(&[gtk::accessible::Property::Label("More Options")]);
        self.add_action_widget(&button);
        button
    }

    /// Adds any widget, such as a sort button, after the action buttons.
    pub fn add_action_widget(&self, widget: &impl IsA<gtk::Widget>) {
        let actions = &self.imp().actions;
        actions.append(widget);
        actions.set_visible(true);
    }

    /// Adds a row under the subtitle, e.g. a link to the artist or quality badges.
    pub fn add_detail(&self, widget: &impl IsA<gtk::Widget>) {
        let details = &self.imp().details;
        details.append(widget);
        details.set_visible(true);
    }

    /// A full-width row under the header, e.g. filter chips. The row is hidden, gap
    /// included, while `widget` is hidden.
    pub fn set_extra(&self, widget: &impl IsA<gtk::Widget>) {
        let extra = &self.imp().extra;
        while let Some(child) = extra.first_child() {
            extra.remove(&child);
        }
        extra.append(widget);
        widget
            .bind_property("visible", extra, "visible")
            .sync_create()
            .build();
    }

    /// Arranges the image, text and actions for the header's kind and width.
    fn update_layout(&self) {
        let imp = self.imp();
        let hero = imp.leading.is_visible();
        let narrow = imp.narrow.get();

        let (orientation, spacing) = if hero && narrow {
            (gtk::Orientation::Vertical, 18)
        } else {
            (gtk::Orientation::Horizontal, 24)
        };
        imp.layout.set_orientation(orientation);
        imp.layout.set_spacing(spacing);
        imp.leading.set_halign(gtk::Align::Start);
        imp.leading.set_valign(if hero && !narrow {
            gtk::Align::End
        } else {
            gtk::Align::Start
        });

        let under_title = hero || narrow;
        let target = if under_title { &imp.text } else { &imp.layout };
        let current = imp.actions.parent().and_downcast::<gtk::Box>();
        if current.as_ref() != Some(target) {
            if let Some(current) = current {
                current.remove(&imp.actions);
            }
            target.append(&imp.actions);
        }
        imp.actions.set_margin_top(if under_title { 12 } else { 0 });
        imp.actions.set_valign(if under_title {
            gtk::Align::Start
        } else {
            gtk::Align::Center
        });
    }
}
