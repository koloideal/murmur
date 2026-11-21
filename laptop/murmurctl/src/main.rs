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
    /// Issue an invite code for a new device (QR + pasteable string).
    Invite,
    /// Pairing requests: list pending / accept / confirm after SAS check.
    Pair {
        #[command(subcommand)]
        op: Option<PairOp>,
    },
    /// Broadcast a Message (JSON from stdin) to all devices (debug).
    Send,
}

#[derive(Subcommand)]
enum PairOp {
    /// Send our pubkey to a pending joiner (it can then show the SAS too).
    Accept { session: String },
    /// After the codes match: register the device and ship credentials.
    Confirm { session: String },
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

fn print_qr(data: &str) {
    match qrcode::QrCode::new(data) {
        Ok(code) => {
            let img = code
                .render::<qrcode::render::unicode::Dense1x2>()
                .quiet_zone(false)
                .build();
            println!("{img}");
        }
        Err(e) => eprintln!("(qr render failed: {e})"),
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let code = match cli.cmd {
        Cmd::Status => return reply(serde_json::json!({"cmd": "status"})),
        Cmd::Devices => return reply(serde_json::json!({"cmd": "devices"})),
        Cmd::Revoke { device_id } => {
            return reply(serde_json::json!({"cmd": "revoke", "device_id": device_id}));
        }
        Cmd::Send => {
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            let msg: serde_json::Value =
                serde_json::from_str(buf.trim()).context("send expects a Message JSON on stdin")?;
            return reply(serde_json::json!({"cmd": "send", "msg_json": msg}));
        }
        Cmd::Invite => {
            let res = request(serde_json::json!({"cmd": "invite"}))?;
            let code = res["data"]["code"].as_str().unwrap_or_default().to_owned();
            print_qr(&code);
            println!("invite code (on the new device: `murmurd pair --code …`):");
            println!("{code}");
            return Ok(());
        }
        Cmd::Pair { op: None } => {
            let res = request(serde_json::json!({"cmd": "pair"}))?;
            let pending = res["data"]["pending"].as_array();
            match pending.map(|p| p.len()) {
                Some(0) => println!("no pending pairings"),
                _ => {
                    for p in pending.into_iter().flatten() {
                        println!(
                            "{}  {}  code {}",
                            p["session"].as_str().unwrap_or("?"),
                            p["name"].as_str().unwrap_or("?"),
                            p["sas"].as_str().unwrap_or("?"),
                        );
                    }
                    eprintln!(
                        "\non the new device: murmurd pair --code <invite>\n\
                         then here: murmurctl pair accept <session> → compare codes → murmurctl pair confirm <session>"
                    );
                }
            }
            return Ok(());
        }
        Cmd::Pair {
            op: Some(PairOp::Accept { session }),
        } => serde_json::json!({"cmd": "pair_accept", "session": session}),
        Cmd::Pair {
            op: Some(PairOp::Confirm { session }),
        } => serde_json::json!({"cmd": "pair_confirm", "session": session}),
    };
    reply(code)
}

fn reply(req: serde_json::Value) -> anyhow::Result<()> {
    let res = request(req)?;
    println!("{}", serde_json::to_string_pretty(&res)?);
    if res.get("ok") == Some(&serde_json::json!(false)) {
        std::process::exit(1);
    }
    Ok(())
}
