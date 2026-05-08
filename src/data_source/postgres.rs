use std::error::Error;
use std::fmt::Write as FmtWrite;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Instant;
use std::{fs::File, io::Write, path::Path};

use async_trait::async_trait;
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, NaiveTime, SecondsFormat};
use tokio::runtime::Runtime;
use tokio_postgres::types::{FromSql, Type};
use tokio_postgres::{Client, Row};
use rustls::ClientConfig;
use postgres_rustls::MakeTlsConnector;

use crate::data_source::{
    ColumnInfo, DataSource, DataSourceConfig, DataSourceError, DatabaseSchema, FunctionInfo,
    IndexInfo, QueryResult, SchemaInfo, SequenceInfo, TableInfo, TableKind, TriggerInfo,
};

pub struct PostgresDataSource {
    config: DataSourceConfig,
    runtime: Arc<Runtime>,
    client: Option<Arc<Client>>,
}

impl PostgresDataSource {
    pub fn new(config: DataSourceConfig) -> Result<Self, DataSourceError> {
        let runtime =
            Arc::new(Runtime::new().map_err(|e| DataSourceError::ConnectionFailed(e.to_string()))?);
        Ok(Self {
            config,
            runtime,
            client: None,
        })
    }

    fn connection_string(&self) -> String {
        let base = format!(
            "host={} port={} user={} password={} dbname={}",
            self.config.host,
            self.config.port,
            self.config.user,
            self.config.password,
            self.config.database
        );
        if self.config.query_string.is_empty() {
            base
        } else {
            format!("{} {}", base, self.config.query_string)
        }
    }

    pub fn connect_blocking(&mut self) -> Result<(), DataSourceError> {
        let connection_string = self.connection_string();
        let tls = make_rustls_connector()?;

        let (client, connection) = self
            .runtime
            .block_on(async {
                tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    tokio_postgres::connect(&connection_string, tls),
                )
                .await
            })
            .map_err(|_| {
                DataSourceError::ConnectionFailed(
                    "Connection timed out after 10 seconds".to_string(),
                )
            })?
            .map_err(|e| DataSourceError::ConnectionFailed(format_postgres_error(e)))?;

