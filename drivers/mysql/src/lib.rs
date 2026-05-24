use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use mysql_async::prelude::Queryable;
use mysql_async::{Conn, OptsBuilder, Row, Value};
use sqlab_drivers_core::{
    ColumnInfo, ColumnMetadata, DataSource, DataSourceConfig, DataSourceError, Database,
    DatabaseSchema, ForeignKeyInfo, FunctionInfo, IndexInfo, QueryExecutionOptions, QueryResult,
    SchemaInfo, SequenceInfo, TableEditBatch, TableEditValue, TableInfo, TableKind, TriggerInfo,
};
use sqlparser::ast::Statement;
use sqlparser::dialect::MySqlDialect;
use sqlparser::parser::Parser;
use tokio::runtime::Runtime;
use tokio::sync::Mutex;

const DEFAULT_ROW_LIMIT: usize = 1000;

pub struct MySqlDataSource {
    config: DataSourceConfig,
    runtime: Arc<Runtime>,
    conn: Option<Arc<Mutex<Conn>>>,
}

impl MySqlDataSource {
    pub fn new(config: DataSourceConfig) -> Result<Self, DataSourceError> {
        let runtime =
            Arc::new(Runtime::new().map_err(|e| DataSourceError::ConnectionFailed(e.to_string()))?);
        Ok(Self {
            config,
            runtime,
            conn: None,
        })
    }

    fn schema_filter(&self) -> String {
        if self.config.schema.is_empty() {
            self.config.database.clone()
        } else {
            self.config.schema.clone()
        }
    }

    fn connection(&self) -> Result<Arc<Mutex<Conn>>, DataSourceError> {
        self.conn.clone().ok_or(DataSourceError::NotConnected)
    }

    fn opts(&self) -> OptsBuilder {
        let mut builder = OptsBuilder::default()
            .ip_or_hostname(self.config.host.clone())
            .tcp_port(self.config.port)
            .user(Some(self.config.user.clone()))
            .pass(if self.config.password.is_empty() {
                None
            } else {
                Some(self.config.password.clone())
            })
            .db_name(Some(self.config.database.clone()));

        for (key, value) in self
            .config
            .query_string
            .split('&')
            .filter_map(|pair| pair.split_once('='))
        {
            if key.eq_ignore_ascii_case("ssl_mode") && value.eq_ignore_ascii_case("disabled") {
                builder = builder.ssl_opts(None);
            }
        }

        builder
    }

    async fn connect_async(&mut self) -> Result<(), DataSourceError> {
        let conn = Conn::new(self.opts())
            .await
            .map_err(|e| DataSourceError::ConnectionFailed(e.to_string()))?;
        self.conn = Some(Arc::new(Mutex::new(conn)));
        Ok(())
    }

    async fn disconnect_async(&mut self) -> Result<(), DataSourceError> {
        if let Some(conn) = self.conn.take() {
            let conn = Arc::try_unwrap(conn)
                .map_err(|_| DataSourceError::ConnectionFailed("MySQL connection is busy".into()))?
                .into_inner();
            conn.disconnect()
                .await
                .map_err(|e| DataSourceError::ConnectionFailed(e.to_string()))?;
        }
        Ok(())
    }

    async fn execute_query_async(
        &self,
        query: &str,
        apply_limit: bool,
    ) -> Result<QueryResult, DataSourceError> {
        let conn = self.connection()?;
        let mut conn = conn.lock().await;
        let query = if apply_limit {
            apply_limit_if_missing(query, DEFAULT_ROW_LIMIT)
        } else {
            query.to_string()
        };
        let start = Instant::now();
        let mut result = conn
            .query_iter(query)
            .await
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;

        let column_metadata = result
            .columns_ref()
            .iter()
            .map(|column| ColumnMetadata {
                name: column.name_str().into_owned(),
                data_type: format!("{:?}", column.column_type()),
                is_pk: false,
                is_fk: false,
            })
            .collect::<Vec<_>>();
        let columns = column_metadata
            .iter()
            .map(|column| column.name.clone())
            .collect::<Vec<_>>();
        let rows = result
            .collect::<Row>()
            .await
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;

        let row_count = rows.len();
        let (rows, nulls) = rows
            .into_iter()
            .map(mysql_row_to_strings_and_nulls)
            .unzip::<_, _, Vec<_>, Vec<_>>();

        Ok(QueryResult {
            columns,
            column_metadata,
            rows,
            nulls,
            row_count,
            execution_time_ms: start.elapsed().as_millis(),
        })
    }

