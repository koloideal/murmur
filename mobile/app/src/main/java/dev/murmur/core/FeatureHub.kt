package dev.murmur.core

/**
 * Feature dispatch — the phone-side mirror of murmurd's feature.rs.
 * Features declare the wire tags they consume; the hub routes by tag
 * in O(1) instead of one growing `when` in the service. Adding a
 * feature never touches the transport pipeline: write the feature,
 * wire it, done.
 */
interface Feature {
    /** Wire tags this feature consumes — the `t` discriminator values. */
    val consumes: Set<String>
    fun onMessage(from: String, msg: Msg)
}

class FeatureHub {
    private val sinks = mutableMapOf<String, MutableList<Feature>>()

    fun wire(feature: Feature) {
        for (tag in feature.consumes) {
            sinks.getOrPut(tag) { mutableListOf() }.add(feature)
        }
    }

    fun dispatch(from: String, msg: Msg) {
        sinks[msg.tag()]?.forEach { it.onMessage(from, msg) }
    }
}

/** The serde wire tag of every message variant — compile-enforced
 *  mirror of murmur-proto's `Message::tag`: an unmapped new variant
 *  fails to compile here, exactly like there. */
fun Msg.tag(): String = when (this) {
    is Msg.Presence -> "presence"
    is Msg.Notification -> "notification"
    is Msg.NotificationDismiss -> "notification_dismiss"
    is Msg.MediaState -> "media_state"
    is Msg.MediaCommand -> "media_command"
    is Msg.Clipboard -> "clipboard"
    is Msg.Identity -> "identity"
    is Msg.LanBeacon -> "lan_beacon"
    is Msg.LanPing -> "lan_ping"
    is Msg.LanPong -> "lan_pong"
    is Msg.KeyRotate -> "key_rotate"
}