        self.runtime.spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("postgres connection error: {}", format_postgres_error(e));
            }
        });

        self.client = Some(Arc::new(client));
        Ok(())
    }

    pub fn disconnect_blocking(&mut self) -> Result<(), DataSourceError> {
        self.client = None;
        Ok(())
    }

    pub fn execute_query_blocking(
        &self,
        query: &str,
        row_limit: Option<usize>,
    ) -> Result<QueryResult, DataSourceError> {
        let client = self.client.as_ref().ok_or(DataSourceError::NotConnected)?;
        let start = Instant::now();
        let (columns, rows) = self
            .runtime
            .block_on(async {
                let statement = client.prepare(query).await?;
                let columns = statement
                    .columns()
                    .iter()
                    .map(|column| column.name().to_string())
                    .collect::<Vec<_>>();
                let rows = client.query(&statement, &[]).await?;
                Ok::<_, tokio_postgres::Error>((columns, rows))
            })
            .map_err(|e| DataSourceError::QueryFailed(format_postgres_error(e)))?;

        let row_count = rows.len();
        let rows = rows
            .iter()
            .take(row_limit.unwrap_or(usize::MAX))
            .map(row_to_strings)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(QueryResult {
            columns,
            rows,
            row_count,
            execution_time_ms: start.elapsed().as_millis(),
        })
    }

    pub fn export_query_to_csv(
        &self,
        query: &str,
        path: impl AsRef<Path>,
    ) -> Result<(), DataSourceError> {
        let result = self.execute_query_blocking(query, None)?;
        let mut file =
            File::create(path).map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;

        writeln!(
            file,
            "{}",
            result
                .columns
                .iter()
                .map(|column| escape_csv_field(column))
                .collect::<Vec<_>>()
                .join(",")
        )
        .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;

        for row in result.rows {
            writeln!(
                file,
                "{}",
                row.iter()
                    .map(|cell| escape_csv_field(cell))
                    .collect::<Vec<_>>()
                    .join(",")
            )
            .map_err(|e| DataSourceError::QueryFailed(e.to_string()))?;
        }

        Ok(())
    }

    pub fn introspect_schema_blocking(&self) -> Result<DatabaseSchema, DataSourceError> {
        let client = self.client.as_ref().ok_or(DataSourceError::NotConnected)?;
        let configured_schema = self.config.schema.clone();

        let (schema_rows, column_rows, function_rows, sequence_rows, index_rows, trigger_rows) = self
            .runtime
            .block_on(async {
                let schema_rows = client
                    .query(
                        "
                        select n.nspname as schema_name, r.rolname as owner
                        from pg_catalog.pg_namespace n
                        join pg_catalog.pg_roles r on r.oid = n.nspowner
                        where n.nspname !~ '^pg_'
                          and n.nspname <> 'information_schema'
                        order by n.nspname
                        ",
                        &[],
                    )
                    .await?;

                let column_rows = client
                    .query(
                        "
                        select
                            n.nspname as schema_name,
                            c.relname as table_name,
                            c.relkind::text as relkind,
                            a.attname as column_name,
                            pg_catalog.format_type(a.atttypid, a.atttypmod) as data_type,
                            a.attnotnull,
                            a.attnum
                        from pg_catalog.pg_class c
                        join pg_catalog.pg_namespace n on n.oid = c.relnamespace
                        join pg_catalog.pg_attribute a on a.attrelid = c.oid
                        where c.relkind in ('r', 'p', 'v', 'm', 'f')
                          and a.attnum > 0
                          and not a.attisdropped
                          and n.nspname !~ '^pg_'
                          and n.nspname <> 'information_schema'
                          and ($1 = '' or n.nspname = $1)
                        order by n.nspname, c.relname, a.attnum
                        ",
                        &[&configured_schema],
                    )
                    .await?;

                let function_rows = client
                    .query(
                        "
                        select
                            n.nspname as schema_name,
                            p.proname as function_name,
                            pg_catalog.pg_get_function_arguments(p.oid) as arguments,
                            pg_catalog.pg_get_function_result(p.oid) as return_type
                        from pg_catalog.pg_proc p
                        join pg_catalog.pg_namespace n on n.oid = p.pronamespace
                        where n.nspname !~ '^pg_'
                          and n.nspname <> 'information_schema'
                          and ($1 = '' or n.nspname = $1)
                        order by n.nspname, p.proname
                        ",
                        &[&configured_schema],
                    )
                    .await?;

                let sequence_rows = client
                    .query(
                        "
                        select
                            schemaname as schema_name,
                            sequencename as sequence_name,
                            data_type::text as data_type,
                            start_value::text,
                            min_value::text,
                            max_value::text,
                            increment_by::text
                        from pg_sequences
                        where schemaname !~ '^pg_'
                          and schemaname <> 'information_schema'
                          and ($1 = '' or schemaname = $1)
                        order by schemaname, sequencename
                        ",
                        &[&configured_schema],
                    )
                    .await?;

                let index_rows = client
                    .query(
                        "
                        select
                            n.nspname as schema_name,
                            t.relname as table_name,
                            i.relname as index_name,
                            ix.indisunique as is_unique,
                            ix.indisprimary as is_primary,
                            array_agg(a.attname order by array_position(ix.indkey, a.attnum)) as columns
                        from pg_index ix
                        join pg_class i on i.oid = ix.indexrelid
                        join pg_class t on t.oid = ix.indrelid
                        join pg_namespace n on n.oid = t.relnamespace
                        join pg_attribute a on a.attrelid = t.oid and a.attnum = any(ix.indkey)
                        where n.nspname !~ '^pg_'
                          and n.nspname <> 'information_schema'
                          and ($1 = '' or n.nspname = $1)
                        group by n.nspname, t.relname, i.relname, ix.indisunique, ix.indisprimary
                        order by n.nspname, t.relname, i.relname
                        ",
                        &[&configured_schema],
                    )
                    .await?;

                let trigger_rows = client
                    .query(
                        "
                        select
                            n.nspname as schema_name,
                            c.relname as table_name,
                            t.tgname as trigger_name,
                            case t.tgtype & 2 when 2 then 'BEFORE' else 'AFTER' end as timing,
                            case
                                when t.tgtype & 4 = 4 then 'INSERT'
                                when t.tgtype & 8 = 8 then 'DELETE'
                                when t.tgtype & 16 = 16 then 'UPDATE'
                                when t.tgtype & 28 = 28 then 'INSERT OR UPDATE OR DELETE'
                                else 'UNKNOWN'
                            end as event,
                            pg_get_triggerdef(t.oid) as definition
                        from pg_trigger t
                        join pg_class c on c.oid = t.tgrelid
                        join pg_namespace n on n.oid = c.relnamespace
                        where not t.tgisinternal
                          and n.nspname !~ '^pg_'
                          and n.nspname <> 'information_schema'
                          and ($1 = '' or n.nspname = $1)
                        order by n.nspname, c.relname, t.tgname
                        ",
                        &[&configured_schema],
                    )
                    .await?;

                Ok::<_, tokio_postgres::Error>((schema_rows, column_rows, function_rows, sequence_rows, index_rows, trigger_rows))
            })
            .map_err(|e| DataSourceError::QueryFailed(format_postgres_error(e)))?;

        let schemas = schema_rows
            .iter()
            .map(|row| SchemaInfo {
                name: row.get("schema_name"),
                owner: row.get("owner"),
            })
            .collect::<Vec<_>>();

        let mut tables = Vec::<TableInfo>::new();
        for row in column_rows {
            let schema = row.get::<_, String>("schema_name");
            let table_name = row.get::<_, String>("table_name");
            let relkind = row.get::<_, String>("relkind");
            let column = ColumnInfo {
                name: row.get("column_name"),
                data_type: row.get("data_type"),
                nullable: !row.get::<_, bool>("attnotnull"),
                ordinal: i32::from(row.get::<_, i16>("attnum")),
            };

            if let Some(table) = tables
                .iter_mut()
                .find(|table| table.schema == schema && table.name == table_name)
            {
                table.columns.push(column);
            } else {
                tables.push(TableInfo {
                    schema,
                    name: table_name,
                    kind: table_kind(&relkind),
                    columns: vec![column],
                });
            }
        }

        let functions = function_rows
            .iter()
            .map(|row| FunctionInfo {
                schema: row.get("schema_name"),
                name: row.get("function_name"),
                arguments: row.get("arguments"),
                return_type: row.get("return_type"),
            })
            .collect();

        let sequences = sequence_rows
            .iter()
            .map(|row| SequenceInfo {
                schema: row.get("schema_name"),
                name: row.get("sequence_name"),
                data_type: row.get("data_type"),
                start_value: row.get("start_value"),
                min_value: row.get("min_value"),
                max_value: row.get("max_value"),
                increment_by: row.get("increment_by"),
            })
            .collect();

        let indexes = index_rows
            .iter()
            .map(|row| IndexInfo {
                schema: row.get("schema_name"),
                table_name: row.get("table_name"),
                name: row.get("index_name"),
                is_unique: row.get("is_unique"),
                is_primary: row.get("is_primary"),
                columns: row.get("columns"),
            })
            .collect();

        let triggers = trigger_rows
            .iter()
            .map(|row| TriggerInfo {
                schema: row.get("schema_name"),
                table_name: row.get("table_name"),
                name: row.get("trigger_name"),
                event: row.get("event"),
                timing: row.get("timing"),
                definition: row.get("definition"),
            })
            .collect();

        Ok(DatabaseSchema {
            schemas,
            tables,
            functions,
            sequences,
            indexes,
            triggers,
        })
    }
}

