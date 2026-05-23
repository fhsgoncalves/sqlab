use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use duckdb::Connection;
use duckdb::types::{TimeUnit, ValueRef};
use sqlab_drivers_core::{
    ColumnInfo, ColumnMetadata, DataSource, DataSourceConfig, DataSourceError, Database,
    DatabaseSchema, ForeignKeyInfo, FunctionInfo, IndexInfo, QueryResult, SchemaInfo, SequenceInfo,
    TableEditBatch, TableEditValue, TableInfo, TableKind, TriggerInfo,
};
use sqlparser::ast::Statement;
use sqlparser::dialect::DuckDbDialect;
use sqlparser::parser::Parser;

const DEFAULT_ROW_LIMIT: usize = 1000;

pub struct DuckDbDataSource {
    config: DataSourceConfig,
    conn: Option<Arc<Mutex<Connection>>>,
}

impl DuckDbDataSource {
    pub fn new(config: DataSourceConfig) -> Self {
        Self { config, conn: None }
    }

    fn database_path(&self) -> &str {
        if self.config.database.is_empty() {
            ":memory:"
        } else {
            &self.config.database
        }
    }

    fn connection(&self) -> Result<Arc<Mutex<Connection>>, DataSourceError> {
        self.conn.clone().ok_or(DataSourceError::NotConnected)
    }

    pub fn connect_blocking(&mut self) -> Result<(), DataSourceError> {
        let path = self.database_path();
        let conn = if path == ":memory:" {
            Connection::open_in_memory()
        } else {
            Connection::open(Path::new(path))
        }
        .map_err(|e| DataSourceError::ConnectionFailed(e.to_string()))?;
        self.conn = Some(Arc::new(Mutex::new(conn)));
        Ok(())
    }

    pub fn disconnect_blocking(&mut self) -> Result<(), DataSourceError> {
        self.conn = None;
        Ok(())
    }

    pub fn execute_query_blocking(
        &self,
        query: &str,
        apply_limit: bool,
    ) -> Result<QueryResult, DataSourceError> {
        let conn = self.connection()?;
        let conn = conn
            .lock()
            .map_err(|_| DataSourceError::QueryFailed("DuckDB connection lock poisoned".into()))?;
        let query = if apply_limit {
            apply_limit_if_missing(query, DEFAULT_ROW_LIMIT)
        } else {
            query.to_string()
        };
        let start = Instant::now();
        let mut statement = match conn.prepare(&query) {
            Ok(statement) => statement,
            Err(error) => {
                return Err(DataSourceError::QueryFailed(error.to_string()));
            }
        };
        let mut rows = match statement.query([]) {
            Ok(rows) => rows,
            Err(_) => {
                conn.execute_batch(&query)
                    .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;
                return Ok(QueryResult {
                    columns: Vec::new(),
                    column_metadata: Vec::new(),
                    rows: Vec::new(),
                    nulls: Vec::new(),
                    row_count: 0,
                    execution_time_ms: start.elapsed().as_millis(),
                });
            }
        };

        let statement_ref = rows.as_ref().ok_or_else(|| {
            DataSourceError::QueryFailed("DuckDB statement metadata unavailable".into())
        })?;
        let column_count = statement_ref.column_count();
        let columns = (0..column_count)
            .map(|ix| {
                statement_ref
                    .column_name(ix)
                    .map(|name| name.to_string())
                    .unwrap_or_else(|_| format!("column_{}", ix + 1))
            })
            .collect::<Vec<_>>();
        let column_metadata = (0..column_count)
            .map(|ix| ColumnMetadata {
                name: columns[ix].clone(),
                data_type: format!("{:?}", statement_ref.column_logical_type(ix)),
                is_pk: false,
                is_fk: false,
            })
            .collect::<Vec<_>>();

        let mut result_rows = Vec::new();
        let mut result_nulls = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?
        {
            let mut values = Vec::with_capacity(column_count);
            let mut nulls = Vec::with_capacity(column_count);
            for ix in 0..column_count {
                let value = row
                    .get_ref(ix)
                    .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;
                nulls.push(matches!(value, ValueRef::Null));
                values.push(duckdb_value_to_string(value));
            }
            result_rows.push(values);
            result_nulls.push(nulls);
        }

        Ok(QueryResult {
            columns,
            column_metadata,
            row_count: result_rows.len(),
            rows: result_rows,
            nulls: result_nulls,
            execution_time_ms: start.elapsed().as_millis(),
        })
    }

