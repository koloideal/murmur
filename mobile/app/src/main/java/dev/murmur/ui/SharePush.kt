package dev.murmur.ui

import android.content.Context
import dev.murmur.MurmurService
import dev.murmur.core.PendingPush
import dev.murmur.core.Relay
import dev.murmur.core.Secrets

/**
 * The one and only decision tree for "user pushed text at us" — the
 * share sheet and the selection-menu action used to duplicate it and
 * drifted apart (one queued offline pushes, the other dropped them).
 *
 * The queue is the universal answer to "can't deliver right now":
 * offline is transient by nature, so nothing the user explicitly
 * pushed should ever be dropped for it. The only hard drops are the
 * states the user owns: never paired, or clipboard sharing turned off.
 */
object SharePush {

    /** Returns the toast text to show — the activities are dumb shells. */
    fun handle(ctx: Context, text: String?): String {
        if (text.isNullOrBlank()) return "nothing to send"
        val sec = Secrets(ctx)
        if (sec.deviceId == null) {
            return "pair the phone with your laptop first — open murmur"
        }
        val svc = MurmurService.instance
        if (svc != null) {
            val st = MurmurService.status.value
            // Serverless families have no relay at all — a live LAN peer
            // is enough to deliver the push.
            val online = st.relay == Relay.State.ONLINE || st.lanPeers.isNotEmpty()
            if (online) {
                if (!sec.clipEnabled || !sec.clipSend) {
                    return "clipboard sharing is off in murmur settings"
                }
                svc.pushClipboard(text)
                return "sent to laptop"
            }
        }
        // Service dead or transport down — same answer: queue and wake.
        PendingPush.enqueue(ctx, text)
        MurmurService.start(ctx)
        return "queued — sends when connected"
    }
}
