//! Song lyrics: plain text, or timed line by line in the LRC format.
//!
//! LRC puts one or more `[mm:ss.xx]` times before each line. Other tags in brackets
//! (`[ar:Artist]`, `[length:3:20]`) describe the song and are skipped, except
//! `[offset:±ms]`, which moves every time. Times on words (`<mm:ss.xx>`, from
//! "enhanced" LRC) are dropped, so those lines are timed only as a whole.

use std::time::Duration;

/// A song's lyrics, timed line by line or plain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lyrics {
    lines: Vec<LyricLine>,
    synced: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricLine {
    /// When the line starts; `None` in plain lyrics.
    pub start: Option<Duration>,
    /// The words. Empty for a pause in timed lyrics, or a break between verses in
    /// plain ones.
    pub text: String,
}

impl Lyrics {
    /// Reads LRC or plain text. Timed if any line has a time, in which case lines
    /// without one are left out. `None` if there are no words at all.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut timed = Vec::new();
        let mut plain = Vec::new();
        let mut offset_ms = 0i64;
        for line in text.lines() {
            match parse_line(line.trim()) {
                Line::Timed(times, words) => {
                    timed.extend(times.into_iter().map(|start| (start, words.clone())));
                }
                Line::Plain(words) => plain.push(words),
                Line::Offset(ms) => offset_ms = ms,
                Line::Info => {}
            }
        }
        let lyrics = if timed.is_empty() {
            Self::plain(plain)
        } else {
            Self::timed(timed, offset_ms)
        };
        lyrics
            .lines
            .iter()
            .any(|line| !line.text.is_empty())
            .then_some(lyrics)
    }

    fn plain(lines: Vec<String>) -> Self {
        let mut kept: Vec<LyricLine> = Vec::new();
        for text in lines {
            let after_break = kept.last().is_none_or(|line| line.text.is_empty());
            if !text.is_empty() || !after_break {
                kept.push(LyricLine { start: None, text });
            }
        }
        if kept.last().is_some_and(|line| line.text.is_empty()) {
            kept.pop();
        }
        Self {
            lines: kept,
            synced: false,
        }
    }

    /// A positive offset shows every line sooner.
    fn timed(mut lines: Vec<(Duration, String)>, offset_ms: i64) -> Self {
        lines.sort_by_key(|(start, _)| *start);
        let shift = Duration::from_millis(offset_ms.unsigned_abs());
        let mut kept: Vec<LyricLine> = Vec::new();
        for (start, text) in lines {
            // A pause before the first words, or right after another, changes nothing.
            let pause_after_pause = kept.last().is_none_or(|line| line.text.is_empty());
            if text.is_empty() && pause_after_pause {
                continue;
            }
            let start = if offset_ms > 0 {
                start.saturating_sub(shift)
            } else {
                start + shift
            };
            kept.push(LyricLine {
                start: Some(start),
                text,
            });
        }
        Self {
            lines: kept,
            synced: true,
        }
    }

    pub fn lines(&self) -> &[LyricLine] {
        &self.lines
    }

    /// Whether the lines are timed, so the one being sung can be followed.
    pub fn is_synced(&self) -> bool {
        self.synced
    }

    /// The line being sung at `position`: the last to start by then. Always `None`
    /// for plain lyrics.
    pub fn line_at(&self, position: Duration) -> Option<usize> {
        self.started_by(position).checked_sub(1)
    }

    /// When the next line after those started by `position` starts.
    pub fn next_start(&self, position: Duration) -> Option<Duration> {
        self.lines.get(self.started_by(position))?.start
    }

    fn started_by(&self, position: Duration) -> usize {
        if !self.synced {
            return 0;
        }
        self.lines
            .partition_point(|line| line.start.is_some_and(|start| start <= position))
    }
}

enum Line {
    Timed(Vec<Duration>, String),
    Plain(String),
    Offset(i64),
    /// A tag about the song, such as `[ar:Artist]`.
    Info,
}

