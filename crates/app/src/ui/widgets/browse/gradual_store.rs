//! A list store that grows a chunk per frame, for views that create a widget for
//! every item, such as the lists inside album and artist pages.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

/// Items added per frame: enough to fill the visible part of a page at once, few
/// enough to lay out well within a frame.
const ITEMS_PER_FRAME: usize = 24;

/// A `gio::ListStore` whose contents are swapped in gradually.
///
/// Views that don't scroll by themselves (see [`super::DetailPage`]) create a widget
/// for every item, so a long list would stall the frame that shows it. This store
/// adds what doesn't fit in the first frame on the following frames of the widget
/// it was created for.
#[derive(Clone)]
pub struct GradualStore {
    inner: Rc<Inner>,
}

struct Inner {
    store: gio::ListStore,
    /// Paces the filling; usually the page that shows the store.
    widget: glib::WeakRef<gtk::Widget>,
    /// Everything the store holds once filled.
    target: RefCell<Vec<glib::Object>>,
    filling: RefCell<Option<gtk::TickCallbackId>>,
}

impl GradualStore {
    /// A store of `T` items that fills on the frames of `widget`, which should live
    /// as long as the views showing the store.
    pub fn new<T: IsA<glib::Object>>(widget: &impl IsA<gtk::Widget>) -> Self {
        Self::with_pacer::<T>(widget.upcast_ref::<gtk::Widget>().downgrade())
    }

    fn with_pacer<T: IsA<glib::Object>>(widget: glib::WeakRef<gtk::Widget>) -> Self {
        let inner = Inner {
            store: gio::ListStore::new::<T>(),
            widget,
            target: RefCell::default(),
            filling: RefCell::default(),
        };
        Self {
            inner: Rc::new(inner),
        }
    }

    pub fn model(&self) -> &gio::ListStore {
        &self.inner.store
    }

    /// Shows `objects`, leaving the store alone if it already shows them. Items
    /// already in the store are replaced at once, which only rebinds their rows;
    /// the remainder is added a chunk per frame.
    pub fn set(&self, objects: &[impl IsA<glib::Object>]) {
        let objects: Vec<glib::Object> = objects.iter().map(|o| o.clone().upcast()).collect();
        if *self.inner.target.borrow() == objects {
            return;
        }
        if let Some(filling) = self.inner.filling.take() {
            filling.remove();
        }
        let store = &self.inner.store;
        let shown = store.n_items() as usize;
        let unchanged = (0..shown.min(objects.len()))
            .take_while(|&position| {
                store.item(position as u32).as_ref() == Some(&objects[position])
            })
            .count();
        let ready = shown.max(ITEMS_PER_FRAME).min(objects.len());
        if unchanged < shown || unchanged < ready {
            store.splice(
                unchanged as u32,
                (shown - unchanged) as u32,
                &objects[unchanged..ready],
            );
        }
        let complete = ready == objects.len();
        self.inner.target.replace(objects);
        if !complete {
            self.fill_gradually();
        }
    }

    fn fill_gradually(&self) {
        let Some(widget) = self.inner.widget.upgrade() else {
            return;
        };
        let inner = Rc::downgrade(&self.inner);
        let filling = widget.add_tick_callback(move |_, _| match inner.upgrade() {
            Some(inner) if inner.add_chunk() => glib::ControlFlow::Continue,
            Some(inner) => {
                inner.filling.take();
                glib::ControlFlow::Break
            }
            None => glib::ControlFlow::Break,
        });
        self.inner.filling.replace(Some(filling));
    }
}

impl Inner {
    /// Appends the next chunk of the target. Returns whether more remain.
    fn add_chunk(&self) -> bool {
        let shown = self.store.n_items() as usize;
        let (chunk, remaining) = {
            let target = self.target.borrow();
            let end = (shown + ITEMS_PER_FRAME).min(target.len());
            (target[shown.min(end)..end].to_vec(), target.len() - end)
        };
        self.store.extend_from_slice(&chunk);
        remaining > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn objects(count: usize) -> Vec<glib::Object> {
        (0..count).map(|_| glib::Object::new()).collect()
    }

    fn contents(store: &GradualStore) -> Vec<glib::Object> {
        store.model().iter().filter_map(Result::ok).collect()
    }

    #[test]
    fn fills_in_chunks_and_skips_unchanged_contents() {
        // Without a widget nothing fills by itself; the test adds the chunks.
        let store = GradualStore::with_pacer::<glib::Object>(glib::WeakRef::new());
        let changes = Rc::new(std::cell::Cell::new(0));
        let counter = changes.clone();
        store
            .model()
            .connect_items_changed(move |_, _, _, _| counter.set(counter.get() + 1));

        let short = objects(3);
        store.set(&short);
        store.set(&short);
        assert_eq!(contents(&store), short);
        assert_eq!(changes.get(), 1);

        let long = objects(ITEMS_PER_FRAME * 2 + 5);
        store.set(&long);
        assert_eq!(contents(&store), long[..ITEMS_PER_FRAME]);
        while store.inner.add_chunk() {}
        assert_eq!(contents(&store), long);

        // Replacing the shown items happens at once, however many there are.
        let mut edited = long.clone();
        edited[1] = glib::Object::new();
        edited.truncate(ITEMS_PER_FRAME * 2);
        store.set(&edited);
        assert_eq!(contents(&store), edited);
    }
}
