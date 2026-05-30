pub mod db;
pub mod models;

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use rusqlite::params;

use crate::schema_cache::db::with_conn;
use crate::schema_cache::models::{
    ColumnRow, ForeignKeyRow, FunctionRow, IndexRow, SchemaRow, SequenceRow, TableRow, TriggerRow,
    rows_to_schema, schema_to_rows,
};
use sqlab_drivers_core::{DataSourceConfig, Database, DatabaseSchema};

#[derive(Debug, thiserror::Error)]
pub enum SchemaCacheError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

pub fn cache_key(config: &DataSourceConfig) -> String {
    let mut hasher = DefaultHasher::new();
    config.db_type.hash(&mut hasher);
    config.host.hash(&mut hasher);
    config.port.hash(&mut hasher);
    config.database.hash(&mut hasher);
    config.user.hash(&mut hasher);
    config.schema.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

pub fn save(
    connection_key: &str,
    connection_name: &str,
    schema: &DatabaseSchema,
) -> Result<(), SchemaCacheError> {
    with_conn(|conn| {
        db::clear_connection(conn, connection_key)?;

        let (schemas, tables, columns, functions, sequences, indexes, triggers, foreign_keys) =
            schema_to_rows(connection_key, schema);

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        conn.execute(
            "INSERT INTO cache_metadata (connection_key, connection_name, db_type, refreshed_at) VALUES (?1, ?2, ?3, ?4)",
            params![connection_key, connection_name, schema.db_type.as_str(), now],
        )?;

        for s in &schemas {
            conn.execute(
                "INSERT INTO schemas (connection_key, name, owner) VALUES (?1, ?2, ?3)",
                params![s.connection_key, s.name, s.owner],
            )?;
        }

        for t in &tables {
            conn.execute(
                "INSERT INTO tables (connection_key, schema_name, name, kind, comment) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![t.connection_key, t.schema_name, t.name, t.kind, t.comment],
            )?;
        }

        for c in &columns {
            conn.execute(
                "INSERT INTO columns (connection_key, schema_name, table_name, name, data_type, enum_values, nullable, ordinal, is_pk, is_fk, default_value, is_generated, generation_expression, comment) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![c.connection_key, c.schema_name, c.table_name, c.name, c.data_type, c.enum_values, c.nullable, c.ordinal, c.is_pk, c.is_fk, c.default_value, c.is_generated, c.generation_expression, c.comment],
            )?;
        }

        for f in &functions {
            conn.execute(
                "INSERT INTO functions (connection_key, schema_name, name, arguments, return_type, definition, language, body, library, owner) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![f.connection_key, f.schema_name, f.name, f.arguments, f.return_type, f.definition, f.language, f.body, f.library, f.owner],
            )?;
        }

        for s in &sequences {
            conn.execute(
                "INSERT INTO sequences (connection_key, schema_name, name, data_type, start_value, min_value, max_value, increment_by) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![s.connection_key, s.schema_name, s.name, s.data_type, s.start_value, s.min_value, s.max_value, s.increment_by],
            )?;
        }

        for i in &indexes {
            conn.execute(
                "INSERT INTO indexes (connection_key, schema_name, table_name, name, is_unique, is_primary, columns) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![i.connection_key, i.schema_name, i.table_name, i.name, i.is_unique, i.is_primary, i.columns],
            )?;
        }

        for t in &triggers {
            conn.execute(
                "INSERT INTO triggers (connection_key, schema_name, table_name, name, event, timing, definition) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![t.connection_key, t.schema_name, t.table_name, t.name, t.event, t.timing, t.definition],
            )?;
        }

        for fk in &foreign_keys {
            conn.execute(
                "INSERT INTO foreign_keys (connection_key, name, source_schema, source_table, source_columns, target_schema, target_table, target_columns) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![fk.connection_key, fk.name, fk.source_schema, fk.source_table, fk.source_columns, fk.target_schema, fk.target_table, fk.target_columns],
            )?;
        }

        Ok(())
    })
}

pub fn load(connection_key: &str) -> Result<Option<DatabaseSchema>, SchemaCacheError> {
    with_conn(|conn| {
        let db_type_str: String = match conn.query_row(
            "SELECT db_type FROM cache_metadata WHERE connection_key = ?1",
            params![connection_key],
            |row| row.get(0),
        ) {
            Ok(s) => s,
            Err(_) => return Ok(None),
        };

        let db_type = match Database::try_from(db_type_str.as_str()) {
            Ok(db) => db,
            Err(e) => {
                eprintln!("Warning: invalid db_type in cache: {}", e);
                return Ok(None);
            }
        };

        let schemas = load_schemas(conn, connection_key)?;
        if schemas.is_empty() {
            return Ok(None);
        }

        let tables = load_tables(conn, connection_key)?;
        let columns = load_columns(conn, connection_key)?;
        let functions = load_functions(conn, connection_key)?;
        let sequences = load_sequences(conn, connection_key)?;
        let indexes = load_indexes(conn, connection_key)?;
        let triggers = load_triggers(conn, connection_key)?;
        let foreign_keys = load_foreign_keys(conn, connection_key)?;

        Ok(Some(rows_to_schema(
            db_type,
            schemas,
            tables,
            columns,
            functions,
            sequences,
            indexes,
            triggers,
            foreign_keys,
        )))
    })
}

pub fn clear(connection_key: &str) -> Result<(), SchemaCacheError> {
    with_conn(|conn| {
        db::clear_connection(conn, connection_key)?;
        Ok(())
    })
}

fn load_schemas(conn: &rusqlite::Connection, key: &str) -> Result<Vec<SchemaRow>, rusqlite::Error> {
    let mut stmt =
        conn.prepare("SELECT connection_key, name, owner FROM schemas WHERE connection_key = ?1")?;
    let rows = stmt.query_map(params![key], |row| {
        Ok(SchemaRow {
            connection_key: row.get(0)?,
            name: row.get(1)?,
            owner: row.get(2)?,
        })
    })?;
    rows.collect()
}

fn load_tables(conn: &rusqlite::Connection, key: &str) -> Result<Vec<TableRow>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT connection_key, schema_name, name, kind, comment FROM tables WHERE connection_key = ?1",
    )?;
    let rows = stmt.query_map(params![key], |row| {
        Ok(TableRow {
            connection_key: row.get(0)?,
            schema_name: row.get(1)?,
            name: row.get(2)?,
            kind: row.get(3)?,
            comment: row.get(4)?,
        })
    })?;
    rows.collect()
}

