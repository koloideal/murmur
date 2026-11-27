package dev.murmur.feature

import android.content.Context
import dev.murmur.bridge.ClipboardBridge
import dev.murmur.bridge.MediaBridge
import dev.murmur.bridge.NotificationBridge
import dev.murmur.core.Envelope
import dev.murmur.core.FamilyKey
import dev.murmur.core.Feature
import dev.murmur.core.IdentityKey
import dev.murmur.core.Msg
import dev.murmur.core.PresenceTable
import dev.murmur.core.Secrets

/** Family control plane: TOFU-pin member identities, adopt key
 *  rotations sealed to ours. Mirrors murmurd's keyring feature. */
class KeyringFeature(
    private val secrets: Secrets,
    private val identity: IdentityKey,
    private val presence: PresenceTable,
    private val selfId: String,
    private val send: (Msg, String) -> Unit,
    /** The transport key itself rotates — the service swaps it in. */
    private val onKeyRotated: (newKey: FamilyKey, epoch: Long) -> Unit,
    /** A rotation arrived that revokes *us* — the family kicked this
     *  device. The service stops itself; there is nothing to retry. */
    private val onKicked: () -> Unit = {},
) : Feature {
    override val consumes = setOf("identity", "key_rotate")

    override fun onMessage(from: String, msg: Msg) = when (msg) {
        is Msg.Identity -> pin(from, msg)
        is Msg.KeyRotate -> applyRotation(from, msg)
        else -> Unit
    }

    /** TOFU: the first identity a member announces under the current
     *  family key is the one we trust for its key rotations. */
    private fun pin(from: String, id: Msg.Identity) {
        if (from == selfId || from in secrets.revokedIds) return
        id.name?.let { secrets.peerNames = secrets.peerNames + (from to it) }
        var roster = secrets.roster
        // Re-pairing reuse: the inviter mints a fresh device id per
        // pairing, but the long-term identity key survives — the same
        // physical device reappears under a new id with a pubkey the
        // roster already knows. Keeping both ghosts the stale one.
        val replaced = roster.entries
            .filter { it.key != from && it.value.first == id.pub_b64 }
            .map { it.key }
        if (replaced.isNotEmpty()) {
            android.util.Log.i("KeyringFeature", "pin: $from replaces $replaced — same identity key")
            roster = roster - replaced
        }
        if (roster.containsKey(from)) return
        secrets.roster = roster + (from to (id.pub_b64 to id.kind))
        announceBack(from)
    }

    /** Adopt a rotation sealed to our identity by a pinned laptop.
     *  Newer epoch wins; same epoch → smaller rotator id (two laptops
     *  rotating at once still converge). */
    private fun applyRotation(from: String, r: Msg.KeyRotate) {
        val cur = secrets.keyEpoch
        val rot = secrets.keyRotator
        val newer = r.epoch > cur || (r.epoch == cur && rot != null && from < rot)
        if (!newer) return
        if (selfId in r.revoked) {
            android.util.Log.w("KeyringFeature", "this device was revoked by the rotation — kicked")
            onKicked()
            return
        }
        val rotatorPub = secrets.roster[from]?.first ?: return
        val mine = r.keys.firstOrNull { it.device_id == selfId } ?: return
        val newKey = identity.openRekey(rotatorPub, r.epoch, mine.sealed_b64) ?: return
        secrets.rotateFamilyKey(newKey, r.epoch, from, r.revoked)
        r.revoked.forEach { presence.remove(it) }
        onKeyRotated(FamilyKey.fromBytes(newKey), r.epoch)
        send(
            Msg.Identity(identity.publicB64, "phone", secrets.deviceName, PHONE_CAPS),
            Envelope.ALL,
        )
    }

    private fun announceBack(to: String) =
        send(Msg.Identity(identity.publicB64, "phone", secrets.deviceName, PHONE_CAPS), to)

    companion object {
        val PHONE_CAPS = listOf("clipboard", "media", "notify")
    }
}

/** Clipboard inbound: apply only what the active laptop sent. */
class ClipboardFeature(
    private val ctx: Context,
    private val secrets: Secrets,
    private val activeDevice: () -> String?,
) : Feature {
    override val consumes = setOf("clipboard")

    override fun onMessage(from: String, msg: Msg) {
        when (msg) {
            is Msg.Clipboard -> {
                android.util.Log.d("ClipboardBridge", "clip from ${msg.origin} active=${activeDevice()}")
                if (msg.origin == activeDevice() && secrets.clipEnabled && secrets.clipRecv) {
                    ClipboardBridge.apply(ctx, msg)
                }
            }
            // Same gate as the content it erases — only the active node's
            // clear is honored, anything else is some idle laptop's hygiene.
            is Msg.ClipboardClear ->
                if (msg.origin == activeDevice() && secrets.clipEnabled && secrets.clipRecv) {
                    ClipboardBridge.applyClear(ctx)
                }
            else -> Unit
        }
    }
}

/** Media commands from laptops execute on the phone's session. */
class MediaFeature(private val media: MediaBridge) : Feature {
    override val consumes = setOf("media_command")
    override fun onMessage(from: String, msg: Msg) = media.execute(msg as Msg.MediaCommand)
}

/** Laptop dismissed a notification mirror — drop the original. */
class NotifyFeature(private val secrets: Secrets) : Feature {
    override val consumes = setOf(
        "notification_dismiss", "notification_invoke",
        "notification_action", "notification_reply",
    )

    override fun onMessage(from: String, msg: Msg) = when (msg) {
        // The dismiss toggle is the user's: some want the laptop close
        // to clean the phone too, some use the laptop as a read-only
        // triage surface. Default on — one mental inbox is the point.
        is Msg.NotificationDismiss ->
            if (secrets.dismissRemote) NotificationBridge.requestCancel(msg.key) else Unit
        is Msg.NotificationInvoke -> NotificationBridge.performOpen(msg.key)
        is Msg.NotificationAction -> NotificationBridge.performAction(msg.key, msg.index)
        is Msg.NotificationReply -> NotificationBridge.performReply(msg.key, msg.text)
        else -> Unit
    }
}

/** Presence frames feed the election table — nothing more. */
class PresenceFeature(private val table: PresenceTable) : Feature {
    override val consumes = setOf("presence")
    override fun onMessage(from: String, msg: Msg) {
        val p = msg as Msg.Presence
        table.record(from, p.state, p.ts)
    }
}
