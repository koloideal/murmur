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
        val roster = secrets.roster
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
        val clip = msg as Msg.Clipboard
        android.util.Log.d("ClipboardBridge", "clip from ${clip.origin} active=${activeDevice()}")
        if (clip.origin == activeDevice() && secrets.clipEnabled && secrets.clipRecv) {
            ClipboardBridge.apply(ctx, clip)
        }
    }
}

/** Media commands from laptops execute on the phone's session. */
class MediaFeature(private val media: MediaBridge) : Feature {
    override val consumes = setOf("media_command")
    override fun onMessage(from: String, msg: Msg) = media.execute(msg as Msg.MediaCommand)
}

/** Laptop dismissed a notification mirror — drop the original. */
class NotifyFeature : Feature {
    override val consumes = setOf("notification_dismiss")
    override fun onMessage(from: String, msg: Msg) =
        NotificationBridge.requestCancel((msg as Msg.NotificationDismiss).key)
}

/** Presence frames feed the election table — nothing more. */
class PresenceFeature(private val table: PresenceTable) : Feature {
    override val consumes = setOf("presence")
    override fun onMessage(from: String, msg: Msg) {
        val p = msg as Msg.Presence
        table.record(from, p.state, p.ts)
    }
}
