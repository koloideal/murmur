package dev.murmur.core

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * The simultaneous-dial tie-break must be mirror-symmetric with
 * murmurd's `winner_dir` or the two sides close different sockets and
 * the pair flaps forever. Rust pins its side in
 * `lan_adopt_tiebreak_matrix` — this pins the Kotlin mirror.
 */
class TieBreakTest {

    private val me = "13f1db90123e5ddd"
    private val biggerPeer = "4a57cb2a8f3c6eff" // lexically greater? checked below
    private val smallerPeer = "0a57cb2a8f3c6eff"

    @Test
    fun smallerIdDialsWinnerIsOutboundForBiggerPeer() {
        // peer > me → I am the smaller id → the surviving socket is the
        // one I dialed: outbound for me.
        assertEquals(
            LanLink.Dir.Outbound,
            LanLink.winnerDirOf(smallerPeer, me),
        )
    }

    @Test
    fun biggerIdNeverDialsWinnerIsInbound() {
        // peer < me → the peer is the smaller id → it dials, I accept:
        // inbound survives for me.
        assertEquals(
            LanLink.Dir.Inbound,
            LanLink.winnerDirOf(me, smallerPeer),
        )
    }

    @Test
    fun mirrorSymmetryBothSidesPickTheSameSocket() {
        // For any pair (a, b): a's outbound IS b's inbound — the same
        // physical socket. The rule converges only if exactly one side
        // sees its own outbound as winner.
        val pairs = listOf(
            "13f1db90123e5ddd" to "4a57cb2a8f3c6eff",
            "4a57cb2a8f3c6eff" to "13f1db90123e5ddd",
            "ecfe5dbac62c3a17" to "13f1db90123e5ddd",
        )
        for ((a, b) in pairs) {
            val aWin = LanLink.winnerDirOf(a, b)
            val bWin = LanLink.winnerDirOf(b, a)
            // winner directions must be opposite ends of one socket:
            // exactly one of them is Outbound.
            val outboundCount = listOf(aWin, bWin).count { it == LanLink.Dir.Outbound }
            assertEquals("pair $a/$b must keep exactly one dialed socket", 1, outboundCount)
        }
    }
}
