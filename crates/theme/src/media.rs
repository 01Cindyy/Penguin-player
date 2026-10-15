//! Resolving the `prefers-color-scheme` media queries of a theme's own stylesheet for
//! one colour scheme (see [`crate::Theme::to_css_for_scheme`]).
//!
//! The stylesheet is user input, so this takes time linear in its length whatever it
//! contains, and uses no recursion however deeply its blocks are nested.

use std::ops::Range;

use crate::css_scan::opaque_at;

const MEDIA_KEYWORD: &[u8] = b"@media";
/// The two conditions that are resolved, without whitespace and in lower case.
const DARK_QUERY: &[u8] = b"(prefers-color-scheme:dark)";
const LIGHT_QUERY: &[u8] = b"(prefers-color-scheme:light)";

/// `css` for one scheme: the rules of `@media (prefers-color-scheme: light)` and
/// `@media (prefers-color-scheme: dark)` blocks are kept, unwrapped, for the matching
/// scheme and dropped for the other. Every other part of the stylesheet, including
/// other media queries and blocks that are never closed, is left exactly as it is.
pub(crate) fn resolve_color_scheme(css: &str, dark: bool) -> String {
    let mut splicer = Splicer::new(css);
    // Closing braces of the unwrapped blocks around the current position, innermost last.
    let mut unwrapped_closes = Vec::new();
    for block in color_scheme_blocks(css) {
        let Some(close) = block.close else {
            continue;
        };
        if block.start < splicer.position {
            // Inside a block that was dropped.
            continue;
        }
        while let Some(&end) = unwrapped_closes.last().filter(|&&end| end < block.start) {
            unwrapped_closes.pop();
            splicer.leave_out(end..end + 1);
        }
        if block.dark == dark {
            splicer.leave_out(block.start..block.open + 1);
            unwrapped_closes.push(close);
        } else {
            splicer.leave_out(block.start..close + 1);
        }
    }
    while let Some(end) = unwrapped_closes.pop() {
        splicer.leave_out(end..end + 1);
    }
    splicer.finish()
}

/// Copies a text, leaving out ranges given in increasing order.
struct Splicer<'a> {
    text: &'a str,
    copied: String,
    /// `text[..position]` has been copied or left out.
    position: usize,
}

impl<'a> Splicer<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            copied: String::with_capacity(text.len()),
            position: 0,
        }
    }

    /// Copies the text up to `range` and skips `range`.
    fn leave_out(&mut self, range: Range<usize>) {
        self.copied.push_str(&self.text[self.position..range.start]);
        self.position = range.end;
    }

    fn finish(mut self) -> String {
        self.copied.push_str(&self.text[self.position..]);
        self.copied
    }
}

/// An `@media (prefers-color-scheme: ...) { ... }` block, as byte offsets.
struct ColorSchemeBlock {
    dark: bool,
    /// Where `@media` starts.
    start: usize,
    open: usize,
    /// `None` if the block is never closed.
    close: Option<usize>,
}

/// The colour-scheme blocks of `css`, in the order they start: outer blocks come
/// before the blocks inside them. Comments, strings and escapes are skipped.
fn color_scheme_blocks(css: &str) -> Vec<ColorSchemeBlock> {
    let bytes = css.as_bytes();
    let mut blocks = Vec::new();
    let mut depth = 0usize;
    // The depth and index of each colour-scheme block that is still open.
    let mut open_blocks: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if let Some((_, end)) = opaque_at(bytes, i) {
            i = end;
            continue;
        }
        match bytes[i] {
            b'@' => {
                if let Some((dark, open)) = color_scheme_query(bytes, i) {
                    depth += 1;
                    open_blocks.push((depth, blocks.len()));
                    blocks.push(ColorSchemeBlock {
                        dark,
                        start: i,
                        open,
                        close: None,
                    });
                    i = open;
                }
            }
            b'{' => depth += 1,
            // A stray `}` outside any block closes nothing.
            b'}' if depth > 0 => {
                if let Some(&(block_depth, index)) = open_blocks.last()
                    && block_depth == depth
                {
                    blocks[index].close = Some(i);
                    open_blocks.pop();
                }
                depth -= 1;
            }
            _ => {}
        }
        i += 1;
    }
    blocks
}

/// If a `@media` rule whose whole condition is a single `prefers-color-scheme` test
/// starts at `at`, whether it tests for dark and the offset of its `{`.
///
/// The condition is compared ignoring whitespace and case; anything unusual, such as a
/// comment, makes it match nothing. Reading stops at the first byte that cannot be part
/// of the condition, so it never goes past the next rule.
fn color_scheme_query(bytes: &[u8], at: usize) -> Option<(bool, usize)> {
    let keyword_end = at + MEDIA_KEYWORD.len();
    let is_keyword = bytes
        .get(at..keyword_end)
        .is_some_and(|word| word.eq_ignore_ascii_case(MEDIA_KEYWORD))
        && !bytes.get(keyword_end).copied().is_some_and(is_name_byte);
    if !is_keyword {
        return None;
    }
    let (mut dark, mut light, mut matched) = (true, true, 0);
    for (offset, &byte) in bytes[keyword_end..].iter().enumerate() {
        if byte.is_ascii_whitespace() {
            continue;
        }
        if byte == b'{' {
            let open = keyword_end + offset;
            return if dark && matched == DARK_QUERY.len() {
                Some((true, open))
            } else if light && matched == LIGHT_QUERY.len() {
                Some((false, open))
            } else {
                None
            };
        }
        let byte = byte.to_ascii_lowercase();
        dark &= DARK_QUERY.get(matched) == Some(&byte);
        light &= LIGHT_QUERY.get(matched) == Some(&byte);
        if !dark && !light {
            return None;
        }
        matched += 1;
    }
    None
}

