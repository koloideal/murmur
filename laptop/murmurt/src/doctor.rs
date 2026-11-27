//! Dependency check for the waybar island — one command answers
//! "why doesn't it look like the screenshot".

use anyhow::Result;
use std::process::Command;

fn have(bin: &str) -> bool {
    Command::new("which")
        .arg(bin)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn python_module(modname: &str) -> bool {
    Command::new("python3")
        .args(["-c", &format!("import {modname}")])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn gtk4() -> bool {
    Command::new("python3")
        .args(["-c", "import gi; gi.require_version('Gtk','4.0'); from gi.repository import Gtk"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn font_installed(name: &str) -> bool {
    Command::new("fc-list")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_lowercase().contains(&name.to_lowercase()))
        .unwrap_or(false)
}

fn distro() -> String {
    if let Ok(osr) = std::fs::read_to_string("/etc/os-release") {
        for line in osr.lines() {
            if let Some(id) = line.strip_prefix("ID=") {
                return id.trim_matches('"').to_string();
            }
        }
    }
    "unknown".into()
}

/// Package install hint for the missing items, per distro family.
fn install_hint(distro: &str, missing: &[&str]) -> Option<String> {
    // rough mapping; names follow arch since that's the author's distro
    let mut pkgs: Vec<&str> = Vec::new();
    for &m in missing {
        match m {
            "waybar" => pkgs.push("waybar"),
            "hyprctl" => pkgs.push("hyprland"),
            "python3" => pkgs.push("python"),
            "grim" => pkgs.push("grim"),
            "wl-copy/wl-paste" => pkgs.push("wl-clipboard"),
            "playerctl" => pkgs.push("playerctl"),
            "fc-cache" => pkgs.push("fontconfig"),
            "python-gi/gtk4" => pkgs.push("python-gobject gtk4"),
            "python-atspi" => pkgs.push("python-atspi"),
            "python-PIL" => pkgs.push("python-pillow"),
            "python-cairo" => pkgs.push("python-cairo"),
            _ => {}
        }
    }
    if pkgs.is_empty() {
        return None;
    }
    let joined = pkgs.join(" ");
    Some(match distro {
        "arch" | "cachyos" | "manjaro" | "endeavouros" => format!("sudo pacman -S {joined}"),
        "debian" | "ubuntu" | "pop" => format!("sudo apt install {joined}"),
        "fedora" => format!("sudo dnf install {joined}"),
        _ => format!("install with your package manager: {joined}"),
    })
}

pub fn doctor() -> Result<()> {
    let distro = distro();
    println!("murmurt doctor — distro: {distro}\n");

    let mut missing: Vec<&str> = Vec::new();
    let mut check = |label: &str, ok: bool, required: bool| {
        let mark = if ok { "ok  " } else if required { "MISS" } else { "warn" };
        println!("  [{mark}] {label}");
        if !ok {
            missing.push(Box::leak(label.split(' ').next().unwrap_or(label).to_string().into_boxed_str()));
        }
    };

    check("waybar", have("waybar"), true);
    check("hyprctl (hyprland)", have("hyprctl"), true);
    check("python3", have("python3"), true);
    check("python-gi/gtk4", gtk4(), true);
    check("python-atspi (hover hotkeys, geometry)", python_module("gi") && python_module("gi.repository.Atspi") || python_module("atspi"), false);
    check("python-PIL", python_module("PIL"), false);
    check("python-cairo", python_module("cairo"), false);
    check("playerctl", have("playerctl"), true);
    check("wl-copy/wl-paste", have("wl-copy") && have("wl-paste"), true);
    check("grim", have("grim"), false);
    check("fc-cache", have("fc-cache"), true);
    check("font: JetBrainsMono Nerd Font", font_installed("JetBrainsMono"), true);
    check("font: Unbounded", font_installed("Unbounded"), false);
    check("font: Onest", font_installed("Onest"), false);

    let home = std::env::var("HOME").unwrap_or_default();
    for f in [
        format!("{home}/.local/bin/murmur-island.py"),
        format!("{home}/.local/bin/murmur-player.py"),
        format!("{home}/.local/bin/murmur-player-toggle.sh"),
        format!("{home}/.local/bin/murmur-island-hover.py"),
        format!("{home}/.local/bin/murmur-player-swipe.sh"),
        format!("{home}/.local/share/dbus-1/services/murmur.player.service"),
    ] {
        check(&format!("file: {f}"), std::path::Path::new(&f).exists(), true);
    }

    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/run/user/1000".into());
    let sock = format!("{runtime}/murmur.sock");
    check("murmurd socket", std::path::Path::new(&sock).exists(), true);
    check("hover daemon running", have("pgrep") && Command::new("pgrep").args(["-f", "murmur-island-hover"]).output().map(|o| o.status.success()).unwrap_or(false), false);

    let all_missing: Vec<&str> = missing.clone();
    let missing: Vec<&str> = missing.iter().filter(|m| !m.starts_with("file:") && !m.starts_with("murmurd") && !m.starts_with("hover") && !m.starts_with("font:")).copied().collect();
    println!();
    if all_missing.is_empty() {
        println!("everything essential is present.");
    } else {
        if !missing.is_empty() {
            if let Some(hint) = install_hint(&distro, &missing) {
                println!("missing packages, try:\n  {hint}");
            }
        }
        // every failure class gets a fix hint — no dead ends
        for m in &all_missing {
            if m.starts_with("file:") {
                println!("files missing — reinstall the island:\n  murmurt install");
                break;
            }
        }
        if all_missing.iter().any(|m| m.starts_with("font: Unbounded") || m.starts_with("font: Onest")) {
            println!("island fonts missing — they are embedded, reinstall:\n  murmurt install");
        }
        if all_missing.iter().any(|m| m.starts_with("font: JetBrainsMono")) {
            println!("JetBrainsMono Nerd Font missing — install it with your package manager (e.g. ttf-jetbrains-mono-nerd)");
        }
        if all_missing.iter().any(|m| m.starts_with("murmurd")) {
            println!("murmurd not running — start it:\n  systemctl --user start murmurd   # or: murmurd run");
        }
        if all_missing.iter().any(|m| m.starts_with("hover")) {
            println!("hover daemon not running — it autostarts via hyprland exec-once; re-login or run murmur-island-hover.py manually");
        }
    }
    Ok(())
}
