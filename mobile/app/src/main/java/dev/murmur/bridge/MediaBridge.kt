package dev.murmur.bridge

import android.content.ComponentName
import android.content.Context
import android.media.MediaMetadata
import android.media.session.MediaController
import android.media.session.MediaSessionManager
import android.media.session.PlaybackState
import android.util.Base64
import dev.murmur.core.Msg
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

    private val sessionListener =
        MediaSessionManager.OnActiveSessionsChangedListener { controllers ->
            bind(controllers?.firstOrNull())
        }

    private val callback = object : MediaController.Callback() {
        override fun onMetadataChanged(metadata: MediaMetadata?) = publishNow()
        override fun onPlaybackStateChanged(state: PlaybackState?) {
            publishNow()
            restartTicker()
        }
        override fun onSessionDestroyed() {
            controller = null
            publishNow()
        }
    }

    init {
        runCatching {
            manager.addOnActiveSessionsChangedListener(sessionListener, listenerName)
            bind(manager.getActiveSessions(listenerName).firstOrNull())
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
                delay(5_000)
                publishNow()
            }
        }
    }

    /** Snapshot → Msg.MediaState → all laptops. */
    fun publishNow() {
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
                pos_ms = ps?.position ?: 0,
                dur_ms = md?.getLong(MediaMetadata.METADATA_KEY_DURATION) ?: 0,
                art_b64 = null, // TODO: md.art bitmap → downscale → PNG → b64
                ts = System.currentTimeMillis() / 1000,
            ),
        )
    }

    fun execute(cmd: Msg.MediaCommand) {
        val tc = controller?.transportControls ?: return
        when (cmd.op) {
            "play" -> tc.play()
            "pause" -> tc.pause()
            "next" -> tc.skipToNext()
            "previous" -> tc.skipToPrevious()
            "seek" -> cmd.pos_ms?.let(tc::seekTo)
        }
    }
}