fn parse_line(line: &str) -> Line {
    let mut times = Vec::new();
    let mut rest = line;
    while let Some(tag) = rest
        .strip_prefix('[')
        .and_then(|after| after.split_once(']'))
    {
        let (inside, after) = tag;
        if let Some(time) = parse_time(inside) {
            times.push(time);
        } else if times.is_empty() && is_info_tag(inside) {
            return match inside.split_once(':') {
                Some((name, value)) if name.trim().eq_ignore_ascii_case("offset") => {
                    value.trim().parse().map_or(Line::Info, Line::Offset)
                }
                _ => Line::Info,
            };
        } else {
            // Not a tag, such as "[Chorus]": part of the words.
            break;
        }
        rest = after;
    }
    let words = strip_word_times(rest).trim().to_owned();
    if times.is_empty() {
        Line::Plain(words)
    } else {
        Line::Timed(times, words)
    }
}

/// `mm:ss`, `mm:ss.xx` or `mm:ss:xx`, with any number of minutes and up to three
/// digits of fractions.
fn parse_time(text: &str) -> Option<Duration> {
    let (minutes, rest) = text.split_once(':')?;
    let (seconds, fraction) = match rest.split_once(['.', ':']) {
        Some((seconds, fraction)) => (seconds, fraction),
        None => (rest, ""),
    };
    let digits = |part: &str, max_len: usize| {
        (!part.is_empty() && part.len() <= max_len && part.bytes().all(|b| b.is_ascii_digit()))
            .then(|| part.parse::<u64>().ok())
            .flatten()
    };
    let minutes = digits(minutes, 6)?;
    let seconds = digits(seconds, 2).filter(|&seconds| seconds < 60)?;
    let millis = if fraction.is_empty() {
        0
    } else {
        digits(fraction, 3)? * 10u64.pow(3 - fraction.len() as u32)
    };
    Some(Duration::from_millis(
        (minutes * 60 + seconds) * 1000 + millis,
    ))
}

/// LRC's tags about the song. Others, like `[Chorus: Someone]`, are words.
const INFO_TAGS: [&str; 11] = [
    "ar", "al", "ti", "au", "by", "length", "offset", "re", "ve", "tool", "la",
];

/// `[name:value]` with one of [`INFO_TAGS`] as its name.
fn is_info_tag(inside: &str) -> bool {
    inside.split_once(':').is_some_and(|(name, _)| {
        let name = name.trim();
        INFO_TAGS.iter().any(|tag| tag.eq_ignore_ascii_case(name))
    })
}

