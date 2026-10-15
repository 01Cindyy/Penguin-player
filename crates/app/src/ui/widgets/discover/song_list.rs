//! A short list of songs, such as a search's best matches, that sits inside a page
//! instead of scrolling on its own. It behaves like a track list: activating a row
//! plays, and right-click, long press, the Menu key or Shift+F10 open the track menu.
//!
//! Every row is a real widget, so keep it to a handful of songs; use
//! [`crate::ui::widgets::TrackList`] for anything longer.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, gio, glib, graphene};
use mp_core::util::format_duration;

use crate::context::AppContext;
use crate::model::TrackObject;
use crate::services::covers::CoverCache;
use crate::ui::track_actions::{self, MenuSongs};
use crate::ui::widgets::{FormatBadge, SongHeading};
use crate::ui::{context_menu, disconnect_on_destroy, ellipsized_label, set_css_class};

type ActivateHandler = Box<dyn Fn(&TrackObject)>;

mod imp {
    use super::*;

    pub struct SongList {
        pub(super) ctx: OnceCell<Rc<AppContext>>,
        pub(super) model: OnceCell<gio::ListModel>,
        pub(super) rows: gtk::ListBox,
        pub(super) popover: gtk::PopoverMenu,
        /// The row the context menu was opened for.
        pub(super) menu_position: Cell<Option<u32>>,
        pub(super) on_activate: RefCell<Option<ActivateHandler>>,
    }

