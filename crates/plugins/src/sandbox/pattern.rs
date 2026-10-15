//! Lua 5.4's pattern matching, ported from `lstrlib.c` so it can be stopped.
//!
//! Lua's own matcher runs entirely in C, where the time limit can't interrupt it, and
//! a careless pattern can backtrack for hours on a long string (`(.-)%s*$` is quadratic,
//! `.-.-.-x` far worse). This port follows the C code step by step, so it matches
//! exactly the same things, but counts its work and asks `out_of_time` every few
//! thousand steps.
//!
//! Positions are byte offsets. Reading one past the end yields `0`, standing in for the
//! NUL terminator the C code relies on.

use std::ops::Range;

/// Lua's limit on captures in one pattern.
const MAX_CAPTURES: usize = 32;
/// Lua's limit on nested pattern items (`MAXCCALLS` in `lstrlib.c`).
const MAX_DEPTH: usize = 200;
const STEPS_PER_TIME_CHECK: u32 = 4096;
const ESCAPE: u8 = b'%';
const SPECIALS: &[u8] = b"^$*+?.([%-";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PatternError {
    /// The pattern or replacement is malformed; the message is Lua's own.
    Invalid(String),
    /// The plugin ran out of time while matching.
    OutOfTime,
}

fn invalid(message: impl Into<String>) -> PatternError {
    PatternError::Invalid(message.into())
}

/// What a capture holds: a slice of the subject, or a position from `()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Capture {
    Text(Range<usize>),
    /// A 1-based position, as Lua reports it.
    Position(usize),
}

#[derive(Debug, Clone, Copy)]
enum CaptureLength {
    Unfinished,
    Position,
    Closed(usize),
}

#[derive(Debug, Clone, Copy)]
struct CaptureState {
    start: usize,
    length: CaptureLength,
}

/// Whether `pattern` has no special characters, so a plain substring search finds
/// the same thing.
pub(crate) fn is_plain(pattern: &[u8]) -> bool {
    !pattern.iter().any(|byte| SPECIALS.contains(byte))
}

/// Splits a leading `^` off `pattern`: whether the match is anchored, and the rest.
pub(crate) fn split_anchor(pattern: &[u8]) -> (bool, &[u8]) {
    match pattern.strip_prefix(b"^") {
        Some(rest) => (true, rest),
        None => (false, pattern),
    }
}

/// Matches one pattern against one subject, at whichever positions the caller tries.
pub(crate) struct Matcher<'a> {
    subject: &'a [u8],
    pattern: &'a [u8],
    out_of_time: &'a dyn Fn() -> bool,
    depth_left: usize,
    level: usize,
    captures: [CaptureState; MAX_CAPTURES],
    steps: u32,
}

impl<'a> Matcher<'a> {
    /// `pattern` must not include the anchor; see [`split_anchor`].
    pub fn new(subject: &'a [u8], pattern: &'a [u8], out_of_time: &'a dyn Fn() -> bool) -> Self {
        Self {
            subject,
            pattern,
            out_of_time,
            depth_left: MAX_DEPTH,
            level: 0,
            captures: [CaptureState {
                start: 0,
                length: CaptureLength::Unfinished,
            }; MAX_CAPTURES],
            steps: 0,
        }
    }

    /// Tries to match the whole pattern starting at `start`, returning where the match
    /// ends. Captures of a successful match stay available until the next call.
    pub fn match_at(&mut self, start: usize) -> Result<Option<usize>, PatternError> {
        self.depth_left = MAX_DEPTH;
        self.level = 0;
        self.match_here(start, 0)
    }

    /// The captures of the last match `start..end`. Without explicit captures the whole
    /// match counts as one when `whole_match` is set (as for `string.match`).
    pub fn captures(
        &self,
        start: usize,
        end: usize,
        whole_match: bool,
    ) -> Result<Vec<Capture>, PatternError> {
        let count = if self.level == 0 && whole_match {
            1
        } else {
            self.level
        };
        (0..count).map(|i| self.capture(i, start, end)).collect()
    }

