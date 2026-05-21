use std::error::Error;
use std::fmt::Write as FmtWrite;
use std::io::{BufRead, BufReader, Cursor};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Instant;
use std::{fs::File, io::Write, path::Path};

use async_trait::async_trait;
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, NaiveTime, SecondsFormat};
use postgres_rustls::MakeTlsConnector;
use rustls::ClientConfig;
use sqlab_drivers_core::{
    ColumnInfo, ColumnMetadata, DataSource, DataSourceConfig, DataSourceError, Database,
    DatabaseSchema, ForeignKeyInfo, FunctionInfo, IndexInfo, QueryExecutionOptions, QueryResult,
    SchemaInfo, SequenceInfo, TableEditBatch, TableEditValue, TableInfo, TableKind, TriggerInfo,
    display_data_type,
};
use sqlparser::ast::Statement;
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::Parser;
use tokio::runtime::Runtime;
use tokio_postgres::types::{FromSql, Type};
use tokio_postgres::{Client, Row};

const DEFAULT_ROW_LIMIT: usize = 1000;
const AWS_RDS_GLOBAL_BUNDLE_PEM: &str = include_str!("aws_rds_global_bundle.pem");

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

    fn connection_string(&self) -> Result<(String, Option<String>), DataSourceError> {
        let base = format!(
            "host={} port={} user={} password={} dbname={}",
            self.config.host,
            self.config.port,
            self.config.user,
            self.config.password,
            self.config.database
        );
        let (query_string, ssl_root_cert) = postgres_driver_options(&self.config.query_string)?;
        if query_string.is_empty() {
            Ok((base, ssl_root_cert))
        } else {
            Ok((format!("{} {}", base, query_string), ssl_root_cert))
        }
    }

    pub fn connect_blocking(&mut self) -> Result<(), DataSourceError> {
        let (connection_string, ssl_root_cert) = self.connection_string()?;
        let tls =
            make_rustls_connector(ssl_root_cert.as_deref(), is_aws_rds_host(&self.config.host))?;

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
        apply_limit: bool,
    ) -> Result<QueryResult, DataSourceError> {
        self.execute_query_blocking_with_options(
            query,
            apply_limit,
            &QueryExecutionOptions::default(),
        )
    }

    pub fn execute_query_blocking_with_options(
        &self,
        query: &str,
        apply_limit: bool,
        options: &QueryExecutionOptions,
    ) -> Result<QueryResult, DataSourceError> {
        let client = self.client.as_ref().ok_or(DataSourceError::NotConnected)?;
        let search_path = options
            .search_path
            .as_deref()
            .map(str::trim)
            .filter(|schema| !schema.is_empty())
            .map(postgres_search_path_value);
        let query = if apply_limit {
            apply_limit_if_missing(query, DEFAULT_ROW_LIMIT)
        } else {
            query.to_string()
        };
        let start = Instant::now();
        let (columns, column_metadata, rows) = self
            .runtime
            .block_on(async {
                if let Some(search_path) = search_path.as_deref() {
                    client
                        .execute(
                            "select pg_catalog.set_config('search_path', $1, false)",
                            &[&search_path],
                        )
                        .await?;
                }
                let statement = client.prepare(&query).await?;
                let column_metadata: Vec<ColumnMetadata> = statement
                    .columns()
                    .iter()
                    .map(|column| ColumnMetadata {
                        name: column.name().to_string(),
                        data_type: display_data_type(Database::Postgres, column.type_().name()),
                        is_pk: false,
                        is_fk: false,
                    })
                    .collect();
                let columns: Vec<String> =
                    column_metadata.iter().map(|cm| cm.name.clone()).collect();
                let rows = client.query(&statement, &[]).await?;
                Ok::<_, tokio_postgres::Error>((columns, column_metadata, rows))
            })
            .map_err(|e| DataSourceError::QueryFailed(format_postgres_error(e)))?;

        let row_count = rows.len();
        let (rows, nulls): (Vec<_>, Vec<_>) = rows
            .iter()
            .map(row_to_strings_and_nulls)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .unzip();

        Ok(QueryResult {
            columns,
            column_metadata,
            rows,
            nulls,
            row_count,
            execution_time_ms: start.elapsed().as_millis(),
        })
    }

    pub fn export_query_to_csv(
        &self,
        query: &str,
        path: impl AsRef<Path>,
    ) -> Result<(), DataSourceError> {
        let result = self.execute_query_blocking(query, false)?;
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

        let (schema_rows, column_rows, function_rows, sequence_rows, index_rows, trigger_rows, pk_rows, fk_rows, foreign_key_rows) = self
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
                            a.attnum,
                            pg_get_expr(d.adbin, d.adrelid) as column_default,
                            a.attgenerated::text as attgenerated
                        from pg_catalog.pg_class c
                        join pg_catalog.pg_namespace n on n.oid = c.relnamespace
                        join pg_catalog.pg_attribute a on a.attrelid = c.oid
                        left join pg_catalog.pg_attrdef d on d.adrelid = c.oid and d.adnum = a.attnum
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
                            pg_catalog.pg_get_function_result(p.oid) as return_type,
                            pg_catalog.pg_get_functiondef(p.oid) as definition,
                            l.lanname as language,
                            p.prosrc as body,
                            p.probin as library,
                            pg_catalog.pg_get_userbyid(p.proowner) as owner
                        from pg_catalog.pg_proc p
                        join pg_catalog.pg_namespace n on n.oid = p.pronamespace
                        join pg_catalog.pg_language l on l.oid = p.prolang
                        where n.nspname !~ '^pg_'
                          and n.nspname <> 'information_schema'
                          and p.prokind = 'f'
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

                let pk_rows = client
                    .query(
                        "
                        select
                            n.nspname as schema_name,
                            c.relname as table_name,
                            a.attname as column_name
                        from pg_index ix
                        join pg_class c on c.oid = ix.indrelid
                        join pg_namespace n on n.oid = c.relnamespace
                        join pg_attribute a on a.attrelid = c.oid and a.attnum = any(ix.indkey)
                        where ix.indisprimary
                          and n.nspname !~ '^pg_'
                          and n.nspname <> 'information_schema'
                          and ($1 = '' or n.nspname = $1)
                        ",
                        &[&configured_schema],
                    )
                    .await?;

                let fk_rows = client
                    .query(
                        "
                        select
                            n.nspname as schema_name,
                            c.relname as table_name,
                            a.attname as column_name
                        from pg_constraint con
                        join pg_class c on c.oid = con.conrelid
                        join pg_namespace n on n.oid = c.relnamespace
                        join pg_attribute a on a.attrelid = c.oid and a.attnum = any(con.conkey)
                        where con.contype = 'f'
                          and n.nspname !~ '^pg_'
                          and n.nspname <> 'information_schema'
                          and ($1 = '' or n.nspname = $1)
                        ",
                        &[&configured_schema],
                    )
                    .await?;

                let foreign_key_rows = client
                    .query(
                        "
                        select
                            con.conname as constraint_name,
                            src_ns.nspname as source_schema,
                            src.relname as source_table,
                            array_agg(src_attr.attname order by src_cols.ord) as source_columns,
                            tgt_ns.nspname as target_schema,
                            tgt.relname as target_table,
                            array_agg(tgt_attr.attname order by src_cols.ord) as target_columns
                        from pg_constraint con
                        join pg_class src on src.oid = con.conrelid
                        join pg_namespace src_ns on src_ns.oid = src.relnamespace
                        join pg_class tgt on tgt.oid = con.confrelid
                        join pg_namespace tgt_ns on tgt_ns.oid = tgt.relnamespace
                        join unnest(con.conkey) with ordinality as src_cols(attnum, ord) on true
                        join unnest(con.confkey) with ordinality as tgt_cols(attnum, ord) on tgt_cols.ord = src_cols.ord
                        join pg_attribute src_attr on src_attr.attrelid = src.oid and src_attr.attnum = src_cols.attnum
                        join pg_attribute tgt_attr on tgt_attr.attrelid = tgt.oid and tgt_attr.attnum = tgt_cols.attnum
                        where con.contype = 'f'
                          and src_ns.nspname !~ '^pg_'
                          and src_ns.nspname <> 'information_schema'
                          and ($1 = '' or src_ns.nspname = $1 or tgt_ns.nspname = $1)
                        group by con.conname, src_ns.nspname, src.relname, tgt_ns.nspname, tgt.relname
                        order by src_ns.nspname, src.relname, con.conname
                        ",
                        &[&configured_schema],
                    )
                    .await?;

                Ok::<_, tokio_postgres::Error>((schema_rows, column_rows, function_rows, sequence_rows, index_rows, trigger_rows, pk_rows, fk_rows, foreign_key_rows))
            })
            .map_err(|e| DataSourceError::QueryFailed(format_postgres_error(e)))?;

        let schemas = schema_rows
            .iter()
            .map(|row| SchemaInfo {
                name: row.get("schema_name"),
                owner: row.get("owner"),
            })
            .collect::<Vec<_>>();

        use std::collections::HashSet;
        let pk_set: HashSet<(String, String, String)> = pk_rows
            .iter()
            .map(|row| {
                (
                    row.get("schema_name"),
                    row.get("table_name"),
                    row.get("column_name"),
                )
            })
            .collect();
        let fk_set: HashSet<(String, String, String)> = fk_rows
            .iter()
            .map(|row| {
                (
                    row.get("schema_name"),
                    row.get("table_name"),
                    row.get("column_name"),
                )
            })
            .collect();

        let mut tables = Vec::<TableInfo>::new();
        for row in column_rows {
            let schema = row.get::<_, String>("schema_name");
            let table_name = row.get::<_, String>("table_name");
            let column_name = row.get::<_, String>("column_name");
            let relkind = row.get::<_, String>("relkind");

            let is_pk = pk_set.contains(&(schema.clone(), table_name.clone(), column_name.clone()));
            let is_fk = fk_set.contains(&(schema.clone(), table_name.clone(), column_name.clone()));

            let default_value: Option<String> = row.get("column_default");
            let attgenerated: String = row
                .get::<_, Option<String>>("attgenerated")
                .unwrap_or_default();
            let is_generated = !attgenerated.is_empty();
            let generation_expression = if is_generated {
                default_value.clone()
            } else {
                None
            };

            let column = ColumnInfo {
                name: column_name,
                data_type: display_data_type(Database::Postgres, row.get::<_, String>("data_type")),
                nullable: !row.get::<_, bool>("attnotnull"),
                ordinal: i32::from(row.get::<_, i16>("attnum")),
                is_pk,
                is_fk,
                default_value: if !is_generated { default_value } else { None },
                is_generated,
                generation_expression,
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
                arguments: row
                    .get::<_, Option<String>>("arguments")
                    .unwrap_or_default(),
                return_type: display_data_type(
                    Database::Postgres,
                    row.get::<_, Option<String>>("return_type")
                        .unwrap_or_default(),
                ),
                definition: row.get("definition"),
                language: row
                    .get::<_, Option<String>>("language")
                    .unwrap_or_else(|| "unknown".to_string()),
                body: row.get("body"),
                library: row.get("library"),
                owner: row.get::<_, Option<String>>("owner").unwrap_or_default(),
            })
            .collect();

        let sequences = sequence_rows
            .iter()
            .map(|row| SequenceInfo {
                schema: row.get("schema_name"),
                name: row.get("sequence_name"),
                data_type: display_data_type(Database::Postgres, row.get::<_, String>("data_type")),
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

        let foreign_keys = foreign_key_rows
            .iter()
            .map(|row| ForeignKeyInfo {
                name: row.get("constraint_name"),
                source_schema: row.get("source_schema"),
                source_table: row.get("source_table"),
                source_columns: row.get("source_columns"),
                target_schema: row.get("target_schema"),
                target_table: row.get("target_table"),
                target_columns: row.get("target_columns"),
            })
            .collect();

        Ok(DatabaseSchema {
            db_type: Database::Postgres,
            schemas,
            tables,
            functions,
            sequences,
            indexes,
            triggers,
            foreign_keys,
        })
    }

    pub fn apply_table_edits_blocking(&self, batch: TableEditBatch) -> Result<(), DataSourceError> {
        let client = self.client.as_ref().ok_or(DataSourceError::NotConnected)?;
        if batch.rows.is_empty() {
            return Ok(());
        }

        let statements = batch
            .rows
            .iter()
            .map(|row| postgres_update_statement(&batch, row))
            .collect::<Result<Vec<_>, _>>()?;

        self.runtime.block_on(async {
            client
                .batch_execute("BEGIN")
                .await
                .map_err(|e| DataSourceError::QueryFailed(format_postgres_error(e)))?;
            for statement in statements {
                let affected = match client.execute(statement.as_str(), &[]).await {
                    Ok(affected) => affected,
                    Err(error) => {
                        let _ = client.batch_execute("ROLLBACK").await;
                        return Err(DataSourceError::QueryFailed(format_postgres_error(error)));
                    }
                };
                if affected != 1 {
                    let _ = client.batch_execute("ROLLBACK").await;
                    return Err(DataSourceError::QueryFailed(format!(
                        "Expected edit to update 1 row, updated {affected} rows instead."
                    )));
                }
            }
            client
                .batch_execute("COMMIT")
                .await
                .map_err(|e| DataSourceError::QueryFailed(format_postgres_error(e)))?;
            Ok::<(), DataSourceError>(())
        })
    }
}

