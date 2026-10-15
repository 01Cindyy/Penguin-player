//! What a plugin script has registered with the host: event handlers, timers, web
//! request callbacks and a lyrics finder.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::events::EventKind;
use crate::lyrics::LyricsSearch;

/// A plugin can't have more timers than this at once.
pub(crate) const MAX_TIMERS: usize = 256;

/// A plugin can't register more event handlers than this, all events together.
pub(crate) const MAX_HANDLERS: usize = 256;

/// Event handlers, timers and other callbacks of one plugin. `F` is the callback type
/// (a Lua function in practice; anything cloneable in tests).
#[derive(Debug)]
pub(crate) struct Registry<F> {
    handlers: HashMap<EventKind, Vec<F>>,
    pub timers: Timers<F>,
    /// Callbacks of web requests still running, by request id.
    pub requests: HashMap<u64, F>,
    /// What finds lyrics, and for which songs, if the plugin provides them.
    pub lyrics: Option<(F, LyricsSearch)>,
}

impl<F> Default for Registry<F> {
    fn default() -> Self {
        Self {
            handlers: HashMap::new(),
            timers: Timers::default(),
            requests: HashMap::new(),
            lyrics: None,
        }
    }
}

impl<F: Clone> Registry<F> {
    pub fn on(&mut self, kind: EventKind, handler: F) -> Result<(), TooManyHandlers> {
        let count: usize = self.handlers.values().map(Vec::len).sum();
        if count >= MAX_HANDLERS {
            return Err(TooManyHandlers);
        }
        self.handlers.entry(kind).or_default().push(handler);
        Ok(())
    }

    /// The handlers for `kind`, cloned so they can run while the registry is
    /// modified (a handler may register more handlers or timers).
    pub fn handlers(&self, kind: EventKind) -> Vec<F> {
        self.handlers.get(&kind).cloned().unwrap_or_default()
    }
}

#[derive(Debug, thiserror::Error)]
#[error("a plugin can register at most {MAX_HANDLERS} event handlers")]
pub(crate) struct TooManyHandlers;

#[derive(Debug, thiserror::Error)]
#[error("a plugin can have at most {MAX_TIMERS} timers at once")]
pub(crate) struct TooManyTimers;

#[derive(Debug)]
struct Timer<F> {
    id: u64,
    due: Instant,
    /// `Some` for repeating timers.
    interval: Option<Duration>,
    callback: F,
}

#[derive(Debug)]
pub(crate) struct Timers<F> {
    next_id: u64,
    timers: Vec<Timer<F>>,
}

impl<F> Default for Timers<F> {
    fn default() -> Self {
        Self {
            next_id: 1,
            timers: Vec::new(),
        }
    }
}

impl<F: Clone> Timers<F> {
    /// Schedules `callback` after `delay`, then every `interval` if one is given.
    pub fn add(
        &mut self,
        now: Instant,
        delay: Duration,
        interval: Option<Duration>,
        callback: F,
    ) -> Result<u64, TooManyTimers> {
        if self.timers.len() >= MAX_TIMERS {
            return Err(TooManyTimers);
        }
        let id = self.next_id;
        self.next_id += 1;
        self.timers.push(Timer {
            id,
            due: now + delay,
            interval,
            callback,
        });
        Ok(id)
    }

    /// Returns whether a timer with this id existed.
    pub fn cancel(&mut self, id: u64) -> bool {
        let before = self.timers.len();
        self.timers.retain(|timer| timer.id != id);
        self.timers.len() != before
    }

    pub fn next_due(&self) -> Option<Instant> {
        self.timers.iter().map(|timer| timer.due).min()
    }

