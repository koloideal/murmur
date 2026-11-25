mod doctor;
mod geometry;
mod install;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "murmurt", version, about = "murmur tools: waybar island install, doctor, geometry")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Install island scripts, fonts, dbus service; merge config snippets
    Install {
        /// Don't touch waybar/hyprland configs, only copy files
        #[arg(long)]
        no_merge: bool,
        /// Override waybar config path
        #[arg(long)]
        waybar_config: Option<String>,
    },
    /// Remove everything install put in place (marker blocks and files)
    Uninstall {
        #[arg(long)]
        waybar_config: Option<String>,
    },
    /// Check dependencies and report what's missing
    Doctor,
    /// Probe the island's on-screen geometry via AT-SPI, print JSON
    Geometry,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Install { no_merge, waybar_config } => install::install(no_merge, waybar_config),
        Cmd::Uninstall { waybar_config } => install::uninstall(waybar_config),
        Cmd::Doctor => doctor::doctor(),
        Cmd::Geometry => geometry::geometry().await,
    }
}
