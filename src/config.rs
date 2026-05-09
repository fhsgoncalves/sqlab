use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::credentials;
use crate::data_source::DataSourceConfig;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub data_sources: Vec<DataSourceConfig>,
    #[serde(skip)]
    pub credential_error: Option<String>,
}

impl Config {
    pub fn path() -> PathBuf {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".zql")
            .join("config.toml")
    }

    pub fn load() -> anyhow::Result<Self> {
        let path = Self::path();
        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            let mut config: Self = toml::from_str(&content)?;
            config.load_passwords();
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
        self.save_passwords()?;
        std::fs::write(&path, toml::to_string_pretty(self)?)?;
        Ok(())
    }

    fn load_passwords(&mut self) {
        match credentials::load_passwords() {
            Ok(passwords) => {
                for data_source in &mut self.data_sources {
                    if let Some(password) = passwords.get(&data_source.name) {
                        data_source.password = password.clone();
                    }
                }
            }
            Err(error) => {
                eprintln!("failed to load passwords from keychain: {error}");
                self.credential_error = Some(credentials::recovery_error_message(&error));
            }
        }
    }

    fn save_passwords(&self) -> anyhow::Result<()> {
        credentials::save_passwords_from_configs(
            self.data_sources
                .iter()
                .map(|data_source| (data_source.name.as_str(), data_source.password.as_str())),
        )
        .map_err(|error| anyhow::anyhow!(credentials::saving_error_message(&error)))?;

        Ok(())
    }
}
