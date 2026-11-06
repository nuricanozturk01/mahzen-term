//! Plain-text search in a buffer: smart case, optional whole words.

use ropey::Rope;

/// A match as character indexes `start..end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    pub start: usize,
    pub end: usize,
}

/// Search options.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchOptions {
    /// Compare case exactly. When false, a query without capitals ignores case ("smart case").
    pub case_sensitive: bool,
    /// Skip matches inside a longer word.
    pub whole_word: bool,
}

/// Most matches returned.
const MAX_MATCHES: usize = 100_000;

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// All non-overlapping matches of `query` in `text`, in order. An empty query matches nothing.
pub fn find_all(text: &Rope, query: &str, opts: SearchOptions) -> Vec<Match> {
    if query.is_empty() {
        return Vec::new();
    }
    let ignore_case = !opts.case_sensitive && !query.chars().any(char::is_uppercase);
    let fold = |c: char| {
        if ignore_case {
            c.to_lowercase().next().unwrap_or(c)
        } else {
            c
        }
    };
    let needle: Vec<char> = query.chars().map(fold).collect();
    let hay: Vec<char> = text.chars().map(fold).collect();
    let orig: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + needle.len() <= hay.len() && out.len() < MAX_MATCHES {
        if hay[i..i + needle.len()] == needle[..] {
            let end = i + needle.len();
            let before_ok = i == 0 || !is_word(orig[i - 1]);
            let after_ok = end == orig.len() || !is_word(orig[end]);
            if !opts.whole_word || (before_ok && after_ok) {
                out.push(Match { start: i, end });
                i = end;
                continue;
            }
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(text: &str, q: &str, o: SearchOptions) -> Vec<(usize, usize)> {
        find_all(&Rope::from_str(text), q, o)
            .iter()
            .map(|m| (m.start, m.end))
            .collect()
    }

    #[test]
    fn smart_case_and_exact_case() {
        let t = "Foo foo FOO";
        assert_eq!(
            all(t, "foo", SearchOptions::default()),
            [(0, 3), (4, 7), (8, 11)]
        );
        assert_eq!(
            all(t, "Foo", SearchOptions::default()),
            [(0, 3)],
            "a capital makes it exact"
        );
        let exact = SearchOptions {
            case_sensitive: true,
            ..Default::default()
        };
        assert_eq!(all(t, "foo", exact), [(4, 7)]);
    }

    #[test]
    fn whole_words_and_overlaps() {
        let words = SearchOptions {
            whole_word: true,
            ..Default::default()
        };
        assert_eq!(
            all("foo foobar _foo foo.", "foo", words),
            [(0, 3), (16, 19)]
        );
        assert_eq!(
            all("aaaa", "aa", SearchOptions::default()),
            [(0, 2), (2, 4)],
            "no overlaps"
        );
        assert!(all("abc", "", SearchOptions::default()).is_empty());
        assert!(all("abc", "abcd", SearchOptions::default()).is_empty());
        assert_eq!(
            all("ünï ÜNÏ", "ünï", SearchOptions::default()),
            [(0, 3), (4, 7)],
            "unicode case folding"
        );
    }
}
