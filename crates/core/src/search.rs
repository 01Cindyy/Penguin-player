use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// Lowercases text and strips diacritics so "Björk" matches "bjork".
pub fn normalize(text: &str) -> String {
    // Most tags are plain ASCII, for which this is just lowercasing.
    if text.is_ascii() {
        return text.to_ascii_lowercase();
    }
    text.nfkd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .collect()
}

/// A parsed search: every whitespace-separated term must appear somewhere in the
/// candidate's text, in any order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchQuery {
    terms: Vec<String>,
}

impl SearchQuery {
    pub fn new(query: &str) -> Self {
        Self {
            terms: normalize(query)
                .split_whitespace()
                .map(str::to_owned)
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Matches against text that has already been passed through [`normalize`].
    /// An empty query matches everything.
    pub fn matches(&self, normalized_haystack: &str) -> bool {
        self.terms
            .iter()
            .all(|term| normalized_haystack.contains(term.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::tests::track;

    #[test]
    fn normalizes_case_and_accents() {
        assert_eq!(normalize("Björk ÉCLAIR"), "bjork eclair");
        assert_eq!(normalize("東京"), "東京");
    }

    #[test]
    fn all_terms_must_match() {
        let text = track(1, "Hyperballad", "Björk", "Post").search_text();
        assert!(SearchQuery::new("bjork post").matches(&text));
        assert!(SearchQuery::new("  HYPER  ").matches(&text));
        assert!(SearchQuery::new("flac").matches(&text));
        assert!(!SearchQuery::new("bjork debut").matches(&text));
        assert!(SearchQuery::new("").matches(&text));
    }
}
