//! Keeping plugin-supplied text to a sensible size.

/// `text` cut to at most `max_chars` characters, with an ellipsis when shortened.
pub(crate) fn truncate(text: &str, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_on_char_boundaries() {
        assert_eq!(truncate("héllo", 10), "héllo");
        assert_eq!(truncate("héllo", 5), "héllo");
        assert_eq!(truncate("héllo", 2), "hé…");
    }
}
