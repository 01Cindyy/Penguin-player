//! Validation of theme values. They are user input that ends up inside a
//! stylesheet, so anything that could escape its declaration is rejected.

use crate::css_scan::without_comments;

/// Colour strings longer than this are rejected outright.
const MAX_COLOR_LEN: usize = 64;
const COLOR_FUNCTIONS: [&str; 4] = ["rgb", "rgba", "hsl", "hsla"];

const MAX_FONT_FAMILIES: usize = 8;
const MAX_FONT_FAMILY_LEN: usize = 100;
const GENERIC_FONT_FAMILIES: [&str; 7] = [
    "serif",
    "sans-serif",
    "monospace",
    "cursive",
    "fantasy",
    "system-ui",
    "emoji",
];

pub(crate) const MAX_WINDOW_RADIUS: u32 = 48;
pub(crate) const MAX_RADIUS: u32 = 999;
pub(crate) const MAX_SPACING: u32 = 48;
/// Windows and panels never become more transparent than this, so text stays readable.
pub(crate) const MIN_OPACITY: f32 = 0.5;

/// Returns the trimmed colour if it is a hex colour (`#rgb`, `#rgba`, `#rrggbb`,
/// `#rrggbbaa`) or an `rgb()`/`rgba()`/`hsl()`/`hsla()` call made of safe characters.
pub(crate) fn color(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_COLOR_LEN {
        return None;
    }
    let valid = match value.strip_prefix('#') {
        Some(hex) => {
            matches!(hex.len(), 3 | 4 | 6 | 8) && hex.bytes().all(|b| b.is_ascii_hexdigit())
        }
        None => is_color_function(value),
    };
    valid.then_some(value)
}

fn is_color_function(value: &str) -> bool {
    let Some((name, rest)) = value.split_once('(') else {
        return false;
    };
    let Some(arguments) = rest.strip_suffix(')') else {
        return false;
    };
    // Letters are allowed for units such as `deg`; quotes, brackets, `;`, `*` and `\`
    // are not, which rules out nested functions, comments and escapes.
    let safe = |b: u8| b.is_ascii_alphanumeric() || b" .,%/+-".contains(&b);
    COLOR_FUNCTIONS
        .iter()
        .any(|function| function.eq_ignore_ascii_case(name))
        && !arguments.trim().is_empty()
        && arguments.bytes().all(safe)
}

/// Turns a comma-separated font list into a CSS `font-family` value. Generic
/// families stay bare, every other name is quoted and escaped.
pub(crate) fn font_family(value: &str) -> Option<String> {
    let families: Vec<String> = value
        .split(',')
        .map(|family| family.trim().trim_matches(['"', '\'']).trim())
        .filter(|family| !family.is_empty() && family.chars().count() <= MAX_FONT_FAMILY_LEN)
        .take(MAX_FONT_FAMILIES)
        .map(|family| {
            let lower = family.to_ascii_lowercase();
            if GENERIC_FONT_FAMILIES.contains(&lower.as_str()) {
                lower
            } else {
                quote_css_string(family)
            }
        })
        .collect();
    (!families.is_empty()).then(|| families.join(", "))
}

