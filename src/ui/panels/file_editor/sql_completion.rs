use std::collections::HashSet;
use std::rc::Rc;
use std::sync::{Arc, RwLock};

use anyhow::Result;
use gpui::{Context, Entity, Task, Window};
use gpui_component::input::{CompletionProvider, InputState, Rope, RopeExt};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionItemLabelDetails,
    CompletionResponse, CompletionTextEdit, Documentation, TextEdit,
};
use serde_json::json;

use crate::data_source::manager::DataSourceManager;
use crate::data_source::{DataSourceConfig, DatabaseSchema, TableInfo};
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
        let _ = trigger;
        let config = self.manager.read(cx).active_config().cloned();
        let cache = self.cache.clone();
        let context = CompletionContextData::new(rope, offset);

        if let Some(config) = config {
            let key = schema_cache::cache_key(&config);

            // Check in-memory cache first
            if let Some(schema) = cached_schema(&cache, &key) {
                return Task::ready(Ok(CompletionResponse::Array(build_items(
                    &context,
                    schema.as_ref(),
                    &config,
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
                        &config,
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
        new_text.chars().any(|ch| {
            ch == '.'
                || ch == '_'
                || ch == ','
                || ch == '"'
                || ch.is_ascii_alphanumeric()
                || ch.is_whitespace()
        })
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
    fn new(rope: &Rope, offset: usize) -> Self {
        let text = rope.to_string();
        let statement_start = text[..offset]
            .rfind(';')
            .map(|ix| ix + 1)
            .unwrap_or_default();
        let statement_end = text[offset..]
            .find(';')
            .map(|ix| offset + ix)
            .unwrap_or(text.len());
        let current_token = current_completion_token(rope, offset);
        let mut replace_start = offset.saturating_sub(current_token.len());
        let mut prefix = current_token.clone();
        let mut qualifier = None;

        if let Some((before_dot, after_dot)) = current_token.rsplit_once('.') {
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
    JoinReference,
    SelectList,
    General,
}

#[derive(Clone)]
struct TableRef {
    table_name: String,
    schema_name: Option<String>,
    alias: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SqlDiagnostic {
    pub range: std::ops::Range<usize>,
    pub message: String,
}

pub fn sql_diagnostics_at(
    text: &str,
    schema: &DatabaseSchema,
    cursor: Option<usize>,
) -> Vec<SqlDiagnostic> {
    let mut diagnostics = Vec::new();
    let mut statement_start = 0;
    for statement in text.split_inclusive(';') {
        let statement_text = statement.trim_end_matches(';');
        diagnostics.extend(sql_diagnostics_for_statement(
            statement_text,
            statement_start,
            schema,
            cursor,
        ));
        statement_start += statement.len();
    }
    diagnostics
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

fn build_items(
    context: &CompletionContextData,
    schema: &DatabaseSchema,
    config: &DataSourceConfig,
) -> Vec<CompletionItem> {
    if let Some(qualifier) = context.qualifier.as_deref() {
        if matches!(
            context.scope,
            CompletionScope::TableReference | CompletionScope::JoinReference
        ) {
            let mut items = schema_table_items(context, schema, qualifier, config);
            if !items.is_empty() {
                items.extend(keyword_items(context, 60));
                return limit_items(items);
            }
        }

        let items = qualified_column_items(context, schema, qualifier);
        return limit_items(items);
    }

    if matches!(
        context.scope,
        CompletionScope::TableReference | CompletionScope::JoinReference
    ) {
        let mut items = table_reference_items(context, schema, config);
        items.extend(keyword_items(context, 60));
        return limit_items(items);
    }

    if context.scope == CompletionScope::SelectList {
        let mut items = involved_column_items(context, schema);
        if !items.is_empty() {
            items.extend(keyword_items(context, 50));
            return limit_items(items);
        }
    }

    if context.prefix.is_empty() {
        return Vec::new();
    }

    limit_items(general_items(context, schema, config))
}

fn schema_table_items(
    context: &CompletionContextData,
    schema: &DatabaseSchema,
    qualifier: &str,
    config: &DataSourceConfig,
) -> Vec<ScoredCompletion> {
    schema
        .tables
        .iter()
        .filter(|table| {
            table.schema.eq_ignore_ascii_case(qualifier)
                && matches_prefix(&table.name, &context.prefix)
        })
        .map(|table| table_item(context, table, config, 0))
        .collect()
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
        items.extend(column_items_for_table(context, table, qualifier, 0));
    }

    for table_ref in context
        .table_refs
        .iter()
        .filter(|table_ref| table_ref.alias_matches(qualifier))
    {
        if let Some(table) = table_for_ref(schema, table_ref) {
            let owner = table_ref.alias.as_deref().unwrap_or(&table.name);
            items.extend(column_items_for_table(context, table, owner, 0));
        }
    }

    items
}

fn table_reference_items(
    context: &CompletionContextData,
    schema: &DatabaseSchema,
    config: &DataSourceConfig,
) -> Vec<ScoredCompletion> {
    let mut items = Vec::new();

    if context.scope == CompletionScope::JoinReference {
        items.extend(join_items(context, schema, config));
    }

    for table_ref in &context.table_refs {
        if let Some(alias) = table_ref.alias.as_deref() {
            if matches_prefix(alias, &context.prefix) {
                items.push(scored_completion_item(
                    context,
                    alias,
                    CompletionItemKind::VARIABLE,
                    Some(format!("alias for {}", table_ref.display_name())),
                    None,
                    None,
                    0,
                ));
            }
        }
    }

    for table in &schema.tables {
        if matches_prefix(&table.name, &context.prefix) {
            items.push(table_item(context, table, config, 10));
        }
    }

    for schema_name in &schema.schemas {
        if matches_prefix(&schema_name.name, &context.prefix) {
            items.push(scored_completion_item(
                context,
                &schema_name.name,
                CompletionItemKind::MODULE,
                Some("schema".to_string()),
                None,
                Some(config.name.clone()),
                20,
            ));
        }
    }

    items
}

fn join_items(
    context: &CompletionContextData,
    schema: &DatabaseSchema,
    config: &DataSourceConfig,
) -> Vec<ScoredCompletion> {
    let mut items = Vec::new();
    for existing_ref in &context.table_refs {
        let Some(existing_table) = table_for_ref(schema, existing_ref) else {
            continue;
        };
        let existing_alias = existing_ref
            .alias
            .as_deref()
            .unwrap_or(&existing_table.name);

        for fk in &schema.foreign_keys {
            let candidate =
                if table_identity_matches(existing_table, &fk.source_schema, &fk.source_table) {
                    schema.tables.iter().find(|table| {
                        table_identity_matches(table, &fk.target_schema, &fk.target_table)
                    })
                } else if table_identity_matches(
                    existing_table,
                    &fk.target_schema,
                    &fk.target_table,
                ) {
                    schema.tables.iter().find(|table| {
                        table_identity_matches(table, &fk.source_schema, &fk.source_table)
                    })
                } else {
                    None
                };

            let Some(candidate) = candidate else {
                continue;
            };
            if context.table_refs.iter().any(|table_ref| {
                table_for_ref(schema, table_ref).is_some_and(|table| {
                    table.schema.eq_ignore_ascii_case(&candidate.schema)
                        && table.name.eq_ignore_ascii_case(&candidate.name)
                })
            }) {
                continue;
            }
            if !matches_prefix(&candidate.name, &context.prefix) {
                continue;
            }

            let candidate_alias = unique_alias(&candidate.name, &context.table_refs);
            let conditions =
                if table_identity_matches(existing_table, &fk.source_schema, &fk.source_table) {
                    join_conditions(
                        existing_alias,
                        &fk.source_columns,
                        &candidate_alias,
                        &fk.target_columns,
                    )
                } else {
                    join_conditions(
                        existing_alias,
                        &fk.target_columns,
                        &candidate_alias,
                        &fk.source_columns,
                    )
                };
            let insert_text = format!(
                "{}.{} {} on {}",
                candidate.schema, candidate.name, candidate_alias, conditions
            );
            let label = format!("{} {} on {}", candidate.name, candidate_alias, conditions);
            items.push(scored_completion_item(
                context,
                &label,
                CompletionItemKind::CLASS,
                Some(format!("({}.{})", config.database, candidate.schema)),
                Some(insert_text),
                Some(config.name.clone()),
                0,
            ));
        }
    }
    items
}

fn table_identity_matches(table: &TableInfo, schema: &str, name: &str) -> bool {
    table.schema.eq_ignore_ascii_case(schema) && table.name.eq_ignore_ascii_case(name)
}

fn join_conditions(
    left_alias: &str,
    left_columns: &[String],
    right_alias: &str,
    right_columns: &[String],
) -> String {
    left_columns
        .iter()
        .zip(right_columns.iter())
        .map(|(left, right)| format!("{left_alias}.{left} = {right_alias}.{right}"))
        .collect::<Vec<_>>()
        .join(" and ")
}

fn unique_alias(table_name: &str, refs: &[TableRef]) -> String {
    let mut alias = table_name
        .split('_')
        .filter_map(|part| part.chars().next())
        .collect::<String>();
    if alias.is_empty() {
        alias = table_name.chars().next().unwrap_or('t').to_string();
    }
    let base = alias.clone();
    for ix in 2.. {
        if !refs.iter().any(|table_ref| {
            table_ref
                .alias
                .as_deref()
                .is_some_and(|existing| existing.eq_ignore_ascii_case(&alias))
        }) {
            return alias;
        }
        alias = format!("{base}{ix}");
    }
    unreachable!()
}

fn involved_column_items(
    context: &CompletionContextData,
    schema: &DatabaseSchema,
) -> Vec<ScoredCompletion> {
    let mut items = Vec::new();
    for table_ref in &context.table_refs {
        if let Some(table) = table_for_ref(schema, table_ref) {
            let owner = table_ref.alias.as_deref().unwrap_or(&table.name);
            items.extend(column_items_for_table(context, table, owner, 0));
        }
    }
    items
}

fn general_items(
    context: &CompletionContextData,
    schema: &DatabaseSchema,
    config: &DataSourceConfig,
) -> Vec<ScoredCompletion> {
    let mut items = keyword_items(context, 50);

    let scoped_columns = involved_column_items(context, schema);
    if !scoped_columns.is_empty() {
        items.extend(scoped_columns.into_iter().map(|mut item| {
            item.score = item.score.min(20);
            item
        }));
    }

    for schema_name in &schema.schemas {
        if matches_prefix(&schema_name.name, &context.prefix) {
            items.push(scored_completion_item(
                context,
                &schema_name.name,
                CompletionItemKind::MODULE,
                Some("schema".to_string()),
                None,
                Some(config.name.clone()),
                40,
            ));
        }
    }

    for table in &schema.tables {
        if matches_prefix(&table.name, &context.prefix) {
            items.push(table_item(context, table, config, 30));
        }
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
                None,
                Some(function.return_type.clone()),
                45,
            ));
        }
    }

    items
}

fn column_items_for_table(
    context: &CompletionContextData,
    table: &TableInfo,
    owner: &str,
    score: usize,
) -> Vec<ScoredCompletion> {
    table
        .columns
        .iter()
        .filter(|column| matches_prefix(&column.name, &context.prefix))
        .map(|column| {
            let key_score = if column.is_pk {
                0
            } else if column.is_fk {
                1
            } else {
                4
            };
            scored_completion_item(
                context,
                &column.name,
                CompletionItemKind::FIELD,
                Some(format!("({})", owner)),
                None,
                Some(column.data_type.clone()),
                score + key_score,
            )
        })
        .collect()
}

fn table_item(
    context: &CompletionContextData,
    table: &TableInfo,
    config: &DataSourceConfig,
    score: usize,
) -> ScoredCompletion {
    scored_completion_item(
        context,
        &table.name,
        CompletionItemKind::CLASS,
        Some(format!("({}.{})", config.database, table.schema)),
        None,
        Some(config.name.clone()),
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
                None,
                None,
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
    insert_text: Option<String>,
    right: Option<String>,
    score: usize,
) -> ScoredCompletion {
    let new_text = insert_text.unwrap_or_else(|| label.to_string());
    ScoredCompletion {
        score,
        item: CompletionItem {
            label: label.to_string(),
            kind: Some(kind),
            label_details: Some(CompletionItemLabelDetails {
                detail: documentation.clone(),
                description: right.clone(),
            }),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range: context.replace_range(),
                new_text,
            })),
            documentation: documentation.map(Documentation::String),
            data: Some(json!({ "right": right })),
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

fn sql_diagnostics_for_statement(
    statement: &str,
    statement_start: usize,
    schema: &DatabaseSchema,
    cursor: Option<usize>,
) -> Vec<SqlDiagnostic> {
    let tokens = positioned_sql_tokens(statement);
    let refs = table_refs_for_statement(statement);
    let mut diagnostics = if cursor
        .and_then(|cursor| cursor.checked_sub(statement_start))
        .is_some_and(|cursor| current_token_before_cursor(statement, cursor).ends_with('.'))
    {
        Vec::new()
    } else {
        syntax_diagnostics_for_statement(statement, statement_start, cursor)
    };

    for ix in 0..tokens.len() {
        if !is_table_reference_keyword(
            &tokens.iter().map(|t| t.text.clone()).collect::<Vec<_>>(),
            ix,
        ) {
            continue;
        }
        let Some(table_token) = tokens.get(ix + 1) else {
            continue;
        };
        if is_reserved_token(&table_token.text) {
            continue;
        }
        let (schema_name, table_name) = split_table_name(&table_token.text);
        if !schema.tables.iter().any(|table| {
            table.name.eq_ignore_ascii_case(&table_name)
                && schema_name
                    .as_deref()
                    .is_none_or(|schema_name| table.schema.eq_ignore_ascii_case(schema_name))
        }) {
            diagnostics.push(SqlDiagnostic {
                range: statement_start + table_token.start..statement_start + table_token.end,
                message: format!("Unknown table `{}`", table_token.text),
            });
        }
    }

    let token_texts = tokens
        .iter()
        .map(|token| token.text.clone())
        .collect::<Vec<_>>();
    for (ix, token) in tokens.iter().enumerate() {
        if is_reserved_token(&token.text)
            || is_numeric_token(&token.text)
            || is_operator_token(&token.text)
            || cursor.is_some_and(|cursor| {
                (statement_start + token.start..=statement_start + token.end).contains(&cursor)
            })
        {
            continue;
        }
        if ix > 0 && is_table_reference_keyword(&token_texts, ix - 1) {
            continue;
        }
        if let Some((qualifier, column)) = token.text.rsplit_once('.') {
            if column.is_empty() {
                continue;
            }
            if schema.tables.iter().any(|table| {
                table.schema.eq_ignore_ascii_case(qualifier)
                    && table.name.eq_ignore_ascii_case(column)
            }) {
                continue;
            }
            if let Some(table) = table_for_qualifier(schema, &refs, qualifier) {
                if !table
                    .columns
                    .iter()
                    .any(|candidate| candidate.name.eq_ignore_ascii_case(column))
                {
                    diagnostics.push(SqlDiagnostic {
                        range: statement_start + token.start..statement_start + token.end,
                        message: format!("Unknown column `{}` on `{}`", column, qualifier),
                    });
                }
            } else if !schema
                .schemas
                .iter()
                .any(|schema_info| schema_info.name.eq_ignore_ascii_case(qualifier))
            {
                diagnostics.push(SqlDiagnostic {
                    range: statement_start + token.start..statement_start + token.end,
                    message: format!("Unknown qualifier `{}`", qualifier),
                });
            }
        } else if refs.len() > 0
            && is_expression_position(&token_texts, ix)
            && !refs.iter().any(|table_ref| {
                table_ref
                    .alias
                    .as_deref()
                    .is_some_and(|alias| alias.eq_ignore_ascii_case(&token.text))
                    || table_ref.table_name.eq_ignore_ascii_case(&token.text)
            })
            && !schema
                .functions
                .iter()
                .any(|function| function.name.eq_ignore_ascii_case(&token.text))
            && !refs
                .iter()
                .filter_map(|table_ref| table_for_ref(schema, table_ref))
                .any(|table| {
                    table
                        .columns
                        .iter()
                        .any(|column| column.name.eq_ignore_ascii_case(&token.text))
                })
        {
            diagnostics.push(SqlDiagnostic {
                range: statement_start + token.start..statement_start + token.end,
                message: format!("Unknown column `{}`", token.text),
            });
        }
    }

    diagnostics
}

fn syntax_diagnostics_for_statement(
    statement: &str,
    statement_start: usize,
    cursor: Option<usize>,
) -> Vec<SqlDiagnostic> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&tree_sitter_sequel::LANGUAGE.into())
        .is_err()
    {
        return Vec::new();
    }
    let Some(tree) = parser.parse(statement, None) else {
        return Vec::new();
    };
    let mut diagnostics = Vec::new();
    collect_syntax_diagnostics(tree.root_node(), statement_start, cursor, &mut diagnostics);
    diagnostics
}

fn collect_syntax_diagnostics(
    node: tree_sitter::Node,
    statement_start: usize,
    cursor: Option<usize>,
    diagnostics: &mut Vec<SqlDiagnostic>,
) {
    if node.is_error() || node.is_missing() {
        let start = statement_start + node.start_byte();
        let end = statement_start + node.end_byte().max(node.start_byte() + 1);
        if cursor.is_some_and(|cursor| start <= cursor && cursor <= end) {
            return;
        }
        diagnostics.push(SqlDiagnostic {
            range: start..end,
            message: "SQL syntax error".to_string(),
        });
        return;
    }

    let mut walk = node.walk();
    for child in node.children(&mut walk) {
        collect_syntax_diagnostics(child, statement_start, cursor, diagnostics);
    }
}

fn table_for_qualifier<'a>(
    schema: &'a DatabaseSchema,
    refs: &[TableRef],
    qualifier: &str,
) -> Option<&'a TableInfo> {
    refs.iter()
        .find(|table_ref| table_ref.alias_matches(qualifier))
        .and_then(|table_ref| table_for_ref(schema, table_ref))
        .or_else(|| {
            refs.iter()
                .find(|table_ref| table_ref.table_name.eq_ignore_ascii_case(qualifier))
                .and_then(|table_ref| table_for_ref(schema, table_ref))
        })
}

fn is_numeric_token(token: &str) -> bool {
    token.chars().all(|ch| ch.is_ascii_digit())
}

fn is_operator_token(token: &str) -> bool {
    matches!(
        token,
        "=" | "<" | ">" | "<>" | "!=" | "<=" | ">=" | "+" | "-" | "*" | "/" | "%"
    )
}

fn is_expression_position(tokens: &[String], ix: usize) -> bool {
    tokens[..ix]
        .iter()
        .rev()
        .find(|token| token.as_str() != "," && token.as_str() != ".")
        .is_some_and(|token| {
            matches!(
                token.as_str(),
                "select" | "where" | "and" | "or" | "on" | "=" | "<" | ">" | "by"
            )
        })
}

fn completion_scope(statement: &str, cursor: usize) -> CompletionScope {
    let before_cursor = &statement[..cursor.min(statement.len())];
    let tokens = sql_tokens(before_cursor);
    if let Some(keyword) = table_keyword_before_cursor(&tokens) {
        return if keyword == "join" {
            CompletionScope::JoinReference
        } else {
            CompletionScope::TableReference
        };
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

fn table_keyword_before_cursor(tokens: &[String]) -> Option<&str> {
    let mut ix = tokens.len();
    while ix > 0 {
        ix -= 1;
        let token = &tokens[ix];
        if token == "," {
            continue;
        }
        if token == "from" || token == "join" {
            return Some(token);
        }
        return None;
    }
    None
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
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\'' {
            if !token.is_empty() {
                tokens.push(std::mem::take(&mut token));
            }
            while let Some(next) = chars.next() {
                if next == '\'' {
                    if chars.peek() == Some(&'\'') {
                        chars.next();
                    } else {
                        break;
                    }
                }
            }
            continue;
        }

        if ch == '_' || ch == '.' || ch.is_ascii_alphanumeric() {
            token.push(ch.to_ascii_lowercase());
        } else {
            if !token.is_empty() {
                tokens.push(std::mem::take(&mut token));
            }
            if matches!(ch, ',' | '=' | '<' | '>' | '!') {
                let mut operator = ch.to_string();
                if matches!(ch, '<' | '>' | '!')
                    && chars.peek().is_some_and(|next| matches!(next, '=' | '>'))
                {
                    operator.push(chars.next().unwrap());
                }
                tokens.push(operator);
            }
        }
    }

    if !token.is_empty() {
        tokens.push(token);
    }

    tokens
}

#[derive(Clone)]
struct PositionedToken {
    text: String,
    start: usize,
    end: usize,
}

fn positioned_sql_tokens(text: &str) -> Vec<PositionedToken> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut token_start = 0;
    let mut chars = text.char_indices().peekable();

    while let Some((ix, ch)) = chars.next() {
        if ch == '\'' {
            if !token.is_empty() {
                tokens.push(PositionedToken {
                    text: std::mem::take(&mut token),
                    start: token_start,
                    end: ix,
                });
            }
            while let Some((_, next)) = chars.next() {
                if next == '\'' {
                    if chars.peek().is_some_and(|(_, peek)| *peek == '\'') {
                        chars.next();
                    } else {
                        break;
                    }
                }
            }
            continue;
        }

        if ch == '_' || ch == '.' || ch.is_ascii_alphanumeric() {
            if token.is_empty() {
                token_start = ix;
            }
            token.push(ch.to_ascii_lowercase());
        } else {
            if !token.is_empty() {
                tokens.push(PositionedToken {
                    text: std::mem::take(&mut token),
                    start: token_start,
                    end: ix,
                });
            }
            if matches!(ch, ',' | '=' | '<' | '>' | '!') {
                let mut text = ch.to_string();
                let mut end = ix + ch.len_utf8();
                if matches!(ch, '<' | '>' | '!')
                    && chars
                        .peek()
                        .is_some_and(|(_, next)| matches!(next, '=' | '>'))
                {
                    let (next_ix, next) = chars.next().unwrap();
                    text.push(next);
                    end = next_ix + next.len_utf8();
                }
                tokens.push(PositionedToken {
                    text,
                    start: ix,
                    end,
                });
            }
        }
    }

    if !token.is_empty() {
        tokens.push(PositionedToken {
            text: token,
            start: token_start,
            end: text.len(),
        });
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

fn current_completion_token(rope: &Rope, offset: usize) -> String {
    let text = rope.slice(0..offset).to_string();
    current_token_before_cursor(&text, text.len())
}

fn current_token_before_cursor(text: &str, offset: usize) -> String {
    text[..offset.min(text.len())]
        .chars()
        .rev()
        .take_while(|ch| is_completion_token_char(*ch))
        .collect::<String>()
        .chars()
        .rev()
        .collect()
}

fn is_completion_token_char(ch: char) -> bool {
    ch == '_' || ch == '.' || ch == '"' || ch.is_ascii_alphanumeric()
}

fn matches_prefix(value: &str, prefix: &str) -> bool {
    prefix.is_empty()
        || value
            .to_ascii_lowercase()
            .starts_with(&prefix.to_ascii_lowercase())
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
    use crate::data_source::{ColumnInfo, TableKind};

    #[test]
    fn detects_table_reference_scope_with_partial_prefix() {
        let statement = "select * from customers c join ord";
        let cursor = statement.rfind("ord").unwrap();
        assert!(matches!(
            completion_scope(statement, cursor),
            CompletionScope::JoinReference
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

    #[test]
    fn derives_completion_prefix_from_rope() {
        let rope = Rope::from("select * from ord");
        let context = CompletionContextData::new(&rope, rope.len());

        assert_eq!(context.prefix, "ord");
        assert_eq!(context.replace_start, "select * from ".len());
        assert_eq!(context.qualifier, None);
        assert!(matches!(context.scope, CompletionScope::TableReference));
    }

    #[test]
    fn derives_qualifier_after_dot_from_rope() {
        let rope = Rope::from("select u. from users u");
        let offset = "select u.".len();
        let context = CompletionContextData::new(&rope, offset);

        assert_eq!(context.prefix, "");
        assert_eq!(context.replace_start, offset);
        assert_eq!(context.qualifier.as_deref(), Some("u"));
    }

    #[test]
    fn suggests_tables_after_schema_qualifier_in_table_scope() {
        let rope = Rope::from("select * from public.us");
        let context = CompletionContextData::new(&rope, rope.len());
        let schema = DatabaseSchema {
            tables: vec![TableInfo {
                schema: "public".to_string(),
                name: "users".to_string(),
                kind: TableKind::Table,
                columns: Vec::new(),
            }],
            ..Default::default()
        };

        let config = DataSourceConfig {
            name: "local".to_string(),
            database: "app".to_string(),
            ..Default::default()
        };

        let items = schema_table_items(&context, &schema, "public", &config);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].item.label, "users");
    }

    #[test]
    fn diagnostics_ignore_strings_and_not_equal_operator() {
        let schema = test_schema();
        let text = "select o.created_at from customers c inner join orders o on o.customer_id = c.id where c.city <> 'S';";

        let diagnostics = sql_diagnostics_at(text, &schema, None);

        assert_eq!(diagnostics, Vec::new());
    }

    #[test]
    fn diagnostics_ignore_current_incomplete_qualified_column() {
        let schema = test_schema();
        let text =
            "select name, c.city, o. from customers c inner join orders o on o.customer_id = c.id;";
        let cursor = text.find("o. from").unwrap() + "o.".len();

        let diagnostics = sql_diagnostics_at(text, &schema, Some(cursor));

        assert_eq!(diagnostics, Vec::new());
    }

    #[test]
    fn key_columns_sort_before_regular_columns() {
        let rope = Rope::from("select o. from orders o");
        let context = CompletionContextData::new(&rope, "select o.".len());
        let schema = test_schema();

        let items = limit_items(qualified_column_items(&context, &schema, "o"));

        assert_eq!(
            items
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            vec!["id", "customer_id", "created_at", "status"]
        );
    }

    fn test_schema() -> DatabaseSchema {
        DatabaseSchema {
            tables: vec![
                TableInfo {
                    schema: "public".to_string(),
                    name: "customers".to_string(),
                    kind: TableKind::Table,
                    columns: vec![
                        ColumnInfo {
                            name: "id".to_string(),
                            data_type: "bigint".to_string(),
                            nullable: false,
                            ordinal: 1,
                            is_pk: true,
                            is_fk: false,
                        },
                        ColumnInfo {
                            name: "name".to_string(),
                            data_type: "text".to_string(),
                            nullable: false,
                            ordinal: 2,
                            is_pk: false,
                            is_fk: false,
                        },
                        ColumnInfo {
                            name: "city".to_string(),
                            data_type: "text".to_string(),
                            nullable: true,
                            ordinal: 3,
                            is_pk: false,
                            is_fk: false,
                        },
                    ],
                },
                TableInfo {
                    schema: "public".to_string(),
                    name: "orders".to_string(),
                    kind: TableKind::Table,
                    columns: vec![
                        ColumnInfo {
                            name: "created_at".to_string(),
                            data_type: "timestamp with time zone".to_string(),
                            nullable: false,
                            ordinal: 3,
                            is_pk: false,
                            is_fk: false,
                        },
                        ColumnInfo {
                            name: "customer_id".to_string(),
                            data_type: "uuid".to_string(),
                            nullable: false,
                            ordinal: 2,
                            is_pk: false,
                            is_fk: true,
                        },
                        ColumnInfo {
                            name: "id".to_string(),
                            data_type: "bigint".to_string(),
                            nullable: false,
                            ordinal: 1,
                            is_pk: true,
                            is_fk: false,
                        },
                        ColumnInfo {
                            name: "status".to_string(),
                            data_type: "text".to_string(),
                            nullable: true,
                            ordinal: 4,
                            is_pk: false,
                            is_fk: false,
                        },
                    ],
                },
            ],
            ..Default::default()
        }
    }
}
