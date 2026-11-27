use std::{fs, path::PathBuf};
use serde::{Deserialize, Serialize};
use crate::catalog::{Result, xdg};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Theme {
    System,
    Dark,
    Light,
}

impl Default for Theme {
    fn default() -> Self {
        Theme::System
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub install_dir: Option<String>,
    pub auto_check: bool,
    pub auto_install: bool,
    pub notify_updates: bool,
    pub include_prereleases: bool,
    pub theme: Theme,
    pub autostart: bool,
    pub language: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            install_dir: None,
            auto_check: true,
            auto_install: false,
            notify_updates: true,
            include_prereleases: false,
            theme: Theme::System,
            autostart: false,
            language: "Auto".into(),
        }
    }
}

impl Settings {
    fn file() -> Result<PathBuf> {
        Ok(xdg("XDG_CONFIG_HOME", ".config")?.join("suppaworld/launcher/settings.json"))
    }

    pub fn load() -> Settings {
        Self::file()
            .ok()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::file()?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn install_root(&self) -> PathBuf {
        if let Some(dir) = self.install_dir.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            PathBuf::from(dir).join("apps")
        } else {
            xdg("XDG_DATA_HOME", ".local/share")
                .map(|p| p.join("suppaworld/apps"))
                .unwrap_or_else(|_| PathBuf::from("suppaworld/apps"))
        }
    }
}