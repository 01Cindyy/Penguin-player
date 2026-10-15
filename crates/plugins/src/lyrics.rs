//! The app's requests for lyrics, answered by plugins that find them
//! (`mp.lyrics.provide`). The first lyrics a plugin finds answer the request; if every
//! plugin asked finds none, the answer is none.

use mp_core::Lyrics;

use crate::host::HostEvent;

/// Most requests waiting for answers at once. The app only waits for the playing
/// song's, so older ones are given up.
const MAX_PENDING: usize = 8;

/// Most bytes of lyrics a plugin may answer with.
pub(crate) const MAX_LYRICS_BYTES: usize = 1024 * 1024;

/// The lyrics a song has of its own, in its tags or an `.lrc` file beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnLyrics {
    Missing,
    Untimed,
    Timed,
}

impl OwnLyrics {
    pub fn of(lyrics: Option<&Lyrics>) -> Self {
        match lyrics {
            None => Self::Missing,
            Some(lyrics) if lyrics.is_synced() => Self::Timed,
            Some(_) => Self::Untimed,
        }
    }

    /// Whether `found` may replace them: untimed lyrics never replace timed ones.
    pub fn yield_to(self, found: &Lyrics) -> bool {
        self != Self::Timed || found.is_synced()
    }

    /// What plugins see in `track.lyrics`.
    pub(crate) fn name(self) -> Option<&'static str> {
        match self {
            Self::Missing => None,
            Self::Untimed => Some("untimed"),
            Self::Timed => Some("timed"),
        }
    }
}

/// The songs plugins find lyrics for, by the lyrics the songs have of their own.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum LyricsSearch {
    /// No plugin finds lyrics.
    #[default]
    Off,
    /// Songs without lyrics.
    Missing,
    /// Songs without timed lyrics.
    Untimed,
    /// Every song.
    All,
}

impl LyricsSearch {
    /// Whether songs with `own` lyrics are looked up.
    pub fn covers(self, own: OwnLyrics) -> bool {
        match own {
            OwnLyrics::Missing => self >= Self::Missing,
            OwnLyrics::Untimed => self >= Self::Untimed,
            OwnLyrics::Timed => self == Self::All,
        }
    }
}

pub(crate) struct LyricsRequests {
    events: async_channel::Sender<HostEvent>,
    pending: Vec<Pending>,
}

struct Pending {
    id: u64,
    /// The song's own lyrics, which the answer must be allowed to replace.
    own: OwnLyrics,
    /// The plugins still to answer.
    waiting: Vec<String>,
}

impl LyricsRequests {
    pub fn new(events: async_channel::Sender<HostEvent>) -> Self {
        Self {
            events,
            pending: Vec::new(),
        }
    }

    /// Starts request `id` for a song with `own` lyrics, to be answered by `plugins`.
    /// With no plugins to ask, the answer is none at once.
    pub fn start(&mut self, id: u64, own: OwnLyrics, plugins: Vec<String>) {
        if plugins.is_empty() {
            self.finish(id, None);
            return;
        }
        if self.pending.len() >= MAX_PENDING {
            let oldest = self.pending.remove(0);
            self.finish(oldest.id, None);
        }
        self.pending.push(Pending {
            id,
            own,
            waiting: plugins,
        });
    }

    /// `plugin`'s answer to request `id`. Answers to finished requests, or from plugins
    /// that weren't asked, change nothing; lyrics that may not replace the song's own
    /// count as none.
    pub fn answer(&mut self, id: u64, plugin: &str, lyrics: Option<String>) {
        let Some(index) = self.pending.iter().position(|pending| pending.id == id) else {
            return;
        };
        let pending = &mut self.pending[index];
        let Some(asked) = pending.waiting.iter().position(|asked| asked == plugin) else {
            return;
        };
        pending.waiting.swap_remove(asked);
        let own = pending.own;
        let lyrics = lyrics.filter(|lyrics| {
            !lyrics.trim().is_empty()
                && (own != OwnLyrics::Timed
                    || Lyrics::parse(lyrics).is_some_and(|found| own.yield_to(&found)))
        });
        if lyrics.is_some() || pending.waiting.is_empty() {
            self.pending.remove(index);
            self.finish(id, lyrics);
        }
    }

    /// `plugin` stopped, so it won't answer anything.
    pub fn forget(&mut self, plugin: &str) {
        let asked: Vec<u64> = self
            .pending
            .iter()
            .filter(|pending| pending.waiting.iter().any(|asked| asked == plugin))
            .map(|pending| pending.id)
            .collect();
        for id in asked {
            self.answer(id, plugin, None);
        }
    }

