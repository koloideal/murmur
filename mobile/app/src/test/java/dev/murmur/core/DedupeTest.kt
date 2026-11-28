package dev.murmur.core

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Pins the acceptance matrix against murmurd's `Seen` — the two
 * implementations must agree bit-for-bit on every sequence or the mesh
 * splits (one side floods a frame the other swallows).
 */
class DedupeTest {

    @Test
    fun firstSeenDeliversTwinDies() {
        val d = Dedupe()
        assertTrue(d.accept("aa", 1))
        assertFalse(d.accept("aa", 1)) // the LAN/relay twin collapses
        assertTrue(d.accept("aa", 2))
        assertFalse(d.accept("aa", 2))
    }

    @Test
    fun outOfOrderWithinWindowIsStillDupe() {
        val d = Dedupe()
        assertTrue(d.accept("aa", 10))
        assertTrue(d.accept("aa", 7))  // late arrival, never seen — delivers
        assertFalse(d.accept("aa", 7)) // but only once
        assertFalse(d.accept("aa", 10))
    }

    @Test
    fun peersDoNotShareWindows() {
        val d = Dedupe()
        assertTrue(d.accept("aa", 1))
        assertTrue(d.accept("bb", 1)) // same seq, different sender — fresh
        assertFalse(d.accept("bb", 1))
    }

    @Test
    fun restartGapResetsSenderWindow() {
        val d = Dedupe(restartGap = 64)
        // Peer boots its seq up high, then restarts back at 1.
        assertTrue(d.accept("aa", 500))
        assertTrue(d.accept("aa", 3)) // far below high-water — fresh, not dupe
        assertFalse(d.accept("aa", 3))
        // after the reset the window tracks the new epoch normally
        assertTrue(d.accept("aa", 4))
        // A straggler from before the restart was wiped with the window —
        // it delivers again. Honest cost both sides pay identically:
        // murmurd's Seen resets the same way, so the mesh never splits.
        assertTrue(d.accept("aa", 500))
    }

    @Test
    fun boundedWindowEvictsOldest() {
        val d = Dedupe(maxEntries = 8)
        repeat(8) { assertTrue(d.accept("aa", it.toLong())) }
        // window full — a ninth distinct frame evicts the oldest entry
        assertTrue(d.accept("aa", 8))
        // the evicted (aa, 0) is no longer remembered: it would deliver
        // again — accepted cost of a bounded window, same as murmurd.
        assertTrue(d.accept("aa", 0))
        // but everything still inside the window stays deduped
        assertFalse(d.accept("aa", 8))
    }

    @Test
    fun smallBackwardStepIsDupeNotRestart() {
        val d = Dedupe(restartGap = 64)
        assertTrue(d.accept("aa", 100))
        // a 10-step reordering is far inside the restart gap — must NOT
        // wipe the window
        assertTrue(d.accept("aa", 90))
        assertFalse(d.accept("aa", 100)) // original still deduped
    }
}