    pub fn introspect_schema_blocking(&self) -> Result<DatabaseSchema, DataSourceError> {
        let conn = self.connection()?;
        let conn = conn
            .lock()
            .map_err(|_| DataSourceError::QueryFailed("DuckDB connection lock poisoned".into()))?;
        let configured_schema = self.config.schema.trim();
        let schema_predicate = duckdb_schema_predicate("schema_name", configured_schema);
        let table_schema_predicate = duckdb_schema_predicate("table_schema", configured_schema);

        let schemas = query_map_collect(
            &conn,
            &format!(
                "SELECT DISTINCT schema_name
             FROM information_schema.schemata
             WHERE {schema_predicate}
             ORDER BY schema_name"
            ),
            |row| {
                Ok(SchemaInfo {
                    name: row.get(0)?,
                    owner: String::new(),
                })
            },
        )?;

        let table_rows = query_map_collect(
            &conn,
            &format!(
                "SELECT DISTINCT table_schema, table_name, table_type
             FROM information_schema.tables
             WHERE {table_schema_predicate}
             ORDER BY table_schema, table_name"
            ),
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )?;

        let column_rows = query_map_collect(
            &conn,
            &format!(
                "SELECT DISTINCT table_schema, table_name, column_name, data_type, is_nullable,
                    ordinal_position, column_default
             FROM information_schema.columns
             WHERE {table_schema_predicate}
             ORDER BY table_schema, table_name, ordinal_position"
            ),
            |row| {
                Ok(DuckDbColumnRow {
                    schema: row.get(0)?,
                    table: row.get(1)?,
                    name: row.get(2)?,
                    data_type: row.get(3)?,
                    is_nullable: row.get(4)?,
                    ordinal: row.get::<_, i32>(5)?,
                    default_value: row.get(6)?,
                })
            },
        )?;

        let pk_rows = query_map_collect(
            &conn,
            &format!(
                "SELECT kcu.table_schema, kcu.table_name, kcu.column_name
             FROM information_schema.table_constraints tc
             JOIN information_schema.key_column_usage kcu
               ON kcu.constraint_schema = tc.constraint_schema
              AND kcu.constraint_name = tc.constraint_name
              AND kcu.table_schema = tc.table_schema
              AND kcu.table_name = tc.table_name
             WHERE tc.constraint_type = 'PRIMARY KEY'
               AND {table_schema_predicate}"
            ),
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .unwrap_or_default();

        let foreign_keys = query_foreign_keys(&conn, &table_schema_predicate).unwrap_or_default();
        let mut tables = build_tables(table_rows, column_rows, &pk_rows, &foreign_keys);
        mark_fk_columns(&mut tables, &foreign_keys);

        Ok(DatabaseSchema {
            db_type: Database::DuckDB,
            schemas,
            tables,
            functions: Vec::<FunctionInfo>::new(),
            sequences: Vec::<SequenceInfo>::new(),
            indexes: Vec::<IndexInfo>::new(),
            triggers: Vec::<TriggerInfo>::new(),
            foreign_keys,
        })
    }

    pub fn apply_table_edits_blocking(&self, batch: TableEditBatch) -> Result<(), DataSourceError> {
        if batch.rows.is_empty() {
            return Ok(());
        }
        let conn = self.connection()?;
        let conn = conn
            .lock()
            .map_err(|_| DataSourceError::QueryFailed("DuckDB connection lock poisoned".into()))?;
        conn.execute_batch("BEGIN TRANSACTION")
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;
        for row in &batch.rows {
            let statement = duckdb_update_statement(&batch, row)?;
            match conn.execute(&statement, []) {
                Ok(1) => {}
                Ok(affected) => {
                    let _ = conn.execute_batch("ROLLBACK");
                    return Err(DataSourceError::QueryFailed(format!(
                        "Expected edit to update 1 row, updated {affected} rows instead."
                    )));
                }
                Err(error) => {
                    let _ = conn.execute_batch("ROLLBACK");
                    return Err(DataSourceError::QueryFailed(error.to_string()));
                }
            }
        }
        conn.execute_batch("COMMIT")
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;
        Ok(())
    }
}

#[async_trait]
impl DataSource for DuckDbDataSource {
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

