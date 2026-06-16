use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use databend_driver::{Client, Connection, Row, Value};
use sqlab_drivers_core::{
    ColumnInfo, ColumnMetadata, DataSource, DataSourceConfig, DataSourceError, Database,
    DatabaseSchema, ForeignKeyInfo, FunctionInfo, IndexInfo, QueryExecutionOptions, QueryResult,
    SchemaInfo, SequenceInfo, TableEditBatch, TableEditValue, TableInfo, TableKind, TriggerInfo,
};
use sqlparser::ast::Statement;
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;
use tokio::runtime::Runtime;
use tokio::sync::Mutex;
use tokio_stream::StreamExt;

const DEFAULT_ROW_LIMIT: usize = 1000;

pub struct DatabendDataSource {
    config: DataSourceConfig,
    runtime: Arc<Runtime>,
    conn: Option<Arc<Mutex<Connection>>>,
}

impl DatabendDataSource {
    pub fn new(config: DataSourceConfig) -> Result<Self, DataSourceError> {
        let runtime =
            Arc::new(Runtime::new().map_err(|e| DataSourceError::ConnectionFailed(e.to_string()))?);
        Ok(Self {
            config,
            runtime,
            conn: None,
        })
    }

    fn dsn(&self) -> String {
        let scheme = databend_scheme(&self.config.query_string);
        let sslmode = databend_sslmode(&self.config.query_string);
        let query_string = databend_driver_query_string(&self.config.query_string, sslmode);
        let auth = if self.config.user.is_empty() {
            String::new()
        } else {
            format!(
                "{}:{}@",
                percent_encode(&self.config.user),
                percent_encode(&self.config.password)
            )
        };
        let database = if self.config.database.is_empty() {
            "default".to_string()
        } else {
            percent_encode(&self.config.database)
        };
        format!(
            "{scheme}://{auth}{}:{}/{}{}",
            self.config.host, self.config.port, database, query_string
        )
    }

    fn schema_filter(&self) -> String {
        if self.config.schema.is_empty() {
            self.config.database.clone()
        } else {
            self.config.schema.clone()
        }
    }

    fn connection(&self) -> Result<Arc<Mutex<Connection>>, DataSourceError> {
        self.conn.clone().ok_or(DataSourceError::NotConnected)
    }

    async fn connect_async(&mut self) -> Result<(), DataSourceError> {
        let client = Client::new(self.dsn()).with_name("sqlab".to_string());
        let conn = client
            .get_conn()
            .await
            .map_err(|e| DataSourceError::ConnectionFailed(e.to_string()))?;
        self.conn = Some(Arc::new(Mutex::new(conn)));
        Ok(())
    }

