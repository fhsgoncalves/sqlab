use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct AppSettings {
    pub theme: Option<String>,
    #[serde(default)]
    pub keymap: BTreeMap<String, String>,
    #[serde(default)]
    pub keymap_context: BTreeMap<String, String>,
}

impl AppSettings {
    pub fn load() -> Self {
        let Ok(content) = std::fs::read_to_string(settings_path()) else {
            return Self::default();
        };

        match toml::from_str(&content) {
            Ok(settings) => settings,
            Err(error) => {
                eprintln!("failed to parse settings.toml: {}", error);
                Self::default()
            }
        }
    }

    pub fn save(&self) {
        if let Err(error) = save_settings(self) {
            eprintln!("failed to save settings.toml: {}", error);
        }
    }

    pub fn keymap_value(&self, action_name: &str) -> Option<String> {
        self.keymap
            .get(action_name)
            .map(|value| setting_keystroke_to_gpui(value))
    }

    pub fn set_keymap_value(&mut self, action_name: &str, keystroke: &str) {
        self.keymap.insert(
            action_name.to_string(),
            gpui_keystroke_to_setting(keystroke),
        );
    }

    pub fn context_value(&self, action_name: &str) -> Option<String> {
        self.keymap_context.get(action_name).cloned()
    }
}

pub fn app_data_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".sqlab")
}

pub fn settings_path() -> PathBuf {
    app_data_dir().join("settings.toml")
}

pub fn setting_keystroke_to_gpui(value: &str) -> String {
    value.trim().replace('+', "-")
}

pub fn gpui_keystroke_to_setting(value: &str) -> String {
    value.trim().replace('-', "+")
}

fn save_settings(settings: &AppSettings) -> Result<(), SaveSettingsError> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(settings)?;
    std::fs::write(path, content)?;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
enum SaveSettingsError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("toml error: {0}")]
    Toml(#[from] toml::ser::Error),
}

pub fn display_path(path: &Path) -> String {
    path.to_string_lossy().to_string()
}
