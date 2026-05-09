use std::collections::HashMap;

use crate::config::Config;
use crate::data_source::postgres::PostgresDataSource;
use crate::data_source::{ConnectionStatus, DataSource, DataSourceConfig, DataSourceError};
use crate::schema_cache::{self, cache_key};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntrospectionStatus {
    Idle,
    Running,
    Cached,
    Failed,
}

pub struct DataSourceManager {
    configs: Vec<DataSourceConfig>,
    active_name: Option<String>,
    statuses: HashMap<String, ConnectionStatus>,
    introspection_statuses: HashMap<String, IntrospectionStatus>,
    last_errors: HashMap<String, String>,
}

impl DataSourceManager {
    pub fn load() -> anyhow::Result<Self> {
        let config = Config::load()?;
        Ok(Self {
            configs: config.data_sources,
            active_name: None,
            statuses: HashMap::new(),
            introspection_statuses: HashMap::new(),
            last_errors: HashMap::new(),
        })
    }

    pub fn empty() -> Self {
        Self {
            configs: Vec::new(),
            active_name: None,
            statuses: HashMap::new(),
            introspection_statuses: HashMap::new(),
            last_errors: HashMap::new(),
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        Config {
            data_sources: self.configs.clone(),
        }
        .save()
    }

    pub fn configs(&self) -> &[DataSourceConfig] {
        &self.configs
    }

    pub fn active_name(&self) -> Option<&str> {
        self.active_name.as_deref()
    }

    pub fn active_config(&self) -> Option<&DataSourceConfig> {
        let active_name = self.active_name.as_deref()?;
        self.configs
            .iter()
            .find(|config| config.name == active_name)
    }

    pub fn set_active(&mut self, name: Option<String>) {
        self.active_name =
            name.filter(|name| self.configs.iter().any(|config| config.name == *name));
    }

    pub fn status(&self, name: &str) -> ConnectionStatus {
        self.statuses
            .get(name)
            .copied()
            .unwrap_or(ConnectionStatus::Idle)
    }

    pub fn set_status(&mut self, name: &str, status: ConnectionStatus) {
        self.statuses.insert(name.to_string(), status);
    }

    pub fn last_error(&self, name: &str) -> Option<&str> {
        self.last_errors.get(name).map(|s| s.as_str())
    }

    pub fn set_last_error(&mut self, name: &str, error: String) {
        self.last_errors.insert(name.to_string(), error);
    }

    pub fn clear_last_error(&mut self, name: &str) {
        self.last_errors.remove(name);
    }

    pub fn introspection_status(&self, name: &str) -> IntrospectionStatus {
        self.introspection_statuses
            .get(name)
            .copied()
            .unwrap_or(IntrospectionStatus::Idle)
    }

    pub fn set_introspection_status(&mut self, name: &str, status: IntrospectionStatus) {
        self.introspection_statuses.insert(name.to_string(), status);
    }

    pub fn add(&mut self, config: DataSourceConfig) {
        self.remove(&config.name);
        self.configs.push(config);
    }

    pub fn update(&mut self, old_name: &str, config: DataSourceConfig) {
        let was_active = self.active_name.as_deref() == Some(old_name);
        let old_status = self.status(old_name);

        self.remove(old_name);
        self.add(config.clone());

        if was_active {
            self.active_name = Some(config.name.clone());
            self.set_status(&config.name, old_status);
        }
    }

    pub fn remove(&mut self, name: &str) {
        if let Some(config) = self.configs.iter().find(|c| c.name == name) {
            let key = cache_key(config);
            if let Err(e) = schema_cache::clear(&key) {
                eprintln!("failed to clear schema cache for {}: {}", name, e);
            }
        }
        self.configs.retain(|config| config.name != name);
        self.statuses.remove(name);
        self.introspection_statuses.remove(name);
        self.last_errors.remove(name);
        if self.active_name.as_deref() == Some(name) {
            self.active_name = None;
        }
    }

    pub fn duplicate(&mut self, name: &str) {
        let Some(config) = self
            .configs
            .iter()
            .find(|config| config.name == name)
            .cloned()
        else {
            return;
        };
        let mut copy = config;
        copy.name = self.unique_name(&format!("{} copy", copy.name));
        self.configs.push(copy);
    }

    fn unique_name(&self, base: &str) -> String {
        if !self.configs.iter().any(|config| config.name == base) {
            return base.to_string();
        }
        for ix in 2.. {
            let candidate = format!("{} {}", base, ix);
            if !self.configs.iter().any(|config| config.name == candidate) {
                return candidate;
            }
        }
        unreachable!()
    }
}

pub fn create_data_source(
    config: &DataSourceConfig,
) -> Result<Box<dyn DataSource>, DataSourceError> {
    match config.db_type.as_str() {
        "postgres" => Ok(Box::new(PostgresDataSource::new(config.clone())?)),
        other => Err(DataSourceError::ConnectionFailed(format!(
            "Unsupported database type: {}",
            other
        ))),
    }
}
