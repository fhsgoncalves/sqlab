use std::collections::HashSet;
use std::rc::Rc;
use std::sync::{Arc, RwLock};

use anyhow::Result;
use gpui::{Context, Entity, Task, Window};
use gpui_component::input::{CompletionProvider, InputState, Rope, RopeExt};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    Documentation, TextEdit,
};

use crate::data_source::manager::DataSourceManager;
use crate::data_source::{DatabaseSchema, TableInfo, TableKind};
use crate::schema_cache;

const SQL_KEYWORDS: &[&str] = &[
    "select",
    "from",
    "where",
    "join",
    "left join",
    "right join",
    "inner join",
    "full join",
    "on",
    "group by",
    "order by",
    "having",
    "limit",
    "offset",
    "insert into",
    "update",
    "delete from",
    "create table",
    "alter table",
    "drop table",
    "with",
    "union",
    "union all",
    "returning",
    "case",
    "when",
    "then",
    "else",
    "end",
    "and",
    "or",
    "not",
    "is null",
    "is not null",
];

#[derive(Clone)]
pub struct SqlCompletionProvider {
    manager: Entity<DataSourceManager>,
    cache: Arc<RwLock<Option<SchemaCache>>>,
}

impl SqlCompletionProvider {
    pub fn new(manager: Entity<DataSourceManager>) -> Rc<Self> {
        Rc::new(Self {
            manager,
            cache: Arc::new(RwLock::new(None)),
        })
    }
}

#[derive(Clone)]
struct SchemaCache {
    key: String,
    schema: Arc<DatabaseSchema>,
}

impl CompletionProvider for SqlCompletionProvider {
    fn completions(
        &self,
        rope: &Rope,
        offset: usize,
        trigger: CompletionContext,
        _: &mut Window,
        cx: &mut Context<InputState>,
    ) -> Task<Result<CompletionResponse>> {
        let query = trigger.trigger_character.unwrap_or_default();
        let config = self.manager.read(cx).active_config().cloned();
        let cache = self.cache.clone();
        let context = CompletionContextData::new(rope, offset, &query);

        if let Some(config) = config {
            let key = schema_cache::cache_key(&config);

            // Check in-memory cache first
            if let Some(schema) = cached_schema(&cache, &key) {
                return Task::ready(Ok(CompletionResponse::Array(build_items(
                    &context,
                    schema.as_ref(),
                ))));
            }

            // Check persistent cache
            match schema_cache::load(&key) {
                Ok(Some(schema)) => {
                    let schema = Arc::new(schema);
                    if let Ok(mut guard) = cache.write() {
                        *guard = Some(SchemaCache {
                            key,
                            schema: schema.clone(),
                        });
                    }
                    return Task::ready(Ok(CompletionResponse::Array(build_items(
                        &context,
                        schema.as_ref(),
                    ))));
                }
                _ => {}
            }
        }

        Task::ready(Ok(CompletionResponse::Array(limit_items(keyword_items(
            &context, 0,
        )))))
    }

    fn is_completion_trigger(
        &self,
        _offset: usize,
        new_text: &str,
        _cx: &mut Context<InputState>,
    ) -> bool {
        new_text
            .chars()
            .any(|ch| ch == '.' || ch == '_' || ch.is_ascii_alphanumeric() || ch.is_whitespace())
    }
}

#[derive(Clone)]
struct CompletionContextData {
    offset: usize,
    replace_start: usize,
    prefix: String,
    qualifier: Option<String>,
    scope: CompletionScope,
    table_refs: Vec<TableRef>,
    rope: Rope,
}

impl CompletionContextData {
    fn new(rope: &Rope, offset: usize, query: &str) -> Self {
        let text = rope.to_string();
        let statement_start = text[..offset]
            .rfind(';')
            .map(|ix| ix + 1)
            .unwrap_or_default();
        let statement_end = text[offset..]
            .find(';')
            .map(|ix| offset + ix)
            .unwrap_or(text.len());
        let mut replace_start = offset.saturating_sub(query.len());
        let mut prefix = query.to_string();
        let mut qualifier = None;

        if let Some((before_dot, after_dot)) = query.rsplit_once('.') {
            prefix = after_dot.to_string();
            qualifier = (!before_dot.is_empty()).then(|| before_dot.to_string());
            replace_start = offset.saturating_sub(prefix.len());
        } else if replace_start > 0
            && rope.slice(replace_start - 1..replace_start).to_string() == "."
        {
            qualifier = previous_identifier(rope, replace_start - 1);
        }

        let statement = text[statement_start..statement_end].to_string();
        let context_start_in_statement = replace_start.saturating_sub(statement_start);
        let table_refs = table_refs_for_statement(&statement);
        let scope = completion_scope(&statement, context_start_in_statement);

        Self {
            offset,
            replace_start,
            prefix,
            qualifier,
            scope,
            table_refs,
            rope: rope.clone(),
        }
    }

