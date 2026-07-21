use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub window: WindowConfig,
    pub terminal: TerminalConfig,
    pub theme: ThemeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowConfig {
    pub title: String,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalConfig {
    pub font_size: f32,
    pub scrollback: usize,
    pub shell: Option<String>,
    pub working_directory: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeConfig {
    pub background: [u8; 3],
    pub foreground: [u8; 3],
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            window: WindowConfig {
                title: "Lumi-Term".to_string(),
                width: 1280.0,
                height: 760.0,
            },
            terminal: TerminalConfig {
                font_size: 16.0,
                scrollback: 10_000,
                shell: None,
                working_directory: Some(PathBuf::from("/")),
            },
            theme: ThemeConfig {
                background: [17, 17, 17],
                foreground: [233, 233, 233],
            },
        }
    }
}

impl AppConfig {
    pub fn load_or_create() -> Result<Self> {
        let config_path = Self::path()?;
        if config_path.exists() {
            let raw = fs::read_to_string(&config_path)
                .with_context(|| format!("reading {}", config_path.display()))?;
            let config = toml::from_str::<Self>(&raw)
                .with_context(|| format!("parsing {}", config_path.display()))?;
            Ok(config)
        } else {
            let config = Self::default();
            config.write_to_disk()?;
            Ok(config)
        }
    }

    pub fn write_to_disk(&self) -> Result<()> {
        let config_path = Self::path()?;
        let parent = config_path
            .parent()
            .context("unable to determine config directory")?;
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        let serialized =
            toml::to_string_pretty(self).context("serializing Lumi-Term config to TOML")?;
        fs::write(&config_path, serialized)
            .with_context(|| format!("writing {}", config_path.display()))?;
        Ok(())
    }

    pub fn path() -> Result<PathBuf> {
        let project_dirs = ProjectDirs::from("com", "lumi", "lumi-term")
            .context("unable to derive config path from platform")?;
        Ok(project_dirs.config_dir().join("lumi-term.toml"))
    }
}
