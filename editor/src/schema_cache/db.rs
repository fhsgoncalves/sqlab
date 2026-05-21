use std::path::PathBuf;

use rusqlite::{Connection, params};

use crate::schema_cache::SchemaCacheError;

pub fn cache_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".sqlab")
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
    // Migration: recreate indexes table with table_name in primary key
    let needs_migration: bool = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='indexes'",
            [],
            |row| {
                let sql: String = row.get(0)?;
                Ok(!sql.contains("table_name, name)"))
            },
        )
        .unwrap_or(false);
    if needs_migration {
        conn.execute_batch("DROP TABLE IF EXISTS indexes;")?;
    }

    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS cache_metadata (
            connection_key TEXT PRIMARY KEY,
            connection_name TEXT NOT NULL,
            db_type TEXT NOT NULL DEFAULT 'postgres',
            refreshed_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS app_settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
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
            enum_values TEXT NOT NULL DEFAULT '[]',
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
            arguments TEXT NOT NULL DEFAULT '',
            return_type TEXT,
            definition TEXT,
            language TEXT,
            body TEXT,
            library TEXT,
            owner TEXT,
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
            PRIMARY KEY (connection_key, schema_name, table_name, name)
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
    conn.execute(
        "ALTER TABLE cache_metadata ADD COLUMN db_type TEXT NOT NULL DEFAULT 'postgres'",
        [],
    )
    .ok();
    conn.execute("ALTER TABLE columns ADD COLUMN default_value TEXT", [])
        .ok();
    conn.execute(
        "ALTER TABLE columns ADD COLUMN is_generated INTEGER NOT NULL DEFAULT 0",
        [],
    )
    .ok();
    conn.execute(
        "ALTER TABLE columns ADD COLUMN generation_expression TEXT",
        [],
    )
    .ok();
    conn.execute(
        "ALTER TABLE columns ADD COLUMN enum_values TEXT NOT NULL DEFAULT '[]'",
        [],
    )
    .ok();
    conn.execute("ALTER TABLE functions ADD COLUMN definition TEXT", [])
        .ok();
    conn.execute("ALTER TABLE functions ADD COLUMN language TEXT", [])
        .ok();
    conn.execute("ALTER TABLE functions ADD COLUMN body TEXT", [])
        .ok();
    conn.execute("ALTER TABLE functions ADD COLUMN library TEXT", [])
        .ok();
    conn.execute("ALTER TABLE functions ADD COLUMN owner TEXT", [])
        .ok();

    // Migrate functions table to ensure arguments is NOT NULL (fixes overloaded function support)
    conn.execute("DROP TABLE IF EXISTS functions_new", []).ok();
    conn.execute(
        "CREATE TABLE functions_new (
            connection_key TEXT NOT NULL,
            schema_name TEXT NOT NULL,
            name TEXT NOT NULL,
            arguments TEXT NOT NULL DEFAULT '',
            return_type TEXT,
            definition TEXT,
            language TEXT,
            body TEXT,
            library TEXT,
            owner TEXT,
            PRIMARY KEY (connection_key, schema_name, name, arguments)
        )",
        [],
    )
    .ok();
    conn.execute(
        "INSERT INTO functions_new SELECT connection_key, schema_name, name, COALESCE(arguments, ''), return_type, definition, language, body, library, owner FROM functions",
        [],
    )
    .ok();
    conn.execute("DROP TABLE IF EXISTS functions", []).ok();
    conn.execute("ALTER TABLE functions_new RENAME TO functions", [])
        .ok();

    Ok(())
}

pub fn load_setting(conn: &Connection, key: &str) -> Result<Option<String>, rusqlite::Error> {
    let mut stmt = conn.prepare("SELECT value FROM app_settings WHERE key = ?1")?;
    let mut rows = stmt.query(params![key])?;
    match rows.next()? {
        Some(row) => row.get(0).map(Some),
        None => Ok(None),
    }
}

pub fn save_setting(conn: &Connection, key: &str, value: &str) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO app_settings (key, value) VALUES (?1, ?2)
        ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
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
