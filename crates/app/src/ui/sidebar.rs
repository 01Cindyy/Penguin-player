//! The navigation sidebar: library sections, playlists and the library status.
//!
//! Dragged narrow, it shows only the icons, with the names in tooltips.

use std::cell::Cell;
use std::rc::{Rc, Weak};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, gio, glib, graphene};
use mp_core::PlaylistId;

use crate::context::AppContext;
use crate::model::PlaylistObject;
use crate::ui::icons;
use crate::ui::navigation::Route;
use crate::ui::widgets::collections::playlist_actions;
use crate::ui::widgets::track_list::TrackDrag;
use crate::ui::{context_menu, dialogs, icon_button, set_css_class, set_tooltip, track_actions};
use crate::util::{format_count, song_count, total_length};

/// Library sections in sidebar order. The first four show how many there are.
const SECTIONS: [(Route, &str, &str); 5] = [
    (Route::Songs, "Songs", icons::SONG),
    (Route::Albums, "Albums", icons::ALBUM),
    (Route::Artists, "Artists", icons::ARTIST),
    (Route::Formats, "Formats", icons::FORMATS),
    (Route::NowPlaying, "Now Playing", icons::HEADPHONES),
];

mod imp {
    use super::*;

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::Sidebar)]
    pub struct Sidebar {
        /// Shows only icons (`mp-icons-only`), for a sidebar dragged narrow.
        #[property(get, set = Self::set_icons_only, explicit_notify)]
        icons_only: Cell<bool>,
    }

    impl Sidebar {
        fn set_icons_only(&self, icons_only: bool) {
            if self.icons_only.replace(icons_only) != icons_only {
                set_css_class(&*self.obj(), "mp-icons-only", icons_only);
                self.obj().notify_icons_only();
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Sidebar {
        const NAME: &'static str = "MpSidebar";
        type Type = super::Sidebar;
        type ParentType = adw::Bin;
    }

    #[glib::derived_properties]
    impl ObjectImpl for Sidebar {}
    impl WidgetImpl for Sidebar {}
    impl BinImpl for Sidebar {}
}

glib::wrapper! {
    /// The navigation sidebar (`mp-sidebar`).
    pub struct Sidebar(ObjectSubclass<imp::Sidebar>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sidebar {
    pub fn new(ctx: &Rc<AppContext>) -> Self {
        let sidebar: Self = glib::Object::new();
        sidebar.add_css_class("mp-sidebar");
        let (library, counts) = library_list(ctx, &sidebar);
        let playlists = playlist_list(ctx, &sidebar);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&library);
        content.append(&section_header(ctx, &sidebar));
        content.append(&playlists);
        content.append(&playlist_hint(ctx, &sidebar));
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&content)
            .build();

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&scrolled);
        root.append(&footer(ctx, &sidebar));
        sidebar.set_child(Some(&root));

        connect_selection(ctx, &library, &playlists);
        keep_counts_current(ctx, counts);
        sidebar
    }
}

/// Shows `widget` only while the sidebar shows text.
fn hide_when_icons_only(sidebar: &Sidebar, widget: &impl IsA<gtk::Widget>) {
    sidebar
        .bind_property("icons-only", widget, "visible")
        .invert_boolean()
        .sync_create()
        .build();
}

/// Centres `widget` while the sidebar shows only icons.
fn centre_when_icons_only(sidebar: &Sidebar, widget: &impl IsA<gtk::Widget>) {
    sidebar
        .bind_property("icons-only", widget, "halign")
        .transform_to(|_, icons_only: bool| {
            Some(if icons_only {
                gtk::Align::Center
            } else {
                gtk::Align::Fill
            })
        })
        .sync_create()
        .build();
}

/// Songs, Albums, Artists and Formats, each with a live count, and Now Playing.
fn library_list(ctx: &Rc<AppContext>, sidebar: &Sidebar) -> (gtk::ListBox, [gtk::Label; 5]) {
    let list = gtk::ListBox::builder()
        .css_classes(["navigation-sidebar", "mp-sidebar-library"])
        .build();
    list.update_property(&[gtk::accessible::Property::Label("Library")]);
    let counts = SECTIONS.map(|(_, title, icon)| {
        let (row, _, count) = sidebar_row(sidebar, title, icon);
        list.append(&row);
        count
    });
    list.connect_row_activated(glib::clone!(
        #[weak]
        ctx,
        move |_, row| {
            if let Some((route, ..)) = SECTIONS.get(row.index() as usize) {
                ctx.navigator.navigate(route.clone());
            }
        }
    ));
    (list, counts)
}

/// User playlists, bound live to the store. Each row has a context menu (right
/// click, long press, or the Menu key) and takes songs dropped onto it.
fn playlist_list(ctx: &Rc<AppContext>, sidebar: &Sidebar) -> gtk::ListBox {
    let list = gtk::ListBox::builder()
        .css_classes(["navigation-sidebar", "mp-sidebar-playlists"])
        .build();
    list.update_property(&[gtk::accessible::Property::Label("Playlists")]);
    list.insert_action_group(
        playlist_actions::GROUP,
        Some(&playlist_actions::action_group(ctx)),
    );
    let menu = playlist_menu(&list);

    let weak = Rc::downgrade(ctx);
    let popover = menu.downgrade();
    let sidebar = sidebar.downgrade();
    list.bind_model(Some(&ctx.store.playlists()), move |item| {
        let playlist = item
            .downcast_ref::<PlaylistObject>()
            .expect("the playlist model only holds playlists");
        let sidebar = sidebar
            .upgrade()
            .expect("rows are made while the sidebar exists");
        let row = playlist_row(&sidebar, playlist);
        open_menu_on_press(&row, &weak, &popover);
        row.add_controller(add_songs_target(&weak, playlist.playlist_id()));
        row.upcast()
    });
    list.connect_row_activated(glib::clone!(
        #[weak]
        ctx,
        move |_, row| {
            if let Some(playlist) = playlist_of(&ctx, row) {
                ctx.navigator
                    .navigate(Route::Playlist(playlist.playlist_id()));
            }
        }
    ));
    open_menu_on_keys(&list, ctx, &menu);
    keep_open_playlist_selected(ctx, &list);
    list
}

/// The playlist a row of the playlist list shows.
fn playlist_of(ctx: &AppContext, row: &gtk::ListBoxRow) -> Option<PlaylistObject> {
    let index = u32::try_from(row.index()).ok()?;
    ctx.store.playlists().item(index).and_downcast()
}

/// The context menu shared by the playlist rows, filled in when it opens.
fn playlist_menu(list: &gtk::ListBox) -> gtk::PopoverMenu {
    let popover = gtk::PopoverMenu::from_model(None::<&gio::MenuModel>);
    popover.set_has_arrow(false);
    popover.set_halign(gtk::Align::Start);
    popover.set_parent(list);
    list.connect_destroy(glib::clone!(
        #[weak]
        popover,
        move |_| popover.unparent()
    ));
    popover
}

/// Opens the menu for `row`, pointing at `point` in the row (below its start when
/// opened from the keyboard).
fn show_playlist_menu(
    ctx: &AppContext,
    popover: &gtk::PopoverMenu,
    row: &gtk::ListBoxRow,
    point: Option<(f64, f64)>,
) {
    let (Some(playlist), Some(list)) = (playlist_of(ctx, row), popover.parent()) else {
        return;
    };
    let (x, y) = point.unwrap_or((24.0, f64::from(row.height())));
    let point = row
        .compute_point(&list, &graphene::Point::new(x as f32, y as f32))
        .unwrap_or_else(|| graphene::Point::new(0.0, 0.0));
    popover.set_menu_model(Some(&playlist_actions::menu(playlist.playlist_id(), true)));
    popover.set_pointing_to(Some(&gdk::Rectangle::new(
        point.x() as i32,
        point.y() as i32,
        1,
        1,
    )));
    popover.popup();
}

/// Right-clicking or long-pressing `row` opens its playlist's menu.
fn open_menu_on_press(
    row: &gtk::ListBoxRow,
    ctx: &Weak<AppContext>,
    popover: &glib::WeakRef<gtk::PopoverMenu>,
) {
    let (ctx, popover) = (ctx.clone(), popover.clone());
    context_menu::connect_press(row, move |row, x, y| {
        let row = row.downcast_ref::<gtk::ListBoxRow>();
        if let (Some(ctx), Some(popover), Some(row)) = (ctx.upgrade(), popover.upgrade(), row) {
            show_playlist_menu(&ctx, &popover, row, Some((x, y)));
        }
    });
}

/// The Menu key and Shift+F10 open the menu of the focused row's playlist.
fn open_menu_on_keys(list: &gtk::ListBox, ctx: &Rc<AppContext>, popover: &gtk::PopoverMenu) {
    let (ctx, popover, list_weak) = (Rc::downgrade(ctx), popover.downgrade(), list.downgrade());
    context_menu::connect_keys(list, move || {
        let row = list_weak
            .upgrade()
            .and_then(|list| list.focus_child())
            .and_downcast::<gtk::ListBoxRow>();
        if let (Some(ctx), Some(popover), Some(row)) = (ctx.upgrade(), popover.upgrade(), row) {
            show_playlist_menu(&ctx, &popover, &row, None);
        }
    });
}

/// Adds songs dragged from a track list to playlist `id`.
fn add_songs_target(ctx: &Weak<AppContext>, id: PlaylistId) -> gtk::DropTarget {
    let target = gtk::DropTarget::new(TrackDrag::static_type(), gdk::DragAction::COPY);
    let ctx = ctx.clone();
    target.connect_drop(move |_, value, _, _| {
        let (Some(ctx), Ok(TrackDrag(ids))) = (ctx.upgrade(), value.get::<TrackDrag>()) else {
            return false;
        };
        track_actions::add_to_playlist(&ctx, id, &ctx.store.tracks_for(&ids));
        true
    });
    target
}

/// Songs dropped on the "Playlists" heading (or the hint under it) go into a new
/// playlist, named in a dialog.
fn new_playlist_target(ctx: &Rc<AppContext>) -> gtk::DropTarget {
    let target = gtk::DropTarget::new(TrackDrag::static_type(), gdk::DragAction::COPY);
    let weak = Rc::downgrade(ctx);
    target.connect_drop(move |_, value, _, _| {
        let (Some(ctx), Ok(TrackDrag(ids))) = (weak.upgrade(), value.get::<TrackDrag>()) else {
            return false;
        };
        track_actions::add_to_new_playlist(&ctx, &ctx.store.tracks_for(&ids));
        true
    });
    target
}

/// Rows are rebuilt when playlists are added, removed or reordered by a rename,
/// which drops the selection: select the open playlist's row again.
fn keep_open_playlist_selected(ctx: &Rc<AppContext>, list: &gtk::ListBox) {
    let weak = Rc::downgrade(ctx);
    let list = list.downgrade();
    ctx.store
        .playlists()
        .connect_items_changed(move |model, _, _, _| {
            let (Some(ctx), Some(list)) = (weak.upgrade(), list.upgrade()) else {
                return;
            };
            if let Some(Route::Playlist(id)) = ctx.navigator.top_level_route() {
                let row = position_of_playlist(model, id)
                    .and_then(|index| list.row_at_index(index as i32));
                list.select_row(row.as_ref());
            }
        });
}

/// A sidebar row: icon, title and a dim count at the end; only the icon, with the title
/// as its tooltip, while the sidebar shows only icons.
fn sidebar_row(
    sidebar: &Sidebar,
    title: &str,
    icon_name: &str,
) -> (gtk::ListBoxRow, gtk::Label, gtk::Label) {
    let icon = gtk::Image::from_icon_name(icon_name);
    let label = gtk::Label::builder()
        .label(title)
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    let count = gtk::Label::builder()
        .xalign(1.0)
        .css_classes(["mp-sidebar-count", "numeric", "caption", "dim-label"])
        .build();
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    content.append(&icon);
    content.append(&label);
    content.append(&count);
    hide_when_icons_only(sidebar, &label);
    hide_when_icons_only(sidebar, &count);
    centre_when_icons_only(sidebar, &content);

    let row = gtk::ListBoxRow::builder()
        .child(&content)
        .has_tooltip(true)
        .build();
    row.update_relation(&[
        gtk::accessible::Relation::LabelledBy(&[label.upcast_ref()]),
        gtk::accessible::Relation::DescribedBy(&[count.upcast_ref()]),
    ]);
    let weak = sidebar.downgrade();
    let name = label.downgrade();
    row.connect_query_tooltip(move |_, _, _, _, tooltip| {
        let icons_only = weak.upgrade().is_some_and(|sidebar| sidebar.icons_only());
        if let (true, Some(name)) = (icons_only, name.upgrade()) {
            tooltip.set_text(Some(&name.label()));
        }
        icons_only
    });
    (row, label, count)
}

fn playlist_row(sidebar: &Sidebar, playlist: &PlaylistObject) -> gtk::ListBoxRow {
    let (row, label, count) = sidebar_row(sidebar, &playlist.name(), icons::MUSIC_FOLDER);
    playlist
        .bind_property("name", &label, "label")
        .sync_create()
        .build();
    playlist
        .bind_property("track-count", &count, "label")
        .transform_to(|_, count: u32| Some(format_count(count as usize)))
        .sync_create()
        .build();
    row
}

/// "Playlists" with a button to create one (just the button while showing icons only).
fn section_header(ctx: &Rc<AppContext>, sidebar: &Sidebar) -> gtk::Widget {
    let title = gtk::Label::builder()
        .label("Playlists")
        .xalign(0.0)
        .hexpand(true)
        .css_classes(["mp-sidebar-heading", "caption-heading", "dim-label"])
        .build();
    let add = icon_button(
        icons::ADD,
        "New Playlist",
        &["flat", "circular", "mp-sidebar-add"],
    );
    add.connect_clicked(glib::clone!(
        #[weak]
        ctx,
        move |_| {
            let weak = Rc::downgrade(&ctx);
            dialogs::new_playlist(&ctx, Vec::new(), move |id, _| {
                if let Some(ctx) = weak.upgrade() {
                    ctx.navigator.navigate(Route::Playlist(id));
                }
            });
        }
    ));
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    header.add_css_class("mp-sidebar-section");
    header.append(&title);
    header.append(&add);
    header.add_controller(new_playlist_target(ctx));
    hide_when_icons_only(sidebar, &title);
    centre_when_icons_only(sidebar, &header);
    header.upcast()
}

/// A gentle hint shown while there are no playlists (and there is room for it).
fn playlist_hint(ctx: &Rc<AppContext>, sidebar: &Sidebar) -> gtk::Widget {
    let hint = gtk::Label::builder()
        .label("Create a playlist with +, from any song’s menu, or by dragging songs here.")
        .wrap(true)
        .xalign(0.0)
        .css_classes(["mp-sidebar-hint", "caption", "dim-label"])
        .build();
    let playlists = ctx.store.playlists();
    let update = Rc::new(glib::clone!(
        #[weak]
        hint,
        #[weak]
        sidebar,
        #[weak]
        playlists,
        move || hint.set_visible(playlists.n_items() == 0 && !sidebar.icons_only())
    ));
    update();
    let refresh = update.clone();
    playlists.connect_items_changed(move |_, _, _, _| refresh());
    sidebar.connect_icons_only_notify(move |_| update());
    hint.add_controller(new_playlist_target(ctx));
    hint.upcast()
}

/// Library status (song count, or scan progress) and the preferences button. While the
/// sidebar shows only icons the status is left out and the spinner goes above the button.
fn footer(ctx: &Rc<AppContext>, sidebar: &Sidebar) -> gtk::Widget {
    let spinner = adw::Spinner::builder()
        .visible(false)
        .halign(gtk::Align::Center)
        .build();
    let status = gtk::Label::builder()
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["mp-library-status", "caption", "dim-label"])
        .build();
    let preferences = gtk::Button::builder()
        .icon_name(icons::PREFERENCES)
        .action_name("app.preferences")
        .halign(gtk::Align::Center)
        .css_classes(["flat", "circular"])
        .build();
    set_tooltip(&preferences, "Preferences (Ctrl+,)");

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.add_css_class("mp-sidebar-footer");
    footer.append(&spinner);
    footer.append(&status);
    footer.append(&preferences);
    hide_when_icons_only(sidebar, &status);
    sidebar
        .bind_property("icons-only", &footer, "orientation")
        .transform_to(|_, icons_only: bool| {
            Some(if icons_only {
                gtk::Orientation::Vertical
            } else {
                gtk::Orientation::Horizontal
            })
        })
        .sync_create()
        .build();

    let update = glib::clone!(
        #[weak]
        ctx,
        #[weak]
        spinner,
        #[weak]
        status,
        move || {
            let service = &ctx.library_service;
            let scanning = service.scanning();
            spinner.set_visible(scanning);
            set_css_class(&status, "warning", service.folder_missing() && !scanning);
            let (text, details) = library_status(&ctx);
            status.set_label(&text);
            status.set_tooltip_text(details.as_deref());
        }
    );
    update();

    let update = Rc::new(update);
    let service = &ctx.library_service;
    let refresh = update.clone();
    service.connect_scanning_notify(move |_| refresh());
    let refresh = update.clone();
    service.connect_scan_processed_notify(move |_| refresh());
    let refresh = update.clone();
    service.connect_scan_total_notify(move |_| refresh());
    let refresh = update.clone();
    service.connect_folder_missing_notify(move |_| refresh());
    let refresh = update.clone();
    ctx.store
        .connect_total_duration_ms_notify(move |_| refresh());
    ctx.store.connect_loaded_notify(move |_| update());
    footer.upcast()
}