    /// The callback of the earliest timer due at `now`, if any. A one-shot timer is
    /// removed and a repeating one rescheduled (skipping runs missed while busy).
    ///
    /// Taking one timer at a time means a callback can still cancel timers that are
    /// due in the same round.
    pub fn pop_due(&mut self, now: Instant) -> Option<F> {
        let index = self
            .timers
            .iter()
            .enumerate()
            .filter(|(_, timer)| timer.due <= now)
            .min_by_key(|(_, timer)| (timer.due, timer.id))
            .map(|(index, _)| index)?;
        let timer = &mut self.timers[index];
        match timer.interval {
            Some(interval) => {
                timer.due += interval;
                if timer.due <= now {
                    timer.due = now + interval;
                }
                Some(timer.callback.clone())
            }
            None => Some(self.timers.swap_remove(index).callback),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_due<F: Clone>(timers: &mut Timers<F>, now: Instant) -> Vec<F> {
        std::iter::from_fn(|| timers.pop_due(now)).collect()
    }

    #[test]
    fn one_shot_and_repeating_timers() {
        let start = Instant::now();
        let ms = Duration::from_millis;
        let mut timers = Timers::default();
        let once = timers.add(start, ms(30), None, "once").unwrap();
        timers.add(start, ms(10), Some(ms(10)), "every").unwrap();
        assert_ne!(once, 0);
        assert_eq!(timers.next_due(), Some(start + ms(10)));

        assert!(all_due(&mut timers, start + ms(5)).is_empty());
        assert_eq!(all_due(&mut timers, start + ms(10)), ["every"]);
        assert_eq!(timers.next_due(), Some(start + ms(20)));
        assert_eq!(all_due(&mut timers, start + ms(30)), ["every", "once"]);
        assert_eq!(timers.next_due(), Some(start + ms(40)));

        // Missed runs are skipped rather than fired in a burst.
        assert_eq!(all_due(&mut timers, start + ms(500)), ["every"]);
        assert_eq!(timers.next_due(), Some(start + ms(510)));
    }

    #[test]
    fn cancel_removes_timers() {
        let start = Instant::now();
        let mut timers = Timers::default();
        let id = timers
            .add(start, Duration::ZERO, Some(Duration::from_millis(1)), ())
            .unwrap();
        assert!(timers.cancel(id));
        assert!(!timers.cancel(id));
        assert_eq!(timers.next_due(), None);
        assert_eq!(timers.pop_due(start + Duration::from_secs(1)), None);
    }

    #[test]
    fn timers_due_together_can_cancel_each_other() {
        let start = Instant::now();
        let mut timers = Timers::default();
        timers.add(start, Duration::ZERO, None, "first").unwrap();
        let second = timers.add(start, Duration::ZERO, None, "second").unwrap();
        assert_eq!(timers.pop_due(start), Some("first"));
        assert!(timers.cancel(second));
        assert_eq!(timers.pop_due(start), None);
    }

    #[test]
    fn timer_count_is_limited() {
        let mut timers = Timers::default();
        let now = Instant::now();
        for _ in 0..MAX_TIMERS {
            timers.add(now, Duration::ZERO, None, ()).unwrap();
        }
        assert!(timers.add(now, Duration::ZERO, None, ()).is_err());
    }

    #[test]
    fn handlers_are_grouped_by_event() {
        let mut registry = Registry::default();
        registry.on(EventKind::TrackChanged, 1).unwrap();
        registry.on(EventKind::TrackChanged, 2).unwrap();
        registry.on(EventKind::Shutdown, 3).unwrap();
        assert_eq!(registry.handlers(EventKind::TrackChanged), [1, 2]);
        assert_eq!(registry.handlers(EventKind::Seeked), Vec::<i32>::new());
    }

    /// Handlers are limited, so a loop registering them can't grow without bound and
    /// make every event run up to the time limit.
    #[test]
    fn handler_count_is_limited_across_events() {
        let mut registry = Registry::default();
        for i in 0..MAX_HANDLERS {
            let kind = EventKind::ALL[i % EventKind::ALL.len()];
            registry.on(kind, ()).unwrap();
        }
        assert!(registry.on(EventKind::Seeked, ()).is_err());
    }
}