#[async_trait]
impl DataSource for PostgresDataSource {
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
        self.client.is_some()
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

/// Factory function for creating a PostgresDataSource
pub fn create_postgres_data_source(
    config: &DataSourceConfig,
) -> Result<Box<dyn DataSource>, DataSourceError> {
    Ok(Box::new(PostgresDataSource::new(config.clone())?))
}

fn make_rustls_connector(
    ssl_root_cert: Option<&str>,
    include_aws_rds_roots: bool,
) -> Result<MakeTlsConnector, DataSourceError> {
    let certs = rustls_native_certs::load_native_certs().map_err(|e| {
        DataSourceError::ConnectionFailed(format!("Failed to load root certs: {}", e))
    })?;
    let mut root_store = rustls::RootCertStore::empty();
    for cert in certs {
        let _ = root_store.add(cert);
    }
    if include_aws_rds_roots {
        let mut reader = Cursor::new(AWS_RDS_GLOBAL_BUNDLE_PEM.as_bytes());
        add_pem_root_certs_from_reader(&mut root_store, &mut reader, "embedded AWS RDS CA bundle")?;
    }
    if let Some(path) = ssl_root_cert {
        add_pem_root_certs(&mut root_store, path)?;
    }
    let config = ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| {
        DataSourceError::ConnectionFailed(format!(
            "Failed to configure rustls protocol versions: {}",
            e
        ))
    })?
    .with_root_certificates(std::sync::Arc::new(root_store))
    .with_no_client_auth();
    Ok(MakeTlsConnector::new(std::sync::Arc::new(config).into()))
}

