//! MPRIS feature: the phone appears as a normal system player.
//!
//! Serves `org.mpris.MediaPlayer2.murmur` on the session bus — playerctl,
//! media keys and bar widgets work against it unchanged. `MediaState`
//! updates the served properties (position is extrapolated between the
//! phone's ~5s updates, per MPRIS convention no Position signals are
//! emitted). MPRIS methods → `MediaCommand`, unicast to the device the
//! state last came from. Art bytes land in the XDG cache and surface as
//! a `file://` `mpris:artUrl`.

use crate::feature::{Feature, FeatureCtx};
use crate::state::Shared;
use anyhow::Context;
use murmur_proto::{DeviceId, MediaOp, Message, PlayStatus, Route};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, Value};
use zbus::{fdo, interface};

const BUS_NAME: &str = "org.mpris.MediaPlayer2.murmur";
const PATH: &str = "/org/mpris/MediaPlayer2";
const TRACKID: &str = "/org/mpris/murmur/track0";

pub fn feature() -> Feature {
    Feature::new(
        "mpris",
        Some(|m| matches!(m, Message::MediaState { .. })),
        |ctx| Box::pin(run(ctx)),
    )
}

/// Latest phone media snapshot + when `pos_ms` was sampled (for
/// extrapolation). `src` is the phone the commands go back to.
struct Media {
    src: Option<DeviceId>,
    player: String,
    title: String,
    artist: String,
    album: String,
    status: PlayStatus,
    pos_ms: u64,
    dur_ms: u64,
    sampled: std::time::Instant,
    art_path: Option<std::path::PathBuf>,
}

impl Default for Media {
    fn default() -> Self {
        Self {
            src: None,
            player: String::new(),
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            status: PlayStatus::Stopped,
            pos_ms: 0,
            dur_ms: 0,
            sampled: std::time::Instant::now(),
            art_path: None,
        }
    }
}

impl Media {
    fn position_ms(&self) -> u64 {
        if self.status == PlayStatus::Playing {
            (self.pos_ms + self.sampled.elapsed().as_millis() as u64).min(self.dur_ms)
        } else {
            self.pos_ms
        }
    }
}

/// org.mpris.MediaPlayer2 — root interface, all controls are no-ops.
struct Root;

#[interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {}
    fn quit(&self) {}

    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn can_raise(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn identity(&self) -> &str {
        "murmur (phone)"
    }
    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        Vec::new()
    }
    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }
}

/// org.mpris.MediaPlayer2.Player — backed by the latest `MediaState`.
struct Player {
    st: Arc<Mutex<Media>>,
    shared: Arc<Shared>,
}

impl Player {
    /// Method → MediaCommand, unicast to the phone. No-op until a phone
    /// has reported state.
    async fn cmd(&self, op: MediaOp) -> fdo::Result<()> {
        let Some(phone) = self.st.lock().await.src else {
            return Ok(());
        };
        let _ = self
            .shared
            .send(Route::Device(phone), &Message::MediaCommand { op });
        Ok(())
    }
}

