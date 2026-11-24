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

/// ~/.local/share/murmur — secrets, roster.
pub(crate) fn data_path(name: &str) -> PathBuf {
    data_dir().join(name)
}

/// ~/.cache/murmur — artwork and other transient payloads.
pub(crate) fn cache_dir() -> PathBuf {
    xdg_dir("XDG_CACHE_HOME", ".cache").join("murmur")
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    /// What kind of family this device belongs to — chosen at family
    /// creation, carried by invites. Explicit, never inferred from
    /// `server_url`: a serverless family simply has no server field.
    #[serde(default)]
    pub family_kind: murmur_proto::FamilyKind,
    /// wss://host[:port]/murmur — unset in a serverless family.
    #[serde(default)]
    pub server_url: String,
    /// Human name shown in device lists ("thinkpad", "desktop").
    pub name: String,
    /// Seconds without input before this node reports `idle`. ~180
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
    /// Phone app → local twin processes. When a listed process is alive
    /// the phone's notification from that app is a duplicate of what
    /// the desktop client already shows — drop it.
    #[serde(default = "default_notify_suppress")]
    pub notify_suppress_if_running: std::collections::HashMap<String, Vec<String>>,
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

fn default_notify_suppress() -> std::collections::HashMap<String, Vec<String>> {
    // Telegram: phone mirroring doubles Telegram Desktop's own popups.
    let tg = || {
        vec![
            "telegram-desktop".to_string(),
            "Telegram".to_string(),
            "kotatogram-desktop".to_string(),
        ]
    };
    [
        "org.telegram.messenger",
        "org.telegram.messenger.web",
        "org.telegram.plus",
        "tw.nekomimi.nekogram",
        "org.thunderdog.challegram",
    ]
    .iter()
    .map(|pkg| (pkg.to_string(), tg()))
    .collect()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            family_kind: murmur_proto::FamilyKind::Serverful,
            server_url: String::new(),
            name: String::new(),
            idle_timeout_secs: default_idle(),
            clipboard_mime_deny: default_mime_deny(),
            lock_procs: default_lock_procs(),
            notify_suppress_if_running: default_notify_suppress(),
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
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Secrets {
    pub device_id: String,
    pub auth_token_hex: String,
    pub family_key_hex: String,
    /// Rotation epoch of `family_key_hex` (0 = never rotated).
    #[serde(default)]
    pub key_epoch: u64,
    /// Device that set the current epoch — tie-break for two laptops
    /// rotating at once (smaller id wins).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_rotator: Option<String>,
    /// A few previous family keys, newest first — only to recognise a
    /// member still on an old key and re-key it. Never used to accept
    /// messages.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub old_keys_hex: Vec<String>,
    /// Members excluded by past rotations — passed along with every
    /// re-key so late joiners drop them too, never re-pinned.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub revoked: Vec<String>,
    /// Long-term X25519 identity secret (generated on first start).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_hex: Option<String>,
    /// Display name of the family — founder-set via `init --family`.
    /// Serverful members mirror the relay's name over it on connect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family_name: Option<String>,
}

impl Secrets {
    pub fn load() -> anyhow::Result<Self> {
        let path = data_dir().join("secrets.toml");
        let s = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("read {}: {e}", path.display()))?;
        Ok(toml::from_str(&s)?)
    }

    /// Rewrite secrets.toml in place (atomic rename, 0600).
    pub fn save(&self) -> anyhow::Result<()> {
        let dir = data_dir();
        let tmp = dir.join("secrets.toml.tmp");
        let _ = std::fs::remove_file(&tmp);
        write_new(&tmp, &toml::to_string_pretty(self)?, 0o600)?;
        std::fs::rename(&tmp, dir.join("secrets.toml"))?;
        Ok(())
    }

    pub fn device_id(&self) -> anyhow::Result<DeviceId> {
        Ok(self.device_id.parse()?)
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
/// `murmurs add-device`. Writes config.toml + secrets.toml (0600) and
/// generates the family key locally — it never touches the server.
pub fn init(args: &[String]) -> anyhow::Result<()> {
    fn flag<'a>(args: &'a [String], name: &str) -> anyhow::Result<&'a str> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing {name}"))
    }
    let kind = match args.iter().position(|a| a == "--kind") {
        Some(i) => match args.get(i + 1).map(String::as_str) {
            Some("serverful") => murmur_proto::FamilyKind::Serverful,
            Some("serverless") => murmur_proto::FamilyKind::Serverless,
            _ => anyhow::bail!("--kind must be serverful|serverless"),
        },
        None => murmur_proto::FamilyKind::Serverful,
    };
    // The relay URL exists only in a serverful family's world.
    let server_url = match kind {
        murmur_proto::FamilyKind::Serverful => flag(args, "--server")?,
        murmur_proto::FamilyKind::Serverless => args
            .iter()
            .position(|a| a == "--server")
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
            .unwrap_or(""),
    };
    let name = flag(args, "--name")
        .map(str::to_owned)
        .map_or_else(
            |_| prompt("device name", std::env::var("HOSTNAME").ok().as_deref()),
            Ok,
        )?;
    // Credentials are relay-issued in a serverful family; a serverless
    // founder mints them locally (the token is only ever sent to a
    // relay, and there is none — the family key authenticates links).
    let (device_id, token_hex) = match kind {
        murmur_proto::FamilyKind::Serverful => (
            flag(args, "--device-id")?.to_owned(),
            flag(args, "--token")?.to_owned(),
        ),
        murmur_proto::FamilyKind::Serverless => (
            DeviceId::random().to_string(),
            AuthToken::random().to_hex(),
        ),
    };

    // Fail early on typos — better here than at first connect.
    let id: DeviceId = device_id.parse()?;
    AuthToken::from_hex(&token_hex)?;

    let cfg = Config {
        family_kind: kind,
        server_url: server_url.into(),
        name: name.into(),
        ..Config::default()
    };
    let secrets = Secrets {
        device_id: id.to_string(),
        auth_token_hex: token_hex.into(),
        family_key_hex: hex::encode(FamilyKey::random().0),
        family_name: args
            .iter()
            .position(|a| a == "--family")
            .and_then(|i| args.get(i + 1))
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty()),
        ..Secrets::default()
    };
    provision(&cfg, &secrets)?;
    eprintln!("device {id} provisioned");
    if let Err(e) = crate::install::install_user_unit() {
        eprintln!("(unit install skipped: {e:#} — start murmurd manually)");
    }
    Ok(())
}

/// Interactive fallback for `--name`-style flags: asks on a TTY, prefilled
/// default shown in brackets and accepted on empty input.
fn prompt(label: &str, default: Option<&str>) -> anyhow::Result<String> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        anyhow::bail!("missing flag — pass it explicitly or run on a TTY");
    }
    match default {
        Some(d) => eprint!("{label} [{d}]: "),
        None => eprint!("{label}: "),
    }
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let line = line.trim();
    if !line.is_empty() {
        return Ok(line.into());
    }
    default.map(str::to_owned).ok_or_else(|| anyhow::anyhow!("{label} required"))
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
    eprintln!("  │    wrote {}", cfg_dir.join("config.toml").display());
    eprintln!(
        "  │    wrote {} (mode 0600)",
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
