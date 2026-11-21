//! murmurctl: thin CLI over the murmurd unix socket.
//! Sends one JSON line, prints one JSON line.

use anyhow::Context;
use clap::{Parser, Subcommand};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "murmurctl", about = "control murmurd")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show connection state, presence table, active node.
    Status,
    /// List family devices known to the server.
    Devices,
    /// Revoke a device by id (kills it instantly on the server).
    Revoke { device_id: String },
    /// Accept a pending pairing request (SAS code shown on both sides).
    Pair,
    /// Push stdin into the shared clipboard (debug).
    Send,
}

fn sock_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("murmur.sock")
}

fn request(req: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    let path = sock_path();
    let stream = UnixStream::connect(&path)
        .with_context(|| format!("connect {} — is murmurd running?", path.display()))?;
    let mut w = stream.try_clone()?;
    w.write_all(serde_json::to_string(&req)?.as_bytes())?;
    w.write_all(b"\n")?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    Ok(serde_json::from_str(line.trim())?)
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let req = match cli.cmd {
        Cmd::Status => serde_json::json!({"cmd": "status"}),
        Cmd::Devices => serde_json::json!({"cmd": "devices"}),
        Cmd::Revoke { device_id } => serde_json::json!({"cmd": "revoke", "device_id": device_id}),
        Cmd::Pair => serde_json::json!({"cmd": "pair"}),
        Cmd::Send => {
            use base64::Engine;
            let mut buf = Vec::new();
            std::io::stdin().read_to_end(&mut buf)?;
            serde_json::json!({
                "cmd": "send",
                "data_b64": base64::engine::general_purpose::STANDARD.encode(&buf)
            })
        }
    };

    let res = request(req)?;
    println!("{}", serde_json::to_string_pretty(&res)?);
    if res.get("ok") == Some(&serde_json::json!(false)) {
        std::process::exit(1);
    }
    Ok(())
}
