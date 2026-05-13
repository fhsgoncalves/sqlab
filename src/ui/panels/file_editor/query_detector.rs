use std::ops::Range;

use tree_sitter::Parser;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryRange {
    pub range: Range<usize>,
    pub trimmed_range: Range<usize>,
    pub text: String,
}

pub fn query_ranges_for_execution(text: &str, cursor: usize) -> Vec<QueryRange> {
    if current_line(text, cursor).trim().is_empty() {
        return Vec::new();
    }

    let mut ranges = query_ranges_at_cursor(text, cursor);
    let line_ranges = query_ranges_on_line(text, cursor);

    if !line_ranges.is_empty()
        && !ranges
            .iter()
            .any(|query| parser_query_should_stay_first(query, &line_ranges, cursor))
    {
        let mut prioritized_ranges = line_ranges;
        append_missing_ranges(cursor, &mut prioritized_ranges, ranges);
        return prioritized_ranges;
    }

    for range in line_ranges {
        if let Some(existing) = ranges
            .iter_mut()
            .find(|existing| existing.trimmed_range.start == range.trimmed_range.start)
        {
            if range.trimmed_range.end <= existing.trimmed_range.end {
                *existing = range;
            }
        } else {
            ranges.push(range);
        }
    }

    ranges
}

pub fn query_ranges_at_cursor(text: &str, cursor: usize) -> Vec<QueryRange> {
    if current_line(text, cursor).trim().is_empty() {
        return Vec::new();
    }

    match parse_queries(text, cursor) {
        Some(queries) if !queries.is_empty() => queries,
        _ => fallback_query(text, cursor),
    }
}

/// Detect top-level SQL queries in a block of selected text.
pub fn queries_in_text(text: &str) -> Vec<String> {
    query_ranges_in_text(text)
        .into_iter()
        .map(|query| query.text)
        .collect()
}

/// Detect top-level SQL query ranges in a block of selected text.
pub fn query_ranges_in_text(text: &str) -> Vec<QueryRange> {
    if text.trim().is_empty() {
        return Vec::new();
    }

    let parsed = parse_top_level_queries(text).unwrap_or_default();
    let fallback = fallback_queries_in_text(text);

    if fallback.len() > parsed.len()
        || fallback_ranges_cover_more_text(&fallback, &parsed)
        || fallback_ranges_group_parsed_ranges(&fallback, &parsed)
    {
        fallback
    } else if parsed.is_empty() {
        vec![trimmed_query_range(text, 0, text.len()).expect("text is not empty")]
    } else {
        parsed
    }
}

fn parse_queries(text: &str, cursor: usize) -> Option<Vec<QueryRange>> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_sequel::LANGUAGE.into())
        .ok()?;
    let tree = parser.parse(text, None)?;
    let root = tree.root_node();

    let mut node = root
        .descendant_for_byte_range(cursor, cursor)
        .or_else(|| root.named_descendant_for_byte_range(cursor, cursor));

    let mut queries = Vec::new();
    let mut seen_ranges = std::collections::HashSet::new();

    while let Some(current) = node {
        let kind = current.kind();
        if is_query_node(kind) {
            let start = current.start_byte();
            let end = current.end_byte();
            if end > start {
                if let Some(query) = query_range_for_node(text, start, end, kind) {
                    let key = canonical_query_key(text, &query.trimmed_range);
                    if seen_ranges.insert(key) {
                        queries.push(query);
                    }
                }
            }
        }
        node = current.parent();
    }

    Some(queries)
}

fn parse_top_level_queries(text: &str) -> Option<Vec<QueryRange>> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_sequel::LANGUAGE.into())
        .ok()?;
    let tree = parser.parse(text, None)?;
    let root = tree.root_node();
    let mut queries = Vec::new();
    collect_top_level_queries(text, root, &mut queries);
    Some(queries)
}

fn query_range_for_node(text: &str, start: usize, end: usize, kind: &str) -> Option<QueryRange> {
    if kind == "subquery" {
        let raw = text.get(start..end)?;
        let leading = raw.len() - raw.trim_start().len();
        let trailing = raw.len() - raw.trim_end().len();
        let trimmed_start = start + leading;
        let trimmed_end = end - trailing;
        let trimmed = text.get(trimmed_start..trimmed_end)?;

        if trimmed.starts_with('(') && trimmed.ends_with(')') {
            return trimmed_query_range(text, trimmed_start + 1, trimmed_end - 1);
        }
    }

    trimmed_query_range(text, start, end)
}