fn is_aws_rds_host(host: &str) -> bool {
    host.split(',').any(|host| {
        let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
        host.ends_with(".rds.amazonaws.com") || host.ends_with(".rds.amazonaws.com.cn")
    })
}

fn add_pem_root_certs(
    root_store: &mut rustls::RootCertStore,
    path: &str,
) -> Result<(), DataSourceError> {
    let file = File::open(path).map_err(|e| {
        DataSourceError::ConnectionFailed(format!("Failed to open sslrootcert '{}': {}", path, e))
    })?;
    let mut reader = BufReader::new(file);
    add_pem_root_certs_from_reader(root_store, &mut reader, &format!("sslrootcert '{}'", path))
}

fn add_pem_root_certs_from_reader(
    root_store: &mut rustls::RootCertStore,
    reader: &mut dyn BufRead,
    source: &str,
) -> Result<(), DataSourceError> {
    let mut added = 0;
    for cert in rustls_pemfile::certs(reader) {
        let cert = cert.map_err(|e| {
            DataSourceError::ConnectionFailed(format!("Failed to read {}: {}", source, e))
        })?;
        root_store.add(cert).map_err(|e| {
            DataSourceError::ConnectionFailed(format!(
                "Failed to add certificate from {}: {}",
                source, e
            ))
        })?;
        added += 1;
    }
    if added == 0 {
        return Err(DataSourceError::ConnectionFailed(format!(
            "{} did not contain any PEM certificates",
            source
        )));
    }
    Ok(())
}