fn load_columns(conn: &rusqlite::Connection, key: &str) -> Result<Vec<ColumnRow>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT connection_key, schema_name, table_name, name, data_type, enum_values, nullable, ordinal, is_pk, is_fk, default_value, is_generated, generation_expression, comment FROM columns WHERE connection_key = ?1",
    )?;
    let rows = stmt.query_map(params![key], |row| {
        Ok(ColumnRow {
            connection_key: row.get(0)?,
            schema_name: row.get(1)?,
            table_name: row.get(2)?,
            name: row.get(3)?,
            data_type: row.get(4)?,
            enum_values: row.get(5)?,
            nullable: row.get(6)?,
            ordinal: row.get(7)?,
            is_pk: row.get(8)?,
            is_fk: row.get(9)?,
            default_value: row.get(10)?,
            is_generated: row.get(11)?,
            generation_expression: row.get(12)?,
            comment: row.get(13)?,
        })
    })?;
    rows.collect()
}

fn load_functions(
    conn: &rusqlite::Connection,
    key: &str,
) -> Result<Vec<FunctionRow>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT connection_key, schema_name, name, arguments, return_type, definition, language, body, library, owner FROM functions WHERE connection_key = ?1",
    )?;
    let rows = stmt.query_map(params![key], |row| {
        Ok(FunctionRow {
            connection_key: row.get(0)?,
            schema_name: row.get(1)?,
            name: row.get(2)?,
            arguments: row.get(3)?,
            return_type: row.get(4)?,
            definition: row.get(5)?,
            language: row.get(6)?,
            body: row.get(7)?,
            library: row.get(8)?,
            owner: row.get(9)?,
        })
    })?;
    rows.collect()
}

fn load_sequences(
    conn: &rusqlite::Connection,
    key: &str,
) -> Result<Vec<SequenceRow>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT connection_key, schema_name, name, data_type, start_value, min_value, max_value, increment_by FROM sequences WHERE connection_key = ?1",
    )?;
    let rows = stmt.query_map(params![key], |row| {
        Ok(SequenceRow {
            connection_key: row.get(0)?,
            schema_name: row.get(1)?,
            name: row.get(2)?,
            data_type: row.get(3)?,
            start_value: row.get(4)?,
            min_value: row.get(5)?,
            max_value: row.get(6)?,
            increment_by: row.get(7)?,
        })
    })?;
    rows.collect()
}

fn load_indexes(conn: &rusqlite::Connection, key: &str) -> Result<Vec<IndexRow>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT connection_key, schema_name, table_name, name, is_unique, is_primary, columns FROM indexes WHERE connection_key = ?1",
    )?;
    let rows = stmt.query_map(params![key], |row| {
        Ok(IndexRow {
            connection_key: row.get(0)?,
            schema_name: row.get(1)?,
            table_name: row.get(2)?,
            name: row.get(3)?,
            is_unique: row.get(4)?,
            is_primary: row.get(5)?,
            columns: row.get(6)?,
        })
    })?;
    rows.collect()
}

fn load_triggers(
    conn: &rusqlite::Connection,
    key: &str,
) -> Result<Vec<TriggerRow>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT connection_key, schema_name, table_name, name, event, timing, definition FROM triggers WHERE connection_key = ?1",
    )?;
    let rows = stmt.query_map(params![key], |row| {
        Ok(TriggerRow {
            connection_key: row.get(0)?,
            schema_name: row.get(1)?,
            table_name: row.get(2)?,
            name: row.get(3)?,
            event: row.get(4)?,
            timing: row.get(5)?,
            definition: row.get(6)?,
        })
    })?;
    rows.collect()
}

fn load_foreign_keys(
    conn: &rusqlite::Connection,
    key: &str,
) -> Result<Vec<ForeignKeyRow>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT connection_key, name, source_schema, source_table, source_columns, target_schema, target_table, target_columns FROM foreign_keys WHERE connection_key = ?1",
    )?;
    let rows = stmt.query_map(params![key], |row| {
        Ok(ForeignKeyRow {
            connection_key: row.get(0)?,
            name: row.get(1)?,
            source_schema: row.get(2)?,
            source_table: row.get(3)?,
            source_columns: row.get(4)?,
            target_schema: row.get(5)?,
            target_table: row.get(6)?,
            target_columns: row.get(7)?,
        })
    })?;
    rows.collect()
}