    async fn execute_query_async_with_options(
        &self,
        query: &str,
        apply_limit: bool,
        options: &QueryExecutionOptions,
    ) -> Result<QueryResult, DataSourceError> {
        if let Some(schema) = options
            .search_path
            .as_deref()
            .map(str::trim)
            .filter(|schema| !schema.is_empty())
        {
            let conn = self.connection()?;
            let mut conn = conn.lock().await;
            conn.query_drop(format!("USE {}", quote_mysql_identifier(schema)))
                .await
                .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;
        } else if !self.config.database.trim().is_empty() {
            let conn = self.connection()?;
            let mut conn = conn.lock().await;
            conn.query_drop(format!(
                "USE {}",
                quote_mysql_identifier(self.config.database.trim())
            ))
            .await
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;
        }

        self.execute_query_async(query, apply_limit).await
    }

    async fn introspect_schema_async(&self) -> Result<DatabaseSchema, DataSourceError> {
        let conn = self.connection()?;
        let mut conn = conn.lock().await;
        let configured_schema = self.schema_filter();
        let schema_predicate = mysql_schema_predicate("schema_name", &configured_schema);
        let table_schema_predicate = mysql_schema_predicate("table_schema", &configured_schema);
        let trigger_schema_predicate = mysql_schema_predicate("trigger_schema", &configured_schema);
        let routine_schema_predicate = mysql_schema_predicate("routine_schema", &configured_schema);

        let schemas = conn
            .query::<(String, Option<String>), _>(format!(
                "SELECT schema_name, default_character_set_name
                 FROM information_schema.schemata
                 WHERE {schema_predicate}
                 ORDER BY schema_name"
            ))
            .await
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?
            .into_iter()
            .map(|(name, owner)| SchemaInfo {
                name,
                owner: owner.unwrap_or_default(),
            })
            .collect::<Vec<_>>();

        let column_rows = conn
            .query::<MySqlColumnRow, _>(format!(
                "SELECT table_schema, table_name, column_name, column_type, is_nullable,
                        ordinal_position, column_default, extra, column_key
                 FROM information_schema.columns
                 WHERE {table_schema_predicate}
                 ORDER BY table_schema, table_name, ordinal_position"
            ))
            .await
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;

        let table_rows = conn
            .query::<(String, String, String), _>(format!(
                "SELECT table_schema, table_name, table_type
                 FROM information_schema.tables
                 WHERE {table_schema_predicate}
                 ORDER BY table_schema, table_name"
            ))
            .await
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;

        let fk_rows = conn
            .query::<MySqlForeignKeyRow, _>(format!(
                "SELECT constraint_name, table_schema, table_name, column_name,
                        referenced_table_schema, referenced_table_name, referenced_column_name,
                        ordinal_position
                 FROM information_schema.key_column_usage
                 WHERE referenced_table_name IS NOT NULL
                   AND {table_schema_predicate}
                 ORDER BY table_schema, table_name, constraint_name, ordinal_position"
            ))
            .await
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;

        let index_rows = conn
            .query::<MySqlIndexRow, _>(format!(
                "SELECT table_schema, table_name, index_name, non_unique,
                        seq_in_index, column_name
                 FROM information_schema.statistics
                 WHERE {table_schema_predicate}
                 ORDER BY table_schema, table_name, index_name, seq_in_index"
            ))
            .await
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;

        let triggers = conn
            .query::<MySqlTriggerRow, _>(format!(
                "SELECT trigger_schema, event_object_table, trigger_name,
                        event_manipulation, action_timing, action_statement
                 FROM information_schema.triggers
                 WHERE {trigger_schema_predicate}
                 ORDER BY trigger_schema, event_object_table, trigger_name"
            ))
            .await
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?
            .into_iter()
            .map(|row| TriggerInfo {
                schema: row.0,
                table_name: row.1,
                name: row.2,
                event: row.3,
                timing: row.4,
                definition: row.5,
            })
            .collect::<Vec<_>>();

        let functions = conn
            .query::<MySqlRoutineRow, _>(format!(
                "SELECT routine_schema, routine_name, routine_type, dtd_identifier,
                        routine_definition, external_language, definer
                 FROM information_schema.routines
                 WHERE {routine_schema_predicate}
                 ORDER BY routine_schema, routine_name"
            ))
            .await
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?
            .into_iter()
            .map(|row| FunctionInfo {
                schema: row.0,
                name: row.1,
                arguments: String::new(),
                return_type: row.3.unwrap_or(row.2),
                definition: row.4.clone(),
                language: row.5.unwrap_or_else(|| "sql".to_string()),
                body: row.4,
                library: None,
                owner: row.6.unwrap_or_default(),
            })
            .collect::<Vec<_>>();

        let foreign_keys = group_foreign_keys(fk_rows);
        let indexes = group_indexes(index_rows);
        let mut tables = build_tables(table_rows, column_rows, &foreign_keys);

        for index in &indexes {
            if index.is_primary {
                if let Some(table) = tables
                    .iter_mut()
                    .find(|table| table.schema == index.schema && table.name == index.table_name)
                {
                    for column in &mut table.columns {
                        if index.columns.iter().any(|name| name == &column.name) {
                            column.is_pk = true;
                        }
                    }
                }
            }
        }

        Ok(DatabaseSchema {
            db_type: Database::MySql,
            schemas,
            tables,
            functions,
            sequences: Vec::<SequenceInfo>::new(),
            indexes,
            triggers,
            foreign_keys,
        })
    }