    fn replace_range(&self) -> lsp_types::Range {
        lsp_types::Range::new(
            self.rope.offset_to_position(self.replace_start),
            self.rope.offset_to_position(self.offset),
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CompletionScope {
    TableReference,
    SelectList,
    General,
}

#[derive(Clone)]
struct TableRef {
    table_name: String,
    schema_name: Option<String>,
    alias: Option<String>,
}

fn cached_schema(
    cache: &Arc<RwLock<Option<SchemaCache>>>,
    key: &str,
) -> Option<Arc<DatabaseSchema>> {
    cache
        .read()
        .ok()
        .and_then(|guard| guard.as_ref().filter(|cache| cache.key == key).cloned())
        .map(|cache| cache.schema)
}

fn build_items(context: &CompletionContextData, schema: &DatabaseSchema) -> Vec<CompletionItem> {
    if let Some(qualifier) = context.qualifier.as_deref() {
        let mut items = qualified_column_items(context, schema, qualifier);
        if !items.is_empty() {
            items.extend(general_items(context, schema));
            return limit_items(items);
        }
    }

    if context.scope == CompletionScope::TableReference {
        let mut items = table_reference_items(context, schema);
        items.extend(keyword_items(context, 60));
        return limit_items(items);
    }

    if context.scope == CompletionScope::SelectList {
        let mut items = involved_column_items(context, schema);
        if !items.is_empty() {
            items.extend(general_items(context, schema));
            return limit_items(items);
        }
    }

    if context.prefix.is_empty() {
        return Vec::new();
    }

    limit_items(general_items(context, schema))
}

fn qualified_column_items(
    context: &CompletionContextData,
    schema: &DatabaseSchema,
    qualifier: &str,
) -> Vec<ScoredCompletion> {
    let mut items = Vec::new();

    for table in schema.tables.iter().filter(|table| {
        table.name.eq_ignore_ascii_case(qualifier)
            || format!("{}.{}", table.schema, table.name).eq_ignore_ascii_case(qualifier)
    }) {
        items.extend(column_items_for_table(context, table, 0));
    }

    for table_ref in context
        .table_refs
        .iter()
        .filter(|table_ref| table_ref.alias_matches(qualifier))
    {
        if let Some(table) = table_for_ref(schema, table_ref) {
            items.extend(column_items_for_table(context, table, 0));
        }
    }

    items
}

fn table_reference_items(
    context: &CompletionContextData,
    schema: &DatabaseSchema,
) -> Vec<ScoredCompletion> {
    let mut items = Vec::new();

    for table_ref in &context.table_refs {
        if let Some(alias) = table_ref.alias.as_deref() {
            if matches_prefix(alias, &context.prefix) {
                items.push(scored_completion_item(
                    context,
                    alias,
                    CompletionItemKind::VARIABLE,
                    Some(format!("alias for {}", table_ref.display_name())),
                    0,
                ));
            }
        }
    }

    for table in &schema.tables {
        if matches_prefix(&table.name, &context.prefix) {
            items.push(table_item(context, table, 10));
        }
    }

    for schema_name in &schema.schemas {
        if matches_prefix(&schema_name.name, &context.prefix) {
            items.push(scored_completion_item(
                context,
                &schema_name.name,
                CompletionItemKind::MODULE,
                Some("schema".to_string()),
                20,
            ));
        }
    }

    items
}

fn involved_column_items(
    context: &CompletionContextData,
    schema: &DatabaseSchema,
) -> Vec<ScoredCompletion> {
    let mut items = Vec::new();
    for table_ref in &context.table_refs {
        if let Some(table) = table_for_ref(schema, table_ref) {
            items.extend(column_items_for_table(context, table, 0));
        }
    }
    items
}

fn general_items(
    context: &CompletionContextData,
    schema: &DatabaseSchema,
) -> Vec<ScoredCompletion> {
    let mut items = keyword_items(context, 50);

    for schema_name in &schema.schemas {
        if matches_prefix(&schema_name.name, &context.prefix) {
            items.push(scored_completion_item(
                context,
                &schema_name.name,
                CompletionItemKind::MODULE,
                Some("schema".to_string()),
                40,
            ));
        }
    }

    for table in &schema.tables {
        if matches_prefix(&table.name, &context.prefix) {
            items.push(table_item(context, table, 30));
        }

        items.extend(column_items_for_table(context, table, 20));
    }

    for function in &schema.functions {
        if matches_prefix(&function.name, &context.prefix) {
            items.push(scored_completion_item(
                context,
                &function.name,
                CompletionItemKind::FUNCTION,
                Some(format!(
                    "{}.{}({}) -> {}",
                    function.schema, function.name, function.arguments, function.return_type
                )),
                45,
            ));
        }
    }

    items
}

fn column_items_for_table(
    context: &CompletionContextData,
    table: &TableInfo,
    score: usize,
) -> Vec<ScoredCompletion> {
    table
        .columns
        .iter()
        .filter(|column| matches_prefix(&column.name, &context.prefix))
        .map(|column| {
            scored_completion_item(
                context,
                &column.name,
                CompletionItemKind::FIELD,
                Some(format!(
                    "{}.{} {}{} #{}",
                    table.name,
                    column.name,
                    column.data_type,
                    if column.nullable { "" } else { " not null" },
                    column.ordinal
                )),
                score,
            )
        })
        .collect()
}

fn table_item(
    context: &CompletionContextData,
    table: &TableInfo,
    score: usize,
) -> ScoredCompletion {
    scored_completion_item(
        context,
        &table.name,
        CompletionItemKind::CLASS,
        Some(format!(
            "{} {}",
            table.schema,
            table_kind_label(&table.kind)
        )),
        score,
    )
}

fn keyword_items(context: &CompletionContextData, score: usize) -> Vec<ScoredCompletion> {
    SQL_KEYWORDS
        .iter()
        .filter(|keyword| matches_prefix(keyword, &context.prefix))
        .map(|keyword| {
            scored_completion_item(
                context,
                keyword,
                CompletionItemKind::KEYWORD,
                Some("SQL keyword".to_string()),
                score,
            )
        })
        .collect()
}

#[derive(Clone)]
struct ScoredCompletion {
    score: usize,
    item: CompletionItem,
}

fn scored_completion_item(
    context: &CompletionContextData,
    label: &str,
    kind: CompletionItemKind,
    documentation: Option<String>,
    score: usize,
) -> ScoredCompletion {
    ScoredCompletion {
        score,
        item: CompletionItem {
            label: label.to_string(),
            kind: Some(kind),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range: context.replace_range(),
                new_text: label.to_string(),
            })),
            documentation: documentation.map(Documentation::String),
            ..Default::default()
        },
    }
}

