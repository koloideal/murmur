@file:OptIn(kotlinx.serialization.ExperimentalSerializationApi::class)

package dev.murmur.core

import android.util.Base64
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonClassDiscriminator

/**
 * Wire protocol, mirror of murmur-proto (Rust). Keep field names and the
 * `t`/`c` discriminators in sync — the Rust enums serialize identically.
 *
 * Two layers:
 * - [Control]: cleartext, device ↔ server and pairing.
 * - [Msg]: E2E payload, always sealed inside [Envelope.payload].
 */

val protoJson = Json {
    ignoreUnknownKeys = true
    explicitNulls = false
}

@Serializable
data class Envelope(
    val v: Int,
    val from: String,
    val to: String, // "*" | "@server" | "<device-hex>"
    val seq: Long,
    val payload: String, // base64: ciphertext, or cleartext Control JSON
) {
    fun toJson(): String = protoJson.encodeToString(this)

    fun controlPayload(): Control? = runCatching {
        protoJson.decodeFromString<Control>(Base64.decode(payload, Base64.NO_WRAP).decodeToString())
    }.getOrNull()

    /** Drop-check for ingress — a peer on another protocol generation
     *  can't be reasoned with; its frames die loudly, not silently. */
    fun protoMismatch(): Boolean = v != VERSION

    companion object {
        fun fromJson(s: String): Envelope {
            require(s.length <= MAX_ENVELOPE_JSON) {
                "envelope over wire cap (${s.length}B > ${MAX_ENVELOPE_JSON}B)"
            }
            return protoJson.decodeFromString(s)
        }
        fun wrap(from: String, to: String, seq: Long, ctl: Control) = Envelope(
            v = VERSION,
            from = from,
            to = to,
            seq = seq,
            payload = Base64.encodeToString(
                protoJson.encodeToString(ctl).encodeToByteArray(),
                Base64.NO_WRAP,
            ),
        )

        const val SERVER = "@server"
        const val ALL = "*"
        const val VERSION = 1
        /** Mirror of murmur-proto: 4 MiB sealed payload cap. */
        const val MAX_PAYLOAD = 4 * 1024 * 1024
        /** Mirror of murmur-proto: JSON envelope cap (b64 inflates ~4/3). */
        const val MAX_ENVELOPE_JSON = 6 * 1024 * 1024
        /** Mirror of murmur-proto: clipboard payload cap — drop, never truncate. */
        const val MAX_CLIP = 2 * 1024 * 1024
    }
}

/** One thing a phone notification offers to do — mirror of murmur-proto's
 *  NotifAction. `index` round-trips back in action/reply frames so the
 *  right PendingIntent fires without re-scanning titles. */
@Serializable
data class NotifAction(
    val index: Int,
    val title: String,
    /** "reply" — carries a RemoteInput, takes text; "action" — plain button. */
    val kind: String,
)

/** E2E messages — decrypted payload. Rust serde `tag = "t"`. */
@Serializable
@JsonClassDiscriminator("t")
sealed class Msg {
    @Serializable
    @SerialName("presence")
    data class Presence(val state: String, val ts: Long) : Msg()

    @Serializable
    @SerialName("notification")
    data class Notification(
        val key: String,
        val pkg: String,
        val app: String? = null,
        val title: String,
        val body: String,
        val icon_b64: String? = null,
        val actions: List<NotifAction> = emptyList(),
        val ts: Long,
    ) : Msg()

    @Serializable
    @SerialName("notification_dismiss")
    data class NotificationDismiss(val key: String) : Msg()

    @Serializable
    @SerialName("notification_invoke")
    data class NotificationInvoke(val key: String) : Msg()

    @Serializable
    @SerialName("notification_action")
    data class NotificationAction(val key: String, val index: Int) : Msg()

    @Serializable
    @SerialName("notification_reply")
    data class NotificationReply(val key: String, val text: String) : Msg()

    @Serializable
    @SerialName("media_state")
    data class MediaState(
        val player: String,
        val title: String,
        val artist: String,
        val album: String,
        val status: String,
        val pos_ms: Long,
        val dur_ms: Long,
        val art_b64: String? = null,
        /** Phone-reported shuffle (null = session doesn't expose it). */
        val shuffle: Boolean? = null,
        /** Phone-reported repeat: "none" | "track" | "playlist". */
        val loop_status: String? = null,
        val ts: Long,
    ) : Msg()

    /**
     * Rust `MediaCommand { #[serde(flatten)] op: MediaOp }` — flat fields:
     * `{"t":"media_command","op":"play"}` / `"op":"seek","pos_ms":N` /
     * `"op":"shuffle","on":B` / `"op":"loop","mode":"none|track|playlist"`.
     */
    @Serializable
    @SerialName("media_command")
    data class MediaCommand(
        val op: String,
        val pos_ms: Long? = null,
        val on: Boolean? = null,
        val mode: String? = null,
    ) : Msg()