    async fn introspect_schema(&self) -> Result<DatabaseSchema, DataSourceError> {
        self.introspect_schema_blocking()
    }

    async fn apply_table_edits(&self, batch: TableEditBatch) -> Result<(), DataSourceError> {
        self.apply_table_edits_blocking(batch)
    }
}

pub fn create_duckdb_data_source(
    config: &DataSourceConfig,
) -> Result<Box<dyn DataSource>, DataSourceError> {
    Ok(Box::new(DuckDbDataSource::new(config.clone())))
}

struct DuckDbColumnRow {
    schema: String,
    table: String,
    name: String,
    data_type: String,
    is_nullable: String,
    ordinal: i32,
    default_value: Option<String>,
}

type DuckDbPkRow = (String, String, String);

fn query_map_collect<T>(
    conn: &Connection,
    query: &str,
    mut f: impl FnMut(&duckdb::Row<'_>) -> duckdb::Result<T>,
) -> Result<Vec<T>, DataSourceError> {
    let mut stmt = conn
        .prepare(query)
        .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;
    let rows = stmt
        .query_map([], |row| f(row))
        .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;
    rows.collect::<duckdb::Result<Vec<_>>>()
        .map_err(|e| DataSourceError::QueryFailed(e.to_string()))
}

fn build_tables(
    table_rows: Vec<(String, String, String)>,
    column_rows: Vec<DuckDbColumnRow>,
    pk_rows: &[DuckDbPkRow],
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
            .find(|table| table.schema == row.schema && table.name == row.table)
        {
            let is_pk = pk_rows
                .iter()
                .any(|pk| pk.0 == row.schema && pk.1 == row.table && pk.2 == row.name);
            let is_fk = foreign_keys.iter().any(|fk| {
                fk.source_schema == row.schema
                    && fk.source_table == row.table
                    && fk.source_columns.iter().any(|column| column == &row.name)
            });
            table.columns.push(ColumnInfo {
                name: row.name,
                data_type: row.data_type,
                enum_values: Vec::new(),
                nullable: row.is_nullable.eq_ignore_ascii_case("YES"),
                ordinal: row.ordinal,
                is_pk,
                is_fk,
                default_value: row.default_value,
                is_generated: false,
                generation_expression: None,
            });
        }
    }

    tables
}

fn query_foreign_keys(
    conn: &Connection,
    table_schema_predicate: &str,
) -> Result<Vec<ForeignKeyInfo>, DataSourceError> {
    let rows = query_map_collect(
        conn,
        &format!(
            "SELECT
            tc.constraint_name,
            kcu.table_schema,
            kcu.table_name,
            kcu.column_name,
            ccu.table_schema,
            ccu.table_name,
            ccu.column_name,
            kcu.ordinal_position
         FROM information_schema.table_constraints tc
         JOIN information_schema.key_column_usage kcu
           ON kcu.constraint_schema = tc.constraint_schema
          AND kcu.constraint_name = tc.constraint_name
          AND kcu.table_schema = tc.table_schema
          AND kcu.table_name = tc.table_name
         JOIN information_schema.referential_constraints rc
           ON rc.constraint_schema = tc.constraint_schema
          AND rc.constraint_name = tc.constraint_name
         JOIN information_schema.constraint_column_usage ccu
           ON ccu.constraint_schema = rc.unique_constraint_schema
          AND ccu.constraint_name = rc.unique_constraint_name
         WHERE tc.constraint_type = 'FOREIGN KEY'
           AND {table_schema_predicate}
         ORDER BY kcu.table_schema, kcu.table_name, tc.constraint_name, kcu.ordinal_position"
        ),
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, i32>(7)?,
            ))
        },
    )?;

    let mut foreign_keys = Vec::<ForeignKeyInfo>::new();
    for row in rows {
        if let Some(existing) = foreign_keys
            .iter_mut()
            .find(|fk| fk.name == row.0 && fk.source_schema == row.1 && fk.source_table == row.2)
        {
            existing.source_columns.push(row.3);
            existing.target_columns.push(row.6);
        } else {
            foreign_keys.push(ForeignKeyInfo {
                name: row.0,
                source_schema: row.1,
                source_table: row.2,
                source_columns: vec![row.3],
                target_schema: row.4,
                target_table: row.5,
                target_columns: vec![row.6],
            });
        }
    }
    Ok(foreign_keys)
}

