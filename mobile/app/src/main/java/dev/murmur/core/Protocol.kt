package dev.murmur.core

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
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
    classDiscriminator = "#kind" // replaced per-type below; see encode helpers
    ignoreUnknownKeys = true
}

@Serializable
data class Envelope(
    val v: Int,
    val from: String,
    val to: String,           // "*" | "@server" | "<device-hex>"
    val seq: Long,
    val payload: String,      // base64: ciphertext, or cleartext Control JSON
) {
    fun toJson(): String = protoJson.encodeToString(this)

    companion object {
        fun fromJson(s: String): Envelope = protoJson.decodeFromString(s)
        const val SERVER = "@server"
        const val ALL = "*"
        const val VERSION = 1
    }
}

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
        val title: String,
        val body: String,
        val icon_b64: String? = null,
        val ts: Long,
    ) : Msg()

    @Serializable
    @SerialName("notification_dismiss")
    data class NotificationDismiss(val key: String) : Msg()

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
        val ts: Long,
    ) : Msg()

    @Serializable
    @SerialName("media_command")
    data class MediaCommand(val op: String, val pos_ms: Long? = null) : Msg()

    @Serializable
    @SerialName("clipboard")
    data class Clipboard(
        val origin: String,
        val hash: Long,
        val mime: String,
        val data_b64: String,
        val ts: Long,
    ) : Msg()

    @Serializable
    @SerialName("key_rotate")
    data class KeyRotate(val new_key_b64: String) : Msg()
}

// Note: MediaOp on the Rust side is an internally-tagged enum
// ({op: play|pause|next|previous|seek}). Serialized here as plain fields
// on MediaCommand; decode op+pos_ms accordingly. Align when implementing.

/** Cleartext control-plane. Rust serde `tag = "c"`. */
@Serializable
@JsonClassDiscriminator("c")
sealed class Control {
    @Serializable
    @SerialName("hello")
    data class Hello(
        val device_id: String,
        val name: String,
        val role: String,        // "auth" | "pair"
        val token_hex: String? = null,
    ) : Control()

    @Serializable
    @SerialName("hello_ok")
    object HelloOk : Control()

    @Serializable
    @SerialName("hello_err")
    data class HelloErr(val reason: String) : Control()

    @Serializable
    @SerialName("pair_hello")
    data class PairHello(val pubkey_b64: String) : Control()

    @Serializable
    @SerialName("pair_accept")
    data class PairAccept(val pubkey_b64: String) : Control()

    @Serializable
    @SerialName("pair_data")
    data class PairData(val blob_b64: String) : Control()
}