fn query_ranges_on_line(text: &str, cursor: usize) -> Vec<QueryRange> {
    let cursor = cursor.min(text.len());
    let line_start = line_start(text, cursor);
    let line_end = line_end(text, cursor);
    let line_range = line_start..line_end;

    let mut ranges = query_ranges_in_text(text)
        .into_iter()
        .filter(|query| ranges_intersect(&query.trimmed_range, &line_range))
        .collect::<Vec<_>>();

    ranges.sort_by_key(|query| {
        let distance = if cursor < query.trimmed_range.start {
            query.trimmed_range.start - cursor
        } else if cursor > query.trimmed_range.end {
            cursor - query.trimmed_range.end
        } else {
            0
        };

        (
            distance,
            query.trimmed_range.end - query.trimmed_range.start,
            query.trimmed_range.start,
        )
    });

    ranges
}

fn current_line(text: &str, cursor: usize) -> &str {
    let cursor = cursor.min(text.len());
    &text[line_start(text, cursor)..line_end(text, cursor)]
}

fn line_start(text: &str, cursor: usize) -> usize {
    text[..cursor].rfind('\n').map(|ix| ix + 1).unwrap_or(0)
}

fn line_end(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .find('\n')
        .map(|ix| cursor + ix)
        .unwrap_or(text.len())
}

fn ranges_intersect(left: &Range<usize>, right: &Range<usize>) -> bool {
    left.start < right.end && right.start < left.end
}

fn range_contains_cursor(range: &Range<usize>, cursor: usize) -> bool {
    range.start <= cursor && cursor < range.end
}

fn range_contains_cursor_inclusive_end(range: &Range<usize>, cursor: usize) -> bool {
    range.start <= cursor && cursor <= range.end
}

fn append_missing_ranges(
    cursor: usize,
    ranges: &mut Vec<QueryRange>,
    candidates: Vec<QueryRange>,
) {
    for candidate in candidates {
        if !range_contains_cursor_inclusive_end(&candidate.trimmed_range, cursor) {
            continue;
        }

        if ranges.iter().any(|existing| {
            existing.trimmed_range.start == candidate.trimmed_range.start
                || existing.trimmed_range.start > candidate.trimmed_range.start
        }) {
            continue;
        }

        ranges.push(candidate);
    }
}


fn canonical_query_key(text: &str, range: &Range<usize>) -> (usize, usize) {
    (range.start, canonical_query_end(text, range))
}

fn canonical_query_end(text: &str, range: &Range<usize>) -> usize {
    let Some(raw) = text.get(range.clone()) else {
        return range.end;
    };

    let canonical = raw.trim_end().trim_end_matches(';').trim_end();
    range.start + canonical.len()
}

fn canonical_query_text(text: &str) -> &str {
    text.trim_end().trim_end_matches(';').trim_end()
}

fn collect_top_level_queries(text: &str, node: tree_sitter::Node, queries: &mut Vec<QueryRange>) {
    if is_top_level_query_node(node.kind()) {
        if let Some(query) = trimmed_query_range(text, node.start_byte(), node.end_byte()) {
            queries.push(query);
        }
        return;
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_top_level_queries(text, child, queries);
    }
}

fn is_query_node(kind: &str) -> bool {
    matches!(
        kind,
        "statement"
            | "select_statement"
            | "insert_statement"
            | "update_statement"
            | "delete_statement"
            | "create_statement"
            | "drop_statement"
            | "alter_statement"
            | "truncate_statement"
            | "grant_statement"
            | "transaction_statement"
            | "subquery"
    ) || kind.ends_with("_statement")
}

fn is_top_level_query_node(kind: &str) -> bool {
    matches!(
        kind,
        "statement"
            | "insert_statement"
            | "update_statement"
            | "delete_statement"
            | "create_statement"
            | "drop_statement"
            | "alter_statement"
            | "truncate_statement"
            | "grant_statement"
            | "transaction_statement"
    ) || kind.ends_with("_statement")
}