/// Whether `byte` can continue a CSS name such as `media` in `@media-foo`.
fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'\\') || byte >= 0x80
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    const STYLESHEET: &str = "\
:root { --x: #111; }
@media (prefers-color-scheme: dark) {
  :root { --x: #eee; }
  .a { color: var(--x); }
}
@media (prefers-color-scheme: light) { .b { color: red; } }
.c { color: blue; }
";

    #[test]
    fn keeps_only_the_matching_scheme() {
        let dark = resolve_color_scheme(STYLESHEET, true);
        assert_eq!(
            dark,
            ":root { --x: #111; }\n\n  :root { --x: #eee; }\n  .a { color: var(--x); }\n\n\
             \n.c { color: blue; }\n"
        );
        let light = resolve_color_scheme(STYLESHEET, false);
        assert_eq!(
            light,
            ":root { --x: #111; }\n\n .b { color: red; } \n.c { color: blue; }\n"
        );
        assert!(!dark.contains("@media") && !light.contains("@media"));
    }

    #[test]
    fn spelling_and_spacing_of_the_query_do_not_matter() {
        let css = "@MEDIA(Prefers-Color-Scheme :DARK ){ .a { color: red; } }";
        assert_eq!(resolve_color_scheme(css, true), " .a { color: red; } ");
        assert_eq!(resolve_color_scheme(css, false), "");
    }

    #[test]
    fn other_css_is_left_alone() {
        for css in [
            "@media (prefers-contrast: more) { .a { color: red; } }",
            "@media (prefers-color-scheme: dark) and (min-width: 1px) { .a {} }",
            "@media-like {}",
            "/* @media (prefers-color-scheme: dark) { */ .a {}",
            ".a { content: \"@media (prefers-color-scheme: dark) {\"; }",
            "@media (prefers-color-scheme: dark) { .a { color: red; }",
            "@media (prefers-color-scheme: /* dark */ light) { .a {} }",
            "@media (prefers-color-scheme: dark)",
        ] {
            assert_eq!(resolve_color_scheme(css, true), css);
            assert_eq!(resolve_color_scheme(css, false), css);
        }
    }

    #[test]
    fn braces_in_comments_and_strings_do_not_end_the_block() {
        let css = "@media (prefers-color-scheme: dark) { /* } */ .a { content: \"}\"; } }.b {}";
        assert_eq!(
            resolve_color_scheme(css, true),
            " /* } */ .a { content: \"}\"; } .b {}"
        );
        assert_eq!(resolve_color_scheme(css, false), ".b {}");
    }

    #[test]
    fn nested_queries_are_resolved_too() {
        let css = "@media (prefers-color-scheme: dark) { @media (prefers-color-scheme: light) { .x {} } .y {} }";
        assert_eq!(resolve_color_scheme(css, true), "  .y {} ");
    }

    #[test]
    fn queries_inside_unclosed_or_other_blocks_are_resolved() {
        let css = "} @media (min-width: 1px) { @media (prefers-color-scheme: light) { .x {} } }";
        assert_eq!(
            resolve_color_scheme(css, false),
            "} @media (min-width: 1px) {  .x {}  }"
        );
        let unclosed = "@media (prefers-color-scheme: dark) { \
                        @media (prefers-color-scheme: light) { .x {} } .y {";
        assert_eq!(
            resolve_color_scheme(unclosed, true),
            "@media (prefers-color-scheme: dark) {  .y {"
        );
    }

    /// Stylesheets of the largest size a theme may have, built to make a naive parser
    /// rescan the text for every rule or nesting level.
    fn hostile_stylesheets() -> Vec<(&'static str, String)> {
        const SIZE: usize = 512 * 1024;
        let open = "@media (prefers-color-scheme:dark){";
        let depth = SIZE / (open.len() + 1);
        let nested = format!("{}{}", open.repeat(depth), "}".repeat(depth));
        let wrapping_unclosed = format!("{}/*", open.repeat(SIZE / open.len()));
        let sequential = "@media (prefers-color-scheme: light) { a {} } ";
        vec![
            ("nested", nested),
            ("unclosed", open.repeat(SIZE / open.len())),
            ("wrapping an unclosed comment", wrapping_unclosed),
            ("bare keywords", "@media ".repeat(SIZE / 7)),
            (
                "braces",
                format!("{}{}", "{".repeat(SIZE / 2), "}".repeat(SIZE / 2)),
            ),
            ("sequential", sequential.repeat(SIZE / sequential.len())),
        ]
    }

    #[test]
    fn hostile_stylesheets_resolve_quickly() {
        // Linear time takes milliseconds here even in debug builds; quadratic parsing
        // takes several seconds for some of these, and recursion overflows the stack on
        // deep nesting.
        let limit = Duration::from_secs(2);
        for (name, css) in hostile_stylesheets() {
            let started = Instant::now();
            let dark = resolve_color_scheme(&css, true);
            let light = resolve_color_scheme(&css, false);
            let elapsed = started.elapsed();
            assert!(elapsed < limit, "{name}: took {elapsed:?}");
            assert!(
                dark.len() <= css.len() && light.len() <= css.len(),
                "{name}"
            );
        }
    }

    #[test]
    fn deep_nesting_is_resolved_completely() {
        let depth = 10_000;
        let css = format!(
            "{}.x {{}}{}",
            "@media (prefers-color-scheme: dark) {".repeat(depth),
            "}".repeat(depth)
        );
        assert_eq!(resolve_color_scheme(&css, true), ".x {}");
        assert_eq!(resolve_color_scheme(&css, false), "");
    }
}
