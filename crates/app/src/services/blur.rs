//! Blur behind the translucent window, on compositors that let an app ask for it.
//!
//! On Wayland the compositor is asked through the standard background-effect protocol
//! (KDE Plasma 6.5 and newer) or KWin's older blur protocol. GTK 4.24 speaks the
//! standard protocol itself for windows with a CSS `backdrop-filter`, and a surface may
//! only have one such object, so with that GTK the blur is requested through the
//! appearance CSS instead and this module only checks the compositor can blur. Hyprland blurs translucent
//! windows according to its own configuration, and other desktops (GNOME, X11
//! sessions, Windows, macOS) offer apps no blur, so there this does nothing.

use std::cell::Cell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

/// What to blur: the window's own area, inside its client-side shadow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BlurArea {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    /// Corner radius of the window, so blur doesn't leak past rounded corners.
    radius: i32,
}

/// Keeps the compositor's blur in step with a window's size, state and settings.
pub struct WindowBlur {
    window: glib::WeakRef<gtk::Window>,
    backend: Option<Rc<platform::Backend>>,
    enabled: Cell<bool>,
    radius: Cell<u32>,
}

impl WindowBlur {
    /// Attaches to `window`. Blur starts off; see [`Self::set`].
    pub fn new(window: &gtk::Window) -> Rc<Self> {
        let blur = Rc::new(Self {
            window: window.downgrade(),
            backend: platform::Backend::new(window).map(Rc::new),
            enabled: Cell::new(false),
            radius: Cell::new(0),
        });
        if blur.backend.is_some() {
            Self::follow_layout(&blur, window);
        }
        blur
    }

    /// Whether the compositor supports blur through this module.
    pub fn is_supported(&self) -> bool {
        self.backend.is_some()
    }

    /// Whether blur has to be asked for in CSS (`backdrop-filter` on the window), which
    /// GTK then passes on to the compositor.
    pub fn uses_css(&self) -> bool {
        self.backend
            .as_ref()
            .is_some_and(|backend| backend.uses_css())
    }

    /// Turns blur on or off. `radius` is the window's corner radius in pixels.
    pub fn set(&self, enabled: bool, radius: u32) {
        self.enabled.set(enabled);
        self.radius.set(radius);
        self.update();
    }

    /// Re-applies the blur whenever the window's surface is resized or changes state
    /// (maximising drops the shadow and the rounded corners).
    fn follow_layout(blur: &Rc<Self>, window: &gtk::Window) {
        let weak = Rc::downgrade(blur);
        let Some(surface) = window.surface() else {
            return;
        };
        surface.connect_layout(move |_, _, _| {
            let weak = weak.clone();
            // The window's allocation is updated after the surface's layout.
            glib::idle_add_local_once(move || {
                if let Some(blur) = weak.upgrade() {
                    blur.update();
                }
            });
        });
    }

    fn update(&self) {
        let (Some(backend), Some(window)) = (&self.backend, self.window.upgrade()) else {
            return;
        };
        let area = self
            .enabled
            .get()
            .then(|| blur_area(&window, self.radius.get()));
        backend.apply(area);
        window.queue_draw();
    }
}

/// The window's area within its surface. Maximised, tiled and fullscreen windows have
/// no shadow (so no offset) and square corners.
fn blur_area(window: &gtk::Window, radius: u32) -> BlurArea {
    let (x, y) = window.surface_transform();
    let has_shadow = x > 0.0 || y > 0.0;
    BlurArea {
        x: x.round() as i32,
        y: y.round() as i32,
        width: window.width(),
        height: window.height(),
        radius: if has_shadow { radius as i32 } else { 0 },
    }
}

/// The area as rectangles: full-width rows in the middle and, near the top and bottom,
/// one-pixel rows inset to follow the rounded corners.
fn region_rects(area: BlurArea) -> Vec<(i32, i32, i32, i32)> {
    let BlurArea {
        x,
        y,
        width,
        height,
        radius,
    } = area;
    let radius = radius.min(width / 2).min(height / 2).max(0);
    let mut rects = Vec::with_capacity(radius as usize * 2 + 1);
    for row in 0..radius {
        let from_centre = f64::from(radius) - f64::from(row) - 0.5;
        let inset = f64::from(radius) - (f64::from(radius).powi(2) - from_centre.powi(2)).sqrt();
        let inset = inset.round() as i32;
        let row_width = width - 2 * inset;
        rects.push((x + inset, y + row, row_width, 1));
        rects.push((x + inset, y + height - 1 - row, row_width, 1));
    }
    rects.push((x, y + radius, width, height - 2 * radius));
    rects
}

#[cfg(target_os = "linux")]
mod platform {
    use std::cell::{Cell, RefCell};