fn fallback_query(text: &str, cursor: usize) -> Vec<QueryRange> {
    let start = text[..cursor].rfind(';').map(|i| i + 1).unwrap_or(0);
    let end = text[cursor..]
        .find(';')
        .map(|i| cursor + i + 1)
        .unwrap_or(text.len());

    if let Some(query) = trimmed_query_range(text, start, end) {
        return vec![query];
    }

    let line_start = text[..cursor].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_end = text[cursor..]
        .find('\n')
        .map(|i| cursor + i)
        .unwrap_or(text.len());

    trimmed_query_range(text, line_start, line_end)
        .into_iter()
        .collect()
}

fn fallback_queries_in_text(text: &str) -> Vec<QueryRange> {
    let mut queries = Vec::new();
    let mut start = 0;
    let mut paren_depth = 0usize;
    let mut in_single_quote = false;
    let mut in_double_quote = false;

    for (ix, ch) in text.char_indices() {
        let at_top_level = paren_depth == 0 && !in_single_quote && !in_double_quote;

        if at_top_level && ch == ';' {
            if let Some(query) = trimmed_query_range(text, start, ix) {
                queries.push(query);
            }
            start = ix + ch.len_utf8();
        } else if at_top_level
            && ch == '\n'
            && text[start..ix].trim().is_empty()
            && line_starts_statement(text, ix + ch.len_utf8())
        {
            start = ix + ch.len_utf8();
        } else if at_top_level && ch == '\n' && !text[start..ix].trim().is_empty() {
            let next_start = ix + ch.len_utf8();
            let should_split = line_starts_statement(text, next_start)
                && !cte_continues_with_main_select(text, start, ix, next_start);
            if !should_split {
                continue;
            }
            if let Some(query) = trimmed_query_range(text, start, ix) {
                queries.push(query);
            }
            start = next_start;
        }

        match ch {
            '\'' if !in_double_quote => in_single_quote = !in_single_quote,
            '"' if !in_single_quote => in_double_quote = !in_double_quote,
            '(' if !in_single_quote && !in_double_quote => paren_depth += 1,
            ')' if !in_single_quote && !in_double_quote => {
                paren_depth = paren_depth.saturating_sub(1);
            }
            _ => {}
        }
    }

    if let Some(query) = trimmed_query_range(text, start, text.len()) {
        queries.push(query);
    }

    queries
}

fn fallback_ranges_cover_more_text(fallback: &[QueryRange], parsed: &[QueryRange]) -> bool {
    if fallback.is_empty() || fallback.len() != parsed.len() {
        return false;
    }

    fallback.iter().zip(parsed).any(|(fallback, parsed)| {
        fallback.trimmed_range.start == parsed.trimmed_range.start
            && fallback.trimmed_range.end > parsed.trimmed_range.end
    })
}

fn fallback_ranges_group_parsed_ranges(fallback: &[QueryRange], parsed: &[QueryRange]) -> bool {
    if fallback.is_empty() || parsed.is_empty() || fallback.len() >= parsed.len() {
        return false;
    }

    parsed.iter().all(|parsed| {
        fallback
            .iter()
            .any(|fallback| range_contains_range(&fallback.trimmed_range, &parsed.trimmed_range))
    })
}

fn range_contains_range(outer: &Range<usize>, inner: &Range<usize>) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

fn line_query_extends_query(line_query: &QueryRange, query: &QueryRange) -> bool {
    line_query.trimmed_range.start == query.trimmed_range.start
        && line_query.trimmed_range.end > query.trimmed_range.end
        && canonical_query_text(&line_query.text) != canonical_query_text(&query.text)
}

fn line_query_splits_overbroad_query(line_query: &QueryRange, query: &QueryRange) -> bool {
    line_query.trimmed_range.start == query.trimmed_range.start
        && line_query.trimmed_range.end < query.trimmed_range.end
}

fn line_query_is_narrower_current_query(
    line_query: &QueryRange,
    query: &QueryRange,
    cursor: usize,
) -> bool {
    range_contains_cursor_inclusive_end(&line_query.trimmed_range, cursor)
        && query.trimmed_range.start < line_query.trimmed_range.start
        && line_query.trimmed_range.end <= query.trimmed_range.end
}

fn line_query_surpasses_query_at_cursor(
    line_query: &QueryRange,
    query: &QueryRange,
    cursor: usize,
) -> bool {
    range_contains_cursor_inclusive_end(&line_query.trimmed_range, cursor)
        && query.trimmed_range.start < line_query.trimmed_range.start
        && line_query.trimmed_range.end > query.trimmed_range.end
}

