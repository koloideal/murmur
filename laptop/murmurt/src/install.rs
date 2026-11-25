//! Self-contained installer: every asset is embedded in this binary,
//! so `murmurt install` is all a fresh machine needs. Config merges
//! are marker-delimited blocks — idempotent, replaceable, removable.

use anyhow::{Context, Result};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const BIN_SCRIPTS: &[(&str, &[u8])] = &[
    ("murmur-island.py", include_bytes!("../assets/murmur-island.py")),
    ("murmur-player.py", include_bytes!("../assets/murmur-player.py")),
    ("murmur-player-toggle.sh", include_bytes!("../assets/murmur-player-toggle.sh")),
    ("murmur-island-hover.py", include_bytes!("../assets/murmur-island-hover.py")),
];
const DBUS_SERVICE: &[u8] = include_bytes!("../assets/murmur.player.service");
const WAYBAR_MODULES: &str = include_str!("../assets/waybar.modules.jsonc");
const WAYBAR_STYLE: &str = include_str!("../assets/waybar.style.css");
const HYPR_LUA: &str = include_str!("../assets/hyprland.lua");
const HYPR_CONF: &str = include_str!("../assets/hyprland.conf");
const FONTS: &[(&str, &[u8])] = &[
    ("Unbounded.ttf", include_bytes!("../assets/fonts/Unbounded.ttf")),
    ("Onest.ttf", include_bytes!("../assets/fonts/Onest.ttf")),
    ("OFL-Unbounded.txt", include_bytes!("../assets/fonts/OFL-Unbounded.txt")),
    ("OFL-Onest.txt", include_bytes!("../assets/fonts/OFL-Onest.txt")),
];

/// Marker lines as they appear in each comment syntax.
fn markers(syntax: &str) -> (String, String) {
    match syntax {
        "css" => (
            "/* >>> murmur island */".into(),
            "/* <<< murmur island */".into(),
        ),
        "//" => ("// >>> murmur island".into(), "// <<< murmur island".into()),
        "--" => ("-- >>> murmur island".into(), "-- <<< murmur island".into()),
        _ => ("# >>> murmur island".into(), "# <<< murmur island".into()),
    }
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").expect("HOME unset"))
}

fn write_file(path: &Path, data: &[u8], exec: bool) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, data).with_context(|| format!("writing {}", path.display()))?;
    if exec {
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    println!("  wrote {}", path.display());
    Ok(())
}