    async fn apply_table_edits_async(&self, batch: TableEditBatch) -> Result<(), DataSourceError> {
        if batch.rows.is_empty() {
            return Ok(());
        }
        let conn = self.connection()?;
        let mut conn = conn.lock().await;
        conn.query_drop("START TRANSACTION")
            .await
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;
        for row in &batch.rows {
            let statement = mysql_update_statement(&batch, row)?;
            match conn.exec_iter(statement, ()).await {
                Ok(result) => {
                    let affected = result.affected_rows();
                    if affected != 1 {
                        let _ = conn.query_drop("ROLLBACK").await;
                        return Err(DataSourceError::QueryFailed(format!(
                            "Expected edit to update 1 row, updated {affected} rows instead."
                        )));
                    }
                }
                Err(error) => {
                    let _ = conn.query_drop("ROLLBACK").await;
                    return Err(DataSourceError::QueryFailed(error.to_string()));
                }
            }
        }
        conn.query_drop("COMMIT")
            .await
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;
        Ok(())
    }

    pub fn connect_blocking(&mut self) -> Result<(), DataSourceError> {
        let runtime = self.runtime.clone();
        runtime.block_on(self.connect_async())
    }

    pub fn disconnect_blocking(&mut self) -> Result<(), DataSourceError> {
        let runtime = self.runtime.clone();
        runtime.block_on(self.disconnect_async())
    }

    pub fn execute_query_blocking(
        &self,
        query: &str,
        apply_limit: bool,
    ) -> Result<QueryResult, DataSourceError> {
        self.runtime
            .block_on(self.execute_query_async(query, apply_limit))
    }

    pub fn execute_query_blocking_with_options(
        &self,
        query: &str,
        apply_limit: bool,
        options: &QueryExecutionOptions,
    ) -> Result<QueryResult, DataSourceError> {
        self.runtime
            .block_on(self.execute_query_async_with_options(query, apply_limit, options))
    }

    pub fn introspect_schema_blocking(&self) -> Result<DatabaseSchema, DataSourceError> {
        self.runtime.block_on(self.introspect_schema_async())
    }