impl TableRef {
    fn alias_matches(&self, qualifier: &str) -> bool {
        self.alias
            .as_deref()
            .is_some_and(|alias| alias.eq_ignore_ascii_case(qualifier))
    }

    fn display_name(&self) -> String {
        match self.schema_name.as_deref() {
            Some(schema) => format!("{}.{}", schema, self.table_name),
            None => self.table_name.clone(),
        }
    }
}

fn table_for_ref<'a>(schema: &'a DatabaseSchema, table_ref: &TableRef) -> Option<&'a TableInfo> {
    schema.tables.iter().find(|table| {
        table.name.eq_ignore_ascii_case(&table_ref.table_name)
            && table_ref
                .schema_name
                .as_deref()
                .is_none_or(|schema_name| table.schema.eq_ignore_ascii_case(schema_name))
    })
}

fn completion_scope(statement: &str, cursor: usize) -> CompletionScope {
    let before_cursor = &statement[..cursor.min(statement.len())];
    let tokens = sql_tokens(before_cursor);
    if is_after_table_keyword(&tokens) {
        return CompletionScope::TableReference;
    }

    let all_tokens = sql_tokens(statement);
    let before_tokens = sql_tokens(before_cursor);
    if token_position(&before_tokens, "select").is_some()
        && token_position(&before_tokens, "from").is_none()
        && token_position(&all_tokens, "from").is_some()
    {
        return CompletionScope::SelectList;
    }

    CompletionScope::General
}

fn table_refs_for_statement(statement: &str) -> Vec<TableRef> {
    let tokens = sql_tokens(statement);
    let mut refs = Vec::new();
    let mut ix = 0;

    while ix < tokens.len() {
        if !is_table_reference_keyword(&tokens, ix) {
            ix += 1;
            continue;
        }

        ix += 1;
        let Some(table_token) = tokens.get(ix) else {
            break;
        };
        if is_reserved_token(table_token) {
            continue;
        }

        let (schema_name, table_name) = split_table_name(table_token);
        ix += 1;

        let alias = if tokens.get(ix).is_some_and(|token| token == "as") {
            ix += 1;
            alias_token(tokens.get(ix))
        } else {
            alias_token(tokens.get(ix))
        };

        refs.push(TableRef {
            table_name,
            schema_name,
            alias,
        });
    }

    refs
}