    async fn disconnect_async(&mut self) -> Result<(), DataSourceError> {
        if let Some(conn) = self.conn.take() {
            let conn = Arc::try_unwrap(conn)
                .map_err(|_| {
                    DataSourceError::ConnectionFailed("Databend connection is busy".into())
                })?
                .into_inner();
            conn.close()
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
        self.execute_query_async_with_options(query, apply_limit, &QueryExecutionOptions::default())
            .await
    }

    async fn execute_query_async_with_options(
        &self,
        query: &str,
        apply_limit: bool,
        options: &QueryExecutionOptions,
    ) -> Result<QueryResult, DataSourceError> {
        let conn = self.connection()?;
        let conn = conn.lock().await;
        if let Some(schema) = options
            .search_path
            .as_deref()
            .map(str::trim)
            .filter(|schema| !schema.is_empty())
        {
            conn.exec(&format!("USE {}", quote_databend_identifier(schema)))
                .await
                .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;
        } else if !self.config.database.trim().is_empty() {
            conn.exec(&format!(
                "USE {}",
                quote_databend_identifier(self.config.database.trim())
            ))
            .await
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;
        }

        let query = if apply_limit {
            apply_limit_if_missing(query, DEFAULT_ROW_LIMIT)
        } else {
            query.to_string()
        };
        let start = Instant::now();
        let mut iter = match conn.query_iter(&query).await {
            Ok(iter) => iter,
            Err(query_error) => {
                let affected = conn
                    .exec(&query)
                    .await
                    .map_err(|_| DataSourceError::QueryFailed(query_error.to_string()))?;
                return Ok(QueryResult {
                    columns: Vec::new(),
                    column_metadata: Vec::new(),
                    rows: Vec::new(),
                    nulls: Vec::new(),
                    row_count: affected.max(0) as usize,
                    execution_time_ms: start.elapsed().as_millis(),
                });
            }
        };

        let fields = iter.schema().fields().to_vec();
        let column_metadata = fields
            .iter()
            .map(|field| ColumnMetadata {
                name: field.name.clone(),
                data_type: field.data_type.to_string(),
                is_pk: false,
                is_fk: false,
            })
            .collect::<Vec<_>>();
        let columns = column_metadata
            .iter()
            .map(|column| column.name.clone())
            .collect::<Vec<_>>();

        let mut rows = Vec::new();
        let mut nulls = Vec::new();
        while let Some(row) = iter.next().await {
            let row = row.map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;
            let (values, row_nulls) = databend_row_to_strings_and_nulls(&row);
            rows.push(values);
            nulls.push(row_nulls);
        }

        Ok(QueryResult {
            columns,
            column_metadata,
            row_count: rows.len(),
            rows,
            nulls,
            execution_time_ms: start.elapsed().as_millis(),
        })
    }

    async fn introspect_schema_async(&self) -> Result<DatabaseSchema, DataSourceError> {
        let conn = self.connection()?;
        let conn = conn.lock().await;
        let configured_schema = self.schema_filter();
        let schema_predicate = databend_schema_predicate("schema_name", &configured_schema);
        let table_schema_predicate = databend_schema_predicate("table_schema", &configured_schema);

        let schemas = conn
            .query_all(&format!(
                "SELECT schema_name
                 FROM information_schema.schemata
                 WHERE {schema_predicate}
                 ORDER BY schema_name"
            ))
            .await
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?
            .into_iter()
            .map(|row| SchemaInfo {
                name: string_column(&row, 0),
                owner: String::new(),
            })
            .collect::<Vec<_>>();

        let table_rows = conn
            .query_all(&format!(
                "SELECT table_schema, table_name, table_type
                 FROM information_schema.tables
                 WHERE {table_schema_predicate}
                 ORDER BY table_schema, table_name"
            ))
            .await
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?
            .into_iter()
            .map(|row| {
                (
                    string_column(&row, 0),
                    string_column(&row, 1),
                    string_column(&row, 2),
                )
            })
            .collect::<Vec<_>>();

        let column_rows = conn
            .query_all(&format!(
                "SELECT table_schema, table_name, column_name, data_type, is_nullable,
                        ordinal_position, column_default
                 FROM information_schema.columns
                 WHERE {table_schema_predicate}
                 ORDER BY table_schema, table_name, ordinal_position"
            ))
            .await
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?
            .into_iter()
            .map(|row| DatabendColumnRow {
                schema: string_column(&row, 0),
                table: string_column(&row, 1),
                name: string_column(&row, 2),
                data_type: string_column(&row, 3),
                is_nullable: string_column(&row, 4),
                ordinal: string_column(&row, 5).parse::<i32>().unwrap_or(0),
                default_value: optional_string_column(&row, 6),
            })
            .collect::<Vec<_>>();

        Ok(DatabaseSchema {
            db_type: Database::Databend,
            schemas,
            tables: build_tables(table_rows, column_rows),
            functions: Vec::<FunctionInfo>::new(),
            sequences: Vec::<SequenceInfo>::new(),
            indexes: Vec::<IndexInfo>::new(),
            triggers: Vec::<TriggerInfo>::new(),
            foreign_keys: Vec::<ForeignKeyInfo>::new(),
        })
    }

    async fn apply_table_edits_async(&self, batch: TableEditBatch) -> Result<(), DataSourceError> {
        if batch.rows.is_empty() {
            return Ok(());
        }
        let conn = self.connection()?;
        let conn = conn.lock().await;
        for row in &batch.rows {
            let statement = databend_update_statement(&batch, row)?;
            let affected = conn
                .exec(&statement)
                .await
                .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;
            if affected != 1 {
                return Err(DataSourceError::QueryFailed(format!(
                    "Expected edit to update 1 row, updated {affected} rows instead."
                )));
            }
        }
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
impl DataSource for DatabendDataSource {
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

pub fn create_databend_data_source(
    config: &DataSourceConfig,
) -> Result<Box<dyn DataSource>, DataSourceError> {
    Ok(Box::new(DatabendDataSource::new(config.clone())?))
}

struct DatabendColumnRow {
    schema: String,
    table: String,
    name: String,
    data_type: String,
    is_nullable: String,
    ordinal: i32,
    default_value: Option<String>,
}

fn build_tables(
    table_rows: Vec<(String, String, String)>,
    column_rows: Vec<DatabendColumnRow>,
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
            comment: None,
        })
        .collect::<Vec<_>>();

    for row in column_rows {
        if let Some(table) = tables
            .iter_mut()
            .find(|table| table.schema == row.schema && table.name == row.table)
        {
            table.columns.push(ColumnInfo {
                name: row.name,
                data_type: row.data_type,
                enum_values: Vec::new(),
                nullable: row.is_nullable.eq_ignore_ascii_case("YES"),
                ordinal: row.ordinal,
                is_pk: false,
                is_fk: false,
                default_value: row.default_value,
                is_generated: false,
                generation_expression: None,
                comment: None,
            });
        }
    }

    tables
}

fn databend_row_to_strings_and_nulls(row: &Row) -> (Vec<String>, Vec<bool>) {
    let nulls = row
        .values()
        .iter()
        .map(|value| matches!(value, Value::Null))
        .collect::<Vec<_>>();
    let values = row
        .values()
        .iter()
        .map(databend_value_to_string)
        .collect::<Vec<_>>();
    (values, nulls)
}

fn databend_value_to_string(value: &Value) -> String {
    if matches!(value, Value::Null) {
        String::new()
    } else {
        value.to_string()
    }
}

fn string_column(row: &Row, ix: usize) -> String {
    row.values()
        .get(ix)
        .map(databend_value_to_string)
        .unwrap_or_default()
}

fn optional_string_column(row: &Row, ix: usize) -> Option<String> {
    let value = row.values().get(ix)?;
    (!matches!(value, Value::Null)).then(|| databend_value_to_string(value))
}

fn apply_limit_if_missing(query: &str, limit: usize) -> String {
    let dialect = GenericDialect {};
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

fn databend_update_statement(
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
        quote_databend_identifier(&batch.table)
    } else {
        format!(
            "{}.{}",
            quote_databend_identifier(&batch.schema),
            quote_databend_identifier(&batch.table)
        )
    };
    let assignments = row
        .assignments
        .iter()
        .map(|value| {
            format!(
                "{} = {}",
                quote_databend_identifier(&value.column),
                databend_literal(value)
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
                quote_databend_identifier(&value.column),
                databend_literal(value)
            ),
            None => format!("{} IS NULL", quote_databend_identifier(&value.column)),
        })
        .collect::<Vec<_>>()
        .join(" AND ");
    Ok(format!("UPDATE {table} SET {assignments} WHERE {keys}"))
}

fn quote_databend_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn quote_databend_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn databend_schema_predicate(column: &str, configured_schema: &str) -> String {
    if configured_schema.is_empty() {
        format!("{column} NOT IN ('information_schema', 'system')")
    } else {
        format!("{column} = {}", quote_databend_string(configured_schema))
    }
}

fn databend_literal(value: &TableEditValue) -> String {
    let Some(raw_value) = value.value.as_ref() else {
        return "NULL".to_string();
    };
    let data_type = value.data_type.to_ascii_lowercase();
    if is_numeric_type(&data_type) && raw_value.parse::<f64>().is_ok() {
        return raw_value.to_string();
    }
    if matches!(data_type.as_str(), "bool" | "boolean")
        && matches!(raw_value.to_ascii_lowercase().as_str(), "true" | "false")
    {
        return raw_value.to_ascii_lowercase();
    }
    quote_databend_string(raw_value)
}

fn is_numeric_type(data_type: &str) -> bool {
    [
        "int", "uint", "float", "double", "decimal", "number", "tinyint", "smallint", "bigint",
    ]
    .iter()
    .any(|prefix| data_type.starts_with(prefix))
}

fn databend_scheme(query_string: &str) -> &'static str {
    for (key, value) in query_pairs(query_string) {
        if key.eq_ignore_ascii_case("protocol") {
            return match value.to_ascii_lowercase().as_str() {
                "https" => "databend+https",
                _ => "databend",
            };
        }
    }
    "databend"
}

fn databend_sslmode(query_string: &str) -> &'static str {
    for (key, value) in query_pairs(query_string) {
        if key.eq_ignore_ascii_case("sslmode") {
            return if value.eq_ignore_ascii_case("enable") {
                "enable"
            } else {
                "disable"
            };
        }
    }
    "disable"
}