/// The footer's short status text, and details for its tooltip.
fn library_status(ctx: &AppContext) -> (String, Option<String>) {
    let service = &ctx.library_service;
    if service.scanning() {
        return match (service.scan_processed(), service.scan_total()) {
            (_, 0) => ("Scanning…".to_owned(), None),
            (done, total) => {
                let percent = u64::from(done) * 100 / u64::from(total);
                let details = format!(
                    "{} of {} files scanned",
                    format_count(done as usize),
                    format_count(total as usize)
                );
                (format!("Scanning… {percent}%"), Some(details))
            }
        };
    }
    if service.folder_missing() {
        let folder = ctx.settings().music_folder.clone();
        let details = folder.map(|folder| format!("{} is missing or empty", folder.display()));
        return ("Music folder unavailable".to_owned(), details);
    }
    if !ctx.store.loaded() {
        return ("Loading library…".to_owned(), None);
    }
    let songs = song_count(ctx.store.track_count() as usize);
    let duration = total_length(ctx.store.total_duration_ms());
    (songs, Some(duration))
}

/// Highlights the row of the page being shown; only one row across both lists.
fn connect_selection(ctx: &Rc<AppContext>, library: &gtk::ListBox, playlists: &gtk::ListBox) {
    let library = library.downgrade();
    let playlists = playlists.downgrade();
    let store = ctx.store.downgrade();
    ctx.navigator.connect_navigated(move |route| {
        let (Some(library), Some(playlists), Some(store)) =
            (library.upgrade(), playlists.upgrade(), store.upgrade())
        else {
            return;
        };
        if !route.is_top_level() {
            return;
        }
        let library_row = SECTIONS
            .iter()
            .position(|(section, ..)| section == route)
            .and_then(|index| library.row_at_index(index as i32));
        let playlist_row = match route {
            Route::Playlist(id) => position_of_playlist(&store.playlists(), *id)
                .and_then(|index| playlists.row_at_index(index as i32)),
            _ => None,
        };
        library.select_row(library_row.as_ref());
        playlists.select_row(playlist_row.as_ref());
    });
}

fn position_of_playlist(model: &gio::ListStore, id: PlaylistId) -> Option<u32> {
    model
        .iter::<PlaylistObject>()
        .filter_map(Result::ok)
        .position(|playlist| playlist.playlist_id() == id)
        .map(|index| index as u32)
}

/// Shows song, album, artist and format counts next to their sections (Now Playing
/// has none).
fn keep_counts_current(ctx: &Rc<AppContext>, counts: [gtk::Label; 5]) {
    let update = move |ctx: &AppContext| {
        let store = &ctx.store;
        let formats = store.format_counts().len();
        let values = [
            store.track_count(),
            store.albums().n_items(),
            store.artists().n_items(),
            formats as u32,
        ];
        for (label, value) in counts.iter().zip(values) {
            let text = if value == 0 {
                String::new()
            } else {
                format_count(value as usize)
            };
            label.set_label(&text);
        }
    };
    update(ctx);
    let weak = Rc::downgrade(ctx);
    ctx.store.connect_tracks_changed(move |_| {
        if let Some(ctx) = weak.upgrade() {
            update(&ctx);
        }
    });
}