fn is_after_table_keyword(tokens: &[String]) -> bool {
    let mut ix = tokens.len();
    while ix > 0 {
        ix -= 1;
        let token = &tokens[ix];
        if token == "," {
            continue;
        }
        return token == "from" || token == "join";
    }
    false
}

fn is_table_reference_keyword(tokens: &[String], ix: usize) -> bool {
    tokens[ix] == "from" || tokens[ix] == "join"
}

fn alias_token(token: Option<&String>) -> Option<String> {
    let token = token?;
    (!is_reserved_token(token) && token != "," && token != ".").then(|| token.clone())
}

fn split_table_name(token: &str) -> (Option<String>, String) {
    if let Some((schema, table)) = token.rsplit_once('.') {
        (Some(schema.to_string()), table.to_string())
    } else {
        (None, token.to_string())
    }
}

fn token_position(tokens: &[String], needle: &str) -> Option<usize> {
    tokens.iter().position(|token| token == needle)
}

fn sql_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut token = String::new();

    for ch in text.chars() {
        if ch == '_' || ch == '.' || ch.is_ascii_alphanumeric() {
            token.push(ch.to_ascii_lowercase());
        } else {
            if !token.is_empty() {
                tokens.push(std::mem::take(&mut token));
            }
            if ch == ',' {
                tokens.push(ch.to_string());
            }
        }
    }

    if !token.is_empty() {
        tokens.push(token);
    }

    tokens
}

fn is_reserved_token(token: &str) -> bool {
    matches!(
        token,
        "select"
            | "from"
            | "join"
            | "left"
            | "right"
            | "inner"
            | "full"
            | "outer"
            | "cross"
            | "on"
            | "where"
            | "group"
            | "order"
            | "having"
            | "limit"
            | "offset"
            | "union"
            | "returning"
            | "as"
    )
}

fn previous_identifier(rope: &Rope, before_offset: usize) -> Option<String> {
    let text = rope.slice(0..before_offset).to_string();
    let identifier = text
        .chars()
        .rev()
        .take_while(|ch| *ch == '_' || *ch == '.' || ch.is_ascii_alphanumeric())
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();

    (!identifier.is_empty()).then_some(identifier)
}

fn matches_prefix(value: &str, prefix: &str) -> bool {
    prefix.is_empty()
        || value
            .to_ascii_lowercase()
            .starts_with(&prefix.to_ascii_lowercase())
}

fn table_kind_label(kind: &TableKind) -> &'static str {
    match kind {
        TableKind::Table => "table",
        TableKind::View => "view",
        TableKind::MaterializedView => "materialized view",
        TableKind::ForeignTable => "foreign table",
    }
}

fn limit_items(mut items: Vec<ScoredCompletion>) -> Vec<CompletionItem> {
    items.sort_by(|left, right| {
        left.score
            .cmp(&right.score)
            .then_with(|| left.item.label.cmp(&right.item.label))
    });
    let mut seen = HashSet::new();
    let mut items = items
        .into_iter()
        .filter(|completion| seen.insert(completion.item.label.clone()))
        .take(80)
        .map(|completion| completion.item)
        .collect::<Vec<_>>();
    items.shrink_to_fit();
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_table_reference_scope_with_partial_prefix() {
        let statement = "select * from customers c join ord";
        let cursor = statement.rfind("ord").unwrap();
        assert!(matches!(
            completion_scope(statement, cursor),
            CompletionScope::TableReference
        ));
    }

    #[test]
    fn detects_table_reference_scope_after_keyword_space() {
        let statement = "select * from ";
        assert!(matches!(
            completion_scope(statement, statement.len()),
            CompletionScope::TableReference
        ));
    }

    #[test]
    fn extracts_table_aliases_from_from_and_join() {
        let refs = table_refs_for_statement(
            "select c.id, o.status from public.customers c join orders as o on o.customer_id = c.id",
        );

        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].schema_name.as_deref(), Some("public"));
        assert_eq!(refs[0].table_name, "customers");
        assert_eq!(refs[0].alias.as_deref(), Some("c"));
        assert_eq!(refs[1].table_name, "orders");
        assert_eq!(refs[1].alias.as_deref(), Some("o"));
    }

    #[test]
    fn detects_select_list_scope_before_from() {
        let statement = "select c from customers c join orders o on o.customer_id = c.id";
        let cursor = statement.find("c from").unwrap();
        assert!(matches!(
            completion_scope(statement, cursor),
            CompletionScope::SelectList
        ));
    }
}
