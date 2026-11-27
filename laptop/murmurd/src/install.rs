//! User-unit install: after `init`/`pair` provisioning, murmurd registers
//! itself with the session's systemd — `graphical-session.target` so it
//! only lives inside a Wayland session (where its adapters can work).
//! Non-fatal: provisioning must succeed even on exotic setups.

use anyhow::Context;
use std::path::PathBuf;

/// Write `~/.config/systemd/user/murmurd.service` and enable it (not
/// `--now`: the caller may be provisioning outside a graphical session).
pub fn install_user_unit() -> anyhow::Result<()> {
    let exe = std::env::current_exe()?.display().to_string();
    let unit = format!(
        "[Unit]\n\
         Description=murmur laptop daemon\n\
         PartOf=graphical-session.target\n\
         After=graphical-session.target\n\
         # installed but unpaired → skip quietly instead of crash-looping\n\
         # on 'no config' every RestartSec\n\
         ConditionPathExists=%h/.config/murmur/config.toml\n\n\
         [Service]\n\
         ExecStart={exe} run\n\
         Restart=on-failure\n\
         RestartSec=5\n\n\
         [Install]\n\
         WantedBy=graphical-session.target\n"
    );
    // systemd looks in $XDG_CONFIG_HOME/systemd/user first — honor it.
    let dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .context("no XDG_CONFIG_HOME/HOME")?
        .join("systemd/user");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("murmurd.service");
    std::fs::write(&path, &unit)?;
    let _ = run("systemctl", &["--user", "daemon-reload"]);
    run("systemctl", &["--user", "enable", "murmurd.service"])
        .context("systemctl --user enable")?;
    eprintln!("  › unit installed — starts with the graphical session:");
    eprintln!("      systemctl --user start murmurd");
    Ok(())
}

fn run(cmd: &str, args: &[&str]) -> anyhow::Result<()> {
    let status = std::process::Command::new(cmd)
        .args(args)
        .status()
        .with_context(|| format!("spawn {cmd}"))?;
    anyhow::ensure!(status.success(), "{cmd} {} exited {status}", args.join(" "));
    Ok(())
}
