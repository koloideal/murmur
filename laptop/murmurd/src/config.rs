use murmur_proto::{AuthToken, DeviceId, FamilyKey};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn xdg_dir(var: &str, home_fallback: &str) -> PathBuf {
    if let Some(p) = std::env::var_os(var).filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(home_fallback)
}

fn config_dir() -> PathBuf {
    xdg_dir("XDG_CONFIG_HOME", ".config").join("murmur")
}

fn data_dir() -> PathBuf {
    xdg_dir("XDG_DATA_HOME", ".local/share").join("murmur")
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    /// wss://host[:port]/murmur
    pub server_url: String,
    /// Human name shown in device lists ("thinkpad", "desktop").
    pub name: String,
    /// Minutes without input before this node reports `idle`. ~3
    #[serde(default = "default_idle")]
    pub idle_timeout_secs: u64,
    /// Clipboard offers carrying these MIME types are never synced
    /// (password-manager markers like x-kde-passwordManagerHint).
    #[serde(default = "default_mime_deny")]
    pub clipboard_mime_deny: Vec<String>,
}

fn default_idle() -> u64 {
    180
}

fn default_mime_deny() -> Vec<String> {
    vec!["x-kde-passwordManagerHint".into()]
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let path = config_dir().join("config.toml");
        let s = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("read {}: {e}", path.display()))?;
        Ok(toml::from_str(&s)?)
    }
}

/// Provisioning material from pairing. Stored 0600.
#[derive(Debug, Serialize, Deserialize)]
pub struct Secrets {
    pub device_id: String,
    pub auth_token_hex: String,
    pub family_key_hex: String,
}

impl Secrets {
    pub fn load() -> anyhow::Result<Self> {
        let path = data_dir().join("secrets.toml");
        let s = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("read {}: {e}", path.display()))?;
        Ok(toml::from_str(&s)?)
    }

    pub fn device_id(&self) -> anyhow::Result<DeviceId> {
        Ok(self.device_id.parse()?)
    }

    pub fn auth_token(&self) -> anyhow::Result<AuthToken> {
        AuthToken::from_hex(&self.auth_token_hex)
    }

    pub fn family_key(&self) -> anyhow::Result<FamilyKey> {
        let b = hex::decode(&self.family_key_hex)?;
        let arr: [u8; 32] = b
            .try_into()
            .map_err(|_| anyhow::anyhow!("family key must be 32 bytes"))?;
        Ok(FamilyKey(arr))
    }
}
