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
    /// Process names that mean "the session is locked". Needed because
    /// wlroots lockers (hyprlock, swaylock) don't expose lock state to
    /// observers — presence watches for the process itself.
    #[serde(default = "default_lock_procs")]
    pub lock_procs: Vec<String>,
}

fn default_idle() -> u64 {
    180
}

fn default_mime_deny() -> Vec<String> {
    vec!["x-kde-passwordManagerHint".into()]
}

fn default_lock_procs() -> Vec<String> {
    [
        "hyprlock", "swaylock", "gtklock", "waylock", "i3lock", "slock",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server_url: String::new(),
            name: String::new(),
            idle_timeout_secs: default_idle(),
            clipboard_mime_deny: default_mime_deny(),
            lock_procs: default_lock_procs(),
        }
    }
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

/// `murmurd init` — provision this laptop from credentials printed by
/// `murmur-server add-device`. Writes config.toml + secrets.toml (0600) and
/// generates the family key locally — it never touches the server.
pub fn init(args: &[String]) -> anyhow::Result<()> {
    fn flag<'a>(args: &'a [String], name: &str) -> anyhow::Result<&'a str> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing {name}"))
    }
    let server_url = flag(args, "--server")?;
    let name = flag(args, "--name")?;
    let device_id = flag(args, "--device-id")?;
    let token_hex = flag(args, "--token")?;

    // Fail early on typos — better here than at first connect.
    let id: DeviceId = device_id.parse()?;
    AuthToken::from_hex(token_hex)?;

    let cfg = Config {
        server_url: server_url.into(),
        name: name.into(),
        ..Config::default()
    };
    let secrets = Secrets {
        device_id: id.to_string(),
        auth_token_hex: token_hex.into(),
        family_key_hex: hex::encode(FamilyKey::random().0),
    };
    provision(&cfg, &secrets)?;
    eprintln!("device {id} provisioned — start murmurd");
    Ok(())
}

/// Write config.toml + secrets.toml (0600, never overwritten). Shared by
/// `init` and the pair joiner.
pub fn provision(cfg: &Config, secrets: &Secrets) -> anyhow::Result<()> {
    let cfg_dir = config_dir();
    let data_dir = data_dir();
    std::fs::create_dir_all(&cfg_dir)?;
    std::fs::create_dir_all(&data_dir)?;

    write_new(
        &cfg_dir.join("config.toml"),
        &toml::to_string_pretty(cfg)?,
        0o600,
    )?;
    write_new(
        &data_dir.join("secrets.toml"),
        &toml::to_string_pretty(secrets)?,
        0o600,
    )?;
    eprintln!("wrote {}", cfg_dir.join("config.toml").display());
    eprintln!(
        "wrote {} (mode 0600)",
        data_dir.join("secrets.toml").display()
    );
    Ok(())
}

#[cfg(unix)]
fn write_new(path: &std::path::Path, contents: &str, mode: u32) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)
        .map_err(|e| anyhow::anyhow!("create {}: {e} (already exists?)", path.display()))?;
    f.write_all(contents.as_bytes())?;
    Ok(())
}

#[cfg(not(unix))]
fn write_new(path: &std::path::Path, contents: &str, _mode: u32) -> anyhow::Result<()> {
    std::fs::write(path, contents).map_err(|e| anyhow::anyhow!("write {}: {e}", path.display()))
}
