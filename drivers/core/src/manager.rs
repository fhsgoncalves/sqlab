use std::collections::HashMap;

use crate::{ConnectionStatus, DataSource, DataSourceConfig, DataSourceError};

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
    credential_errors: HashMap<String, String>,
    global_credential_error: Option<String>,
}

impl DataSourceManager {
    pub fn load() -> anyhow::Result<Self> {
        let config = crate::manager::Config::load()?;
        Ok(Self {
            configs: config.data_sources,
            active_name: None,
            statuses: HashMap::new(),
            introspection_statuses: HashMap::new(),
            last_errors: HashMap::new(),
            credential_errors: HashMap::new(),
            global_credential_error: None,
        })
    }

    pub fn empty() -> Self {
        Self {
            configs: Vec::new(),
            active_name: None,
            statuses: HashMap::new(),
            introspection_statuses: HashMap::new(),
            last_errors: HashMap::new(),
            credential_errors: HashMap::new(),
            global_credential_error: None,
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        crate::manager::Config {
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

    pub fn credential_error(&self, name: &str) -> Option<&str> {
        self.credential_errors.get(name).map(|s| s.as_str())
    }

    pub fn set_credential_error(&mut self, name: &str, error: String) {
        self.credential_errors.insert(name.to_string(), error);
    }

    pub fn clear_credential_error(&mut self, name: &str) {
        self.credential_errors.remove(name);
    }

    pub fn global_credential_error(&self) -> Option<&str> {
        self.global_credential_error.as_deref()
    }

    pub fn clear_global_credential_error(&mut self) {
        self.global_credential_error = None;
    }

    pub fn ensure_password_loaded(
        &mut self,
        name: &str,
        load_password: impl Fn(&str) -> Result<Option<String>, String>,
    ) -> Result<(), String> {
        let Some(config) = self.configs.iter_mut().find(|config| config.name == name) else {
            return Ok(());
        };

        if !config.password.is_empty() {
            self.clear_credential_error(name);
            return Ok(());
        }

        match load_password(name) {
            Ok(Some(password)) => {
                config.password = password;
                self.clear_credential_error(name);
                self.clear_global_credential_error();
                Ok(())
            }
            Ok(None) => {
                self.clear_credential_error(name);
                self.clear_global_credential_error();
                Ok(())
            }
            Err(error) => {
                self.set_credential_error(name, error.clone());
                Err(error)
            }
        }
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
        self.remove_internal(&config.name);
        self.configs.push(config);
    }

    pub fn update(&mut self, old_name: &str, config: DataSourceConfig) {
        let was_active = self.active_name.as_deref() == Some(old_name);
        let old_status = self.status(old_name);

        self.remove_internal(old_name);
        self.add(config.clone());

        if was_active {
            self.active_name = Some(config.name.clone());
            self.set_status(&config.name, old_status);
        }
    }

    fn remove_internal(&mut self, name: &str) {
        self.configs.retain(|config| config.name != name);
        self.statuses.remove(name);
        self.introspection_statuses.remove(name);
        self.last_errors.remove(name);
        self.credential_errors.remove(name);
        if self.active_name.as_deref() == Some(name) {
            self.active_name = None;
        }
    }

    pub fn remove(
        &mut self,
        name: &str,
        cache_key: impl Fn(&DataSourceConfig) -> String,
        clear_cache: impl Fn(&str) -> Result<(), String>,
        delete_password: impl Fn(&str) -> Result<(), String>,
    ) {
        if let Some(config) = self.configs.iter().find(|c| c.name == name) {
            let key = cache_key(config);
            if let Err(e) = clear_cache(&key) {
                eprintln!("failed to clear schema cache for {}: {}", name, e);
            }
        }
        self.remove_internal(name);
        if let Err(e) = delete_password(name) {
            eprintln!("failed to delete keychain password for {}: {}", name, e);
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

/// Type alias for the factory function that creates a DataSource
pub type DataSourceFactory = fn(&DataSourceConfig) -> Result<Box<dyn DataSource>, DataSourceError>;

/// Creates a data source using the provided factory function
pub fn create_data_source(
    factory: DataSourceFactory,
    config: &DataSourceConfig,
) -> Result<Box<dyn DataSource>, DataSourceError> {
    factory(config)
}

// Re-export Config for manager operations
mod config {
    use serde::{Deserialize, Serialize};
    use std::path::PathBuf;

    use crate::DataSourceConfig;

    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    pub struct Config {
        #[serde(default)]
        pub data_sources: Vec<DataSourceConfig>,
    }

    impl Config {
        pub fn path() -> PathBuf {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".sqlab")
                .join("config.toml")
        }

        pub fn load() -> anyhow::Result<Self> {
            let path = Self::path();
            if path.exists() {
                let content = std::fs::read_to_string(&path)?;
                let config: Self = toml::from_str(&content)?;
                Ok(config)
            } else {
                Ok(Self::default())
            }
        }

        pub fn save(&self) -> anyhow::Result<()> {
            let path = Self::path();
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, toml::to_string_pretty(self)?)?;
            Ok(())
        }
    }
}

pub use config::Config;