    pub fn apply_table_edits_blocking(&self, batch: TableEditBatch) -> Result<(), DataSourceError> {
        self.runtime.block_on(self.apply_table_edits_async(batch))
    }
}

#[async_trait]
impl DataSource for MySqlDataSource {
    fn name(&self) -> &str {
        &self.config.name
    }

    fn db_type(&self) -> Database {
        self.config.db_type
    }

    fn config(&self) -> &DataSourceConfig {
        &self.config
    }

    fn is_connected(&self) -> bool {
        self.conn.is_some()
    }

    async fn connect(&mut self) -> Result<(), DataSourceError> {
        self.connect_blocking()
    }

    async fn disconnect(&mut self) -> Result<(), DataSourceError> {
        self.disconnect_blocking()
    }

    async fn execute_query(&self, query: &str) -> Result<QueryResult, DataSourceError> {
        self.execute_query_blocking(query, true)
    }

    async fn execute_query_with_options(
        &self,
        query: &str,
        options: &QueryExecutionOptions,
    ) -> Result<QueryResult, DataSourceError> {
        self.execute_query_blocking_with_options(query, true, options)
    }

    async fn introspect_schema(&self) -> Result<DatabaseSchema, DataSourceError> {
        self.introspect_schema_blocking()
    }

    async fn apply_table_edits(&self, batch: TableEditBatch) -> Result<(), DataSourceError> {
        self.apply_table_edits_blocking(batch)
    }
}

pub fn create_mysql_data_source(
    config: &DataSourceConfig,
) -> Result<Box<dyn DataSource>, DataSourceError> {
    Ok(Box::new(MySqlDataSource::new(config.clone())?))
}

type MySqlColumnRow = (
    String,
    String,
    String,
    String,
    String,
    u64,
    Option<String>,
    String,
    String,
);
type MySqlForeignKeyRow = (
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    u64,
);
type MySqlIndexRow = (String, String, String, u64, u64, Option<String>);
type MySqlTriggerRow = (String, String, String, String, String, String);
type MySqlRoutineRow = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn build_tables(
    table_rows: Vec<(String, String, String)>,
    column_rows: Vec<MySqlColumnRow>,
    foreign_keys: &[ForeignKeyInfo],
) -> Vec<TableInfo> {
    let mut tables = table_rows
        .into_iter()
        .map(|(schema, name, kind)| TableInfo {
            schema,
            name,
            kind: if kind.eq_ignore_ascii_case("VIEW") {
                TableKind::View
            } else {
                TableKind::Table
            },
            columns: Vec::new(),
        })
        .collect::<Vec<_>>();

    for row in column_rows {
        if let Some(table) = tables
            .iter_mut()
            .find(|table| table.schema == row.0 && table.name == row.1)
        {
            let is_generated = row.7.to_ascii_lowercase().contains("generated");
            let is_fk = foreign_keys.iter().any(|fk| {
                fk.source_schema == row.0
                    && fk.source_table == row.1
                    && fk.source_columns.iter().any(|column| column == &row.2)
            });
            table.columns.push(ColumnInfo {
                name: row.2,
                data_type: row.3,
                enum_values: Vec::new(),
                nullable: row.4.eq_ignore_ascii_case("YES"),
                ordinal: row.5 as i32,
                is_pk: row.8 == "PRI",
                is_fk,
                default_value: if is_generated { None } else { row.6.clone() },
                is_generated,
                generation_expression: if is_generated { row.6 } else { None },
            });
        }
    }

    tables
}

fn group_foreign_keys(rows: Vec<MySqlForeignKeyRow>) -> Vec<ForeignKeyInfo> {
    let mut foreign_keys = Vec::<ForeignKeyInfo>::new();
    for row in rows {
        let target_schema = row.4.unwrap_or_default();
        let target_table = row.5.unwrap_or_default();
        let target_column = row.6.unwrap_or_default();
        if let Some(existing) = foreign_keys
            .iter_mut()
            .find(|fk| fk.name == row.0 && fk.source_schema == row.1 && fk.source_table == row.2)
        {
            existing.source_columns.push(row.3);
            existing.target_columns.push(target_column);
        } else {
            foreign_keys.push(ForeignKeyInfo {
                name: row.0,
                source_schema: row.1,
                source_table: row.2,
                source_columns: vec![row.3],
                target_schema,
                target_table,
                target_columns: vec![target_column],
            });
        }
    }
    foreign_keys
}