fn parser_query_should_stay_first(
    query: &QueryRange,
    line_ranges: &[QueryRange],
    cursor: usize,
) -> bool {
    range_contains_cursor(&query.trimmed_range, cursor)
        && !line_ranges.iter().any(|line_query| {
            line_query_extends_query(line_query, query)
                || line_query_splits_overbroad_query(line_query, query)
                || line_query_is_narrower_current_query(line_query, query, cursor)
                || line_query_surpasses_query_at_cursor(line_query, query, cursor)
        })
}

fn cte_continues_with_main_select(text: &str, start: usize, end: usize, next_start: usize) -> bool {
    let current = &text[start..end];
    first_keyword(current) == Some("with")
        && cte_prefix_can_continue(current)
        && first_keyword(text.get(next_start..).unwrap_or_default()) == Some("select")
}

fn cte_prefix_can_continue(text: &str) -> bool {
    let trimmed = text.trim_end();
    trimmed.ends_with(')') || trimmed.ends_with(',')
}

fn line_starts_statement(text: &str, start: usize) -> bool {
    let Some(rest) = text.get(start..) else {
        return false;
    };
    let Some(keyword) = first_keyword(rest) else {
        return false;
    };

    matches!(
        keyword,
        "select"
            | "with"
            | "insert"
            | "update"
            | "delete"
            | "create"
            | "drop"
            | "alter"
            | "truncate"
            | "grant"
            | "begin"
            | "commit"
            | "rollback"
    )
}

fn first_keyword(text: &str) -> Option<&str> {
    let line = text
        .split_once('\n')
        .map(|(line, _)| line)
        .unwrap_or(text)
        .trim_start();
    let keyword = line
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .next()?;

    Some(match keyword {
        keyword if keyword.eq_ignore_ascii_case("select") => "select",
        keyword if keyword.eq_ignore_ascii_case("with") => "with",
        keyword if keyword.eq_ignore_ascii_case("insert") => "insert",
        keyword if keyword.eq_ignore_ascii_case("update") => "update",
        keyword if keyword.eq_ignore_ascii_case("delete") => "delete",
        keyword if keyword.eq_ignore_ascii_case("create") => "create",
        keyword if keyword.eq_ignore_ascii_case("drop") => "drop",
        keyword if keyword.eq_ignore_ascii_case("alter") => "alter",
        keyword if keyword.eq_ignore_ascii_case("truncate") => "truncate",
        keyword if keyword.eq_ignore_ascii_case("grant") => "grant",
        keyword if keyword.eq_ignore_ascii_case("begin") => "begin",
        keyword if keyword.eq_ignore_ascii_case("commit") => "commit",
        keyword if keyword.eq_ignore_ascii_case("rollback") => "rollback",
        _ => return None,
    })
}

