//! Finding the comments, strings and escapes of a stylesheet the way GTK's tokenizer
//! reads them, so that a `/*`, quote or brace inside one of them is not taken for CSS.

/// A part of a stylesheet whose text is not CSS syntax.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Opaque {
    Comment,
    String,
    /// A backslash and the character it escapes.
    Escape,
}

/// If a comment, string or escape starts at `i`, which one it is and the offset just
/// past it. Unterminated comments run to the end of the text and unterminated strings
/// to the end of the line, as in CSS.
pub(crate) fn opaque_at(bytes: &[u8], i: usize) -> Option<(Opaque, usize)> {
    match bytes[i] {
        b'/' if bytes.get(i + 1) == Some(&b'*') => {
            let end = bytes[i + 2..]
                .windows(2)
                .position(|pair| pair == b"*/")
                .map_or(bytes.len(), |end| i + 2 + end + 2);
            Some((Opaque::Comment, end))
        }
        b'\\' => Some((Opaque::Escape, (i + 2).min(bytes.len()))),
        quote @ (b'"' | b'\'') => {
            let mut j = i + 1;
            while j < bytes.len() {
                match bytes[j] {
                    b'\\' => j += 2,
                    b'\n' => return Some((Opaque::String, j)),
                    byte if byte == quote => return Some((Opaque::String, j + 1)),
                    _ => j += 1,
                }
            }
            Some((Opaque::String, bytes.len()))
        }
        _ => None,
    }
}

/// `css` without its comments. Strings and escapes are kept as they are, so a `/*`
/// inside a string or after a backslash starts nothing.
pub(crate) fn without_comments(css: &str) -> String {
    let bytes = css.as_bytes();
    let mut plain = String::with_capacity(css.len());
    // `css[..copied]` has been copied or left out.
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        match opaque_at(bytes, i) {
            Some((Opaque::Comment, end)) => {
                plain.push_str(&css[copied..i]);
                copied = end;
                i = end;
            }
            Some((_, end)) => i = end,
            None => i += 1,
        }
    }
    plain.push_str(&css[copied..]);
    plain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_are_removed() {
        assert_eq!(without_comments("a /* b */c/**/d /* e"), "a cd ");
        assert_eq!(without_comments("/* a */"), "");
    }

    #[test]
    fn comment_markers_in_strings_and_escapes_start_nothing() {
        for css in [
            r#".a { font-family: "/*"; } .b { color: red; } .c { font-family: "*/"; }"#,
            r".a { font-family: '/*\'*/'; }",
            r".a\/* { color: red; } */",
            "\"unterminated /*\n.b { color: red; } */",
        ] {
            assert_eq!(without_comments(css), css);
        }
    }

    #[test]
    fn spans_end_where_css_ends_them() {
        let bytes = b"\"a\\\"b\" 'c\nd' /* e */ \\";
        assert_eq!(opaque_at(bytes, 0), Some((Opaque::String, 6)));
        assert_eq!(opaque_at(bytes, 7), Some((Opaque::String, 9)));
        assert_eq!(opaque_at(bytes, 13), Some((Opaque::Comment, 20)));
        assert_eq!(opaque_at(bytes, 21), Some((Opaque::Escape, 22)));
        assert_eq!(opaque_at(bytes, 6), None);
    }
}
