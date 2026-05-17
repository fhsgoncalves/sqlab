pub mod ddl;
pub mod manager;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Database {
    Postgres,
}

impl Database {
    pub fn as_str(&self) -> &'static str {
        match self {
            Database::Postgres => "postgres",
        }
    }
}

impl Default for Database {
    fn default() -> Self {
        Database::Postgres
    }
}

impl TryFrom<&str> for Database {
    type Error = &'static str;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "postgres" => Ok(Database::Postgres),
            _ => Err("unsupported database type"),
        }
    }
}

impl TryFrom<String> for Database {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Database::try_from(value.as_str())
    }
}

impl std::fmt::Display for Database {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataSourceConfig {
    pub name: String,
    #[serde(default)]
    pub db_type: Database,
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_postgres_port")]
    pub port: u16,
    pub user: String,
    #[serde(skip)]
    pub password: String,
    pub database: String,
    #[serde(default = "default_postgres_schema")]
    pub schema: String,
    #[serde(default)]
    pub query_string: String,
}

impl Default for DataSourceConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            db_type: Database::default(),
            host: default_host(),
            port: default_postgres_port(),
            user: String::new(),
            password: String::new(),
            database: String::new(),
            schema: default_postgres_schema(),
            query_string: String::new(),
        }
    }
}

fn default_host() -> String {
    "localhost".into()
}

fn default_postgres_port() -> u16 {
    5432
}

fn default_postgres_schema() -> String {
    "public".into()
}

#[derive(Debug, Clone)]
pub struct ColumnMetadata {
    pub name: String,
    pub data_type: String,
    pub is_pk: bool,
    pub is_fk: bool,
}

#[derive(Debug, Clone)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub column_metadata: Vec<ColumnMetadata>,
    pub rows: Vec<Vec<String>>,
    pub row_count: usize,
    pub execution_time_ms: u128,
}

#[derive(Debug, Clone, Default)]
pub struct DatabaseSchema {
    pub db_type: Database,
    pub schemas: Vec<SchemaInfo>,
    pub tables: Vec<TableInfo>,
    pub functions: Vec<FunctionInfo>,
    pub sequences: Vec<SequenceInfo>,
    pub indexes: Vec<IndexInfo>,
    pub triggers: Vec<TriggerInfo>,
    pub foreign_keys: Vec<ForeignKeyInfo>,
}

#[derive(Debug, Clone)]
pub struct SchemaInfo {
    pub name: String,
    pub owner: String,
}

#[derive(Debug, Clone)]
pub struct TableInfo {
    pub schema: String,
    pub name: String,
    pub kind: TableKind,
    pub columns: Vec<ColumnInfo>,
}

#[derive(Debug, Clone)]
pub enum TableKind {
    Table,
    View,
    MaterializedView,
    ForeignTable,
}

#[derive(Debug, Clone)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub ordinal: i32,
    pub is_pk: bool,
    pub is_fk: bool,
    pub default_value: Option<String>,
    pub is_generated: bool,
    pub generation_expression: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FunctionInfo {
    pub schema: String,
    pub name: String,
    pub arguments: String,
    pub return_type: String,
    pub definition: Option<String>,
    pub language: String,
    pub body: Option<String>,
    pub library: Option<String>,
    pub owner: String,
}

#[derive(Debug, Clone)]
pub struct SequenceInfo {
    pub schema: String,
    pub name: String,
    pub data_type: String,
    pub start_value: String,
    pub min_value: String,
    pub max_value: String,
    pub increment_by: String,
}

#[derive(Debug, Clone)]
pub struct IndexInfo {
    pub schema: String,
    pub table_name: String,
    pub name: String,
    pub is_unique: bool,
    pub is_primary: bool,
    pub columns: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TriggerInfo {
    pub schema: String,
    pub table_name: String,
    pub name: String,
    pub event: String,
    pub timing: String,
    pub definition: String,
}

#[derive(Debug, Clone)]
pub struct ForeignKeyInfo {
    pub name: String,
    pub source_schema: String,
    pub source_table: String,
    pub source_columns: Vec<String>,
    pub target_schema: String,
    pub target_table: String,
    pub target_columns: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum DataSourceError {
    #[error("Not connected")]
    NotConnected,
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
    #[error("Query failed: {0}")]
    QueryFailed(String),
    #[allow(dead_code)]
    #[error("Unsupported database type: {0}")]
    UnsupportedType(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    Idle,
    Connected,
    Failed,
}

#[async_trait]
#[allow(dead_code)]
pub trait DataSource: Send + Sync {
    fn name(&self) -> &str;
    fn db_type(&self) -> Database;
    fn config(&self) -> &DataSourceConfig;
    fn is_connected(&self) -> bool;

    async fn connect(&mut self) -> Result<(), DataSourceError>;
    async fn disconnect(&mut self) -> Result<(), DataSourceError>;
    async fn execute_query(&self, query: &str) -> Result<QueryResult, DataSourceError>;
    async fn introspect_schema(&self) -> Result<DatabaseSchema, DataSourceError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_source_config_does_not_serialize_password() {
        let config = DataSourceConfig {
            name: "local".into(),
            user: "postgres".into(),
            password: "secret".into(),
            database: "app".into(),
            ..Default::default()
        };

        let toml = toml::to_string(&config).expect("serialize data source config");

        assert!(!toml.contains("password"));
        assert!(!toml.contains("secret"));
    }

    #[test]
    fn data_source_config_ignores_toml_password() {
        let toml = r#"
name = "local"
db_type = "postgres"
host = "localhost"
port = 5432
user = "postgres"
password = "legacy-secret"
database = "app"
schema = "public"
query_string = ""
"#;

        let config: DataSourceConfig =
            toml::from_str(toml).expect("deserialize data source config");

        assert!(config.password.is_empty());
    }
}