fn group_indexes(rows: Vec<MySqlIndexRow>) -> Vec<IndexInfo> {
    let mut indexes = Vec::<IndexInfo>::new();
    for row in rows {
        let Some(column) = row.5 else {
            continue;
        };
        if let Some(existing) = indexes
            .iter_mut()
            .find(|index| index.schema == row.0 && index.table_name == row.1 && index.name == row.2)
        {
            existing.columns.push(column);
        } else {
            let is_primary = row.2 == "PRIMARY";
            let name = row.2;
            indexes.push(IndexInfo {
                schema: row.0,
                table_name: row.1,
                is_primary,
                name,
                is_unique: row.3 == 0,
                columns: vec![column],
            });
        }
    }
    indexes
}

fn mysql_row_to_strings_and_nulls(row: Row) -> (Vec<String>, Vec<bool>) {
    let values = row.unwrap();
    let nulls = values
        .iter()
        .map(|value| matches!(value, Value::NULL))
        .collect::<Vec<_>>();
    let values = values.into_iter().map(mysql_value_to_string).collect();
    (values, nulls)
}

fn mysql_value_to_string(value: Value) -> String {
    match value {
        Value::NULL => String::new(),
        Value::Bytes(bytes) => String::from_utf8(bytes).unwrap_or_else(|error| {
            let bytes = error.into_bytes();
            let mut output = String::with_capacity(bytes.len() * 2 + 2);
            output.push_str("0x");
            for byte in bytes {
                output.push_str(&format!("{byte:02x}"));
            }
            output
        }),
        Value::Int(value) => value.to_string(),
        Value::UInt(value) => value.to_string(),
        Value::Float(value) => value.to_string(),
        Value::Double(value) => value.to_string(),
        Value::Date(year, month, day, hour, minute, second, micros) => {
            if hour == 0 && minute == 0 && second == 0 && micros == 0 {
                format!("{year:04}-{month:02}-{day:02}")
            } else if micros == 0 {
                format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
            } else {
                format!(
                    "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}.{micros:06}"
                )
            }
        }
        Value::Time(is_negative, days, hours, minutes, seconds, micros) => {
            let sign = if is_negative { "-" } else { "" };
            let hours = (days * 24) + u32::from(hours);
            if micros == 0 {
                format!("{sign}{hours:02}:{minutes:02}:{seconds:02}")
            } else {
                format!("{sign}{hours:02}:{minutes:02}:{seconds:02}.{micros:06}")
            }
        }
    }
}

fn apply_limit_if_missing(query: &str, limit: usize) -> String {
    let dialect = MySqlDialect {};
    let Ok(statements) = Parser::parse_sql(&dialect, query) else {
        return query.to_string();
    };
    let Some(Statement::Query(query_ast)) = statements.last() else {
        return query.to_string();
    };
    if query_ast.limit_clause.is_some() {
        query.to_string()
    } else {
        append_limit(query, limit)
    }
}

fn append_limit(query: &str, limit: usize) -> String {
    let query_without_trailing_whitespace = query.trim_end();
    let trailing_whitespace = &query[query_without_trailing_whitespace.len()..];

    if let Some(query_without_semicolon) = query_without_trailing_whitespace.strip_suffix(';') {
        format!(
            "{} LIMIT {limit};{trailing_whitespace}",
            query_without_semicolon.trim_end()
        )
    } else {
        format!("{query} LIMIT {limit}")
    }
}

