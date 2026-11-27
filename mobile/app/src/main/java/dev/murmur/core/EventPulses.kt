package dev.murmur.core

import kotlinx.coroutines.flow.MutableSharedFlow

/** Real user-visible events the dashboard wave pulses on. One entry per
 *  genuine occurrence — emitted at the exact feature point where the
 *  event becomes true (a clip applied, a notification forwarded, a
 *  track boundary crossed), never from transport chatter: presence
 *  heartbeats, beacons, pings and reposts don't pass through those
 *  points by construction, so no dedup is needed downstream. */
enum class PulseKind { CLIP, NOTIFY, MEDIA }

object EventPulses {
    val flow = MutableSharedFlow<PulseKind>(extraBufferCapacity = 16)

    fun emit(kind: PulseKind) {
        flow.tryEmit(kind)
    }
}
