use memchr::memmem;
use regex::bytes::{Regex, RegexBuilder};

/// How a search query is interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchKind {
    Literal,
    Regex,
}

impl SearchKind {
    pub fn label(self) -> &'static str {
        match self {
            SearchKind::Literal => "literal",
            SearchKind::Regex => "regex",
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            SearchKind::Literal => SearchKind::Regex,
            SearchKind::Regex => SearchKind::Literal,
        }
    }
}

/// A compiled search. Literal queries with no uppercase letter (and all regex
/// queries) are matched case-insensitively (smart case), mirroring the file
/// finder. A case-sensitive literal uses a fast `memchr` substring finder; all
/// other cases compile to a byte regex.
#[derive(Debug, Clone)]
pub struct Search {
    query: String,
    kind: SearchKind,
    matcher: Matcher,
}

#[derive(Debug, Clone)]
enum Matcher {
    Literal(Box<memmem::Finder<'static>>),
    Regex(Regex),
}

fn has_uppercase(s: &str) -> bool {
    s.chars().any(|c| c.is_uppercase())
}

impl Search {
    /// Compile `query`. Returns `Err(message)` for an invalid regex.
    pub fn new(query: &str, kind: SearchKind) -> Result<Self, String> {
        let case_insensitive = !has_uppercase(query);
        let matcher = match kind {
            SearchKind::Literal if !case_insensitive => {
                Matcher::Literal(Box::new(memmem::Finder::new(query.as_bytes()).into_owned()))
            }
            SearchKind::Literal => {
                // Case-insensitive literal: an escaped, case-folded regex keeps
                // byte ranges exact (lowercasing the haystack would not).
                build_regex(&regex::escape(query), true)?
            }
            SearchKind::Regex => build_regex(query, case_insensitive)?,
        };
        Ok(Search {
            query: query.to_string(),
            kind,
            matcher,
        })
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn kind(&self) -> SearchKind {
        self.kind
    }

    pub fn line_matches(&self, line: &str) -> bool {
        match &self.matcher {
            Matcher::Literal(f) => f.find(line.as_bytes()).is_some(),
            Matcher::Regex(re) => re.is_match(line.as_bytes()),
        }
    }

    /// Byte ranges of every (non-overlapping) match in `line`, for highlighting.
    pub fn match_ranges(&self, line: &str) -> Vec<(usize, usize)> {
        let bytes = line.as_bytes();
        match &self.matcher {
            Matcher::Literal(f) => {
                let len = self.query.len();
                f.find_iter(bytes).map(|start| (start, start + len)).collect()
            }
            Matcher::Regex(re) => re
                .find_iter(bytes)
                .filter(|m| m.end() > m.start())
                .map(|m| (m.start(), m.end()))
                .collect(),
        }
    }
}

fn build_regex(pattern: &str, case_insensitive: bool) -> Result<Matcher, String> {
    RegexBuilder::new(pattern)
        .case_insensitive(case_insensitive)
        .build()
        .map(Matcher::Regex)
        .map_err(|e| format!("invalid regex: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_literal_smart_case() {
        // Lowercase query → case-insensitive.
        let s = Search::new("error", SearchKind::Literal).unwrap();
        assert!(s.line_matches("an ERROR occurred"));
        assert!(s.line_matches("error here"));
        // Mixed-case query → case-sensitive.
        let s = Search::new("Error", SearchKind::Literal).unwrap();
        assert!(s.line_matches("an Error"));
        assert!(!s.line_matches("an error"));
    }

    #[test]
    fn test_literal_match_ranges() {
        let s = Search::new("ab", SearchKind::Literal).unwrap();
        assert_eq!(s.match_ranges("xabyab"), vec![(1, 3), (4, 6)]);
    }

    #[test]
    fn test_regex_match_and_ranges() {
        let s = Search::new(r"\d{3}", SearchKind::Regex).unwrap();
        assert!(s.line_matches("code 404 here"));
        assert_eq!(s.match_ranges("a123b456"), vec![(1, 4), (5, 8)]);
    }

    #[test]
    fn test_regex_smart_case() {
        let s = Search::new("warn", SearchKind::Regex).unwrap();
        assert!(s.line_matches("WARN: x")); // no uppercase in query → insensitive
        let s = Search::new("WARN", SearchKind::Regex).unwrap();
        assert!(!s.line_matches("warn: x")); // uppercase in query → sensitive
    }

    #[test]
    fn test_invalid_regex_is_error_not_panic() {
        let err = Search::new("(unclosed", SearchKind::Regex);
        assert!(err.is_err());
    }

    #[test]
    fn test_zero_width_regex_not_infinite() {
        // A pattern that can match empty must not yield zero-width ranges.
        let s = Search::new("x*", SearchKind::Regex).unwrap();
        assert!(s.match_ranges("zzz").is_empty());
    }
}
