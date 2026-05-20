use sqlab_drivers_core::{
    DataSource, DataSourceConfig, DataSourceError, Database,
    manager::{DataSourceFactory, create_data_source},
};
use sqlab_drivers_mysql::create_mysql_data_source;
use sqlab_drivers_postgres::create_postgres_data_source;
use sqlab_drivers_sqlite::create_sqlite_data_source;

pub fn create_configured_data_source(
    config: &DataSourceConfig,
) -> Result<Box<dyn DataSource>, DataSourceError> {
    create_data_source(factory_for(config.db_type), config)
}

fn factory_for(db_type: Database) -> DataSourceFactory {
    match db_type {
        Database::Postgres => create_postgres_data_source,
        Database::MySql => create_mysql_data_source,
        Database::SQLite => create_sqlite_data_source,
    }
}