fn mark_fk_columns(tables: &mut [TableInfo], foreign_keys: &[ForeignKeyInfo]) {
    for table in tables {
        for column in &mut table.columns {
            column.is_fk = foreign_keys.iter().any(|fk| {
                fk.source_schema == table.schema
                    && fk.source_table == table.name
                    && fk.source_columns.iter().any(|name| name == &column.name)
            });
        }
    }
}

fn duckdb_value_to_string(value: ValueRef<'_>) -> String {
    match value {
        ValueRef::Null => String::new(),
        ValueRef::Boolean(value) => value.to_string(),
        ValueRef::TinyInt(value) => value.to_string(),
        ValueRef::SmallInt(value) => value.to_string(),
        ValueRef::Int(value) => value.to_string(),
        ValueRef::BigInt(value) => value.to_string(),
        ValueRef::HugeInt(value) => value.to_string(),
        ValueRef::UTinyInt(value) => value.to_string(),
        ValueRef::USmallInt(value) => value.to_string(),
        ValueRef::UInt(value) => value.to_string(),
        ValueRef::UBigInt(value) => value.to_string(),
        ValueRef::Float(value) => value.to_string(),
        ValueRef::Double(value) => value.to_string(),
        ValueRef::Decimal(value) => value.to_string(),
        ValueRef::Timestamp(unit, value) => format_timestamp(unit, value),
        ValueRef::Text(value) => String::from_utf8_lossy(value).to_string(),
        ValueRef::Blob(value) => hex_string(value),
        ValueRef::Date32(value) => NaiveDate::from_num_days_from_ce_opt(value + 719_163)
            .map(|date| date.to_string())
            .unwrap_or_else(|| value.to_string()),
        ValueRef::Time64(unit, value) => format_time(unit, value),
        ValueRef::Interval {
            months,
            days,
            nanos,
        } => format!("{months} months {days} days {nanos} ns"),
        other => format!("{other:?}"),
    }
}

fn format_timestamp(unit: TimeUnit, value: i64) -> String {
    DateTime::<Utc>::from_timestamp_micros(unit.to_micros(value))
        .map(|timestamp| timestamp.naive_utc().to_string())
        .unwrap_or_else(|| value.to_string())
}

fn format_time(unit: TimeUnit, value: i64) -> String {
    let micros_per_day = 86_400_i64 * 1_000_000;
    let micros = unit.to_micros(value).rem_euclid(micros_per_day);
    let seconds = (micros / 1_000_000) as u32;
    let nanos = ((micros % 1_000_000) * 1000) as u32;
    NaiveTime::from_num_seconds_from_midnight_opt(seconds, nanos)
        .map(|time| time.to_string())
        .unwrap_or_else(|| value.to_string())
}