fn mysql_update_statement(
    batch: &TableEditBatch,
    row: &sqlab_drivers_core::TableEditRow,
) -> Result<String, DataSourceError> {
    if row.assignments.is_empty() {
        return Err(DataSourceError::QueryFailed(
            "Cannot submit an edit row without assignments.".into(),
        ));
    }
    if row.keys.is_empty() {
        return Err(DataSourceError::QueryFailed(
            "Cannot submit an edit row without primary key values.".into(),
        ));
    }
    let table = if batch.schema.is_empty() {
        quote_mysql_identifier(&batch.table)
    } else {
        format!(
            "{}.{}",
            quote_mysql_identifier(&batch.schema),
            quote_mysql_identifier(&batch.table)
        )
    };
    let assignments = row
        .assignments
        .iter()
        .map(|value| {
            format!(
                "{} = {}",
                quote_mysql_identifier(&value.column),
                mysql_literal(value)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let keys = row
        .keys
        .iter()
        .map(|value| match &value.value {
            Some(_) => format!(
                "{} = {}",
                quote_mysql_identifier(&value.column),
                mysql_literal(value)
            ),
            None => format!("{} IS NULL", quote_mysql_identifier(&value.column)),
        })
        .collect::<Vec<_>>()
        .join(" AND ");
    Ok(format!("UPDATE {table} SET {assignments} WHERE {keys}"))
}

fn quote_mysql_identifier(identifier: &str) -> String {
    format!("`{}`", identifier.replace('`', "``"))
}

fn quote_mysql_string(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''"))
}

fn mysql_schema_predicate(column: &str, configured_schema: &str) -> String {
    if configured_schema.is_empty() {
        format!("{column} NOT IN ('information_schema', 'mysql', 'performance_schema', 'sys')")
    } else {
        format!("{column} = {}", quote_mysql_string(configured_schema))
    }
}

fn mysql_literal(value: &TableEditValue) -> String {
    let Some(raw_value) = value.value.as_ref() else {
        return "NULL".to_string();
    };
    let data_type = value.data_type.to_ascii_lowercase();
    if is_numeric_type(&data_type) && raw_value.parse::<f64>().is_ok() {
        return raw_value.to_string();
    }
    if matches!(data_type.as_str(), "bool" | "boolean" | "tinyint(1)")
        && matches!(raw_value.to_ascii_lowercase().as_str(), "true" | "false")
    {
        return if raw_value.eq_ignore_ascii_case("true") {
            "1".to_string()
        } else {
            "0".to_string()
        };
    }
    quote_mysql_string(raw_value)
}

fn is_numeric_type(data_type: &str) -> bool {
    [
        "int",
        "bigint",
        "smallint",
        "tinyint",
        "mediumint",
        "numeric",
        "decimal",
        "real",
        "double",
        "float",
    ]
    .iter()
    .any(|prefix| data_type.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_limit_to_select() {
        assert_eq!(
            apply_limit_if_missing("SELECT * FROM users", 1000),
            "SELECT * FROM users LIMIT 1000"
        );
    }

    #[test]
    fn adds_limit_before_trailing_semicolon() {
        assert_eq!(
            apply_limit_if_missing("SELECT * FROM users;", 1000),
            "SELECT * FROM users LIMIT 1000;"
        );
    }

    #[test]
    fn groups_foreign_keys() {
        let rows = vec![
            (
                "fk_items_org".into(),
                "app".into(),
                "items".into(),
                "org_id".into(),
                Some("app".into()),
                Some("orgs".into()),
                Some("id".into()),
                1,
            ),
            (
                "fk_items_org".into(),
                "app".into(),
                "items".into(),
                "org_tenant".into(),
                Some("app".into()),
                Some("orgs".into()),
                Some("tenant".into()),
                2,
            ),
        ];
        let keys = group_foreign_keys(rows);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].source_columns, vec!["org_id", "org_tenant"]);
    }

    #[test]
    fn builds_schema_predicate_for_mysql_information_schema_column() {
        assert_eq!(
            mysql_schema_predicate("trigger_schema", "app"),
            "trigger_schema = 'app'"
        );
        assert_eq!(
            mysql_schema_predicate("routine_schema", ""),
            "routine_schema NOT IN ('information_schema', 'mysql', 'performance_schema', 'sys')"
        );
    }
}
