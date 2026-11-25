package dev.murmur

import android.service.quicksettings.Tile
import android.service.quicksettings.TileService
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.launch

/**
 * Quick Settings tile — the VPN-style switch: one tap starts the agent,
 * another exits it outright (no paused notification left behind).
 * State mirrors [MurmurService.status].
 */
class MurmurTileService : TileService() {

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    private var job: Job? = null

    override fun onStartListening() {
        job = scope.launch {
            MurmurService.status.collectLatest { st ->
                qsTile?.let { tile ->
                    tile.state = if (st.running) Tile.STATE_ACTIVE else Tile.STATE_INACTIVE
                    tile.subtitle = when {
                        !st.running -> "off"
                        // Serverless has no relay — a live link is online.
                        st.familyKind == "serverless" ->
                            if (st.lanPeers.isEmpty()) "offline" else "online"
                        else -> relayLabel(st.relay)
                    }
                    tile.updateTile()
                }
            }
        }
    }

    private fun relayLabel(r: dev.murmur.core.Relay.State) =
        when (r) {
            dev.murmur.core.Relay.State.ONLINE -> "online"
            dev.murmur.core.Relay.State.CONNECTING -> "connecting"
            dev.murmur.core.Relay.State.OFFLINE -> "offline"
            dev.murmur.core.Relay.State.REVOKED -> "removed"
        }

    override fun onStopListening() {
        job?.cancel()
        job = null
    }

    override fun onClick() {
        if (MurmurService.status.value.running) {
            MurmurService.stop(this)
        } else {
            MurmurService.start(this)
        }
    }

    override fun onDestroy() {
        scope.cancel()
        super.onDestroy()
    }
}
