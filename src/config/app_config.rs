use gpui::{Pixels, px};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Default dark theme name.
pub const DEFAULT_DARK_THEME: &str = "Kibble Dark";
/// Default light theme name.
pub const DEFAULT_LIGHT_THEME: &str = "Kibble Light";

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum FontSizePreset {
    Small,
    #[default]
    Medium,
    Large,
}

impl FontSizePreset {
    pub fn to_px(self) -> Pixels {
        match self {
            FontSizePreset::Small => px(14.),
            FontSizePreset::Medium => px(16.),
            FontSizePreset::Large => px(18.),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub font_size: FontSizePreset,
    #[serde(default)]
    pub remote_control: RemoteControlConfig,
    #[serde(default)]
    pub theme: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteControlConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_bind_port")]
    pub bind_port: u16,
    #[serde(default)]
    pub bearer_token: Option<String>,
    #[serde(default)]
    pub cloudflared: CloudflaredConfig,
}

impl Default for RemoteControlConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind_port: default_bind_port(),
            bearer_token: None,
            cloudflared: CloudflaredConfig::default(),
        }
    }
}

fn default_bind_port() -> u16 {
    9876
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CloudflaredConfig {
    #[serde(default = "default_cloudflared_command")]
    pub command: String,
    #[serde(default)]
    pub tunnel_token: Option<String>,
    #[serde(default)]
    pub hostname: Option<String>,
    #[serde(default)]
    pub bearer_token: Option<String>,
}

impl Default for CloudflaredConfig {
    fn default() -> Self {
        Self {
            command: default_cloudflared_command(),
            tunnel_token: None,
            hostname: None,
            bearer_token: None,
        }
    }
}

fn default_cloudflared_command() -> String {
    "cloudflared".to_string()
}

impl AppConfig {
    pub fn config_path() -> PathBuf {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("mini-pi");
        std::fs::create_dir_all(&config_dir).ok();
        config_dir.join("config.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if path.exists() {
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            Self::default()
        }
    }

    pub fn save(&self) -> Result<(), std::io::Error> {
        let path = Self::config_path();
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(path, content)
    }
}
