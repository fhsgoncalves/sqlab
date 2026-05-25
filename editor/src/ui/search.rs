#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub use_regex: bool,
    pub whole_word: bool,
    pub use_fuzzy: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextMatch {
    pub line_number: usize,
    pub line_content: String,
    pub match_start: usize,
    pub match_end: usize,
    pub score: i64,
}

pub fn find_text_matches(text: &str, query: &str, options: SearchOptions) -> Vec<TextMatch> {
    if query.is_empty() {
        return Vec::new();
    }

    let matcher = QueryMatcher::new(query, options);
    text.lines()
        .enumerate()
        .flat_map(|(line_ix, line)| {
            matcher
                .matches_in_line(line)
                .into_iter()
                .map(move |(match_start, match_end, score)| TextMatch {
                    line_number: line_ix + 1,
                    line_content: line.to_string(),
                    match_start,
                    match_end,
                    score,
                })
        })
        .collect()
}

pub fn score_text(query: &str, text: &str, options: SearchOptions) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }

    let matcher = QueryMatcher::new(query, options);
    matcher
        .matches_in_line(text)
        .into_iter()
        .map(|(_, _, score)| score)
        .max()
}

struct QueryMatcher<'a> {
    query: &'a str,
    options: SearchOptions,
    regex: Option<regex::Regex>,
}

impl<'a> QueryMatcher<'a> {
    fn new(query: &'a str, options: SearchOptions) -> Self {
        let regex = if options.use_regex {
            regex::RegexBuilder::new(query)
                .case_insensitive(!options.case_sensitive)
                .build()
                .ok()
        } else {
            None
        };

        Self {
            query,
            options,
            regex,
        }
    }

    fn matches_in_line(&self, line: &str) -> Vec<(usize, usize, i64)> {
        if self.options.use_regex {
            return self.regex_matches(line);
        }

        if self.options.use_fuzzy {
            return fuzzy_match(line, self.query, self.options.case_sensitive)
                .into_iter()
                .filter(|(start, end, _)| {
                    !self.options.whole_word || is_whole_word_match(line, *start, *end)
                })
                .collect();
        }

        exact_matches(
            line,
            self.query,
            self.options.case_sensitive,
            self.options.whole_word,
        )
    }

    fn regex_matches(&self, line: &str) -> Vec<(usize, usize, i64)> {
        let Some(regex) = &self.regex else {
            return Vec::new();
        };

        regex
            .find_iter(line)
            .filter(|m| !self.options.whole_word || is_whole_word_match(line, m.start(), m.end()))
            .map(|m| (m.start(), m.end(), 10_000 - m.start() as i64))
            .collect()
    }
}

fn exact_matches(
    line: &str,
    query: &str,
    case_sensitive: bool,
    whole_word: bool,
) -> Vec<(usize, usize, i64)> {
    let haystack = comparable(line, case_sensitive);
    let needle = comparable(query, case_sensitive);
    let mut matches = Vec::new();
    let mut start = 0;

    while start <= haystack.len() {
        let Some(pos) = haystack[start..].find(&needle) else {
            break;
        };
        let match_start = start + pos;
        let match_end = match_start + needle.len();

        if !whole_word || is_whole_word_match(line, match_start, match_end) {
            matches.push((match_start, match_end, 20_000 - match_start as i64));
        }

        start = match_start.saturating_add(1);
    }

    matches
}

fn fuzzy_match(line: &str, query: &str, case_sensitive: bool) -> Option<(usize, usize, i64)> {
    let query_chars: Vec<char> = query.chars().collect();
    if query_chars.is_empty() {
        return Some((0, 0, 0));
    }

    let mut query_ix = 0;
    let mut start = None;
    let mut end;
    let mut gaps = 0usize;
    let mut last_match_end = None;

    for (byte_ix, ch) in line.char_indices() {
        let Some(query_ch) = query_chars.get(query_ix).copied() else {
            break;
        };
        if char_eq(ch, query_ch, case_sensitive) {
            start.get_or_insert(byte_ix);
            if let Some(last_end) = last_match_end {
                gaps += byte_ix.saturating_sub(last_end);
            }
            end = byte_ix + ch.len_utf8();
            last_match_end = Some(end);
            query_ix += 1;

            if query_ix == query_chars.len() {
                let span = end.saturating_sub(start.unwrap_or(0));
                let score = 30_000 - (span as i64 * 4) - gaps as i64;
                return Some((start.unwrap_or(0), end, score));
            }
        }
    }

    None
}

fn char_eq(left: char, right: char, case_sensitive: bool) -> bool {
    if case_sensitive {
        left == right
    } else {
        left.eq_ignore_ascii_case(&right)
    }
}

fn comparable(value: &str, case_sensitive: bool) -> String {
    if case_sensitive {
        value.to_string()
    } else {
        value.to_ascii_lowercase()
    }
}

fn is_whole_word_match(line: &str, start: usize, end: usize) -> bool {
    let starts_on_boundary = start == 0 || !is_word_char_before(line, start);
    let ends_on_boundary = end >= line.len() || !is_word_char_at(line, end);
    starts_on_boundary && ends_on_boundary
}

fn is_word_char_before(line: &str, byte_ix: usize) -> bool {
    line[..byte_ix]
        .chars()
        .next_back()
        .is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
}

fn is_word_char_at(line: &str, byte_ix: usize) -> bool {
    line[byte_ix..]
        .chars()
        .next()
        .is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_search_respects_case_option() {
        let text = "select Theme\nselect theme";
        let insensitive = find_text_matches(text, "theme", SearchOptions::default());
        assert_eq!(insensitive.len(), 2);

        let sensitive = find_text_matches(
            text,
            "theme",
            SearchOptions {
                case_sensitive: true,
                ..SearchOptions::default()
            },
        );
        assert_eq!(sensitive.len(), 1);
        assert_eq!(sensitive[0].line_number, 2);
    }

    #[test]
    fn whole_word_excludes_embedded_matches() {
        let text = "schema theme themed _theme theme_";
        let matches = find_text_matches(
            text,
            "theme",
            SearchOptions {
                whole_word: true,
                ..SearchOptions::default()
            },
        );

        assert_eq!(matches.len(), 1);
        assert_eq!(&text[matches[0].match_start..matches[0].match_end], "theme");
    }

    #[test]
    fn regex_search_uses_regex_boundaries() {
        let text = "foo_1\nfoo_22\nbar";
        let matches = find_text_matches(
            text,
            r"foo_\d{2}",
            SearchOptions {
                use_regex: true,
                ..SearchOptions::default()
            },
        );

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].line_number, 2);
    }

    #[test]
    fn fuzzy_search_matches_subsequence() {
        let score = score_text(
            "psrs",
            "src/ui/panels/project_search.rs",
            SearchOptions {
                use_fuzzy: true,
                ..SearchOptions::default()
            },
        );

        assert!(score.is_some());
    }
}