#[async_trait]
impl DataSource for PostgresDataSource {
    fn name(&self) -> &str {
        &self.config.name
    }

    fn db_type(&self) -> &str {
        &self.config.db_type
    }

    fn config(&self) -> &DataSourceConfig {
        &self.config
    }

    fn is_connected(&self) -> bool {
        self.client.is_some()
    }

    async fn connect(&mut self) -> Result<(), DataSourceError> {
        self.connect_blocking()
    }

    async fn disconnect(&mut self) -> Result<(), DataSourceError> {
        self.disconnect_blocking()
    }

    async fn execute_query(&self, query: &str) -> Result<QueryResult, DataSourceError> {
        self.execute_query_blocking(query, Some(500))
    }

    async fn introspect_schema(&self) -> Result<DatabaseSchema, DataSourceError> {
        self.introspect_schema_blocking()
    }
}

fn make_rustls_connector() -> Result<MakeTlsConnector, DataSourceError> {
    let certs = rustls_native_certs::load_native_certs()
        .map_err(|e| DataSourceError::ConnectionFailed(format!("Failed to load root certs: {}", e)))?;
    let mut root_store = rustls::RootCertStore::empty();
    for cert in certs {
        let _ = root_store.add(cert);
    }
    let config = ClientConfig::builder()
        .with_root_certificates(std::sync::Arc::new(root_store))
        .with_no_client_auth();
    Ok(MakeTlsConnector::new(std::sync::Arc::new(config).into()))
}

