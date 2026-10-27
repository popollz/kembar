use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::select::KeepStrategy;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    /// Which copy Smart Selection keeps.
    pub keep: KeepStrategy,
    /// Ignore files below this size (bytes).
    pub min_size: u64,
    /// Extra excludes, czkawka-style globs (e.g. `*/.build/*`).
    pub excluded: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            keep: KeepStrategy::Oldest,
            min_size: 1,
            excluded: Vec::new(),
        }
    }
}

impl Config {
    pub fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|dir| dir.join("kembar").join("config.toml"))
    }

    /// Missing or broken config falls back to defaults rather than blocking startup.
    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        toml::from_str(&text).unwrap_or_default()
    }

    pub fn write_example(&self) -> std::io::Result<PathBuf> {
        let path = Self::path().unwrap_or_else(|| PathBuf::from("config.toml"));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if !path.exists() {
            let mut copy = self.clone();
            copy.excluded = vec!["*/.build/*".to_string(), "*/.gradle/*".to_string()];
            std::fs::write(&path, toml::to_string_pretty(&copy).unwrap_or_default())?;
        }
        Ok(path)
    }
}