fn databend_driver_query_string(query_string: &str, sslmode: &str) -> String {
    let mut parts = vec![format!("sslmode={sslmode}")];
    for (key, value) in query_pairs(query_string) {
        if key.eq_ignore_ascii_case("protocol") || key.eq_ignore_ascii_case("sslmode") {
            continue;
        }
        parts.push(format!("{}={}", percent_encode(key), percent_encode(value)));
    }
    format!("?{}", parts.join("&"))
}

fn query_pairs(query_string: &str) -> impl Iterator<Item = (&str, &str)> {
    query_string
        .trim()
        .trim_start_matches('?')
        .split('&')
        .flat_map(|part| part.split_whitespace())
        .filter(|part| !part.is_empty())
        .map(|part| part.split_once('=').unwrap_or((part, "")))
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        let ch = byte as char;
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '.' | '_' | '~') {
            encoded.push(ch);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_databend_dsn() {
        let source = DatabendDataSource::new(DataSourceConfig {
            db_type: Database::Databend,
            host: "localhost".into(),
            port: 8000,
            user: "root".into(),
            password: String::new(),
            database: "default".into(),
            query_string: "sslmode=disable".into(),
            ..DataSourceConfig::default()
        })
        .unwrap();

        assert_eq!(
            source.dsn(),
            "databend://root:@localhost:8000/default?sslmode=disable"
        );
    }

    #[test]
    fn builds_databend_dsn_for_email_user_with_empty_password() {
        let source = DatabendDataSource::new(DataSourceConfig {
            db_type: Database::Databend,
            host: "localhost".into(),
            port: 8000,
            user: "fernando.goncalves@email.com".into(),
            password: String::new(),
            database: "default".into(),
            query_string: String::new(),
            ..DataSourceConfig::default()
        })
        .unwrap();

        assert_eq!(
            source.dsn(),
            "databend://fernando.goncalves%40email.com:@localhost:8000/default?sslmode=disable"
        );
    }

    #[test]
    fn adds_limit_to_select() {
        assert_eq!(
            apply_limit_if_missing("SELECT * FROM users", 1000),
            "SELECT * FROM users LIMIT 1000"
        );
    }

    #[test]
    fn builds_update_statement() {
        let batch = TableEditBatch {
            schema: "default".into(),
            table: "users".into(),
            rows: vec![],
        };
        let row = sqlab_drivers_core::TableEditRow {
            keys: vec![TableEditValue {
                column: "id".into(),
                data_type: "int".into(),
                enum_values: vec![],
                value: Some("1".into()),
            }],
            assignments: vec![TableEditValue {
                column: "active".into(),
                data_type: "boolean".into(),
                enum_values: vec![],
                value: Some("true".into()),
            }],
        };

        assert_eq!(
            databend_update_statement(&batch, &row).unwrap(),
            "UPDATE \"default\".\"users\" SET \"active\" = true WHERE \"id\" = 1"
        );
    }
}
