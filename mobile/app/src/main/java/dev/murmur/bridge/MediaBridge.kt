package dev.murmur.bridge

import android.content.ComponentName
import android.content.Context
import android.media.MediaMetadata
import android.media.session.MediaController
import android.media.session.MediaSessionManager
import android.graphics.Bitmap
import android.media.session.PlaybackState
import android.util.Base64
import dev.murmur.core.Msg
import java.io.ByteArrayOutputStream
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * Watches the active media session (same notification-listener permission)
 * and publishes Msg.MediaState; executes Msg.MediaCommand on it.
 *
 * Position is sent periodically while playing — laptops extrapolate.
 */
class MediaBridge(
    private val ctx: Context,
    private val send: (Msg) -> Unit,
    private val scope: CoroutineScope,
) {
    private val manager = ctx.getSystemService(MediaSessionManager::class.java)
    private val listenerName = ComponentName(ctx, NotificationBridge::class.java)
    private var controller: MediaController? = null
    private var ticker: Job? = null
    private var pendingPublish: Job? = null
    /** Art we already sent this track — no point resending every 5s. */
    private var lastArtHash: Int? = null

    private companion object {
        const val ART_PX = 512
    }

    private val sessionListener =
        MediaSessionManager.OnActiveSessionsChangedListener { controllers ->
            bind(pick(controllers))
        }

    /** firstOrNull grabs whatever the system lists first — often a
     *  stale STOPPED video session while the music app sits PAUSED.
     *  Rank by liveliness, freshest update wins ties. */
    private fun pick(controllers: List<MediaController>?): MediaController? =
        controllers?.maxWithOrNull(
            compareBy<MediaController> {
                when (it.playbackState?.state) {
                    PlaybackState.STATE_PLAYING -> 4
                    PlaybackState.STATE_BUFFERING,
                    PlaybackState.STATE_FAST_FORWARDING,
                    PlaybackState.STATE_REWINDING -> 3
                    PlaybackState.STATE_PAUSED -> 2
                    PlaybackState.STATE_STOPPED -> 1
                    else -> 0
                }
            }.thenBy { it.playbackState?.lastPositionUpdateTime ?: 0L },
        )

    private val callback = object : MediaController.Callback() {
        override fun onMetadataChanged(metadata: MediaMetadata?) = publishNow()
        override fun onPlaybackStateChanged(state: PlaybackState?) {
            publishNow()
            restartTicker()
        }
        override fun onSessionDestroyed() {
            controller = null
            // A dying session isn't the end of media — the next-best
            // one may still be alive.
            bind(pick(runCatching {
                manager.getActiveSessions(listenerName)
            }.getOrNull()))
        }
    }

    init {
        runCatching {
            manager.addOnActiveSessionsChangedListener(sessionListener, listenerName)
            bind(pick(manager.getActiveSessions(listenerName)))
        }
        // throws SecurityError until notification access is granted — fine,
        // media just stays dark until then.
    }

    private fun bind(c: MediaController?) {
        controller?.unregisterCallback(callback)
        controller = c
        c?.registerCallback(callback)
        restartTicker()
        publishNow()
    }

    private fun restartTicker() {
        ticker?.cancel()
        if (controller?.playbackState?.state != PlaybackState.STATE_PLAYING) return
        ticker = scope.launch {
            while (true) {
                // Laptops extrapolate position between updates; this is
                // only drift correction, so keep the radio mostly idle.
                delay(20_000)
                publishNow()
            }
        }
    }

    /**
     * Conflated publish: session callbacks burst (metadata + state +
     * queue all fire within ms on a track change) — collapse them into
     * one sealed envelope so each burst costs one radio wakeup.
     */
    fun publishNow() {
        pendingPublish?.cancel()
        pendingPublish = scope.launch {
            delay(180)
            publish()
        }
    }

    /** Snapshot → Msg.MediaState → all laptops. */
    private fun publish() {
        val c = controller
        val md = c?.metadata
        val ps = c?.playbackState
        send(
            Msg.MediaState(
                player = c?.packageName ?: "",
                title = md?.getString(MediaMetadata.METADATA_KEY_TITLE) ?: "",
                artist = md?.getString(MediaMetadata.METADATA_KEY_ARTIST) ?: "",
                album = md?.getString(MediaMetadata.METADATA_KEY_ALBUM) ?: "",
                status = when (ps?.state) {
                    PlaybackState.STATE_PLAYING -> "playing"
                    PlaybackState.STATE_PAUSED -> "paused"
                    else -> "stopped"
                },
                // ps.position is sampled at state-write time — while
                // playing, advance it by wall-clock * speed so laptops
                // extrapolate from a fresh base, not a stale one.
                pos_ms = ps?.let {
                    if (it.state == PlaybackState.STATE_PLAYING) {
                        it.position + ((
                            android.os.SystemClock.elapsedRealtime() -
                                it.lastPositionUpdateTime
                            ) * it.playbackSpeed).toLong()
                    } else {
                        it.position
                    }
                } ?: 0,
                dur_ms = md?.getLong(MediaMetadata.METADATA_KEY_DURATION) ?: 0,
                art_b64 = artB64(md),
                shuffle = controlState(c, "shuffle")?.let { it != 0 },
                loop_status = controlState(c, "repeat", "loop")?.let {
                    when (it) {
                        0 -> "none"
                        1 -> "track"
                        else -> "playlist"
                    }
                },
                ts = System.currentTimeMillis() / 1000,
            ),
        )
    }

    /**
     * Album art → PNG → b64, only when it changed since the last publish.
     * `Some("")` clears stale art on the laptop; `null` keeps it (heartbeat
     * updates don't resend the image).
     */
    private fun artB64(md: MediaMetadata?): String? {
        val bmp = md?.getBitmap(MediaMetadata.METADATA_KEY_ALBUM_ART)
            ?: md?.getBitmap(MediaMetadata.METADATA_KEY_ART)
        if (bmp == null) {
            val was = lastArtHash
            lastArtHash = null
            return if (was == null) null else "" // tell laptops to drop it
        }
        val marker = bmp.generationId
        if (lastArtHash == marker) return null
        lastArtHash = marker
        val scale = minOf(1f, ART_PX.toFloat() / maxOf(bmp.width, bmp.height, 1))
        val small = if (scale < 1f) {
            Bitmap.createScaledBitmap(bmp, (bmp.width * scale).toInt(), (bmp.height * scale).toInt(), true)
        } else bmp
        val out = ByteArrayOutputStream()
        // JPEG ~5-10x smaller than PNG for photo art — less radio time.
        small.compress(Bitmap.CompressFormat.JPEG, 82, out)
        if (small !== bmp) small.recycle()
        return Base64.encodeToString(out.toByteArray(), Base64.NO_WRAP)
    }

    /**
     * Framework sessions expose shuffle/repeat only as custom actions
     * (no standard TransportControls op). Match by action/name needle.
     * Returns the mode int when the action name encodes a state (e.g.
     * "...SHUFFLE_ON", "...REPEAT_ONE"), else null.
     */
    private fun customAction(
        ps: PlaybackState?, vararg needles: String,
    ): PlaybackState.CustomAction? =
        ps?.customActions?.firstOrNull { ca ->
            needles.any { n ->
                ca.action.contains(n, true) ||
                    ca.name?.toString()?.contains(n, true) == true
            }
        }

    /**
     * Best-effort current value of a session toggle: extras key
     * ("shuffle"=bool/int, "repeat"/"loop"=int) or a state-encoding
     * custom action. null = unmeasurable → laptops keep last write.
     */
    private fun controlState(c: MediaController?, vararg keys: String): Int? {
        val e = c?.playbackState?.extras ?: return null
        for (k in keys) {
            if (e.containsKey(k)) {
                return runCatching {
                    e.getInt(k)
                }.getOrElse {
                    runCatching { if (e.getBoolean(k)) 1 else 0 }.getOrNull()
                } ?: continue
            }
        }
        return null
    }

    fun execute(cmd: Msg.MediaCommand) {
        val c = controller ?: return
        val tc = c.transportControls ?: return
        when (cmd.op) {
            "play" -> tc.play()
            "pause" -> tc.pause()
            "next" -> tc.skipToNext()
            "previous" -> tc.skipToPrevious()
            "seek" -> cmd.pos_ms?.let(tc::seekTo)
            // The player's own custom action performs the toggle/cycle —
            // murmur only fires it; the reported state lands on the next
            // publish so laptops converge on truth, not on what was asked.
            "shuffle" -> customAction(c.playbackState, "shuffle")?.let {
                tc.sendCustomAction(it.action, it.extras)
            }
            "loop" -> customAction(c.playbackState, "repeat", "loop")?.let {
                tc.sendCustomAction(it.action, it.extras)
            }
        }
    }
}