fn hex_string(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2 + 2);
    output.push_str("0x");
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

fn apply_limit_if_missing(query: &str, limit: usize) -> String {
    let dialect = DuckDbDialect {};
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

fn duckdb_update_statement(
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
    let table = if batch.schema.is_empty() || batch.schema == "main" {
        quote_duckdb_identifier(&batch.table)
    } else {
        format!(
            "{}.{}",
            quote_duckdb_identifier(&batch.schema),
            quote_duckdb_identifier(&batch.table)
        )
    };
    let assignments = row
        .assignments
        .iter()
        .map(|value| {
            format!(
                "{} = {}",
                quote_duckdb_identifier(&value.column),
                duckdb_literal(value)
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
                quote_duckdb_identifier(&value.column),
                duckdb_literal(value)
            ),
            None => format!("{} IS NULL", quote_duckdb_identifier(&value.column)),
        })
        .collect::<Vec<_>>()
        .join(" AND ");
    Ok(format!("UPDATE {table} SET {assignments} WHERE {keys}"))
}

fn quote_duckdb_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn quote_duckdb_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn duckdb_schema_predicate(column: &str, configured_schema: &str) -> String {
    if configured_schema.is_empty() {
        format!("{column} NOT IN ('information_schema', 'pg_catalog')")
    } else {
        format!("{column} = {}", quote_duckdb_string(configured_schema))
    }
}

fn duckdb_literal(value: &TableEditValue) -> String {
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
    quote_duckdb_string(raw_value)
}

fn is_numeric_type(data_type: &str) -> bool {
    [
        "tinyint",
        "smallint",
        "integer",
        "int",
        "bigint",
        "hugeint",
        "utinyint",
        "usmallint",
        "uinteger",
        "ubigint",
        "decimal",
        "numeric",
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
    fn preserves_existing_limit() {
        assert_eq!(
            apply_limit_if_missing("SELECT * FROM users LIMIT 10", 1000),
            "SELECT * FROM users LIMIT 10"
        );
    }

    #[test]
    fn builds_update_statement() {
        let batch = TableEditBatch {
            schema: "main".into(),
            table: "users".into(),
            rows: vec![],
        };
        let row = sqlab_drivers_core::TableEditRow {
            keys: vec![TableEditValue {
                column: "id".into(),
                data_type: "integer".into(),
                enum_values: vec![],
                value: Some("1".into()),
            }],
            assignments: vec![TableEditValue {
                column: "name".into(),
                data_type: "varchar".into(),
                enum_values: vec![],
                value: Some("Ada".into()),
            }],
        };

        assert_eq!(
            duckdb_update_statement(&batch, &row).unwrap(),
            "UPDATE \"users\" SET \"name\" = 'Ada' WHERE \"id\" = 1"
        );
    }

    #[test]
    fn introspects_memory_database() {
        let mut source = DuckDbDataSource::new(DataSourceConfig {
            db_type: Database::DuckDB,
            database: ":memory:".into(),
            schema: "main".into(),
            ..DataSourceConfig::default()
        });
        source.connect_blocking().unwrap();
        source
            .execute_query_blocking(
                "CREATE TABLE users (id INTEGER PRIMARY KEY, name VARCHAR NOT NULL)",
                false,
            )
            .unwrap();

        let schema = source.introspect_schema_blocking().unwrap();
        let mut schema_names = schema
            .schemas
            .iter()
            .map(|schema| schema.name.as_str())
            .collect::<Vec<_>>();
        schema_names.sort_unstable();
        schema_names.dedup();
        assert_eq!(schema_names.len(), schema.schemas.len());

        let table = schema
            .tables
            .iter()
            .find(|table| table.schema == "main" && table.name == "users")
            .expect("users table");

        assert_eq!(table.columns.len(), 2);
        assert!(table.columns.iter().any(|column| column.name == "id"));
        assert!(
            table
                .columns
                .iter()
                .any(|column| column.name == "name" && !column.nullable)
        );
    }
}
