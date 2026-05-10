use std::path::PathBuf;

use rusqlite::{Connection, params};

use crate::schema_cache::SchemaCacheError;

pub fn cache_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".zql")
        .join("cache")
        .join("schemas.db")
}

pub fn with_conn<T>(
    f: impl FnOnce(&Connection) -> Result<T, rusqlite::Error>,
) -> Result<T, SchemaCacheError> {
    let path = cache_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(&path)?;
    init_db(&conn)?;
    let result = f(&conn)?;
    Ok(result)
}

fn init_db(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS cache_metadata (
            connection_key TEXT PRIMARY KEY,
            connection_name TEXT NOT NULL,
            refreshed_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS schemas (
            connection_key TEXT NOT NULL,
            name TEXT NOT NULL,
            owner TEXT,
            PRIMARY KEY (connection_key, name)
        );

        CREATE TABLE IF NOT EXISTS tables (
            connection_key TEXT NOT NULL,
            schema_name TEXT NOT NULL,
            name TEXT NOT NULL,
            kind TEXT NOT NULL,
            PRIMARY KEY (connection_key, schema_name, name)
        );

        CREATE TABLE IF NOT EXISTS columns (
            connection_key TEXT NOT NULL,
            schema_name TEXT NOT NULL,
            table_name TEXT NOT NULL,
            name TEXT NOT NULL,
            data_type TEXT NOT NULL,
            nullable INTEGER NOT NULL,
            ordinal INTEGER NOT NULL,
            is_pk INTEGER NOT NULL DEFAULT 0,
            is_fk INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (connection_key, schema_name, table_name, name)
        );

        CREATE TABLE IF NOT EXISTS functions (
            connection_key TEXT NOT NULL,
            schema_name TEXT NOT NULL,
            name TEXT NOT NULL,
            arguments TEXT,
            return_type TEXT,
            PRIMARY KEY (connection_key, schema_name, name, arguments)
        );

        CREATE TABLE IF NOT EXISTS sequences (
            connection_key TEXT NOT NULL,
            schema_name TEXT NOT NULL,
            name TEXT NOT NULL,
            data_type TEXT,
            start_value TEXT,
            min_value TEXT,
            max_value TEXT,
            increment_by TEXT,
            PRIMARY KEY (connection_key, schema_name, name)
        );

        CREATE TABLE IF NOT EXISTS indexes (
            connection_key TEXT NOT NULL,
            schema_name TEXT NOT NULL,
            table_name TEXT NOT NULL,
            name TEXT NOT NULL,
            is_unique INTEGER NOT NULL,
            is_primary INTEGER NOT NULL,
            columns TEXT NOT NULL,
            PRIMARY KEY (connection_key, schema_name, name)
        );

        CREATE TABLE IF NOT EXISTS triggers (
            connection_key TEXT NOT NULL,
            schema_name TEXT NOT NULL,
            table_name TEXT NOT NULL,
            name TEXT NOT NULL,
            event TEXT NOT NULL,
            timing TEXT NOT NULL,
            definition TEXT NOT NULL,
            PRIMARY KEY (connection_key, schema_name, name)
        );

        CREATE TABLE IF NOT EXISTS foreign_keys (
            connection_key TEXT NOT NULL,
            name TEXT NOT NULL,
            source_schema TEXT NOT NULL,
            source_table TEXT NOT NULL,
            source_columns TEXT NOT NULL,
            target_schema TEXT NOT NULL,
            target_table TEXT NOT NULL,
            target_columns TEXT NOT NULL,
            PRIMARY KEY (connection_key, source_schema, source_table, name)
        );
        ",
    )?;

    conn.execute(
        "ALTER TABLE columns ADD COLUMN is_pk INTEGER NOT NULL DEFAULT 0",
        [],
    )
    .ok();
    conn.execute(
        "ALTER TABLE columns ADD COLUMN is_fk INTEGER NOT NULL DEFAULT 0",
        [],
    )
    .ok();

    Ok(())
}

pub fn clear_connection(conn: &Connection, key: &str) -> Result<(), rusqlite::Error> {
    let tables = [
        "cache_metadata",
        "schemas",
        "tables",
        "columns",
        "functions",
        "sequences",
        "indexes",
        "triggers",
        "foreign_keys",
    ];
    for table in &tables {
        conn.execute(
            &format!("DELETE FROM {} WHERE connection_key = ?1", table),
            params![key],
        )?;
    }
    Ok(())
}
