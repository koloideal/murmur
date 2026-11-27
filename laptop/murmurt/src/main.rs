mod doctor;
mod jsonc;
mod geometry;
mod install;

use clap::{CommandFactory, Parser, Subcommand};

#[derive(Parser)]
#[command(name = "murmurt", version, about = "murmur theme — waybar island tooling", disable_help_flag = true)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    /// Print help
    #[arg(short = 'h', long = "help", action = clap::ArgAction::SetTrue)]
    help: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// install island scripts, fonts, dbus service; merge config snippets
    Install {
        /// Don't touch waybar/hyprland configs, only copy files
        #[arg(long)]
        no_merge: bool,
        /// Override waybar config path
        #[arg(long)]
        waybar_config: Option<String>,
    },
    /// remove everything install put in place (marker blocks and files)
    Uninstall {
        /// Override waybar config path
        #[arg(long)]
        waybar_config: Option<String>,
    },
    /// check dependencies and report what's missing
    Doctor,
    /// probe the island's on-screen geometry via AT-SPI, print JSON
    Geometry,
    /// print shell completions (bash|zsh|fish) on stdout
    Completions {
        shell: clap_complete::Shell,
    },
}

// ---------- same paint kit as murmurctl/murmurd ----------

fn paint(code: &str, s: impl std::fmt::Display) -> String {
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

const ART: &str = " _ __ ___  _   _ _ __ _ __ ___  _   _ _ __\n\
| \x27_ ` _ \\| | | | \x27__| \x27_ ` _ \\| | | | \x27__|\n\
| | | | | | |_| | |  | | | | | | |_| | |\n\
|_| |_| |_|\\__,_|_|  |_| |_| |_|\\__,_|_|";

fn print_help() {
    let ac = |s: &str| paint("38;5;141", format!("{s:<21}"));
    let dm = |s: &str| paint("2", s);
    let bd = |s: &str| paint("1", s);
    const TAG: &str = "  _\n | |_\n | __|\n | |_\n  \\__|";
    println!();
    let __aw = ART.lines().map(|l| l.len()).max().unwrap_or(0);
    let __art_off: Vec<&str> = std::iter::once("").chain(ART.lines()).collect();
    for (l, t) in __art_off.iter().zip(TAG.lines()) {
        println!("  {}{}", paint("1;38;5;141", &format!("{l:<width$}", width = __aw)), paint("38;5;215", t));
    }
    println!("  {}", dm("murmur theme - waybar island tooling"));
    println!();
    println!("  {}", bd("commands"));
    for (c, d) in [
        ("murmurt install", "scripts, fonts, dbus service, config merges"),
        ("murmurt uninstall", "strip marker blocks, remove files"),
        ("murmurt doctor", "dependency audit with per-distro hints"),
        ("murmurt geometry", "island position probe (AT-SPI, JSON)"),
        ("murmurt completions", "shell completions on stdout"),
    ] {
        println!("  {} {}", ac(c), dm(d));
    }
    println!();
    println!("  {}", dm("the island runtime lives in ~/.local/bin after install"));
    println!();
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if cli.help || cli.cmd.is_none() {
        print_help();
        return Ok(());
    }
    match cli.cmd.unwrap() {
        Cmd::Install { no_merge, waybar_config } => install::install(no_merge, waybar_config),
        Cmd::Uninstall { waybar_config } => install::uninstall(waybar_config),
        Cmd::Doctor => doctor::doctor(),
        Cmd::Geometry => geometry::geometry().await,
        Cmd::Completions { shell } => {
            let mut cli = Cli::command();
            let name = cli.get_name().to_string();
            clap_complete::generate(shell, &mut cli, name, &mut std::io::stdout());
            Ok(())
        }
    }
}