    /// Capture `index` (0-based) of the last match `start..end`; index 0 is the whole
    /// match when the pattern has no captures.
    pub fn capture(&self, index: usize, start: usize, end: usize) -> Result<Capture, PatternError> {
        if index >= self.level {
            return if index == 0 {
                Ok(Capture::Text(start..end))
            } else {
                Err(invalid(format!("invalid capture index %{}", index + 1)))
            };
        }
        let capture = self.captures[index];
        match capture.length {
            CaptureLength::Unfinished => Err(invalid("unfinished capture")),
            CaptureLength::Position => Ok(Capture::Position(capture.start + 1)),
            CaptureLength::Closed(length) => {
                Ok(Capture::Text(capture.start..capture.start + length))
            }
        }
    }

    fn pattern_byte(&self, p: usize) -> u8 {
        self.pattern.get(p).copied().unwrap_or(0)
    }

    fn subject_byte(&self, s: usize) -> u8 {
        self.subject.get(s).copied().unwrap_or(0)
    }

    /// Counts `amount` steps of work and fails once the plugin is out of time.
    fn work(&mut self, amount: usize) -> Result<(), PatternError> {
        let before = self.steps;
        self.steps = self
            .steps
            .wrapping_add(u32::try_from(amount).unwrap_or(u32::MAX));
        let crossed_check = before / STEPS_PER_TIME_CHECK != self.steps / STEPS_PER_TIME_CHECK
            || amount >= STEPS_PER_TIME_CHECK as usize;
        if crossed_check && (self.out_of_time)() {
            Err(PatternError::OutOfTime)
        } else {
            Ok(())
        }
    }

    /// `match` in `lstrlib.c`: matches `pattern[p..]` at `subject[s..]`.
    fn match_here(&mut self, s: usize, p: usize) -> Result<Option<usize>, PatternError> {
        if self.depth_left == 0 {
            return Err(invalid("pattern too complex"));
        }
        self.depth_left -= 1;
        let result = self.match_items(s, p);
        self.depth_left += 1;
        result
    }

    /// The body of `match_here`; the loop stands in for the C code's `goto init`.
    fn match_items(&mut self, mut s: usize, mut p: usize) -> Result<Option<usize>, PatternError> {
        loop {
            self.work(1)?;
            if p == self.pattern.len() {
                return Ok(Some(s));
            }
            match self.pattern[p] {
                b'(' => {
                    return if self.pattern_byte(p + 1) == b')' {
                        self.start_capture(s, p + 2, CaptureLength::Position)
                    } else {
                        self.start_capture(s, p + 1, CaptureLength::Unfinished)
                    };
                }
                b')' => return self.end_capture(s, p + 1),
                b'$' if p + 1 == self.pattern.len() => {
                    return Ok((s == self.subject.len()).then_some(s));
                }
                ESCAPE if self.pattern_byte(p + 1) == b'b' => match self.match_balance(s, p + 2)? {
                    Some(end) => {
                        s = end;
                        p += 4;
                    }
                    None => return Ok(None),
                },
                ESCAPE if self.pattern_byte(p + 1) == b'f' => {
                    p += 2;
                    if self.pattern_byte(p) != b'[' {
                        return Err(invalid("missing '[' after '%f' in pattern"));
                    }
                    let end = self.class_end(p)?;
                    let previous = if s == 0 { 0 } else { self.subject[s - 1] };
                    if !self.matches_set(previous, p, end - 1)
                        && self.matches_set(self.subject_byte(s), p, end - 1)
                    {
                        p = end;
                    } else {
                        return Ok(None);
                    }
                }
                ESCAPE if self.pattern_byte(p + 1).is_ascii_digit() => {
                    match self.match_back_reference(s, self.pattern_byte(p + 1))? {
                        Some(end) => {
                            s = end;
                            p += 2;
                        }
                        None => return Ok(None),
                    }
                }
                _ => {
                    let end = self.class_end(p)?;
                    let suffix = self.pattern_byte(end);
                    if !self.single_match(s, p, end)? {
                        if matches!(suffix, b'*' | b'?' | b'-') {
                            p = end + 1;
                            continue;
                        }
                        return Ok(None);
                    }
                    match suffix {
                        b'?' => {
                            if let Some(matched) = self.match_here(s + 1, end + 1)? {
                                return Ok(Some(matched));
                            }
                            p = end + 1;
                        }
                        b'+' => return self.max_expand(s + 1, p, end),
                        b'*' => return self.max_expand(s, p, end),
                        b'-' => return self.min_expand(s, p, end),
                        _ => {
                            s += 1;
                            p = end;
                        }
                    }
                }
            }
        }
    }

