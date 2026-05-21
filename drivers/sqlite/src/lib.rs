use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use rusqlite::Connection;
use rusqlite::types::ValueRef;
use sqlab_drivers_core::{
    ColumnInfo, ColumnMetadata, DataSource, DataSourceConfig, DataSourceError, Database,
    DatabaseSchema, ForeignKeyInfo, FunctionInfo, IndexInfo, QueryResult, SchemaInfo, SequenceInfo,
    TableEditBatch, TableEditValue, TableInfo, TableKind, TriggerInfo,
};
use sqlparser::ast::Statement;
use sqlparser::dialect::SQLiteDialect;
use sqlparser::parser::Parser;

const DEFAULT_ROW_LIMIT: usize = 1000;

pub struct SQLiteDataSource {
    config: DataSourceConfig,
    conn: Option<Arc<Mutex<Connection>>>,
}

impl SQLiteDataSource {
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
        conn.pragma_update(None, "foreign_keys", "ON")
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
            .map_err(|_| DataSourceError::QueryFailed("SQLite connection lock poisoned".into()))?;
        let query = if apply_limit {
            apply_limit_if_missing(query, DEFAULT_ROW_LIMIT)
        } else {
            query.to_string()
        };
        let start = Instant::now();
        let mut statement = conn
            .prepare(&query)
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;

        let column_count = statement.column_count();
        if column_count == 0 {
            drop(statement);
            conn.execute_batch(&query)
                .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;
            return Ok(QueryResult {
                columns: Vec::new(),
                column_metadata: Vec::new(),
                rows: Vec::new(),
                nulls: Vec::new(),
                row_count: conn.changes() as usize,
                execution_time_ms: start.elapsed().as_millis(),
            });
        }

        let statement_columns = statement.columns();
        let columns = statement_columns
            .iter()
            .map(|column| column.name().to_string())
            .collect::<Vec<_>>();
        let column_metadata = (0..column_count)
            .map(|ix| ColumnMetadata {
                name: columns[ix].clone(),
                data_type: statement_columns[ix]
                    .decl_type()
                    .unwrap_or("unknown")
                    .to_string(),
                is_pk: false,
                is_fk: false,
            })
            .collect::<Vec<_>>();
        let mut rows = statement
            .query([])
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;
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
                values.push(sqlite_value_to_string(value));
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
            .map_err(|_| DataSourceError::QueryFailed("SQLite connection lock poisoned".into()))?;
        let schema_name = if self.config.schema.is_empty() {
            "main".to_string()
        } else {
            self.config.schema.clone()
        };

        let mut tables = Vec::new();
        let mut stmt = conn
            .prepare(
                "SELECT type, name FROM sqlite_schema
                 WHERE type IN ('table', 'view')
                   AND name NOT LIKE 'sqlite_%'
                 ORDER BY type, name",
            )
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;
        let objects = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;

        for (kind, name) in objects {
            let columns = sqlite_columns(&conn, &schema_name, &name)?;
            tables.push(TableInfo {
                schema: schema_name.clone(),
                name,
                kind: if kind == "view" {
                    TableKind::View
                } else {
                    TableKind::Table
                },
                columns,
            });
        }

        let foreign_keys = sqlite_foreign_keys(&conn, &schema_name, &tables)?;
        mark_fk_columns(&mut tables, &foreign_keys);
        let indexes = sqlite_indexes(&conn, &schema_name, &tables)?;
        let triggers = sqlite_triggers(&conn, &schema_name)?;