fn trimmed_query_range(text: &str, start: usize, end: usize) -> Option<QueryRange> {
    let raw = text.get(start..end)?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let leading = raw.len() - raw.trim_start().len();
    let trailing = raw.len() - raw.trim_end().len();
    let trimmed_start = start + leading;
    let trimmed_end = end - trailing;

    Some(QueryRange {
        range: start..end,
        trimmed_range: trimmed_start..trimmed_end,
        text: trimmed.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_statement_range_at_cursor() {
        let text = "select 1;\n\nselect * from users;";
        let query = query_ranges_at_cursor(text, text.find("users").unwrap())
            .into_iter()
            .next()
            .unwrap();

        assert_eq!(query.text, "select * from users");
        assert_eq!(&text[query.trimmed_range], "select * from users");
    }

    #[test]
    fn returns_outer_statement_for_nested_cursor() {
        let text = "select * from (select id from users) u;";
        let query = query_ranges_at_cursor(text, text.find("users").unwrap())
            .into_iter()
            .last()
            .unwrap();

        assert_eq!(query.text, "select * from (select id from users) u");
    }

    #[test]
    fn returns_nested_queries_for_execution_nearest_first() {
        let text = "select * from (select id from users) u;";
        let queries = query_ranges_for_execution(text, text.find("users").unwrap());

        assert!(queries.len() >= 2);
        assert_eq!(queries[0].text, "select id from users");
        assert_eq!(
            queries.last().unwrap().text,
            "select * from (select id from users) u"
        );
    }

    #[test]
    fn returns_cte_queries_for_execution_nearest_first() {
        let text = "with t as (select 1) select 2;";
        let queries = query_ranges_for_execution(text, text.find('1').unwrap());

        assert!(queries.len() >= 2);
        assert_eq!(queries[0].text, "select 1");
        assert_eq!(
            queries.last().unwrap().text,
            "with t as (select 1) select 2"
        );
    }

    #[test]
    fn returns_no_query_when_cursor_is_on_empty_line() {
        let text = "select 1;\n\nselect 2;";
        let queries = query_ranges_for_execution(text, text.find("\n\n").unwrap() + 1);

        assert!(queries.is_empty());
    }

    #[test]
    fn returns_no_query_when_cursor_is_on_whitespace_only_line() {
        let text = "select 1;\n   \nselect 2;";
        let queries = query_ranges_for_execution(text, text.find("   ").unwrap() + 1);

        assert!(queries.is_empty());
    }

    #[test]
    fn returns_same_line_queries_for_execution_nearest_first() {
        let text = "select 1; select 2;";
        let queries = query_ranges_for_execution(text, text.find('2').unwrap());

        assert_eq!(queries.len(), 2);
        assert_eq!(queries[0].text, "select 2");
        assert_eq!(queries[1].text, "select 1");
    }

    #[test]
    fn returns_same_line_query_when_cursor_is_after_semicolon() {
        let text = "select bla;\n\nselect c.id, o.status from customers c inner join orders o on o.customer_id = c.id";
        let queries = query_ranges_for_execution(text, text.find('\n').unwrap());

        assert!(!queries.is_empty());
        assert_eq!(queries[0].text, "select bla");
    }

    #[test]
    fn does_not_duplicate_single_query_that_only_differs_by_trailing_semicolon() {
        let text = "select * from customers c where c.city <> 'a';";
        let queries = query_ranges_for_execution(text, text.find("city").unwrap());

        assert_eq!(queries.len(), 1);
        assert_eq!(
            queries[0].text,
            "select * from customers c where c.city <> 'a'"
        );
    }

    #[test]
    fn stops_query_at_semicolon_when_cursor_is_after_terminator() {
        let text = "select 2;\n\nselect o.created_at from customers c inner join orders o on o.customer_id = c.id where c.city <> 'S';";
        let queries = query_ranges_for_execution(text, "select 2;".len());

        assert_eq!(queries.len(), 1, "{queries:?}");
        assert_eq!(queries[0].text, "select 2");
    }

    #[test]
    fn keeps_same_line_queries_distinct_when_text_is_equal() {
        let text = "select 1; select 1;";
        let queries = query_ranges_for_execution(text, text.rfind('1').unwrap());

        assert_eq!(queries.len(), 2);
        assert_eq!(queries[0].trimmed_range.start, "select 1; ".len());
        assert_eq!(queries[1].trimmed_range.start, 0);
    }

    #[test]
    fn keeps_query_range_offsets_when_file_starts_with_empty_line() {
        let text = "\nselect 1;";
        let queries = query_ranges_for_execution(text, text.len());

        assert!(!queries.is_empty());
        assert_eq!(queries[0].text, "select 1");
        assert_eq!(queries[0].trimmed_range.start, 1);
        assert_eq!(&text[queries[0].trimmed_range.clone()], "select 1");
    }

    #[test]
    fn extends_unterminated_query_to_end_of_line() {
        let text =
            "select c.id, o.status from customers c inner join orders o on o.customer_id = c.id";
        let queries = query_ranges_for_execution(text, text.len());

        assert!(!queries.is_empty());
        assert_eq!(queries[0].text, text);
        assert_eq!(queries[0].trimmed_range, 0..text.len());
    }

    #[test]
    fn extends_unterminated_query_to_end_of_line_when_cursor_is_inside_query() {
        let first_query =
            "select c.id, o.status from customers c inner join orders o on o.customer_id = c.id";
        let text = format!("{}\n\nselect 1", first_query);
        let queries = query_ranges_for_execution(&text, text.find("select").unwrap());

        assert!(!queries.is_empty());
        assert_eq!(queries[0].text, first_query);
        assert_eq!(queries[0].trimmed_range, 0..first_query.len());
    }

    #[test]
    fn splits_new_statement_line_without_semicolon() {
        let text = "select c.id, o.status from customers c inner join orders o on o.customer_id = c.id\n\nselect 1";
        let queries = query_ranges_in_text(text);

        assert_eq!(queries.len(), 2);
        assert_eq!(
            queries[0].text,
            "select c.id, o.status from customers c inner join orders o on o.customer_id = c.id"
        );
        assert_eq!(queries[1].text, "select 1");
    }

    #[test]
    fn returns_current_line_query_when_previous_query_has_no_semicolon() {
        let text = "select c.id, o.status from customers c inner join orders o on o.customer_id = c.id\n\nselect 1";
        let queries = query_ranges_for_execution(text, text.rfind('1').unwrap());

        assert!(!queries.is_empty());
        assert_eq!(queries[0].text, "select 1");
    }

    #[test]
    fn does_not_split_statement_keyword_inside_parentheses() {
        let text = "with t as (\nselect 1\n)\nselect * from t";
        let queries = query_ranges_in_text(text);

        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].text, text);
    }

    #[test]
    fn stops_cte_query_before_next_statement_after_blank_lines() {
        let expected = "with t as (\n  select 1\n)\nselect \n  c.id, \n  o.status \nfrom customers c \n  inner join orders o on o.customer_id = c.id";
        let text = format!(
            "{}\n\n\nselect c.customer_id, o.status from customers c inner join orders o on o.customer_id = c.id\n\nselect 1",
            expected
        );
        let queries = query_ranges_for_execution(&text, 0);

        assert!(!queries.is_empty());
        assert_eq!(queries[0].text, expected);
        assert_eq!(queries[0].trimmed_range, 0..expected.len());
    }

    #[test]
    fn stops_single_line_query_before_later_multiline_statements() {
        let text = "select 1\n\nwith t as (\n  select 1\n)\nselect \n  c.id, \n  o.status \nfrom customers c \n  inner join orders o on o.customer_id = c.id\n\n\nselect c.customer_id, o.status from customers c inner join orders o on o.customer_id = c.id";
        let queries = query_ranges_for_execution(text, text.find('1').unwrap() + 1);

        assert!(!queries.is_empty());
        assert_eq!(queries[0].text, "select 1");
        assert_eq!(queries[0].trimmed_range, 0.."select 1".len());
    }

    #[test]
    fn stops_query_with_cte_after_unfinished_select_before_later_statements() {
        let expected = "with t as (\n  select c.customer_id, o.status from customers c inner join orders o on o.customer_id = c.id\n)\nselect bla";
        let text = format!(
            "select bla\n{}\n\nselect c.id, o.status from customers c inner join orders o on o.customer_id = c.id\n\nselect 1;",
            expected
        );
        let cursor = text.find("select bla\nwith").unwrap() + "select bla\n".len() + expected.len();
        let queries = query_ranges_for_execution(&text, cursor);

        assert!(!queries.is_empty());
        assert_eq!(queries[0].text, expected);
    }

    #[test]
    fn splits_selected_text_into_query_ranges() {
        let text = " select 1; \n select 2";
        let queries = query_ranges_in_text(text);

        assert_eq!(queries.len(), 2);
        assert_eq!(queries[0].text, "select 1");
        assert_eq!(queries[1].text, "select 2");
    }

    #[test]
    fn only_detects_current_line_when_adjacent_queries_no_semicolons() {
        let text = "select current_user\nselect current_date";
        let queries = query_ranges_for_execution(text, 0);

        assert_eq!(queries.len(), 1, "first query only, got: {queries:?}");
        assert_eq!(queries[0].text, "select current_user");
    }

    #[test]
    fn only_detects_second_query_when_cursor_on_second_line() {
        let text = "select current_user\nselect current_date";
        let cursor = text.find("select current_date").unwrap();
        let queries = query_ranges_for_execution(text, cursor);

        assert_eq!(queries.len(), 1, "second query only, got: {queries:?}");
        assert_eq!(queries[0].text, "select current_date");
    }

    #[test]
    fn only_detects_current_line_when_last_query_has_semicolon() {
        let text = "select current_user\nselect current_date;";
        let queries = query_ranges_for_execution(text, 0);

        assert_eq!(queries.len(), 1, "first query only, got: {queries:?}");
        assert_eq!(queries[0].text, "select current_user");
    }

    #[test]
    fn does_not_detect_other_line_when_cursor_on_first_with_semicolon() {
        let text = "select current_user\nselect current_date;";
        let cursor = text.find("select current_date").unwrap();
        let queries = query_ranges_for_execution(text, cursor);

        assert_eq!(queries.len(), 1, "second query only, got: {queries:?}");
        assert_eq!(queries[0].text, "select current_date");
    }

    #[test]
    fn falls_back_to_current_line_when_no_semicolons_and_multi_line() {
        let text = "select 1\nselect 2\nselect 3";
        let cursor = text.find("select 2").unwrap();
        let queries = query_ranges_for_execution(text, cursor);

        assert_eq!(queries.len(), 1, "second query only, got: {queries:?}");
        assert_eq!(queries[0].text, "select 2");
    }

    #[test]
    fn cursor_on_second_line_select_keyword_only_first_line_detected() {
        let text = "select current_user\nselect current_date";
        let cursor = text.find("select current_date").unwrap();
        let queries = query_ranges_for_execution(text, cursor);

        assert_eq!(queries.len(), 1, "got: {queries:?}");
        assert_eq!(queries[0].text, "select current_date");
    }

    #[test]
    fn cursor_at_newline_between_two_queries_only_detects_first() {
        let text = "select current_user\nselect current_date";
        let cursor = text.find('\n').unwrap();
        let queries = query_ranges_for_execution(text, cursor);

        assert_eq!(queries.len(), 1, "got: {queries:?}");
        assert_eq!(queries[0].text, "select current_user");
    }

    #[test]
    fn cursor_at_end_of_first_line_only_detects_first() {
        let text = "select current_user\nselect current_date";
        let cursor = text.find("current_user").unwrap() + "current_user".len();
        let queries = query_ranges_for_execution(text, cursor);

        assert_eq!(queries.len(), 1, "got: {queries:?}");
        assert_eq!(queries[0].text, "select current_user");
    }

    #[test]
    fn cursor_at_start_of_text_only_detects_first() {
        let text = "select current_user\nselect current_date";
        let queries = query_ranges_for_execution(text, 0);

        assert_eq!(queries.len(), 1, "got: {queries:?}");
        assert_eq!(queries[0].text, "select current_user");
    }

    #[test]
    fn cursor_mid_first_line_no_semicolons() {
        let text = "select current_user\nselect current_date";
        let cursor = text.find("current_user").unwrap();
        let queries = query_ranges_for_execution(text, cursor);

        assert_eq!(queries.len(), 1, "got: {queries:?}");
        assert_eq!(queries[0].text, "select current_user");
    }

    #[test]
    fn cursor_on_first_line_select_keyword_no_semicolons() {
        let text = "select current_user\nselect current_date";
        let cursor = text.find("select ").unwrap();
        let queries = query_ranges_for_execution(text, cursor);

        assert_eq!(queries.len(), 1, "got: {queries:?}");
        assert_eq!(queries[0].text, "select current_user");
    }

    #[test]
    fn cursor_on_second_line_select_keyword_no_semicolons() {
        let text = "select current_user\nselect current_date";
        let cursor = text.find("\nselect").unwrap() + 1;
        let queries = query_ranges_for_execution(text, cursor);

        assert_eq!(queries.len(), 1, "got: {queries:?}");
        assert_eq!(queries[0].text, "select current_date");
    }

    #[test]
    fn cursor_on_second_line_body_no_semicolons() {
        let text = "select current_user\nselect current_date";
        let cursor = text.find("current_date").unwrap();
        let queries = query_ranges_for_execution(text, cursor);

        assert_eq!(queries.len(), 1, "got: {queries:?}");
        assert_eq!(queries[0].text, "select current_date");
    }

    #[test]
    fn trailing_newline_still_only_one_query() {
        let text = "select current_user\nselect current_date\n";
        let cursor = text.find("select current_date").unwrap();
        let queries = query_ranges_for_execution(text, cursor);

        assert_eq!(queries.len(), 1, "got: {queries:?}");
        assert_eq!(queries[0].text, "select current_date");
    }

    #[test]
    fn trailing_newline_cursor_on_first_line() {
        let text = "select current_user\nselect current_date\n";
        let queries = query_ranges_for_execution(text, 0);

        assert_eq!(queries.len(), 1, "got: {queries:?}");
        assert_eq!(queries[0].text, "select current_user");
    }
}