fn table_kind(relkind: &str) -> TableKind {
    match relkind {
        "v" => TableKind::View,
        "m" => TableKind::MaterializedView,
        "f" => TableKind::ForeignTable,
        _ => TableKind::Table,
    }
}

fn row_to_strings(row: &Row) -> Result<Vec<String>, DataSourceError> {
    row.columns()
        .iter()
        .enumerate()
        .map(|(ix, column)| cell_to_string(row, ix, column.type_()))
        .collect()
}

fn cell_to_string(row: &Row, ix: usize, ty: &Type) -> Result<String, DataSourceError> {
    if matches!(
        ty,
        &Type::VARCHAR | &Type::TEXT | &Type::BPCHAR | &Type::NAME
    ) {
        return Ok(row
            .try_get::<_, Option<String>>(ix)
            .unwrap_or(None)
            .unwrap_or_default());
    }
    if matches!(ty, &Type::BOOL) {
        return Ok(row
            .try_get::<_, Option<bool>>(ix)
            .unwrap_or(None)
            .map(|v| v.to_string())
            .unwrap_or_default());
    }
    if matches!(ty, &Type::INT2) {
        return Ok(row
            .try_get::<_, Option<i16>>(ix)
            .unwrap_or(None)
            .map(|v| v.to_string())
            .unwrap_or_default());
    }
    if matches!(ty, &Type::INT4) {
        return Ok(row
            .try_get::<_, Option<i32>>(ix)
            .unwrap_or(None)
            .map(|v| v.to_string())
            .unwrap_or_default());
    }
    if matches!(ty, &Type::INT8) {
        return Ok(row
            .try_get::<_, Option<i64>>(ix)
            .unwrap_or(None)
            .map(|v| v.to_string())
            .unwrap_or_default());
    }
    if matches!(ty, &Type::OID) {
        return Ok(row
            .try_get::<_, Option<u32>>(ix)
            .unwrap_or(None)
            .map(|v| v.to_string())
            .unwrap_or_default());
    }
    if matches!(ty, &Type::CHAR) {
        return Ok(row
            .try_get::<_, Option<i8>>(ix)
            .unwrap_or(None)
            .map(format_postgres_char)
            .unwrap_or_default());
    }
    if matches!(ty, &Type::NUMERIC) {
        return Ok(row
            .try_get::<_, Option<PgNumeric>>(ix)
            .unwrap_or(None)
            .map(|v| v.0)
            .unwrap_or_default());
    }
    if matches!(ty, &Type::FLOAT4) {
        return Ok(row
            .try_get::<_, Option<f32>>(ix)
            .unwrap_or(None)
            .map(|v| v.to_string())
            .unwrap_or_default());
    }
    if matches!(ty, &Type::FLOAT8) {
        return Ok(row
            .try_get::<_, Option<f64>>(ix)
            .unwrap_or(None)
            .map(|v| v.to_string())
            .unwrap_or_default());
    }
    if matches!(ty, &Type::DATE) {
        return Ok(row
            .try_get::<_, Option<NaiveDate>>(ix)
            .unwrap_or(None)
            .map(|v| v.to_string())
            .unwrap_or_default());
    }
    if matches!(ty, &Type::TIME) {
        return Ok(row
            .try_get::<_, Option<NaiveTime>>(ix)
            .unwrap_or(None)
            .map(format_time)
            .unwrap_or_default());
    }
    if matches!(ty, &Type::TIMESTAMP) {
        return Ok(row
            .try_get::<_, Option<NaiveDateTime>>(ix)
            .unwrap_or(None)
            .map(format_timestamp)
            .unwrap_or_default());
    }
    if matches!(ty, &Type::TIMESTAMPTZ) {
        return Ok(row
            .try_get::<_, Option<DateTime<Local>>>(ix)
            .unwrap_or(None)
            .map(format_timestamptz)
            .unwrap_or_default());
    }
    if matches!(ty, &Type::UUID) {
        return Ok(row
            .try_get::<_, Option<uuid::Uuid>>(ix)
            .unwrap_or(None)
            .map(|v| v.to_string())
            .unwrap_or_default());
    }
    if matches!(ty, &Type::JSON | &Type::JSONB) {
        return Ok(row
            .try_get::<_, Option<serde_json::Value>>(ix)
            .unwrap_or(None)
            .map(|v| v.to_string())
            .unwrap_or_default());
    }
    if matches!(ty, &Type::BYTEA) {
        return Ok(row
            .try_get::<_, Option<Vec<u8>>>(ix)
            .unwrap_or(None)
            .map(|v| bytes_to_postgres_hex(&v))
            .unwrap_or_default());
    }
    if matches!(ty, &Type::INET) {
        return Ok(row
            .try_get::<_, Option<IpAddr>>(ix)
            .unwrap_or(None)
            .map(|v| v.to_string())
            .unwrap_or_default());
    }

    if matches!(
        ty,
        &Type::VARCHAR_ARRAY | &Type::TEXT_ARRAY | &Type::BPCHAR_ARRAY | &Type::NAME_ARRAY
    ) {
        return Ok(array_cell_to_string::<String, _>(row, ix, |v| v));
    }
    if matches!(ty, &Type::BOOL_ARRAY) {
        return Ok(array_cell_to_string::<bool, _>(row, ix, |v| v.to_string()));
    }
    if matches!(ty, &Type::INT2_ARRAY) {
        return Ok(array_cell_to_string::<i16, _>(row, ix, |v| v.to_string()));
    }
    if matches!(ty, &Type::INT4_ARRAY) {
        return Ok(array_cell_to_string::<i32, _>(row, ix, |v| v.to_string()));
    }
    if matches!(ty, &Type::INT8_ARRAY) {
        return Ok(array_cell_to_string::<i64, _>(row, ix, |v| v.to_string()));
    }
    if matches!(ty, &Type::OID_ARRAY) {
        return Ok(array_cell_to_string::<u32, _>(row, ix, |v| v.to_string()));
    }
    if matches!(ty, &Type::NUMERIC_ARRAY) {
        return Ok(array_cell_to_string::<PgNumeric, _>(row, ix, |v| v.0));
    }
    if matches!(ty, &Type::FLOAT4_ARRAY) {
        return Ok(array_cell_to_string::<f32, _>(row, ix, |v| v.to_string()));
    }
    if matches!(ty, &Type::FLOAT8_ARRAY) {
        return Ok(array_cell_to_string::<f64, _>(row, ix, |v| v.to_string()));
    }
    if matches!(ty, &Type::DATE_ARRAY) {
        return Ok(array_cell_to_string::<NaiveDate, _>(row, ix, |v| {
            v.to_string()
        }));
    }
    if matches!(ty, &Type::TIME_ARRAY) {
        return Ok(array_cell_to_string::<NaiveTime, _>(row, ix, format_time));
    }
    if matches!(ty, &Type::TIMESTAMP_ARRAY) {
        return Ok(array_cell_to_string::<NaiveDateTime, _>(
            row,
            ix,
            format_timestamp,
        ));
    }
    if matches!(ty, &Type::TIMESTAMPTZ_ARRAY) {
        return Ok(array_cell_to_string::<DateTime<Local>, _>(
            row,
            ix,
            format_timestamptz,
        ));
    }
    if matches!(ty, &Type::UUID_ARRAY) {
        return Ok(array_cell_to_string::<uuid::Uuid, _>(row, ix, |v| {
            v.to_string()
        }));
    }
    if matches!(ty, &Type::JSON_ARRAY | &Type::JSONB_ARRAY) {
        return Ok(array_cell_to_string::<serde_json::Value, _>(row, ix, |v| {
            v.to_string()
        }));
    }
    if matches!(ty, &Type::BYTEA_ARRAY) {
        return Ok(array_cell_to_string::<Vec<u8>, _>(row, ix, |v| {
            bytes_to_postgres_hex(&v)
        }));
    }
    if matches!(ty, &Type::INET_ARRAY) {
        return Ok(array_cell_to_string::<IpAddr, _>(row, ix, |v| {
            v.to_string()
        }));
    }
    if matches!(ty, &Type::CHAR_ARRAY) {
        return Ok(array_cell_to_string::<i8, _>(row, ix, format_postgres_char));
    }

    if let Ok(value) = row.try_get::<_, Option<String>>(ix) {
        return Ok(value.unwrap_or_default());
    }

    Ok(row
        .try_get::<_, Option<String>>(ix)
        .unwrap_or_else(|_| Some(format!("<{}>", ty.name())))
        .unwrap_or_default())
}

