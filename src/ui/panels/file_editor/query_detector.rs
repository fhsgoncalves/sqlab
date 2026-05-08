use tree_sitter::Parser;

/// Detect SQL queries at the given cursor position using tree-sitter AST.
/// Returns queries from least inclusive (innermost) to most inclusive (outermost).
/// Falls back to semicolon-delimited scan if tree-sitter parsing fails.
pub fn queries_at_cursor(text: &str, cursor: usize) -> Vec<String> {
    match parse_queries(text, cursor) {
        Some(queries) if !queries.is_empty() => queries,
        _ => fallback_query(text, cursor),
    }
}

/// Detect top-level SQL queries in a block of selected text.
pub fn queries_in_text(text: &str) -> Vec<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let parsed = parse_top_level_queries(trimmed).unwrap_or_default();
    let fallback = fallback_queries_in_text(trimmed);

    if fallback.len() > parsed.len() {
        fallback
    } else if parsed.is_empty() {
        vec![trimmed.to_string()]
    } else {
        parsed
    }
}

fn parse_queries(text: &str, cursor: usize) -> Option<Vec<String>> {
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
                let query_text = text.get(start..end).unwrap_or("").trim();
                if !query_text.is_empty() {
                    queries.push(query_text.to_string());
                }
            }
        }
        node = current.parent();
    }

    Some(queries)
}

fn parse_top_level_queries(text: &str) -> Option<Vec<String>> {
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

fn collect_top_level_queries(text: &str, node: tree_sitter::Node, queries: &mut Vec<String>) {
    if is_top_level_query_node(node.kind()) {
        let query = text
            .get(node.start_byte()..node.end_byte())
            .unwrap_or("")
            .trim();
        if !query.is_empty() {
            queries.push(query.to_string());
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

fn fallback_query(text: &str, cursor: usize) -> Vec<String> {
    let start = text[..cursor].rfind(';').map(|i| i + 1).unwrap_or(0);
    let end = text[cursor..]
        .find(';')
        .map(|i| cursor + i + 1)
        .unwrap_or(text.len());
    let query = text[start..end].trim();

    if !query.is_empty() {
        return vec![query.to_string()];
    }

    let line_start = text[..cursor].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_end = text[cursor..]
        .find('\n')
        .map(|i| cursor + i)
        .unwrap_or(text.len());
    let line = text[line_start..line_end].trim();
    if !line.is_empty() {
        vec![line.to_string()]
    } else {
        vec![]
    }
}

fn fallback_queries_in_text(text: &str) -> Vec<String> {
    let mut queries = Vec::new();
    let mut start = 0;

    for (ix, ch) in text.char_indices() {
        if ch == ';' {
            let query = text[start..=ix].trim();
            if !query.is_empty() {
                queries.push(query.to_string());
            }
            start = ix + ch.len_utf8();
        }
    }

    let query = text[start..].trim();
    if !query.is_empty() {
        queries.push(query.to_string());
    }

    queries
}
