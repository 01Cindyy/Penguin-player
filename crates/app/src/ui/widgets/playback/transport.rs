//! Shuffle, previous, play/pause, next and repeat, kept in step with the player.

use std::cell::OnceCell;
use std::rc::Rc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;
use mp_core::{PlayerCommand, RepeatMode};

use super::follow;
use crate::context::AppContext;
use crate::player::PlayerState;
use crate::ui::shortcuts::primary;
use crate::ui::{
    disconnect_on_destroy, icon_button, icon_toggle, icons, set_css_class, set_tooltip,
};

struct Buttons {
    shuffle: gtk::ToggleButton,
    previous: gtk::Button,
    play: gtk::Button,
    next: gtk::Button,
    repeat: gtk::Button,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct TransportControls {
        pub(super) ctx: OnceCell<Rc<AppContext>>,
        pub(super) buttons: OnceCell<Buttons>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TransportControls {
        const NAME: &'static str = "MpTransportControls";
        type Type = super::TransportControls;
        type ParentType = gtk::Box;
    }

    impl ObjectImpl for TransportControls {}
    impl WidgetImpl for TransportControls {}
    impl BoxImpl for TransportControls {}
}

glib::wrapper! {
    /// The transport buttons (`mp-transport`).
    pub struct TransportControls(ObjectSubclass<imp::TransportControls>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl TransportControls {
    pub fn new(ctx: &Rc<AppContext>) -> Self {
        let controls: Self = glib::Object::builder()
            .property("spacing", 6)
            .property("halign", gtk::Align::Center)
            .build();
        controls.add_css_class("mp-transport");
        let toggle_classes = ["flat", "circular", "mp-toggle-button"];
        let skip_classes = ["flat", "circular", "mp-transport-button"];
        let buttons = Buttons {
            shuffle: icon_toggle(
                icons::SHUFFLE,
                &format!("Shuffle ({})", primary("S")),
                &toggle_classes,
            ),
            previous: icon_button(
                icons::SKIP_BACKWARD,
                &format!("Previous ({})", primary("←")),
                &skip_classes,
            ),
            play: icon_button(icons::PLAY, "Play (Space)", &["circular", "mp-play-button"]),
            next: icon_button(
                icons::SKIP_FORWARD,
                &format!("Next ({})", primary("→")),
                &skip_classes,
            ),
            repeat: icon_button(
                icons::REPEAT,
                &format!("Repeat ({})", primary("R")),
                &toggle_classes,
            ),
        };
        for button in [
            buttons.shuffle.upcast_ref::<gtk::Widget>(),
            buttons.previous.upcast_ref(),
            buttons.play.upcast_ref(),
            buttons.next.upcast_ref(),
            buttons.repeat.upcast_ref(),
        ] {
            controls.append(button);
        }
        let imp = controls.imp();
        assert!(imp.ctx.set(ctx.clone()).is_ok(), "set once in new");
        assert!(imp.buttons.set(buttons).is_ok(), "set once in new");

        controls.connect_buttons();
        controls.follow_player();
        controls.update_state();
        controls.update_shuffle();
        controls.update_repeat();
        controls.update_skip_buttons();
        controls
    }

    /// Shows or hides shuffle and repeat, which narrow layouts leave out.
    pub fn set_show_modes(&self, show: bool) {
        let buttons = self.buttons();
        buttons.shuffle.set_visible(show);
        buttons.repeat.set_visible(show);
    }

    /// Spreads the buttons evenly over the whole width the controls are given, from edge
    /// to edge, rather than keeping them together in the middle.
    pub fn spread(&self) {
        self.set_halign(gtk::Align::Fill);
        let mut button = self.first_child();
        while let Some(current) = button {
            let next = current.next_sibling();
            if next.is_some() {
                let gap = gtk::Box::builder().hexpand(true).build();
                self.insert_child_after(&gap, Some(&current));
            }
            button = next;
        }
    }

    fn ctx(&self) -> &Rc<AppContext> {
        self.imp().ctx.get().expect("set in new")
    }

    fn buttons(&self) -> &Buttons {
        self.imp().buttons.get().expect("set in new")
    }

    fn connect_buttons(&self) {
        let buttons = self.buttons();
        let player = &self.ctx().player;
        let execute = |command: PlayerCommand| {
            let player = player.downgrade();
            move |_: &gtk::Button| {
                if let Some(player) = player.upgrade() {
                    player.execute(command.clone());
                }
            }
        };
        buttons
            .play
            .connect_clicked(execute(PlayerCommand::TogglePlayPause));
        buttons
            .previous
            .connect_clicked(execute(PlayerCommand::Previous));
        buttons.next.connect_clicked(execute(PlayerCommand::Next));
        buttons.repeat.connect_clicked(glib::clone!(
            #[weak]
            player,
            move |_| player.cycle_repeat()
        ));
        player
            .bind_property("shuffle", &buttons.shuffle, "active")
            .bidirectional()
            .sync_create()
            .build();
    }

    fn follow_player(&self) {
        let ctx = self.ctx();
        let player = &ctx.player;
        for handler in [
            player.connect_state_notify(follow(self, Self::update_state)),
            player.connect_shuffle_notify(follow(self, Self::update_shuffle)),
            player.connect_repeat_notify(follow(self, Self::update_repeat)),
            player.connect_queue_changed(follow(self, Self::update_skip_buttons)),
        ] {
            disconnect_on_destroy(self, player, handler);
        }
        // With an empty queue, play starts the whole library, so it needs songs.
        let handler = ctx
            .store
            .connect_track_count_notify(follow(self, Self::update_skip_buttons));
        disconnect_on_destroy(self, &ctx.store, handler);
    }

    fn update_state(&self) {
        let playing = self.ctx().player.state() == PlayerState::Playing;
        let (icon, tooltip) = if playing {
            (icons::PAUSE, "Pause (Space)")
        } else {
            (icons::PLAY, "Play (Space)")
        };
        let play = &self.buttons().play;
        play.set_icon_name(icon);
        set_tooltip(play, tooltip);
    }

    fn update_shuffle(&self) {
        let state = if self.ctx().player.shuffle() {
            "On"
        } else {
            "Off"
        };
        let tooltip = format!("Shuffle: {state} ({})", primary("S"));
        set_tooltip(&self.buttons().shuffle, &tooltip);
    }

    fn update_repeat(&self) {
        let mode = self.ctx().player.repeat_mode();
        let (icon, state) = match mode {
            RepeatMode::Off => (icons::REPEAT, "Off"),
            RepeatMode::All => (icons::REPEAT, "All"),
            RepeatMode::One => (icons::REPEAT_ONE, "Current Song"),
        };
        let repeat = &self.buttons().repeat;
        repeat.set_icon_name(icon);
        set_tooltip(repeat, &format!("Repeat: {state} ({})", primary("R")));
        set_css_class(repeat, "mp-active", mode != RepeatMode::Off);
    }

    fn update_skip_buttons(&self) {
        let ctx = self.ctx();
        let buttons = self.buttons();
        let has_queue = ctx.player.queue_len() > 0;
        buttons.previous.set_sensitive(has_queue);
        buttons.next.set_sensitive(has_queue);
        buttons
            .play
            .set_sensitive(has_queue || ctx.store.track_count() > 0);
    }
}