        Ok(DatabaseSchema {
            db_type: Database::SQLite,
            schemas: vec![SchemaInfo {
                name: schema_name,
                owner: String::new(),
            }],
            tables,
            functions: Vec::<FunctionInfo>::new(),
            sequences: Vec::<SequenceInfo>::new(),
            indexes,
            triggers,
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
            .map_err(|_| DataSourceError::QueryFailed("SQLite connection lock poisoned".into()))?;
        conn.execute_batch("BEGIN")
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;
        for row in &batch.rows {
            let statement = sqlite_update_statement(&batch, row)?;
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
impl DataSource for SQLiteDataSource {
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

pub fn create_sqlite_data_source(
    config: &DataSourceConfig,
) -> Result<Box<dyn DataSource>, DataSourceError> {
    Ok(Box::new(SQLiteDataSource::new(config.clone())))
}

fn sqlite_columns(
    conn: &Connection,
    schema: &str,
    table: &str,
) -> Result<Vec<ColumnInfo>, DataSourceError> {
    let query = format!(
        "PRAGMA {}.table_xinfo({})",
        quote_sqlite_identifier(schema),
        quote_sqlite_string(table)
    );
    let mut stmt = conn
        .prepare(&query)
        .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;
    stmt.query_map([], |row| {
        let hidden = row.get::<_, i32>(6)?;
        let default_value = row.get::<_, Option<String>>(4)?;
        Ok(ColumnInfo {
            ordinal: row.get::<_, i32>(0)? + 1,
            name: row.get(1)?,
            data_type: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            nullable: row.get::<_, i32>(3)? == 0,
            is_pk: row.get::<_, i32>(5)? > 0,
            is_fk: false,
            default_value: if hidden >= 2 {
                None
            } else {
                default_value.clone()
            },
            is_generated: hidden >= 2,
            generation_expression: if hidden >= 2 { default_value } else { None },
        })
    })
    .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?
    .collect::<Result<Vec<_>, _>>()
    .map_err(|e| DataSourceError::QueryFailed(e.to_string()))
}

fn sqlite_foreign_keys(
    conn: &Connection,
    schema: &str,
    tables: &[TableInfo],
) -> Result<Vec<ForeignKeyInfo>, DataSourceError> {
    let mut foreign_keys = Vec::new();
    for table in tables
        .iter()
        .filter(|table| matches!(table.kind, TableKind::Table))
    {
        let query = format!(
            "PRAGMA {}.foreign_key_list({})",
            quote_sqlite_identifier(schema),
            quote_sqlite_string(&table.name)
        );
        let mut stmt = conn
            .prepare(&query)
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i32>(0)?,
                    row.get::<_, i32>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;

        for (id, _seq, target_table, source_column, target_column) in rows {
            let name = format!("fk_{}_{}", table.name, id);
            if let Some(existing) = foreign_keys
                .iter_mut()
                .find(|fk: &&mut ForeignKeyInfo| fk.name == name && fk.source_table == table.name)
            {
                existing.source_columns.push(source_column);
                existing
                    .target_columns
                    .push(target_column.unwrap_or_else(|| "rowid".to_string()));
            } else {
                foreign_keys.push(ForeignKeyInfo {
                    name,
                    source_schema: schema.to_string(),
                    source_table: table.name.clone(),
                    source_columns: vec![source_column],
                    target_schema: schema.to_string(),
                    target_table,
                    target_columns: vec![target_column.unwrap_or_else(|| "rowid".to_string())],
                });
            }
        }
    }
    Ok(foreign_keys)
}

fn sqlite_indexes(
    conn: &Connection,
    schema: &str,
    tables: &[TableInfo],
) -> Result<Vec<IndexInfo>, DataSourceError> {
    let mut indexes = Vec::new();
    for table in tables
        .iter()
        .filter(|table| matches!(table.kind, TableKind::Table))
    {
        let query = format!(
            "PRAGMA {}.index_list({})",
            quote_sqlite_identifier(schema),
            quote_sqlite_string(&table.name)
        );
        let mut stmt = conn
            .prepare(&query)
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, i32>(2)? != 0,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;

        for (name, is_unique, origin) in rows {
            let columns = sqlite_index_columns(conn, schema, &name)?;
            indexes.push(IndexInfo {
                schema: schema.to_string(),
                table_name: table.name.clone(),
                name,
                is_unique,
                is_primary: origin == "pk",
                columns,
            });
        }
    }
    Ok(indexes)
}

fn sqlite_index_columns(
    conn: &Connection,
    schema: &str,
    index: &str,
) -> Result<Vec<String>, DataSourceError> {
    let query = format!(
        "PRAGMA {}.index_xinfo({})",
        quote_sqlite_identifier(schema),
        quote_sqlite_string(index)
    );
    let mut stmt = conn
        .prepare(&query)
        .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;
    stmt.query_map([], |row| {
        let key = row.get::<_, i32>(5)?;
        let name = row.get::<_, Option<String>>(2)?;
        Ok((key, name))
    })
    .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?
    .filter_map(|row| match row {
        Ok((1, Some(name))) => Some(Ok(name)),
        Ok(_) => None,
        Err(error) => Some(Err(error)),
    })
    .collect::<Result<Vec<_>, _>>()
    .map_err(|e| DataSourceError::QueryFailed(e.to_string()))
}

fn sqlite_triggers(conn: &Connection, schema: &str) -> Result<Vec<TriggerInfo>, DataSourceError> {
    let mut stmt = conn
        .prepare(
            "SELECT name, tbl_name, sql FROM sqlite_schema
             WHERE type = 'trigger'
             ORDER BY tbl_name, name",
        )
        .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;
    stmt.query_map([], |row| {
        let definition = row.get::<_, Option<String>>(2)?.unwrap_or_default();
        Ok(TriggerInfo {
            schema: schema.to_string(),
            table_name: row.get(1)?,
            name: row.get(0)?,
            event: sqlite_trigger_event(&definition),
            timing: sqlite_trigger_timing(&definition),
            definition,
        })
    })
    .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?
    .collect::<Result<Vec<_>, _>>()
    .map_err(|e| DataSourceError::QueryFailed(e.to_string()))
}

fn mark_fk_columns(tables: &mut [TableInfo], foreign_keys: &[ForeignKeyInfo]) {
    for fk in foreign_keys {
        if let Some(table) = tables
            .iter_mut()
            .find(|table| table.schema == fk.source_schema && table.name == fk.source_table)
        {
            for column in &mut table.columns {
                if fk.source_columns.iter().any(|name| name == &column.name) {
                    column.is_fk = true;
                }
            }
        }
    }
}

fn sqlite_value_to_string(value: ValueRef<'_>) -> String {
    match value {
        ValueRef::Null => String::new(),
        ValueRef::Integer(value) => value.to_string(),
        ValueRef::Real(value) => value.to_string(),
        ValueRef::Text(value) => String::from_utf8_lossy(value).into_owned(),
        ValueRef::Blob(value) => bytes_to_hex(value),
    }
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2 + 2);
    output.push_str("0x");
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

fn apply_limit_if_missing(query: &str, limit: usize) -> String {
    let dialect = SQLiteDialect {};
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

fn sqlite_update_statement(
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
        quote_sqlite_identifier(&batch.table)
    } else {
        format!(
            "{}.{}",
            quote_sqlite_identifier(&batch.schema),
            quote_sqlite_identifier(&batch.table)
        )
    };
    let assignments = row
        .assignments
        .iter()
        .map(|value| {
            format!(
                "{} = {}",
                quote_sqlite_identifier(&value.column),
                sqlite_literal(value)
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
                quote_sqlite_identifier(&value.column),
                sqlite_literal(value)
            ),
            None => format!("{} IS NULL", quote_sqlite_identifier(&value.column)),
        })
        .collect::<Vec<_>>()
        .join(" AND ");
    Ok(format!("UPDATE {table} SET {assignments} WHERE {keys}"))
}

fn quote_sqlite_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn quote_sqlite_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn sqlite_literal(value: &TableEditValue) -> String {
    let Some(raw_value) = value.value.as_ref() else {
        return "NULL".to_string();
    };
    let data_type = value.data_type.to_ascii_lowercase();
    if is_numeric_type(&data_type) && raw_value.parse::<f64>().is_ok() {
        return raw_value.to_string();
    }
    format!("'{}'", raw_value.replace('\'', "''"))
}

fn is_numeric_type(data_type: &str) -> bool {
    ["int", "real", "numeric", "decimal", "double", "float"]
        .iter()
        .any(|prefix| data_type.starts_with(prefix))
}

fn sqlite_trigger_event(definition: &str) -> String {
    let lower = definition.to_ascii_lowercase();
    ["insert", "update", "delete"]
        .iter()
        .find(|event| lower.contains(&format!(" {event} ")))
        .map(|event| event.to_ascii_uppercase())
        .unwrap_or_else(|| "UNKNOWN".to_string())
}

fn sqlite_trigger_timing(definition: &str) -> String {
    let lower = definition.to_ascii_lowercase();
    ["before", "after", "instead of"]
        .iter()
        .find(|timing| lower.contains(&format!(" {timing} ")))
        .map(|timing| timing.to_ascii_uppercase())
        .unwrap_or_else(|| "UNKNOWN".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_limit_to_simple_select() {
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
    fn introspects_temp_database() {
        let mut config = DataSourceConfig {
            name: "test".into(),
            db_type: Database::SQLite,
            database: ":memory:".into(),
            schema: "main".into(),
            ..DataSourceConfig::default()
        };
        let mut source = SQLiteDataSource::new(config.clone());
        source.connect_blocking().unwrap();
        source
            .execute_query_blocking(
                "CREATE TABLE users (id INTEGER PRIMARY KEY, org_id INTEGER REFERENCES orgs(id), name TEXT NOT NULL)",
                false,
            )
            .unwrap();
        source
            .execute_query_blocking("CREATE TABLE orgs (id INTEGER PRIMARY KEY)", false)
            .unwrap();
        config.database = ":memory:".into();
        let schema = source.introspect_schema_blocking().unwrap();
        assert_eq!(schema.db_type, Database::SQLite);
        assert!(schema.tables.iter().any(|table| table.name == "users"));
        assert_eq!(schema.foreign_keys.len(), 1);
    }
}