fn quote_css_string(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for c in text.chars().filter(|c| !c.is_control()) {
        if matches!(c, '"' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    quoted
}

/// Makes text safe to place inside a `/* ... */` comment.
pub(crate) fn comment_text(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(120)
        .collect::<String>()
        .replace("*/", "* /")
}

/// Clamps an opacity into `min..=1.0`; non-finite values are dropped.
pub(crate) fn opacity(value: f32, min: f32) -> Option<f32> {
    value.is_finite().then(|| value.clamp(min, 1.0))
}

/// What in a theme's stylesheet would make GTK read another file, if anything:
/// `@import` or `url()`, including spellings hidden behind CSS escapes. GTK reads
/// such files whole on the UI thread, so one pointing at `/dev/zero` or a FIFO would
/// freeze the app; and a theme has no business outside its own folder anyway.
///
/// Comments are removed first, so a commented-out `@import` doesn't count. Anything
/// else that merely looks like a file reference, such as `url(` inside a string, is
/// refused as well: being strict here costs nothing.
pub(crate) fn file_reference(css: &str) -> Option<&'static str> {
    let plain = unescaped(&without_comments(css)).to_lowercase();
    if plain.contains("@import") {
        Some("@import")
    } else if plain.contains("url(") {
        Some("url()")
    } else {
        None
    }
}

/// `css` with its escapes (`\69`, `\000069 `, `\(`) replaced by the characters they
/// stand for, the way GTK's tokenizer reads names.
fn unescaped(css: &str) -> String {
    let mut plain = String::with_capacity(css.len());
    let mut chars = css.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            plain.push(c);
            continue;
        }
        let mut code = String::new();
        while code.len() < 6 {
            match chars.peek() {
                Some(digit) if digit.is_ascii_hexdigit() => code.push(*digit),
                _ => break,
            }
            chars.next();
        }
        if code.is_empty() {
            plain.extend(chars.next());
            continue;
        }
        if chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let decoded = u32::from_str_radix(&code, 16).ok().and_then(char::from_u32);
        plain.push(decoded.unwrap_or(char::REPLACEMENT_CHARACTER));
    }
    plain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_colors() {
        for value in [
            "#abc",
            "#abcd",
            "#a1b2c3",
            "#A1B2C3D4",
            "  #fff  ",
            "rgb(10, 20, 30)",
            "rgba(10, 20, 30, 0.5)",
            "rgb(10 20 30 / 50%)",
            "RGB(0 0 6 / 80%)",
            "hsl(210deg 40% 50%)",
            "hsla(210, 40%, 50%, 0.3)",
        ] {
            assert!(color(value).is_some(), "{value} should be accepted");
        }
        assert_eq!(color("  #fff "), Some("#fff"));
    }

    #[test]
    fn rejects_invalid_colors() {
        for value in [
            "",
            "red",
            "#ff",
            "#fffff",
            "#ggg",
            "rgb()",
            "rgb(1,2,3",
            "rgb(1,2,3);} window { color: red",
            "rgb(1,2,3) }",
            "rgb(var(--x))",
            "rgb(1 /* x */ 2 3)",
            "url(file:///etc/passwd)",
            "lab(50 20 30)",
            "@accent_color",
            "var(--accent-color)",
            "#fff; color: red",
            &format!("rgb({})", "1 ".repeat(40)),
        ] {
            assert!(color(value).is_none(), "{value:?} should be rejected");
        }
    }

    #[test]
    fn font_families_are_quoted() {
        assert_eq!(
            font_family("Inter, 'Noto Sans', sans-serif").as_deref(),
            Some(r#""Inter", "Noto Sans", sans-serif"#)
        );
        assert_eq!(
            font_family(r#"Evil"; } window { color: red; \"#).as_deref(),
            Some(r#""Evil\"; } window { color: red; \\""#)
        );
        assert_eq!(font_family(" , ,"), None);
        assert_eq!(font_family("Monospace").as_deref(), Some("monospace"));
    }

    #[test]
    fn comments_cannot_be_closed() {
        assert_eq!(comment_text("a */ b\n"), "a * / b");
    }

    #[test]
    fn stylesheets_may_not_load_other_files() {
        for css in [
            "@import url('file:///dev/zero');",
            "@IMPORT \"other.css\";",
            r"@\69mport 'other.css';",
            r"@\000069 mport 'other.css';",
            "@im/**/port 'other.css';",
            ".a { background-image: url(\"file:///dev/zero\"); }",
            ".a { background-image: URL(cover.png); }",
            r".a { background-image: \75 rl(cover.png); }",
            ".a { -gtk-icon-source: -gtk-scaled(url(a.png), url(b.png)); }",
            r#".a { font: "/*"; } .b { background-image: url(x.png); } .c { font: "*/"; }"#,
            r".a\/* { background-image: url(x.png); } */",
            ".a { content: \"url(x.png)\"; }",
        ] {
            assert!(file_reference(css).is_some(), "{css}");
        }
        for css in [
            ".a { color: red; }",
            "/* @import url(other.css); */ .a {}",
            ".a { -gtk-icon-source: -gtk-icontheme('audio-x-generic'); }",
            ".curl { color: red; }",
            r".a { font-family: 'Fira\20 Code'; }",
        ] {
            assert_eq!(file_reference(css), None, "{css}");
        }
    }

    #[test]
    fn opacities_are_clamped() {
        assert_eq!(opacity(0.1, MIN_OPACITY), Some(MIN_OPACITY));
        assert_eq!(opacity(2.0, 0.0), Some(1.0));
        assert_eq!(opacity(f32::NAN, 0.0), None);
    }
}