/// Replace the marker block in `content` with `block`, or append it.
/// Idempotent: re-running upgrades the block in place.
fn merge_block(content: &str, block: &str, syntax: &str) -> String {
    let (open, close) = markers(syntax);
    let mut out = String::new();
    let mut inside = false;
    let mut found = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == open {
            inside = true;
            found = true;
            out.push_str(&format!("{open}\n{}\n{close}\n", block.trim_end()));
            continue;
        }
        if trimmed == close {
            inside = false;
            continue;
        }
        if !inside {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !found {
        out.push_str(&format!("\n{open}\n{}\n{close}\n", block.trim_end()));
    }
    out
}

/// Remove the marker block entirely.
fn strip_block(content: &str, syntax: &str) -> String {
    let (open, close) = markers(syntax);
    let mut out = String::new();
    let mut inside = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == open {
            inside = true;
            continue;
        }
        if trimmed == close {
            inside = false;
            continue;
        }
        if !inside {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn backup_once(path: &Path) -> Result<()> {
    let bak = path.with_file_name(format!(
        "{}.murmurt-bak",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("config")
    ));
    if !bak.exists() {
        fs::copy(path, &bak).with_context(|| format!("backing up to {}", bak.display()))?;
        println!("  backup {}", bak.display());
    }
    Ok(())
}

/// The waybar config in use: the running waybar's -c argument wins,
/// then the usual suspects. Canonicalized and deduped.
fn waybar_configs(over: &Option<String>) -> Vec<PathBuf> {
    if let Some(p) = over {
        return vec![PathBuf::from(p)];
    }
    let mut out: Vec<PathBuf> = Vec::new();
    if let Ok(procs) = fs::read_dir("/proc") {
        for entry in procs.flatten() {
            let Ok(cmd) = fs::read_to_string(entry.path().join("cmdline")) else { continue };
            if !cmd.starts_with("waybar") && !cmd.contains("/waybar\0") {
                continue;
            }
            let args: Vec<&str> = cmd.split('\0').collect();
            for (i, a) in args.iter().enumerate() {
                if (*a == "-c" || *a == "--config") && i + 1 < args.len() {
                    out.push(PathBuf::from(args[i + 1]));
                }
            }
        }
    }
    for c in [
        home().join(".config/waybar/config.jsonc"),
        home().join(".config/hypr/waybar/config.jsonc"),
        home().join(".config/waybar/config"),
    ] {
        if c.exists() {
            out.push(c);
        }
    }
    let mut dedup: Vec<PathBuf> = Vec::new();
    for p in out {
        let canon = fs::canonicalize(&p).unwrap_or(p);
        if canon.exists() && !dedup.contains(&canon) {
            dedup.push(canon);
        }
    }
    dedup
}

/// Strip the leading instruction comments from a contrib fragment —
/// everything up to the first blank line, if it's all comments.
fn fragment_body(src: &str, comment: &str) -> String {
    if let Some(idx) = src.find("\n\n") {
        if src[..idx]
            .lines()
            .all(|l| l.trim().is_empty() || l.trim_start().starts_with(comment))
        {
            return src[idx + 2..].trim_end().to_string();
        }
    }
    src.trim_end().to_string()
}

/// Insert `"group/murmur",` right after the `"modules-left": [` opening
/// line, inside marker comments. Skips if the group is already listed.
fn merge_modules_array(content: &str) -> String {
    // the group definition is `"group/murmur": {` — a *use* inside a
    // modules array is the bare string; only the latter should stop us
    let referenced = content.lines().any(|l| {
        l.contains("\"group/murmur\"") && !l.trim_start().starts_with("\"group/murmur\":")
    });
    if referenced {
        println!("  note: \"group/murmur\" already referenced — leaving module arrays untouched");
        return content.to_string();
    }
    let mut out = String::new();
    let mut injected = false;
    for line in content.lines() {
        out.push_str(line);
        out.push('\n');
        if !injected && line.contains("\"modules-left\"") && line.contains('[') {
            let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
            out.push_str(&format!(
                "{indent}    // >>> murmur island\n{indent}    \"group/murmur\",\n{indent}    // <<< murmur island\n"
            ));
            injected = true;
        }
    }
    if !injected {
        println!("  warn: no \"modules-left\" array found — add \"group/murmur\" to your modules manually");
    }
    out
}

/// Insert the module definitions right after the root object's opening
/// brace — first position means no trailing-comma guessing. The fragment
/// defines `group/murmur` itself, so it carries no trailing comma issues:
/// every key line ends with `,` and ordering inside an object is free.
fn merge_module_defs(content: &str) -> String {
    if content.contains("\"custom/murmur-status\"") && !content.contains(">>> murmur island") {
        println!("  note: murmur modules already defined manually — not touching definitions");
        return content.to_string();
    }
    // rebuild from scratch if upgrading: strip old blocks first, but
    // keep the array injection — it re-inserts harmlessly either way
    let content = &strip_blocks_in_jsonc(content);
    let body = fragment_body(WAYBAR_MODULES, "//");
    let mut out = String::new();
    let mut injected = false;
    for line in content.lines() {
        out.push_str(line);
        out.push('\n');
        if !injected && line.trim() == "{" {
            out.push_str("    // >>> murmur island\n");
            for l in body.lines() {
                out.push_str("    ");
                out.push_str(l);
                out.push('\n');
            }
            out.push_str("    // <<< murmur island\n");
            injected = true;
        }
    }
    if !injected {
        out.push_str(&format!("\n// >>> murmur island\n{body}\n// <<< murmur island\n"));
    }
    out
}

fn strip_blocks_in_jsonc(content: &str) -> String {
    strip_block(content, "//")
}

fn merge_waybar(over: &Option<String>) -> Result<()> {
    let cfgs = waybar_configs(over);
    if cfgs.is_empty() {
        println!("  warn: no waybar config found — skipping merge, see contrib/waybar-island/");
        return Ok(());
    }
    for cfg in &cfgs {
        backup_once(cfg)?;
        let content = fs::read_to_string(cfg).with_context(|| format!("reading {}", cfg.display()))?;
        let merged = merge_module_defs(&content);
        let merged = merge_modules_array(&merged);
        if merged != content {
            fs::write(cfg, merged)?;
            println!("  merged module defs + group/murmur into {}", cfg.display());
        }

        let css = cfg.with_file_name("style.css");
        if css.exists() {
            backup_once(&css)?;
            let content = fs::read_to_string(&css)?;
            let body = fragment_body(WAYBAR_STYLE, "/*");
            let merged = merge_block(&content, &body, "css");
            if merged != content {
                fs::write(&css, merged)?;
                println!("  merged island styles into {}", css.display());
            }
        } else {
            println!("  note: {} not found — copy contrib/waybar-island/waybar.style.css manually", css.display());
        }
    }
    Ok(())
}

fn merge_hyprland() -> Result<()> {
    let lua = home().join(".config/hypr/hyprland.lua");
    let conf = home().join(".config/hypr/hyprland.conf");
    if lua.exists() {
        backup_once(&lua)?;
        let content = fs::read_to_string(&lua)?;
        if content.contains("define_submap") && content.contains("murmur") && !content.contains(">>> murmur island") {
            println!("  note: {} already has a murmur submap — not touching it", lua.display());
            return Ok(());
        }
        let merged = merge_block(&content, HYPR_LUA, "--");
        if merged != content {
            fs::write(&lua, merged)?;
            println!("  merged island block into {} — check the autostart note inside", lua.display());
        }
    }
    if conf.exists() {
        backup_once(&conf)?;
        let content = fs::read_to_string(&conf)?;
        if content.contains("submap = murmur") && !content.contains(">>> murmur island") {
            println!("  note: {} already has a murmur submap — not touching it", conf.display());
            return Ok(());
        }
        let merged = merge_block(&content, HYPR_CONF, "#");
        if merged != content {
            fs::write(&conf, merged)?;
            println!("  merged autostart+submap into {}", conf.display());
        }
    }
    if !lua.exists() && !conf.exists() {
        println!("  note: no hyprland config found — skipping (not on hyprland?)");
    }
    Ok(())
}

pub fn install(no_merge: bool, waybar_config: Option<String>) -> Result<()> {
    println!("installing murmur island:");
    let bin = home().join(".local/bin");
    for (name, data) in BIN_SCRIPTS {
        write_file(&bin.join(name), data, true)?;
    }
    write_file(
        &home().join(".local/share/dbus-1/services/murmur.player.service"),
        DBUS_SERVICE,
        false,
    )?;

    // murmurt itself — relocatable means the installer installs itself
    let self_target = bin.join("murmurt");
    let self_exe = std::env::current_exe()?;
    let same = fs::canonicalize(&self_exe).ok() == fs::canonicalize(&self_target).ok();
    if !same {
        fs::copy(&self_exe, &self_target).context("installing murmurt itself")?;
        fs::set_permissions(&self_target, fs::Permissions::from_mode(0o755))?;
        println!("  wrote {}", self_target.display());
    }

    let fontdir = home().join(".local/share/fonts/murmur");
    for (name, data) in FONTS {
        write_file(&fontdir.join(name), data, false)?;
    }
    let _ = std::process::Command::new("fc-cache").arg("-f").arg(&fontdir).status();

    if no_merge {
        println!("--no-merge: leaving configs untouched");
    } else {
        merge_waybar(&waybar_config)?;
        merge_hyprland()?;
    }
    println!("\ndone. restart waybar / reload hyprland to pick everything up.");
    println!("run `murmurt doctor` to check runtime dependencies.");
    Ok(())
}

pub fn uninstall(waybar_config: Option<String>) -> Result<()> {
    println!("removing murmur island:");
    for (name, _) in BIN_SCRIPTS {
        let p = home().join(".local/bin").join(name);
        if p.exists() {
            fs::remove_file(&p)?;
            println!("  removed {}", p.display());
        }
    }
    let svc = home().join(".local/share/dbus-1/services/murmur.player.service");
    if svc.exists() {
        fs::remove_file(&svc)?;
        println!("  removed {}", svc.display());
    }
    let fontdir = home().join(".local/share/fonts/murmur");
    if fontdir.exists() {
        fs::remove_dir_all(&fontdir)?;
        println!("  removed {}", fontdir.display());
    }

    for cfg in waybar_configs(&waybar_config) {
        let content = fs::read_to_string(&cfg)?;
        let stripped = strip_blocks_in_jsonc(&content);
        if stripped != content {
            fs::write(&cfg, stripped)?;
            println!("  stripped marker blocks from {}", cfg.display());
        }
        let css = cfg.with_file_name("style.css");
        if css.exists() {
            let content = fs::read_to_string(&css)?;
            let stripped = strip_block(&content, "css");
            if stripped != content {
                fs::write(&css, stripped)?;
                println!("  stripped marker blocks from {}", css.display());
            }
        }
    }
    for f in [
        home().join(".config/hypr/hyprland.lua"),
        home().join(".config/hypr/hyprland.conf"),
    ] {
        if !f.exists() {
            continue;
        }
        let content = fs::read_to_string(&f)?;
        for syntax in ["--", "#"] {
            let stripped = strip_block(&content, syntax);
            if stripped != content {
                fs::write(&f, stripped)?;
                println!("  stripped marker blocks from {}", f.display());
                break;
            }
        }
    }
    println!("\ndone. murmurt itself is still in ~/.local/bin — delete it manually if you want it gone.");
    Ok(())
}