    use gdk4_wayland::prelude::*;
    use gdk4_wayland::{WaylandDisplay, WaylandSurface};
    use gtk::prelude::*;
    use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
    use wayland_client::protocol::wl_compositor::WlCompositor;
    use wayland_client::protocol::wl_region::WlRegion;
    use wayland_client::protocol::wl_registry::{self, WlRegistry};
    use wayland_client::protocol::wl_surface::WlSurface;
    use wayland_client::{
        Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, delegate_noop,
    };
    use wayland_protocols::ext::background_effect::v1::client::ext_background_effect_manager_v1::{
        self as effect_manager, ExtBackgroundEffectManagerV1,
    };
    use wayland_protocols::ext::background_effect::v1::client::ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1;
    use wayland_protocols_plasma::blur::client::org_kde_kwin_blur::OrgKdeKwinBlur;
    use wayland_protocols_plasma::blur::client::org_kde_kwin_blur_manager::OrgKdeKwinBlurManager;

    use super::{BlurArea, region_rects};

    /// What the compositor told us; only the background-effect manager has events.
    #[derive(Default)]
    struct State {
        can_blur: Cell<bool>,
    }

    impl Dispatch<WlRegistry, GlobalListContents> for State {
        fn event(
            _: &mut Self,
            _: &WlRegistry,
            _: wl_registry::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<ExtBackgroundEffectManagerV1, ()> for State {
        fn event(
            state: &mut Self,
            _: &ExtBackgroundEffectManagerV1,
            event: effect_manager::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let effect_manager::Event::Capabilities {
                flags: WEnum::Value(flags),
            } = event
            {
                state
                    .can_blur
                    .set(flags.contains(effect_manager::Capability::Blur));
            }
        }
    }

    delegate_noop!(State: ExtBackgroundEffectSurfaceV1);
    delegate_noop!(State: OrgKdeKwinBlurManager);
    delegate_noop!(State: OrgKdeKwinBlur);
    delegate_noop!(State: WlRegion);

    /// The ways to ask for blur. The standard background-effect protocol is used where
    /// offered (Plasma 6.5 and newer, among others): through GTK, which owns the
    /// surface's effect object from GTK 4.24, or directly with older GTK. KWin's own
    /// protocol covers older Plasma versions.
    enum Protocol {
        /// GTK blurs behind the window itself when it has a CSS `backdrop-filter`.
        Gtk,
        Standard(ExtBackgroundEffectSurfaceV1),
        Kde {
            manager: OrgKdeKwinBlurManager,
            blur: RefCell<Option<OrgKdeKwinBlur>>,
        },
    }

    /// Blur for one window, sharing GTK's Wayland connection.
    pub(super) struct Backend {
        connection: Connection,
        queue: RefCell<EventQueue<State>>,
        state: RefCell<State>,
        compositor: WlCompositor,
        surface: WlSurface,
        protocol: Protocol,
    }

    impl Backend {
        /// `None` unless this is a Wayland session whose compositor offers blur.
        pub(super) fn new(window: &gtk::Window) -> Option<Self> {
            let display = WidgetExt::display(window)
                .downcast::<WaylandDisplay>()
                .ok()?;
            let surface = window
                .surface()?
                .downcast::<WaylandSurface>()
                .ok()?
                .wl_surface()?;
            let compositor = display.wl_compositor()?;
            let backend = display.wl_display()?.backend().upgrade()?;
            let connection = Connection::from_backend(backend);
            let (globals, mut queue) = registry_queue_init::<State>(&connection).ok()?;
            let mut state = State::default();
            let protocol = standard_protocol(&globals, &mut queue, &mut state, &surface)
                .or_else(|| kde_protocol(&globals, &queue))?;
            Some(Self {
                connection,
                queue: RefCell::new(queue),
                state: RefCell::new(state),
                compositor,
                surface,
                protocol,
            })
        }

        /// Whether blur is requested through CSS (see [`super::WindowBlur::uses_css`]).
        pub(super) fn uses_css(&self) -> bool {
            matches!(self.protocol, Protocol::Gtk)
        }

        /// Blurs `area`, or removes the blur. Takes effect with the window's next frame.
        pub(super) fn apply(&self, area: Option<BlurArea>) {
            let mut queue = self.queue.borrow_mut();
            // Picks up capability changes, and keeps events from piling up.
            if let Err(err) = queue.dispatch_pending(&mut self.state.borrow_mut()) {
                log::debug!("Blur event queue: {err}");
            }
            let handle = queue.handle();
            if matches!(self.protocol, Protocol::Gtk) {
                return;
            }
            let region = area.map(|area| {
                let region = self.compositor.create_region(&handle, ());
                for (x, y, width, height) in region_rects(area) {
                    region.add(x, y, width, height);
                }
                region
            });
            match &self.protocol {
                Protocol::Gtk => {}
                Protocol::Standard(effect) => effect.set_blur_region(region.as_ref()),
                Protocol::Kde { manager, blur } => match &region {
                    Some(region) => {
                        let mut blur = blur.borrow_mut();
                        let blur =
                            blur.get_or_insert_with(|| manager.create(&self.surface, &handle, ()));
                        blur.set_region(Some(region));
                        blur.commit();
                    }
                    None => {
                        if let Some(blur) = blur.take() {
                            manager.unset(&self.surface);
                            blur.release();
                        }
                    }
                },
            }
            if let Some(region) = region {
                region.destroy();
            }
            if let Err(err) = self.connection.flush() {
                log::warn!("Couldn’t update the window blur: {err}");
            }
        }
    }

    /// The standard protocol, if offered and the compositor says it can blur.
    fn standard_protocol(
        globals: &GlobalList,
        queue: &mut EventQueue<State>,
        state: &mut State,
        surface: &WlSurface,
    ) -> Option<Protocol> {
        let handle = queue.handle();
        let manager = globals
            .bind::<ExtBackgroundEffectManagerV1, _, _>(&handle, 1..=1, ())
            .ok()?;
        // The capabilities arrive right after binding.
        queue.roundtrip(state).ok()?;
        if !state.can_blur.get() {
            manager.destroy();
            return None;
        }
        // Only one effect object is allowed per surface, and a GTK that understands
        // `backdrop-filter` creates its own.
        if gtk_handles_backdrop_blur() {
            manager.destroy();
            return Some(Protocol::Gtk);
        }
        let effect = manager.get_background_effect(surface, &handle, ());
        manager.destroy();
        Some(Protocol::Standard(effect))
    }

    /// Whether this GTK knows the CSS `backdrop-filter` property, which it implements
    /// with the background-effect protocol (GTK 4.24 does).
    fn gtk_handles_backdrop_blur() -> bool {
        let provider = gtk::CssProvider::new();
        let understood = std::rc::Rc::new(Cell::new(true));
        let flag = understood.clone();
        provider.connect_parsing_error(move |_, _, _| flag.set(false));
        provider.load_from_string("window { backdrop-filter: blur(1px); }");
        understood.get()
    }

    fn kde_protocol(globals: &GlobalList, queue: &EventQueue<State>) -> Option<Protocol> {
        let manager = globals
            .bind::<OrgKdeKwinBlurManager, _, _>(&queue.handle(), 1..=1, ())
            .ok()?;
        Some(Protocol::Kde {
            manager,
            blur: RefCell::new(None),
        })
    }
}
#[cfg(not(target_os = "linux"))]
mod platform {
    use super::BlurArea;