    impl Default for SongList {
        fn default() -> Self {
            Self {
                ctx: OnceCell::new(),
                model: OnceCell::new(),
                rows: gtk::ListBox::new(),
                popover: gtk::PopoverMenu::from_model(None::<&gio::MenuModel>),
                menu_position: Cell::default(),
                on_activate: RefCell::default(),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SongList {
        const NAME: &'static str = "MpSongList";
        type Type = super::SongList;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for SongList {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.add_css_class("mp-track-list");
            obj.add_css_class("mp-song-list");
            self.rows.set_selection_mode(gtk::SelectionMode::None);
            self.rows.add_css_class("mp-song-rows");
            self.rows
                .update_property(&[gtk::accessible::Property::Label("Songs")]);
            let weak = obj.downgrade();
            self.rows.connect_row_activated(move |_, row| {
                if let Some(list) = weak.upgrade() {
                    list.activate_row(row.index() as u32);
                }
            });
            obj.set_child(Some(&self.rows));

            self.popover.set_has_arrow(false);
            self.popover.set_halign(gtk::Align::Start);
            self.popover.set_parent(&*obj);
            let weak = obj.downgrade();
            context_menu::connect_keys(&*obj, move || {
                if let Some(list) = weak.upgrade() {
                    list.open_menu_for_focus();
                }
            });
        }

        fn dispose(&self) {
            self.popover.unparent();
        }
    }

    impl WidgetImpl for SongList {
        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.parent_size_allocate(width, height, baseline);
            self.popover.present();
        }
    }

    impl BinImpl for SongList {}
}

glib::wrapper! {
    /// See the [module documentation](self). Styled as `mp-track-list mp-song-list`;
    /// rows are `mp-song-row` and gain `mp-playing` for the current track.
    pub struct SongList(ObjectSubclass<imp::SongList>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl SongList {
    /// A list of the [`TrackObject`]s in `model`. Activating a row does nothing until
    /// [`Self::set_on_activate`] says what.
    pub fn new(ctx: &Rc<AppContext>, model: &impl IsA<gio::ListModel>) -> Self {
        let list: Self = glib::Object::new();
        let imp = list.imp();
        assert!(
            imp.ctx.set(ctx.clone()).is_ok(),
            "a new list has no context"
        );
        assert!(
            imp.model.set(model.clone().upcast()).is_ok(),
            "a new list has no model"
        );

        list.install_menu_actions(ctx);

        let covers = ctx.covers.clone();
        let weak = list.downgrade();
        imp.rows.bind_model(Some(model), move |item| {
            let track = item
                .downcast_ref::<TrackObject>()
                .expect("song lists only hold tracks");
            let row = song_row(track, &covers);
            if let Some(list) = weak.upgrade() {
                list.attach_menu_gestures(&row);
            }
            row.upcast()
        });
        list
    }

    /// What activating a row does; `handler` receives the row's song.
    pub fn set_on_activate(&self, handler: impl Fn(&TrackObject) + 'static) {
        self.imp().on_activate.replace(Some(Box::new(handler)));
    }

    fn ctx(&self) -> &Rc<AppContext> {
        self.imp()
            .ctx
            .get()
            .expect("song lists are created with SongList::new")
    }

    /// The `tracks.*` actions of the context menu, which act on the row it was opened
    /// on. "Play" activates that row.
    fn install_menu_actions(&self, ctx: &Rc<AppContext>) {
        let list = self.downgrade();
        let target = move || {
            let track = list.upgrade()?.menu_track()?;
            Some(MenuSongs {
                songs: vec![track.clone()],
                opened_on: track,
            })
        };
        let list = self.downgrade();
        let play = move || {
            if let Some(list) = list.upgrade()
                && let Some(position) = list.imp().menu_position.get()
            {
                list.activate_row(position);
            }
        };
        track_actions::install_actions(self, ctx, target, play);
    }

    /// The song the context menu was opened on.
    fn menu_track(&self) -> Option<TrackObject> {
        let imp = self.imp();
        let position = imp.menu_position.get()?;
        imp.model.get()?.item(position).and_downcast()
    }

    fn activate_row(&self, position: u32) {
        let imp = self.imp();
        let track = imp
            .model
            .get()
            .and_then(|model| model.item(position))
            .and_downcast::<TrackObject>();
        if let (Some(handler), Some(track)) = (imp.on_activate.borrow().as_ref(), track) {
            handler(&track);
        }
    }

    fn attach_menu_gestures(&self, row: &gtk::ListBoxRow) {
        let list = self.downgrade();
        context_menu::connect_press(row, move |row, x, y| {
            let Some(list) = list.upgrade() else { return };
            let point = row
                .compute_point(&list, &graphene::Point::new(x as f32, y as f32))
                .unwrap_or_else(|| graphene::Point::new(0.0, 0.0));
            if let Some(row) = row.downcast_ref::<gtk::ListBoxRow>() {
                list.open_menu(row, point.x() as i32, point.y() as i32);
            }
        });
    }

    /// Keyboard (Menu key, Shift+F10): opens the menu under the focused row.
    fn open_menu_for_focus(&self) {
        let Some(row) = self
            .imp()
            .rows
            .focus_child()
            .and_downcast::<gtk::ListBoxRow>()
        else {
            return;
        };
        if let Some(bounds) = row.compute_bounds(self) {
            let (x, y) = (bounds.x() + 24.0, bounds.y() + bounds.height());
            self.open_menu(&row, x as i32, y as i32);
        }
    }

    fn open_menu(&self, row: &gtk::ListBoxRow, x: i32, y: i32) {
        let imp = self.imp();
        imp.menu_position.set(Some(row.index() as u32));
        imp.popover
            .set_menu_model(Some(&track_actions::context_menu(self.ctx())));
        imp.popover
            .set_pointing_to(Some(&gdk::Rectangle::new(x, y, 1, 1)));
        imp.popover.popup();
    }
}

/// A row: a [`SongHeading`], the format badge and the duration.
fn song_row(track: &TrackObject, covers: &CoverCache) -> gtk::ListBoxRow {
    let heading = SongHeading::new(Some(covers));
    heading.set_track(track);
    let badge = FormatBadge::new();
    badge.set_track(Some(track.track()));
    let duration = ellipsized_label(&["mp-track-duration", "numeric", "dim-label"]);
    duration.set_label(&format_duration(track.track().duration()));
    duration.set_xalign(1.0);

    let content = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    content.append(&heading.root);
    content.append(&badge);
    content.append(&duration);
    let row = gtk::ListBoxRow::builder()
        .child(&content)
        .css_classes(["mp-song-row"])
        .build();
    row.update_property(&[gtk::accessible::Property::Label(&format!(
        "{} by {}",
        track.title(),
        track.artist()
    ))]);

    let weak_row = row.downgrade();
    let show_playing = move |track: &TrackObject| {
        if let Some(row) = weak_row.upgrade() {
            set_css_class(&row, "mp-playing", track.playing());
        }
        heading.set_playing(track.playing());
    };
    show_playing(track);
    let handler = track.connect_playing_notify(show_playing);
    disconnect_on_destroy(&row, track, handler);
    row
}
