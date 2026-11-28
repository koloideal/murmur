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
use murmur_proto::{DeviceId, LoopMode, MediaOp, Message, PlayStatus, Route};
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
        &["media_state"],
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
    /// Last known shuffle — written locally on `Shuffle` set, refreshed
    /// from `MediaState` when the phone reports it.
    shuffle: bool,
    loop_status: LoopMode,
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
            shuffle: false,
            loop_status: LoopMode::None,
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

    // Control surface: everything except volume maps to the phone.
    // Rate kept for MPRIS completeness, writes ignored.
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
        match self.st.lock().await.loop_status {
            LoopMode::None => "None",
            LoopMode::Track => "Track",
            LoopMode::Playlist => "Playlist",
        }
    }
    #[zbus(property)]
    async fn set_loop_status(
        &self,
        mode: String,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) {
        let m = match mode.as_str() {
            "Track" => LoopMode::Track,
            "Playlist" => LoopMode::Playlist,
            _ => LoopMode::None,
        };
        self.st.lock().await.loop_status = m;
        let _ = self.cmd(MediaOp::Loop { mode: m }).await;
        let _ = self.loop_status_changed(&emitter).await;
    }
    #[zbus(property)]
    async fn shuffle(&self) -> bool {
        self.st.lock().await.shuffle
    }
    #[zbus(property)]
    async fn set_shuffle(
        &self,
        on: bool,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) {
        self.st.lock().await.shuffle = on;
        let _ = self.cmd(MediaOp::Shuffle { on }).await;
        let _ = self.shuffle_changed(&emitter).await;
    }
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

/// The bus can drop our well-known name without dropping the
/// connection (observed in the wild: connection alive, name silently
/// released — player gone until a daemon restart). The watchdog
/// re-requests it; a dead connection escalates to a full rebuild via
/// the outer loop.
async fn run(ctx: FeatureCtx) -> anyhow::Result<()> {
    let shared = ctx.shared;
    let mut inbox = ctx.inbox.expect("feature declares wants");
    let st = Arc::new(Mutex::new(Media::default()));

    loop {
        match serve(&shared, &st, &mut inbox).await {
            ServeEnd::InboxClosed => return Ok(()),
            ServeEnd::Broken(e) => {
                warn!("mpris: {e:#} — re-serving in 5s");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    }
}

enum ServeEnd {
    InboxClosed,
    Broken(anyhow::Error),
}

async fn serve(
    shared: &Arc<Shared>,
    st: &Arc<Mutex<Media>>,
    inbox: &mut crate::feature::Inbox,
) -> ServeEnd {
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
        // No session bus (headless, ssh) — retry slowly; the bus may
        // simply not be up yet at boot.
        Err(e) => return ServeEnd::Broken(e),
    };
    info!("mpris: serving {BUS_NAME}");

    let emitter: SignalEmitter<'static> =
        match conn.object_server().interface::<_, Player>(PATH).await {
            Ok(i) => i.signal_emitter().clone(),
            Err(e) => return ServeEnd::Broken(e.into()),
        };

    let mut watchdog = tokio::time::interval(std::time::Duration::from_secs(5));
    watchdog.tick().await; // first tick is immediate — swallow it

    loop {
        tokio::select! {
            msg = inbox.recv() => {
                let Some((from, msg)) = msg else { return ServeEnd::InboxClosed };
                handle_state(&conn, &emitter, st, from, &msg).await;
            }
            _ = watchdog.tick() => {
                use zbus::fdo::{DBusProxy, RequestNameFlags, RequestNameReply};
                let res: anyhow::Result<RequestNameReply> = async {
                    let proxy = DBusProxy::new(&conn).await?;
                    Ok(proxy
                        .request_name(
                            BUS_NAME.try_into()?,
                            RequestNameFlags::DoNotQueue.into(),
                        )
                        .await?)
                }
                .await;
                match res {
                    // Ours already, or just re-acquired — either way the
                    // name is back where playerctl can find it.
                    Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => {}
                    // Someone else holds it — don't fight, retry next tick.
                    Ok(_) => debug!("mpris: {BUS_NAME} owned elsewhere"),
                    Err(e) => return ServeEnd::Broken(e),
                }

                // The media source left the mesh — its last snapshot
                // must not keep "playing" on the bar forever. A goodbye
                // un-hears instantly; a crash takes the silence window.
                let stale = {
                    let m = st.lock().await;
                    m.src.is_some_and(|src| {
                        !shared.heard_recently(&src, std::time::Duration::from_secs(30))
                    })
                };
                if stale {
                    {
                        let mut m = st.lock().await;
                        m.src = None;
                        m.status = PlayStatus::Stopped;
                        m.title.clear();
                        m.artist.clear();
                        m.album.clear();
                        m.pos_ms = 0;
                        m.dur_ms = 0;
                        m.art_path = None;
                    }
                    if let Ok(iface) =
                        conn.object_server().interface::<_, Player>(PATH).await
                    {
                        let player_iface = iface.get().await;
                        let _ = player_iface.playback_status_changed(&emitter).await;
                        let _ = player_iface.metadata_changed(&emitter).await;
                    }
                }
            }
        }
    }
}

async fn handle_state(
    conn: &zbus::Connection,
    emitter: &SignalEmitter<'static>,
    st: &Arc<Mutex<Media>>,
    from: DeviceId,
    msg: &Message,
) {
    {
        let Message::MediaState {
            player,
            title,
            artist,
            album,
            status,
            pos_ms,
            dur_ms,
            art_b64,
            shuffle,
            loop_status,
            ..
        } = (*msg).clone()
        else {
            return;
        };
        debug!("mpris: {player} {title:?} {status:?} from {from}");
        // Art decode+write outside the state lock — a big PNG must not
        // stall concurrent MPRIS property reads.
        let art_path = art_b64.as_deref().and_then(cache_art);
        let (sh_changed, lp_changed) = {
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
            if art_b64.is_some() {
                m.art_path = art_path;
            }
            // Phone-authoritative control state — `None` keeps the
            // locally written value when the session doesn't report.
            let sh = shuffle.is_some_and(|s| {
                let changed = s != m.shuffle;
                m.shuffle = s;
                changed
            });
            let lp = loop_status.is_some_and(|l| {
                let changed = l != m.loop_status;
                m.loop_status = l;
                changed
            });
            (sh, lp)
        };
        let Ok(iface) = conn.object_server().interface::<_, Player>(PATH).await else {
            return;
        };
        let player_iface = iface.get().await;
        let _ = player_iface.playback_status_changed(emitter).await;
        let _ = player_iface.metadata_changed(emitter).await;
        if sh_changed {
            let _ = player_iface.shuffle_changed(emitter).await;
        }
        if lp_changed {
            let _ = player_iface.loop_status_changed(emitter).await;
        }
    }
}