fn strip_word_times(text: &str) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        match rest[open + 1..].split_once('>') {
            Some((inside, after)) if parse_time(inside).is_some() => {
                kept.push_str(&rest[..open]);
                rest = after;
            }
            _ => {
                kept.push_str(&rest[..=open]);
                rest = &rest[open + 1..];
            }
        }
    }
    kept.push_str(rest);
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    fn texts(lyrics: &Lyrics) -> Vec<&str> {
        lyrics
            .lines()
            .iter()
            .map(|line| line.text.as_str())
            .collect()
    }

    fn starts(lyrics: &Lyrics) -> Vec<u64> {
        let start = |line: &LyricLine| line.start.unwrap().as_millis() as u64;
        lyrics.lines().iter().map(start).collect()
    }

    #[test]
    fn timed_lines_are_read_in_order() {
        let lyrics = Lyrics::parse(
            "[ar:Someone]\r\n[ti:A Song]\r\n[00:04.03] Shines on your skin\r\n\
             [00:01.68] The mornin' sun\r\n[00:07.6]'Cause\r\n[01:02] Late\r\n",
        )
        .unwrap();
        assert!(lyrics.is_synced());
        assert_eq!(
            texts(&lyrics),
            ["The mornin' sun", "Shines on your skin", "'Cause", "Late"]
        );
        assert_eq!(starts(&lyrics), [1_680, 4_030, 7_600, 62_000]);
    }

    #[test]
    fn plain_lyrics_keep_single_breaks_between_verses() {
        let lyrics = Lyrics::parse("\u{feff}\n\nOne\nTwo\n\n\n\nThree\n\n").unwrap();
        assert!(!lyrics.is_synced());
        assert_eq!(texts(&lyrics), ["One", "Two", "", "Three"]);
        assert_eq!(lyrics.line_at(ms(60_000)), None);
    }

    #[test]
    fn a_line_can_repeat_at_several_times() {
        let lyrics = Lyrics::parse("[00:10.00][00:30.00]Chorus\n[00:20.00]Verse").unwrap();
        assert_eq!(texts(&lyrics), ["Chorus", "Verse", "Chorus"]);
        assert_eq!(starts(&lyrics), [10_000, 20_000, 30_000]);
    }

    #[test]
    fn pauses_are_kept_once() {
        let lyrics =
            Lyrics::parse("[00:00.50]\n[00:01.00]Hi\n[00:02.00]\n[00:03.00] \n[00:04.00]Bye")
                .unwrap();
        assert_eq!(texts(&lyrics), ["Hi", "", "Bye"]);
    }

    #[test]
    fn untimed_lines_in_timed_lyrics_are_left_out() {
        let lyrics = Lyrics::parse("Lyrics by someone\n[00:01.00]Words").unwrap();
        assert_eq!(texts(&lyrics), ["Words"]);
    }

    #[test]
    fn offset_moves_every_line() {
        let sooner = Lyrics::parse("[offset:+500]\n[00:01.00]A\n[00:00.20]B").unwrap();
        assert_eq!(starts(&sooner), [0, 500]);
        let later = Lyrics::parse("[offset: -250]\n[00:01.00]A").unwrap();
        assert_eq!(starts(&later), [1_250]);
    }

    #[test]
    fn word_times_and_odd_time_formats() {
        let lyrics =
            Lyrics::parse("[00:01:50]<00:01.50>Word <00:02.00>by <00:02.5>word\n[100:00.123]Long")
                .unwrap();
        assert_eq!(texts(&lyrics), ["Word by word", "Long"]);
        assert_eq!(starts(&lyrics), [1_500, 6_000_123]);
    }

    #[test]
    fn brackets_that_are_not_tags_stay_in_the_words() {
        let lyrics = Lyrics::parse("[00:01.00][Chorus] <3 a <b>\n[Verse 2]").unwrap();
        assert_eq!(texts(&lyrics), ["[Chorus] <3 a <b>"]);
        let plain = Lyrics::parse("[ar:Someone]\n[Intro]\n[Chorus: Someone]\nla la").unwrap();
        assert_eq!(texts(&plain), ["[Intro]", "[Chorus: Someone]", "la la"]);
    }

    #[test]
    fn nothing_to_sing_is_no_lyrics() {
        assert_eq!(Lyrics::parse(""), None);
        assert_eq!(Lyrics::parse("\n  \n\n"), None);
        assert_eq!(Lyrics::parse("[ar:Someone]\n[length:03:20]"), None);
        assert_eq!(Lyrics::parse("[00:01.00]\n[00:02.00]"), None);
    }

    #[test]
    fn follows_the_line_being_sung() {
        let lyrics = Lyrics::parse("[00:01.00]A\n[00:02.00]B\n[00:04.00]C").unwrap();
        assert_eq!(lyrics.line_at(ms(0)), None);
        assert_eq!(lyrics.next_start(ms(0)), Some(ms(1_000)));
        assert_eq!(lyrics.line_at(ms(1_000)), Some(0));
        assert_eq!(lyrics.line_at(ms(3_999)), Some(1));
        assert_eq!(lyrics.next_start(ms(3_999)), Some(ms(4_000)));
        assert_eq!(lyrics.line_at(ms(400_000)), Some(2));
        assert_eq!(lyrics.next_start(ms(400_000)), None);
    }
}