    /// Where the single-character class starting at `pattern[p]` ends.
    fn class_end(&self, mut p: usize) -> Result<usize, PatternError> {
        let first = self.pattern[p];
        p += 1;
        match first {
            ESCAPE => {
                if p == self.pattern.len() {
                    return Err(invalid("malformed pattern (ends with '%')"));
                }
                Ok(p + 1)
            }
            b'[' => {
                if self.pattern_byte(p) == b'^' {
                    p += 1;
                }
                // The first character is part of the set even if it is `]`.
                loop {
                    if p == self.pattern.len() {
                        return Err(invalid("malformed pattern (missing ']')"));
                    }
                    let byte = self.pattern[p];
                    p += 1;
                    if byte == ESCAPE && p < self.pattern.len() {
                        p += 1;
                    }
                    if self.pattern_byte(p) == b']' {
                        return Ok(p + 1);
                    }
                }
            }
            _ => Ok(p),
        }
    }

    /// Whether `subject[s]` matches the class `pattern[p..end]`.
    fn single_match(&mut self, s: usize, p: usize, end: usize) -> Result<bool, PatternError> {
        self.work(1)?;
        let Some(&byte) = self.subject.get(s) else {
            return Ok(false);
        };
        Ok(match self.pattern[p] {
            b'.' => true,
            ESCAPE => matches_class(byte, self.pattern_byte(p + 1)),
            b'[' => self.matches_set(byte, p, end - 1),
            literal => literal == byte,
        })
    }

    /// Whether `byte` is in the set `[...]` from `pattern[p]` (the `[`) to
    /// `pattern[close]` (the `]`).
    fn matches_set(&self, byte: u8, mut p: usize, close: usize) -> bool {
        let mut found = true;
        if self.pattern_byte(p + 1) == b'^' {
            found = false;
            p += 1;
        }
        loop {
            p += 1;
            if p >= close {
                return !found;
            }
            let item = self.pattern[p];
            if item == ESCAPE {
                p += 1;
                if matches_class(byte, self.pattern_byte(p)) {
                    return found;
                }
            } else if self.pattern_byte(p + 1) == b'-' && p + 2 < close {
                p += 2;
                if item <= byte && byte <= self.pattern[p] {
                    return found;
                }
            } else if item == byte {
                return found;
            }
        }
    }

    /// `%bxy`: a balanced run from `x` to the matching `y`.
    fn match_balance(&mut self, s: usize, p: usize) -> Result<Option<usize>, PatternError> {
        if p + 1 >= self.pattern.len() {
            return Err(invalid("malformed pattern (missing arguments to '%b')"));
        }
        let (open, close) = (self.pattern[p], self.pattern[p + 1]);
        if self.subject_byte(s) != open {
            return Ok(None);
        }
        let mut depth = 1;
        for (offset, &byte) in self.subject.iter().enumerate().skip(s + 1) {
            self.work(1)?;
            if byte == close {
                depth -= 1;
                if depth == 0 {
                    return Ok(Some(offset + 1));
                }
            } else if byte == open {
                depth += 1;
            }
        }
        Ok(None)
    }

    /// `*` and `+`: as many repetitions as possible, backing off one at a time.
    fn max_expand(
        &mut self,
        s: usize,
        p: usize,
        end: usize,
    ) -> Result<Option<usize>, PatternError> {
        let mut count = 0;
        while self.single_match(s + count, p, end)? {
            count += 1;
        }
        loop {
            if let Some(matched) = self.match_here(s + count, end + 1)? {
                return Ok(Some(matched));
            }
            if count == 0 {
                return Ok(None);
            }
            count -= 1;
        }
    }

