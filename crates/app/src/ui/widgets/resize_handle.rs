//! A thin strip along a pane's edge that resizes the pane when dragged.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::graphene;

/// Which way the pane grows when the handle is dragged (mirrored in right-to-left
/// languages).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grows {
    Right,
    Left,
    Up,
}

/// What a handle does while it is dragged.
pub trait Resizable: 'static {
    /// The pane's size now, when a drag starts.
    fn size(&self) -> f64;
    /// The pointer asks for the pane to be `size` pixels; the pane may snap or clamp it.
    fn resize(&self, size: f64);
    /// The drag ended: a good time to remember the size.
    fn resized(&self);
}

/// A handle for the start (or, for [`Grows::Up`], top) edge of the overlay it is added to.
pub fn resize_handle(grows: Grows, pane: impl Resizable) -> gtk::Widget {
    let sideways = grows != Grows::Up;
    let handle = gtk::Box::builder()
        .css_classes([
            "mp-resize-handle",
            if sideways {
                "mp-resize-width"
            } else {
                "mp-resize-height"
            },
        ])
        .halign(if sideways {
            gtk::Align::Start
        } else {
            gtk::Align::Fill
        })
        .valign(if sideways {
            gtk::Align::Fill
        } else {
            gtk::Align::Start
        })
        .build();
    handle.set_cursor_from_name(Some(if sideways { "col-resize" } else { "row-resize" }));

    // The handle moves with the edge it drags, so pointer positions are taken in the
    // window's coordinates, which stay put.
    let along = move |widget: &gtk::Widget, x: f64, y: f64| {
        let root = widget.root()?;
        let point = widget.compute_point(&root, &graphene::Point::new(x as f32, y as f32))?;
        Some(f64::from(if sideways { point.x() } else { point.y() }))
    };
    let sign = move |widget: &gtk::Widget| {
        let mirrored = widget.direction() == gtk::TextDirection::Rtl;
        match grows {
            Grows::Right if mirrored => -1.0,
            Grows::Left if mirrored => 1.0,
            Grows::Right => 1.0,
            Grows::Left | Grows::Up => -1.0,
        }
    };
    let pane = Rc::new(pane);
    // Where the pointer started along the drag, and the pane's size then.
    let start = Rc::new(Cell::new(None::<(f64, f64)>));

    let drag = gtk::GestureDrag::new();
    drag.connect_drag_begin({
        let (pane, start) = (pane.clone(), start.clone());
        move |gesture, x, y| {
            let Some(widget) = gesture.widget() else {
                return;
            };
            gesture.set_state(gtk::EventSequenceState::Claimed);
            start.set(along(&widget, x, y).map(|pointer| (pointer, pane.size())));
            widget.add_css_class("mp-dragging");
        }
    });
    drag.connect_drag_update({
        let (pane, start) = (pane.clone(), start.clone());
        move |gesture, dx, dy| {
            let (Some(widget), Some((x, y)), Some((pointer, size))) =
                (gesture.widget(), gesture.start_point(), start.get())
            else {
                return;
            };
            if let Some(now) = along(&widget, x + dx, y + dy) {
                pane.resize(size + (now - pointer) * sign(&widget));
            }
        }
    });
    drag.connect_drag_end(move |gesture, _, _| {
        if let Some(widget) = gesture.widget() {
            widget.remove_css_class("mp-dragging");
        }
        if start.take().is_some() {
            pane.resized();
        }
    });
    handle.add_controller(drag);
    handle.upcast()
}