fn postgres_driver_options(
    query_string: &str,
) -> Result<(String, Option<String>), DataSourceError> {
    if query_string.trim().is_empty() {
        return Ok((String::new(), None));
    }

    let mut sanitized = Vec::new();
    let mut ssl_root_cert = None;
    for (key, value) in parse_postgres_options(query_string)? {
        match key.as_str() {
            "sslrootcert" => ssl_root_cert = Some(value),
            "sslmode" if matches!(value.as_str(), "verify-ca" | "verify-full") => {
                sanitized.push((key, "require".to_string()));
            }
            _ => sanitized.push((key, value)),
        }
    }

    Ok((format_postgres_options(&sanitized), ssl_root_cert))
}

fn parse_postgres_options(input: &str) -> Result<Vec<(String, String)>, DataSourceError> {
    let mut options = Vec::new();
    let mut chars = input.char_indices().peekable();

    while let Some((_, ch)) = chars.peek().copied() {
        if ch.is_whitespace() {
            chars.next();
            continue;
        }

        let key_start = chars.peek().map(|(idx, _)| *idx).unwrap_or(input.len());
        while let Some((_, ch)) = chars.peek().copied() {
            if ch == '=' || ch.is_whitespace() {
                break;
            }
            chars.next();
        }
        let key_end = chars.peek().map(|(idx, _)| *idx).unwrap_or(input.len());
        let key = input[key_start..key_end].to_string();
        if key.is_empty() {
            return Err(DataSourceError::ConnectionFailed(
                "Invalid PostgreSQL query string option".to_string(),
            ));
        }

        while let Some((_, ch)) = chars.peek().copied() {
            if ch.is_whitespace() {
                chars.next();
            } else {
                break;
            }
        }
        match chars.next() {
            Some((_, '=')) => {}
            _ => {
                return Err(DataSourceError::ConnectionFailed(format!(
                    "Invalid PostgreSQL query string option '{}': expected '='",
                    key
                )));
            }
        }
        while let Some((_, ch)) = chars.peek().copied() {
            if ch.is_whitespace() {
                chars.next();
            } else {
                break;
            }
        }

        let value = if matches!(chars.peek(), Some((_, '\''))) {
            chars.next();
            let mut value = String::new();
            let mut closed = false;
            while let Some((_, ch)) = chars.next() {
                match ch {
                    '\\' => {
                        if let Some((_, escaped)) = chars.next() {
                            value.push(escaped);
                        }
                    }
                    '\'' => {
                        closed = true;
                        break;
                    }
                    _ => value.push(ch),
                }
            }
            if !closed {
                return Err(DataSourceError::ConnectionFailed(format!(
                    "Invalid PostgreSQL query string option '{}': unterminated quoted value",
                    key
                )));
            }
            value
        } else {
            let value_start = chars.peek().map(|(idx, _)| *idx).unwrap_or(input.len());
            while let Some((_, ch)) = chars.peek().copied() {
                if ch.is_whitespace() {
                    break;
                }
                chars.next();
            }
            let value_end = chars.peek().map(|(idx, _)| *idx).unwrap_or(input.len());
            input[value_start..value_end].to_string()
        };

        options.push((key, value));
    }

    Ok(options)
}