#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    async fn next(&self) -> fdo::Result<()> {
        self.cmd(MediaOp::Next).await
    }
    async fn previous(&self) -> fdo::Result<()> {
        self.cmd(MediaOp::Previous).await
    }
    async fn pause(&self) -> fdo::Result<()> {
        self.cmd(MediaOp::Pause).await
    }
    async fn play(&self) -> fdo::Result<()> {
        self.cmd(MediaOp::Play).await
    }
    async fn play_pause(&self) -> fdo::Result<()> {
        let op = match self.st.lock().await.status {
            PlayStatus::Playing => MediaOp::Pause,
            _ => MediaOp::Play,
        };
        self.cmd(op).await
    }
    async fn stop(&self) -> fdo::Result<()> {
        self.cmd(MediaOp::Pause).await
    }
    /// MPRIS Seek is a *relative* µs offset; our wire op is absolute ms.
    async fn seek(&self, offset_us: i64) -> fdo::Result<()> {
        let cur = self.st.lock().await.position_ms() as i64;
        let target = (cur + offset_us / 1000).max(0) as u64;
        self.cmd(MediaOp::Seek { pos_ms: target }).await
    }
    async fn set_position(&self, _track_id: ObjectPath<'_>, pos_us: i64) -> fdo::Result<()> {
        self.cmd(MediaOp::Seek {
            pos_ms: pos_us.max(0) as u64 / 1000,
        })
        .await
    }

    #[zbus(property)]
    async fn playback_status(&self) -> String {
        match self.st.lock().await.status {
            PlayStatus::Playing => "Playing",
            PlayStatus::Paused => "Paused",
            PlayStatus::Stopped => "Stopped",
        }
        .to_string()
    }

    #[zbus(property)]
    async fn metadata(&self) -> HashMap<String, Value<'_>> {
        let m = self.st.lock().await;
        let mut md: HashMap<String, Value<'_>> = HashMap::new();
        let track = ObjectPath::try_from(TRACKID).expect("static path");
        md.insert("mpris:trackid".into(), Value::ObjectPath(track));
        md.insert("mpris:length".into(), Value::I64(m.dur_ms as i64 * 1000));
        md.insert("xesam:title".into(), Value::Str(m.title.clone().into()));
        md.insert("xesam:album".into(), Value::Str(m.album.clone().into()));
        md.insert(
            "xesam:artist".into(),
            Value::Array(vec![m.artist.clone()].into()),
        );
        if let Some(p) = &m.art_path {
            md.insert(
                "mpris:artUrl".into(),
                Value::Str(format!("file://{}", p.display()).into()),
            );
        }
        md
    }

    #[zbus(property)]
    async fn position(&self) -> i64 {
        self.st.lock().await.position_ms() as i64 * 1000
    }

    // Control surface: everything except volume/loop/shuffle maps to the
    // phone. Rate/shuffle kept for MPRIS completeness, writes ignored.
    #[zbus(property)]
    async fn rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    async fn set_rate(&self, _rate: f64) {}
    #[zbus(property)]
    async fn minimum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    async fn maximum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    async fn loop_status(&self) -> &str {
        "None"
    }
    #[zbus(property)]
    async fn set_loop_status(&self, _loop: String) {}
    #[zbus(property)]
    async fn shuffle(&self) -> bool {
        false
    }
    #[zbus(property)]
    async fn set_shuffle(&self, _shuffle: bool) {}
    #[zbus(property)]
    async fn volume(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    async fn set_volume(&self, _vol: f64) {}
    #[zbus(property)]
    async fn can_control(&self) -> bool {
        true
    }
    #[zbus(property)]
    async fn can_play(&self) -> bool {
        true
    }
    #[zbus(property)]
    async fn can_pause(&self) -> bool {
        true
    }
    #[zbus(property)]
    async fn can_go_next(&self) -> bool {
        true
    }
    #[zbus(property)]
    async fn can_go_previous(&self) -> bool {
        true
    }
    #[zbus(property)]
    async fn can_seek(&self) -> bool {
        true
    }
}

/// Persist artwork once per content; return its cache path.
fn cache_art(b64: &str) -> Option<std::path::PathBuf> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    if bytes.is_empty() {
        return None;
    }
    let dir = crate::config::cache_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("art-{:x}.img", crate::clipboard::hash_clip(&bytes)));
    if !path.exists() {
        std::fs::write(&path, bytes).ok()?;
    }
    Some(path)
}

async fn run(ctx: FeatureCtx) -> anyhow::Result<()> {
    let shared = ctx.shared;
    let mut inbox = ctx.inbox.expect("feature declares wants");
    let st = Arc::new(Mutex::new(Media::default()));

    // No session bus (headless, ssh) — the feature idles instead of
    // killing the daemon, like the other adapter-backed features.
    let conn: anyhow::Result<zbus::Connection> = async {
        Ok(zbus::connection::Builder::session()
            .context("session bus")?
            .name(BUS_NAME)?
            .serve_at(PATH, Root)?
            .serve_at(
                PATH,
                Player {
                    st: st.clone(),
                    shared: shared.clone(),
                },
            )?
            .build()
            .await?)
    }
    .await;
    let conn = match conn {
        Ok(c) => c,
        Err(e) => {
            warn!("mpris feature disabled: {e:#}");
            while inbox.recv().await.is_some() {}
            return Ok(());
        }
    };
    info!("mpris: serving {BUS_NAME}");

    let emitter: SignalEmitter<'static> =
        match conn.object_server().interface::<_, Player>(PATH).await {
            Ok(i) => i.signal_emitter().clone(),
            Err(e) => {
                warn!("mpris feature disabled: {e:#}");
                while inbox.recv().await.is_some() {}
                return Ok(());
            }
        };

    while let Some((from, msg)) = inbox.recv().await {
        let Message::MediaState {
            player,
            title,
            artist,
            album,
            status,
            pos_ms,
            dur_ms,
            art_b64,
            ..
        } = msg
        else {
            continue;
        };
        debug!("mpris: {player} {title:?} {status:?} from {from}");
        {
            let mut m = st.lock().await;
            m.src = Some(from);
            m.player = player;
            m.title = title;
            m.artist = artist;
            m.album = album;
            m.status = status;
            m.pos_ms = pos_ms;
            m.dur_ms = dur_ms;
            m.sampled = std::time::Instant::now();
            if let Some(b64) = art_b64 {
                m.art_path = cache_art(&b64);
            }
        }
        let Ok(iface) = conn.object_server().interface::<_, Player>(PATH).await else {
            continue;
        };
        let player_iface = iface.get().await;
        let _ = player_iface.playback_status_changed(&emitter).await;
        let _ = player_iface.metadata_changed(&emitter).await;
    }
    Ok(())
}