    /// `-`: as few repetitions as possible, adding one at a time.
    fn min_expand(
        &mut self,
        mut s: usize,
        p: usize,
        end: usize,
    ) -> Result<Option<usize>, PatternError> {
        loop {
            if let Some(matched) = self.match_here(s, end + 1)? {
                return Ok(Some(matched));
            }
            if !self.single_match(s, p, end)? {
                return Ok(None);
            }
            s += 1;
        }
    }

    fn start_capture(
        &mut self,
        s: usize,
        p: usize,
        length: CaptureLength,
    ) -> Result<Option<usize>, PatternError> {
        if self.level >= MAX_CAPTURES {
            return Err(invalid("too many captures"));
        }
        self.captures[self.level] = CaptureState { start: s, length };
        self.level += 1;
        let result = self.match_here(s, p)?;
        if result.is_none() {
            self.level -= 1;
        }
        Ok(result)
    }

    fn end_capture(&mut self, s: usize, p: usize) -> Result<Option<usize>, PatternError> {
        let index = (0..self.level)
            .rev()
            .find(|&i| matches!(self.captures[i].length, CaptureLength::Unfinished))
            .ok_or_else(|| invalid("invalid pattern capture"))?;
        self.captures[index].length = CaptureLength::Closed(s - self.captures[index].start);
        let result = self.match_here(s, p)?;
        if result.is_none() {
            self.captures[index].length = CaptureLength::Unfinished;
        }
        Ok(result)
    }

    /// `%1` to `%9`: the same text as an earlier capture.
    fn match_back_reference(&mut self, s: usize, digit: u8) -> Result<Option<usize>, PatternError> {
        let index = usize::from(digit).wrapping_sub(usize::from(b'1'));
        let capture = self
            .captures
            .get(index)
            .copied()
            .filter(|_| index < self.level)
            .filter(|capture| !matches!(capture.length, CaptureLength::Unfinished))
            .ok_or_else(|| {
                invalid(format!(
                    "invalid capture index %{}",
                    i32::from(digit) - i32::from(b'0')
                ))
            })?;
        // A position capture never matches, as in Lua.
        let CaptureLength::Closed(length) = capture.length else {
            return Ok(None);
        };
        self.work(length)?;
        let earlier = &self.subject[capture.start..capture.start + length];
        Ok(self
            .subject
            .get(s..s + length)
            .filter(|text| *text == earlier)
            .map(|_| s + length))
    }
}