fn array_cell_to_string<T, F>(row: &Row, ix: usize, format: F) -> String
where
    T: for<'a> FromSql<'a>,
    F: Fn(T) -> String,
{
    row.try_get::<_, Option<Vec<Option<T>>>>(ix)
        .unwrap_or(None)
        .map(|values| format_postgres_array(values, format))
        .unwrap_or_default()
}

fn format_postgres_array<T, F>(values: Vec<Option<T>>, format: F) -> String
where
    F: Fn(T) -> String,
{
    format!(
        "{{{}}}",
        values
            .into_iter()
            .map(|value| value.map(&format).unwrap_or_else(|| "NULL".to_string()))
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn format_time(value: NaiveTime) -> String {
    value.format("%H:%M:%S%.6f").to_string()
}

fn format_timestamp(value: NaiveDateTime) -> String {
    value.format("%Y-%m-%dT%H:%M:%S%.6f").to_string()
}

fn format_timestamptz(value: DateTime<Local>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Micros, false)
}

fn format_postgres_char(value: i8) -> String {
    char::from(value as u8).to_string()
}

fn bytes_to_postgres_hex(bytes: &[u8]) -> String {
    let mut output = String::from("\\x");
    for byte in bytes {
        let _ = FmtWrite::write_fmt(&mut output, format_args!("{byte:02x}"));
    }
    output
}

#[derive(Debug)]
struct PgNumeric(String);

impl<'a> FromSql<'a> for PgNumeric {
    fn from_sql(_: &Type, raw: &'a [u8]) -> Result<PgNumeric, Box<dyn Error + Sync + Send>> {
        Ok(PgNumeric(format_numeric(raw)?))
    }

    fn accepts(ty: &Type) -> bool {
        matches!(*ty, Type::NUMERIC)
    }
}

fn format_numeric(raw: &[u8]) -> Result<String, Box<dyn Error + Sync + Send>> {
    const NUMERIC_POS: i16 = 0x0000;
    const NUMERIC_NEG: i16 = 0x4000;
    const NUMERIC_NAN: i16 = -0x4000;
    const NUMERIC_PINF: i16 = -0x3000;
    const NUMERIC_NINF: i16 = -0x1000;

    if raw.len() < 8 {
        return Err("invalid numeric value".into());
    }

    let ndigits = read_i16(raw, 0)? as usize;
    let weight = read_i16(raw, 2)?;
    let sign = read_i16(raw, 4)?;
    let dscale = read_i16(raw, 6)?.max(0) as usize;

    match sign {
        NUMERIC_NAN => return Ok("NaN".to_string()),
        NUMERIC_PINF => return Ok("Infinity".to_string()),
        NUMERIC_NINF => return Ok("-Infinity".to_string()),
        NUMERIC_POS | NUMERIC_NEG => {}
        _ => return Err("invalid numeric sign".into()),
    }

    if raw.len() < 8 + ndigits * 2 {
        return Err("invalid numeric digit count".into());
    }

    let mut digits = Vec::with_capacity(ndigits);
    for offset in (8..8 + ndigits * 2).step_by(2) {
        digits.push(read_i16(raw, offset)?);
    }

    let groups_before_decimal = weight as isize + 1;
    let mut integer = String::new();

    if groups_before_decimal <= 0 {
        integer.push('0');
    } else {
        for group_ix in 0..groups_before_decimal as usize {
            let group = digits.get(group_ix).copied().unwrap_or(0);
            if group_ix == 0 {
                integer.push_str(&group.to_string());
            } else {
                let _ = FmtWrite::write_fmt(&mut integer, format_args!("{group:04}"));
            }
        }
    }

    let mut fraction = String::new();
    if dscale > 0 {
        for _ in 0..(-groups_before_decimal).max(0) {
            fraction.push_str("0000");
        }

        let first_fraction_group = groups_before_decimal.max(0) as usize;
        for group in digits.iter().skip(first_fraction_group) {
            let _ = FmtWrite::write_fmt(&mut fraction, format_args!("{group:04}"));
        }

        if fraction.len() < dscale {
            fraction.extend(std::iter::repeat_n('0', dscale - fraction.len()));
        }
        fraction.truncate(dscale);
    }

    let mut output = if dscale == 0 {
        integer
    } else {
        format!("{integer}.{fraction}")
    };

    let is_zero = output.chars().all(|ch| ch == '0' || ch == '.');
    if sign == NUMERIC_NEG && !is_zero {
        output.insert(0, '-');
    }

    Ok(output)
}

fn read_i16(raw: &[u8], offset: usize) -> Result<i16, Box<dyn Error + Sync + Send>> {
    let bytes = raw.get(offset..offset + 2).ok_or("invalid numeric value")?;
    Ok(i16::from_be_bytes([bytes[0], bytes[1]]))
}

fn escape_csv_field(field: &str) -> String {
    if field.contains(',') || field.contains('"') || field.contains('\n') || field.contains('\r') {
        let escaped = field.replace('"', "\"\"");
        format!("\"{}\"", escaped)
    } else {
        field.to_string()
    }
}

fn format_postgres_error(e: tokio_postgres::Error) -> String {
    if let Some(db_err) = e.as_db_error() {
        let mut msg = format!("{}: {}", db_err.severity(), db_err.message());
        if let Some(detail) = db_err.detail() {
            msg.push_str(&format!("\nDetail: {}", detail));
        }
        if let Some(hint) = db_err.hint() {
            msg.push_str(&format!("\nHint: {}", hint));
        }
        if let Some(pos) = db_err.position() {
            msg.push_str(&format!(" (at character {:?})", pos));
        }
        msg
    } else {
        e.to_string()
    }
}