    @Serializable
    @SerialName("clipboard")
    data class Clipboard(
        val origin: String,
        // u64 on the wire — a plain Long overflows on half of all
        // FNV hashes and the whole message silently fails to decode.
        val hash: ULong,
        val mime: String,
        val data_b64: String,
        val explicit: Boolean = false,
        val ts: Long,
    ) : Msg()

    @Serializable
    @SerialName("clipboard_clear")
    data class ClipboardClear(
        val origin: String,
        val ts: Long,
    ) : Msg()

    /** Long-term identity pubkey, announced after every connect.
     *  `name` carries roster gossip a serverless family has no relay
     *  registry for — absent in old frames. */
    @Serializable
    @SerialName("identity")
    data class Identity(val pub_b64: String, val kind: String, val name: String? = null, val caps: List<String> = emptyList()) : Msg()

    /** Sealed beacon — rides UDP broadcast and the relay; the envelope
     *  itself is the auth. `lan_addr` lets relay receivers dial back —
     *  a relay envelope carries no usable source IP. */
    @Serializable
    @SerialName("lan_beacon")
    data class LanBeacon(val tcp_port: Int, val kind: String, val lan_addr: String? = null) : Msg()

    /** LAN-link keepalive — echoed back as [LanPong] with the same
     *  ts; the pinger measures RTT on its own clock. Handled inside
     *  the transport, never reaches the service's dispatch. */
    @Serializable
    @SerialName("lan_ping")
    data class LanPing(val ts: Long) : Msg()

    @Serializable
    @SerialName("lan_pong")
    data class LanPong(val ts: Long) : Msg()

    /** New family key, sealed once per member to its identity key. */
    @Serializable
    @SerialName("key_rotate")
    data class KeyRotate(
        val epoch: Long,
        val revoked: List<String> = emptyList(),
        val keys: List<SealedKey>,
    ) : Msg()
}

@Serializable
data class SealedKey(val device_id: String, val sealed_b64: String)

/** Cleartext control-plane. Rust serde `tag = "c"`. */
@Serializable
@JsonClassDiscriminator("c")
sealed class Control {
    @Serializable
    @SerialName("hello")
    data class Hello(
        val device_id: String,
        val name: String,
        val role: String, // "auth" | "pair"
        val token_hex: String? = null,
        /** Protocol generation we speak — the relay rejects a mismatch
         *  up front instead of silently dropping our frames forever. */
        val proto_v: Int? = null,
    ) : Control()

    @Serializable
    @SerialName("hello_ok")
    object HelloOk : Control()

    @Serializable
    @SerialName("hello_err")
    data class HelloErr(val reason: String) : Control()

    @Serializable
    @SerialName("pair_hello")
    data class PairHello(val pubkey_b64: String, val name: String) : Control()

    @Serializable
    @SerialName("pair_accept")
    data class PairAccept(val pubkey_b64: String) : Control()

    @Serializable
    @SerialName("pair_data")
    data class PairData(val blob_b64: String) : Control()

    @SerialName("pair_reject")
    data class PairReject(val reason: String) : Control()

    @Serializable
    @SerialName("list_devices")
    object ListDevices : Control()

    @Serializable
    @SerialName("devices")
    data class Devices(
        val devices: List<DeviceInfo>,
        val family_name: String? = null,
    ) : Control()

    @Serializable
    @SerialName("rename_device")
    data class RenameDevice(val device_id: String, val name: String) : Control()

    @Serializable
    @SerialName("revoke_device")
    data class RevokeDevice(val device_id: String) : Control()
}

/** Registry entry as the server reports it in `Control.Devices`. */
@Serializable
data class DeviceInfo(
    val device_id: String,
    val name: String,
    val revoked: Boolean,
    val online: Boolean,
    val rtt_ms: Long? = null,
)

/** Invite bundle — base64url JSON `{s,i,p,k}`, same as murmur_proto::Invite. */
@Serializable
data class Invite(
    @SerialName("s") val serverUrl: String,
    @SerialName("i") val inviter: String,
    @SerialName("p") val session: String,
    @SerialName("c") val code: String = "",
    /** Family kind — explicit, never inferred; absent in old
     *  (all-serverful) invites. */
    @SerialName("k") val kind: String = "serverful",
) {
    companion object {
        fun decode(code: String): Invite? = runCatching {
            val json = Base64.decode(code.trim(), Base64.URL_SAFE or Base64.NO_WRAP)
            protoJson.decodeFromString<Invite>(json.decodeToString())
        }.getOrNull()
    }
}

/** DH-sealed credentials inside `Control.PairData`. */
@Serializable
data class PairBlob(
    val device_id: String,
    val token_hex: String,
    val family_key_b64: String,
    val server_url: String,
    val key_epoch: Long = 0,
    val family_name: String? = null,
    /** Family kind — explicit; absent in old (all-serverful) blobs. */
    val kind: String = "serverful",
)