    fn finish(&self, request: u64, lyrics: Option<String>) {
        let _ = self.events.try_send(HostEvent::Lyrics { request, lyrics });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn requests() -> (LyricsRequests, async_channel::Receiver<HostEvent>) {
        let (sender, receiver) = async_channel::unbounded();
        (LyricsRequests::new(sender), receiver)
    }

    fn answers(receiver: &async_channel::Receiver<HostEvent>) -> Vec<(u64, Option<String>)> {
        std::iter::from_fn(|| receiver.try_recv().ok())
            .map(|event| match event {
                HostEvent::Lyrics { request, lyrics } => (request, lyrics),
                other => panic!("unexpected {other:?}"),
            })
            .collect()
    }

    fn plugins(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    #[test]
    fn the_first_lyrics_found_answer() {
        let (mut requests, receiver) = requests();
        requests.start(1, OwnLyrics::Missing, plugins(&["a", "b"]));
        requests.answer(1, "a", Some("la la".into()));
        requests.answer(1, "b", Some("other".into()));
        assert_eq!(answers(&receiver), [(1, Some("la la".into()))]);
    }

    #[test]
    fn none_once_every_plugin_found_none() {
        let (mut requests, receiver) = requests();
        requests.start(1, OwnLyrics::Missing, plugins(&["a", "b"]));
        requests.answer(1, "a", Some("  \n".into()));
        assert!(answers(&receiver).is_empty());
        requests.answer(1, "c", Some("not asked".into()));
        requests.answer(1, "b", None);
        assert_eq!(answers(&receiver), [(1, None)]);

        requests.start(2, OwnLyrics::Missing, Vec::new());
        assert_eq!(answers(&receiver), [(2, None)]);
    }

    #[test]
    fn plugins_that_stop_found_none() {
        let (mut requests, receiver) = requests();
        requests.start(1, OwnLyrics::Missing, plugins(&["a"]));
        requests.start(2, OwnLyrics::Missing, plugins(&["a", "b"]));
        requests.forget("a");
        assert_eq!(answers(&receiver), [(1, None)]);
        requests.answer(2, "b", Some("words".into()));
        assert_eq!(answers(&receiver), [(2, Some("words".into()))]);
    }

    #[test]
    fn untimed_lyrics_never_replace_timed_ones() {
        let (mut requests, receiver) = requests();
        requests.start(1, OwnLyrics::Timed, plugins(&["a", "b"]));
        requests.answer(1, "a", Some("plain words".into()));
        assert!(answers(&receiver).is_empty());
        requests.answer(1, "b", Some("[00:01.00] timed".into()));
        assert_eq!(answers(&receiver), [(1, Some("[00:01.00] timed".into()))]);

        requests.start(2, OwnLyrics::Untimed, plugins(&["a"]));
        requests.answer(2, "a", Some("plain words".into()));
        assert_eq!(answers(&receiver), [(2, Some("plain words".into()))]);
    }

    #[test]
    fn searches_cover_songs_by_their_own_lyrics() {
        use LyricsSearch::*;
        let own = [OwnLyrics::Missing, OwnLyrics::Untimed, OwnLyrics::Timed];
        let covered = |search: LyricsSearch| own.map(|own| search.covers(own));
        assert_eq!(covered(Off), [false, false, false]);
        assert_eq!(covered(Missing), [true, false, false]);
        assert_eq!(covered(Untimed), [true, true, false]);
        assert_eq!(covered(All), [true, true, true]);

        let timed = Lyrics::parse("[00:01.00] timed").unwrap();
        let plain = Lyrics::parse("plain").unwrap();
        assert_eq!(OwnLyrics::of(Some(&timed)), OwnLyrics::Timed);
        assert_eq!(OwnLyrics::of(Some(&plain)), OwnLyrics::Untimed);
        assert!(OwnLyrics::Timed.yield_to(&timed));
        assert!(!OwnLyrics::Timed.yield_to(&plain));
        assert!(OwnLyrics::Untimed.yield_to(&plain));
    }

    #[test]
    fn old_requests_are_given_up() {
        let (mut requests, receiver) = requests();
        for id in 0..=MAX_PENDING as u64 {
            requests.start(id, OwnLyrics::Missing, plugins(&["a"]));
        }
        assert_eq!(answers(&receiver), [(0, None)]);
    }
}