    /// No blur protocol to speak to on this platform.
    pub(super) enum Backend {}

    impl Backend {
        pub(super) fn new(_: &gtk::Window) -> Option<Self> {
            None
        }

        pub(super) fn uses_css(&self) -> bool {
            match *self {}
        }

        pub(super) fn apply(&self, _: Option<BlurArea>) {
            match *self {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(radius: i32) -> BlurArea {
        BlurArea {
            x: 10,
            y: 20,
            width: 100,
            height: 60,
            radius,
        }
    }

    fn covered(rects: &[(i32, i32, i32, i32)]) -> i64 {
        rects
            .iter()
            .map(|&(_, _, width, height)| i64::from(width) * i64::from(height))
            .sum()
    }

    #[test]
    fn square_windows_are_one_rectangle() {
        assert_eq!(region_rects(area(0)), vec![(10, 20, 100, 60)]);
    }

    #[test]
    fn rounded_corners_are_cut_out() {
        let rects = region_rects(area(12));
        let full = 100 * 60;
        let corners_cut = full - covered(&rects);
        // Four corners of a 12px radius lose (4 - π) * 12² ≈ 124 px in total.
        assert!((100..150).contains(&corners_cut), "{corners_cut}");
        // The first row is inset the most, the middle block is full width.
        assert!(rects[0].0 > 10 && rects[0].2 < 100);
        assert_eq!(*rects.last().unwrap(), (10, 32, 100, 36));
        assert!(
            rects
                .iter()
                .all(|&(x, y, w, h)| x >= 10 && y >= 20 && x + w <= 110 && y + h <= 80)
        );
    }

    #[test]
    fn radius_is_limited_by_small_windows() {
        let rects = region_rects(BlurArea {
            width: 10,
            height: 6,
            ..area(50)
        });
        assert!(rects.iter().all(|&(_, _, w, h)| w >= 0 && h >= 0));
    }
}