/// Whether `byte` is in the class `%<class>` (`%a`, `%d`, ...; upper case negates).
/// Lua uses the C library's `<ctype.h>`; this uses its "C" locale meaning, which is
/// what plugins see on every system.
fn matches_class(byte: u8, class: u8) -> bool {
    let matches = match class.to_ascii_lowercase() {
        b'a' => byte.is_ascii_alphabetic(),
        b'c' => byte.is_ascii_control(),
        b'd' => byte.is_ascii_digit(),
        b'g' => byte.is_ascii_graphic(),
        b'l' => byte.is_ascii_lowercase(),
        b'p' => byte.is_ascii_punctuation(),
        // Unlike `u8::is_ascii_whitespace`, C's `isspace` includes the vertical tab.
        b's' => matches!(byte, b' ' | b'\t'..=b'\r'),
        b'u' => byte.is_ascii_uppercase(),
        b'w' => byte.is_ascii_alphanumeric(),
        b'x' => byte.is_ascii_hexdigit(),
        b'z' => byte == 0,
        _ => return class == byte,
    };
    if class.is_ascii_lowercase() {
        matches
    } else {
        !matches
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    fn never() -> bool {
        false
    }

    /// The first match of `pattern` in `subject`, as `string.find` reports it:
    /// 1-based start and end, then the explicit captures.
    fn find(
        subject: &str,
        pattern: &str,
    ) -> Result<Option<(usize, usize, Vec<Capture>)>, PatternError> {
        let (anchored, pattern) = split_anchor(pattern.as_bytes());
        let mut matcher = Matcher::new(subject.as_bytes(), pattern, &never);
        for start in 0..=subject.len() {
            if let Some(end) = matcher.match_at(start)? {
                return Ok(Some((start + 1, end, matcher.captures(start, end, false)?)));
            }
            if anchored {
                break;
            }
        }
        Ok(None)
    }

    fn text(range: Range<usize>) -> Capture {
        Capture::Text(range)
    }

    #[test]
    fn matches_like_lua() {
        assert_eq!(find("hello world", "o w"), Ok(Some((5, 7, vec![]))));
        assert_eq!(find("hello", "l+"), Ok(Some((3, 4, vec![]))));
        assert_eq!(
            find("hello", "^h(.-)o$"),
            Ok(Some((1, 5, vec![text(1..4)])))
        );
        assert_eq!(
            find("key = value", "(%w+)%s*=%s*(%w+)"),
            Ok(Some((1, 11, vec![text(0..3), text(6..11)])))
        );
        assert_eq!(find("f(a(b)c)d", "%b()"), Ok(Some((2, 8, vec![]))));
        assert_eq!(
            find("THE (quick) fox", "%f[%a]%a+"),
            Ok(Some((1, 3, vec![])))
        );
        assert_eq!(
            find("abcabc", "(abc)%1"),
            Ok(Some((1, 6, vec![text(0..3)])))
        );
        assert_eq!(
            find("abc", "()b()"),
            Ok(Some((
                2,
                2,
                vec![Capture::Position(2), Capture::Position(3)]
            )))
        );
        assert_eq!(find("a]b", "[]]"), Ok(Some((2, 2, vec![]))));
        assert_eq!(find("x-y", "[%-]"), Ok(Some((2, 2, vec![]))));
        assert_eq!(find("a\x0bb", "%s"), Ok(Some((2, 2, vec![]))));
        assert_eq!(find("abc", "^b"), Ok(None));
        assert_eq!(find("", "$"), Ok(Some((1, 0, vec![]))));
    }

    #[test]
    fn malformed_patterns_report_lua_messages() {
        for (pattern, message) in [
            ("%", "malformed pattern (ends with '%')"),
            ("[a", "malformed pattern (missing ']')"),
            ("%b", "malformed pattern (missing arguments to '%b')"),
            ("%f", "missing '[' after '%f' in pattern"),
            ("(a", "unfinished capture"),
            ("a)", "invalid pattern capture"),
            ("%1", "invalid capture index %1"),
        ] {
            let result = find("aaa", pattern);
            assert_eq!(result, Err(invalid(message)), "{pattern}");
        }
        // Like Lua, `?` only recurses when its item matches.
        let deep = "a?".repeat(MAX_DEPTH + 1);
        assert_eq!(find("", &deep), Ok(Some((1, 0, vec![]))));
        let subject = "a".repeat(MAX_DEPTH + 1);
        assert_eq!(find(&subject, &deep), Err(invalid("pattern too complex")));
        let many = "()".repeat(MAX_CAPTURES + 1);
        assert_eq!(find("", &many), Err(invalid("too many captures")));
    }

    #[test]
    fn runaway_matches_stop_when_out_of_time() {
        let checks = Cell::new(0);
        let out_of_time = || {
            checks.set(checks.get() + 1);
            checks.get() > 3
        };
        let subject = "a".repeat(5000);
        let mut matcher = Matcher::new(subject.as_bytes(), b".-.-.-.-.-.-b", &out_of_time);
        assert_eq!(matcher.match_at(0), Err(PatternError::OutOfTime));
        assert_eq!(checks.get(), 4);
    }

    #[test]
    fn plain_patterns_are_recognised() {
        assert!(is_plain(b"hello world"));
        assert!(!is_plain(b"a.b"));
        assert!(!is_plain(b"50%"));
    }
}
