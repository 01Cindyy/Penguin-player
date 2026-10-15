//! The mute button and volume slider, kept in step with the player.

use std::cell::OnceCell;
use std::rc::Rc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;

use super::follow;
use crate::context::AppContext;
use crate::ui::{disconnect_on_destroy, icon_button, icons, set_css_class, set_tooltip};

struct Parts {
    mute: gtk::Button,
    slider: gtk::Scale,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct VolumeControls {
        pub(super) ctx: OnceCell<Rc<AppContext>>,
        pub(super) parts: OnceCell<Parts>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for VolumeControls {
        const NAME: &'static str = "MpVolumeControls";
        type Type = super::VolumeControls;
        type ParentType = gtk::Box;
    }

    impl ObjectImpl for VolumeControls {}
    impl WidgetImpl for VolumeControls {}
    impl BoxImpl for VolumeControls {}
}

glib::wrapper! {
    /// A mute button and the volume slider (`mp-volume`).
    pub struct VolumeControls(ObjectSubclass<imp::VolumeControls>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl VolumeControls {
    pub fn new(ctx: &Rc<AppContext>) -> Self {
        let controls: Self = glib::Object::builder()
            .property("spacing", 4)
            .property("valign", gtk::Align::Center)
            .build();
        let parts = Parts {
            mute: icon_button(
                icons::VOLUME_HIGH,
                "Mute",
                &["flat", "circular", "mp-transport-button"],
            ),
            slider: gtk::Scale::builder()
                .orientation(gtk::Orientation::Horizontal)
                .draw_value(false)
                .width_request(110)
                .valign(gtk::Align::Center)
                .css_classes(["mp-volume"])
                .tooltip_text("Volume (Ctrl+↑ / Ctrl+↓)")
                .build(),
        };
        parts.slider.set_range(0.0, 1.0);
        parts.slider.set_increments(0.05, 0.1);
        parts
            .slider
            .update_property(&[gtk::accessible::Property::Label("Volume")]);
        controls.append(&parts.mute);
        controls.append(&parts.slider);

        let player = &ctx.player;
        player
            .bind_property("volume", &parts.slider.adjustment(), "value")
            .bidirectional()
            .sync_create()
            .build();
        parts.mute.connect_clicked(glib::clone!(
            #[weak]
            player,
            move |_| player.set_muted(!player.muted())
        ));
        for handler in [
            player.connect_volume_notify(follow(&controls, Self::update_icon)),
            player.connect_muted_notify(follow(&controls, Self::update_icon)),
        ] {
            disconnect_on_destroy(&controls, player, handler);
        }

        let imp = controls.imp();
        assert!(imp.ctx.set(ctx.clone()).is_ok(), "set once in new");
        assert!(imp.parts.set(parts).is_ok(), "set once in new");
        controls.update_icon();
        controls
    }

    fn update_icon(&self) {
        let (Some(ctx), Some(parts)) = (self.imp().ctx.get(), self.imp().parts.get()) else {
            return;
        };
        let player = &ctx.player;
        let volume = player.volume();
        let icon = if player.muted() || volume <= 0.0 {
            icons::VOLUME_MUTED
        } else if volume < 0.34 {
            icons::VOLUME_LOW
        } else if volume < 0.67 {
            icons::VOLUME_MEDIUM
        } else {
            icons::VOLUME_HIGH
        };
        parts.mute.set_icon_name(icon);
        set_tooltip(&parts.mute, if player.muted() { "Unmute" } else { "Mute" });
        set_css_class(&parts.slider, "mp-muted", player.muted());
    }
}