fn format_postgres_options(options: &[(String, String)]) -> String {
    options
        .iter()
        .map(|(key, value)| format!("{}={}", key, quote_postgres_option_value(value)))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote_postgres_option_value(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|ch| !ch.is_whitespace() && ch != '\'' && ch != '\\')
    {
        return value.to_string();
    }

    let mut quoted = String::from("'");
    for ch in value.chars() {
        if matches!(ch, '\'' | '\\') {
            quoted.push('\\');
        }
        quoted.push(ch);
    }
    quoted.push('\'');
    quoted
}

fn apply_limit_if_missing(query: &str, limit: usize) -> String {
    let dialect = PostgreSqlDialect {};
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

fn table_kind(relkind: &str) -> TableKind {
    match relkind {
        "v" => TableKind::View,
        "m" => TableKind::MaterializedView,
        "f" => TableKind::ForeignTable,
        _ => TableKind::Table,
    }
}

fn row_to_strings_and_nulls(row: &Row) -> Result<(Vec<String>, Vec<bool>), DataSourceError> {
    let mut values = Vec::with_capacity(row.len());
    let mut nulls = Vec::with_capacity(row.len());
    for (ix, column) in row.columns().iter().enumerate() {
        let is_null = row
            .try_get::<_, Option<String>>(ix)
            .ok()
            .flatten()
            .is_none()
            && cell_is_null(row, ix, column.type_());
        values.push(cell_to_string(row, ix, column.type_())?);
        nulls.push(is_null);
    }
    Ok((values, nulls))
}

fn cell_is_null(row: &Row, ix: usize, ty: &Type) -> bool {
    if matches!(
        ty,
        &Type::VARCHAR | &Type::TEXT | &Type::BPCHAR | &Type::NAME
    ) {
        return row
            .try_get::<_, Option<String>>(ix)
            .unwrap_or(None)
            .is_none();
    }
    if matches!(ty, &Type::BOOL) {
        return row.try_get::<_, Option<bool>>(ix).unwrap_or(None).is_none();
    }
    if matches!(ty, &Type::INT2) {
        return row.try_get::<_, Option<i16>>(ix).unwrap_or(None).is_none();
    }
    if matches!(ty, &Type::INT4) {
        return row.try_get::<_, Option<i32>>(ix).unwrap_or(None).is_none();
    }
    if matches!(ty, &Type::INT8) {
        return row.try_get::<_, Option<i64>>(ix).unwrap_or(None).is_none();
    }
    if matches!(ty, &Type::OID) {
        return row.try_get::<_, Option<u32>>(ix).unwrap_or(None).is_none();
    }
    if matches!(ty, &Type::CHAR) {
        return row.try_get::<_, Option<i8>>(ix).unwrap_or(None).is_none();
    }
    if matches!(ty, &Type::NUMERIC) {
        return row
            .try_get::<_, Option<PgNumeric>>(ix)
            .unwrap_or(None)
            .is_none();
    }
    if matches!(ty, &Type::FLOAT4) {
        return row.try_get::<_, Option<f32>>(ix).unwrap_or(None).is_none();
    }
    if matches!(ty, &Type::FLOAT8) {
        return row.try_get::<_, Option<f64>>(ix).unwrap_or(None).is_none();
    }
    if matches!(ty, &Type::DATE) {
        return row
            .try_get::<_, Option<NaiveDate>>(ix)
            .unwrap_or(None)
            .is_none();
    }
    if matches!(ty, &Type::TIME) {
        return row
            .try_get::<_, Option<NaiveTime>>(ix)
            .unwrap_or(None)
            .is_none();
    }
    if matches!(ty, &Type::TIMESTAMP) {
        return row
            .try_get::<_, Option<NaiveDateTime>>(ix)
            .unwrap_or(None)
            .is_none();
    }
    if matches!(ty, &Type::TIMESTAMPTZ) {
        return row
            .try_get::<_, Option<DateTime<Local>>>(ix)
            .unwrap_or(None)
            .is_none();
    }
    if matches!(ty, &Type::UUID) {
        return row
            .try_get::<_, Option<uuid::Uuid>>(ix)
            .unwrap_or(None)
            .is_none();
    }
    if matches!(ty, &Type::JSON | &Type::JSONB) {
        return row
            .try_get::<_, Option<serde_json::Value>>(ix)
            .unwrap_or(None)
            .is_none();
    }
    if matches!(ty, &Type::BYTEA) {
        return row
            .try_get::<_, Option<Vec<u8>>>(ix)
            .unwrap_or(None)
            .is_none();
    }
    if matches!(ty, &Type::INET) {
        return row
            .try_get::<_, Option<IpAddr>>(ix)
            .unwrap_or(None)
            .is_none();
    }
    if let Ok(raw) = row.try_get::<_, Option<&[u8]>>(ix) {
        return raw.is_none();
    }
    false
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

    if let Ok(raw) = row.try_get::<_, Option<&[u8]>>(ix) {
        return Ok(raw
            .and_then(|b| std::str::from_utf8(b).ok())
            .unwrap_or_default()
            .to_string());
    }

    Ok(format!("<{}>", ty.name()))
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

fn postgres_update_statement(
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

    let table = format!(
        "{}.{}",
        quote_postgres_identifier(&batch.schema),
        quote_postgres_identifier(&batch.table)
    );
    let assignments = row
        .assignments
        .iter()
        .map(|value| {
            format!(
                "{} = {}",
                quote_postgres_identifier(&value.column),
                postgres_literal(value)
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
                quote_postgres_identifier(&value.column),
                postgres_literal(value)
            ),
            None => format!("{} IS NULL", quote_postgres_identifier(&value.column)),
        })
        .collect::<Vec<_>>()
        .join(" AND ");

    Ok(format!("UPDATE {table} SET {assignments} WHERE {keys}"))
}

fn quote_postgres_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn postgres_search_path_value(schema: &str) -> String {
    quote_postgres_identifier(schema)
}

fn postgres_literal(value: &TableEditValue) -> String {
    let Some(raw_value) = value.value.as_ref() else {
        return "NULL".to_string();
    };

    let data_type = value.data_type.to_ascii_lowercase();
    let numeric = [
        "int",
        "int2",
        "int4",
        "int8",
        "integer",
        "bigint",
        "smallint",
        "numeric",
        "decimal",
        "real",
        "double",
        "float",
        "serial",
        "bigserial",
    ];
    if numeric.iter().any(|prefix| data_type.starts_with(prefix))
        && raw_value.parse::<f64>().is_ok()
    {
        return raw_value.to_string();
    }
    if matches!(data_type.as_str(), "bool" | "boolean")
        && matches!(raw_value.to_ascii_lowercase().as_str(), "true" | "false")
    {
        return raw_value.to_ascii_uppercase();
    }

    format!("'{}'", raw_value.replace('\'', "''"))
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
        let mut msg = e.to_string();
        let mut source = e.source();
        while let Some(error) = source {
            let _ = write!(msg, "\nCaused by: {}", error);
            source = error.source();
        }
        msg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_limit_to_simple_select() {
        let result = apply_limit_if_missing("SELECT * FROM users", 1000);
        assert_eq!(result, "SELECT * FROM users LIMIT 1000");
    }

    #[test]
    fn adds_limit_before_trailing_semicolon() {
        let result = apply_limit_if_missing("SELECT * FROM users;", 1000);
        assert_eq!(result, "SELECT * FROM users LIMIT 1000;");
    }

    #[test]
    fn preserves_whitespace_after_trailing_semicolon() {
        let result = apply_limit_if_missing("SELECT * FROM users;\n", 1000);
        assert_eq!(result, "SELECT * FROM users LIMIT 1000;\n");
    }

    #[test]
    fn does_not_add_limit_when_already_present() {
        let result = apply_limit_if_missing("SELECT * FROM users LIMIT 50", 1000);
        assert_eq!(result, "SELECT * FROM users LIMIT 50");
    }

    #[test]
    fn does_not_add_limit_with_offset() {
        let result = apply_limit_if_missing("SELECT * FROM users LIMIT 10 OFFSET 5", 1000);
        assert_eq!(result, "SELECT * FROM users LIMIT 10 OFFSET 5");
    }

    #[test]
    fn adds_limit_with_order_by() {
        let result = apply_limit_if_missing("SELECT * FROM users ORDER BY id", 1000);
        assert!(result.contains("LIMIT 1000"));
    }

    #[test]
    fn adds_limit_to_cte_query() {
        let result =
            apply_limit_if_missing("WITH cte AS (SELECT * FROM posts) SELECT * FROM cte", 1000);
        assert!(result.contains("LIMIT 1000"));
    }

    #[test]
    fn does_not_count_inner_limit_in_subquery() {
        let result =
            apply_limit_if_missing("SELECT * FROM (SELECT * FROM posts LIMIT 5) AS sub", 1000);
        assert!(result.contains("LIMIT 1000"));
    }

    #[test]
    fn returns_original_on_parse_failure() {
        let result = apply_limit_if_missing("INVALID SQL QUERY {{{", 1000);
        assert_eq!(result, "INVALID SQL QUERY {{{");
    }

    #[test]
    fn returns_original_for_non_select_statement() {
        let result = apply_limit_if_missing("DELETE FROM users WHERE id = 1", 1000);
        assert_eq!(result, "DELETE FROM users WHERE id = 1");
    }

    #[test]
    fn extracts_sslrootcert_from_postgres_options() {
        let (options, ssl_root_cert) = postgres_driver_options(
            "sslmode=verify-full sslrootcert=/tmp/global-bundle.pem connect_timeout=5",
        )
        .unwrap();

        assert_eq!(ssl_root_cert.as_deref(), Some("/tmp/global-bundle.pem"));
        assert_eq!(options, "sslmode=require connect_timeout=5");
    }

    #[test]
    fn extracts_quoted_sslrootcert_from_postgres_options() {
        let (options, ssl_root_cert) = postgres_driver_options(
            "sslrootcert='/tmp/aws bundle.pem' application_name='sq\\'lab'",
        )
        .unwrap();

        assert_eq!(ssl_root_cert.as_deref(), Some("/tmp/aws bundle.pem"));
        assert_eq!(options, "application_name='sq\\'lab'");
    }

    #[test]
    fn reports_invalid_postgres_options() {
        let error = postgres_driver_options("sslrootcert").unwrap_err();

        assert!(error.to_string().contains("expected '='"));
    }

    #[test]
    fn detects_aws_rds_hosts() {
        assert!(is_aws_rds_host(
            "my-cluster.cluster-abc123.us-east-1.rds.amazonaws.com"
        ));
        assert!(is_aws_rds_host(
            "my-cluster.cluster-abc123.cn-north-1.rds.amazonaws.com.cn"
        ));
        assert!(is_aws_rds_host(
            "localhost,my-cluster.cluster-abc123.eu-west-1.rds.amazonaws.com"
        ));
        assert!(!is_aws_rds_host("localhost"));
        assert!(!is_aws_rds_host("postgres.example.com"));
    }

    #[test]
    fn renders_update_statement_for_table_edit_batch() {
        let batch = TableEditBatch {
            schema: "public".into(),
            table: "users".into(),
            rows: vec![sqlab_drivers_core::TableEditRow {
                keys: vec![TableEditValue {
                    column: "id".into(),
                    data_type: "int4".into(),
                    value: Some("1".into()),
                }],
                assignments: vec![
                    TableEditValue {
                        column: "name".into(),
                        data_type: "text".into(),
                        value: Some("Ada's".into()),
                    },
                    TableEditValue {
                        column: "enabled".into(),
                        data_type: "bool".into(),
                        value: Some("true".into()),
                    },
                    TableEditValue {
                        column: "deleted_at".into(),
                        data_type: "timestamp".into(),
                        value: None,
                    },
                ],
            }],
        };

        assert_eq!(
            postgres_update_statement(&batch, &batch.rows[0]).unwrap(),
            "UPDATE \"public\".\"users\" SET \"name\" = 'Ada''s', \"enabled\" = TRUE, \"deleted_at\" = NULL WHERE \"id\" = 1"
        );
    }
}
