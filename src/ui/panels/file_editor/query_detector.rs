use std::ops::Range;

use tree_sitter::Parser;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryRange {
    pub range: Range<usize>,
    pub trimmed_range: Range<usize>,
    pub text: String,
}

pub fn query_at_cursor(text: &str, cursor: usize) -> Option<QueryRange> {
    query_ranges_at_cursor(text, cursor).pop()
}

pub fn query_ranges_at_cursor(text: &str, cursor: usize) -> Vec<QueryRange> {
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
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let parsed = parse_top_level_queries(trimmed).unwrap_or_default();
    let fallback = fallback_queries_in_text(trimmed);

    if fallback.len() > parsed.len() {
        fallback
    } else if parsed.is_empty() {
        vec![trimmed_query_range(text, 0, text.len()).expect("trimmed text is not empty")]
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
            if end > start && !seen_ranges.contains(&(start, end)) {
                seen_ranges.insert((start, end));
                if let Some(query) = trimmed_query_range(text, start, end) {
                    queries.push(query);
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

    for (ix, ch) in text.char_indices() {
        if ch == ';' {
            if let Some(query) = trimmed_query_range(text, start, ix + ch.len_utf8()) {
                queries.push(query);
            }
            start = ix + ch.len_utf8();
        }
    }

    if let Some(query) = trimmed_query_range(text, start, text.len()) {
        queries.push(query);
    }

    queries
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
        let query = query_at_cursor(text, text.find("users").unwrap()).unwrap();

        assert_eq!(query.text, "select * from users");
        assert_eq!(&text[query.trimmed_range], "select * from users");
    }

    #[test]
    fn returns_outer_statement_for_nested_cursor() {
        let text = "select * from (select id from users) u;";
        let query = query_at_cursor(text, text.find("users").unwrap()).unwrap();

        assert_eq!(query.text, "select * from (select id from users) u");
    }

    #[test]
    fn splits_selected_text_into_query_ranges() {
        let text = " select 1; \n select 2";
        let queries = query_ranges_in_text(text);

        assert_eq!(queries.len(), 2);
        assert_eq!(queries[0].text, "select 1");
        assert_eq!(queries[1].text, "select 2");
    }
}
