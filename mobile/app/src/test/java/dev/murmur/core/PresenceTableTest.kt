package dev.murmur.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** Election parity with murmurd's elect_active tests — both sides must
 *  elect the same laptop from the same presence inputs or clips route
 *  differently per platform. */
class PresenceTableTest {
    private var now = 10_000L
    private fun table() = PresenceTable(clockS = { now })

    @Test
    fun newestActiveWins() {
        val t = table()
        t.record("aaa", "active", now - 10)
        t.record("bbb", "active", now - 5)
        assertEquals("bbb", t.activeDevice())
    }

    @Test
    fun tieBreaksToSmallerId() {
        val t = table()
        t.record("bbb", "active", now)
        t.record("aaa", "active", now)
        assertEquals("aaa", t.activeDevice())
    }

    @Test
    fun staleEntriesLoseTheElection() {
        val t = table()
        t.record("aaa", "active", now - PresenceTable.PRESENCE_STALE_S - 1)
        t.record("bbb", "active", now)
        assertEquals("bbb", t.activeDevice())
    }

    @Test
    fun crashedActiveNeverWinsAlone() {
        val t = table()
        t.record("aaa", "active", now - PresenceTable.PRESENCE_STALE_S - 1)
        assertNull(t.activeDevice())
    }

    @Test
    fun idleAndLockedDontElect() {
        val t = table()
        t.record("aaa", "idle", now)
        t.record("bbb", "locked", now)
        assertNull(t.activeDevice())
    }

    @Test
    fun reorderingConverges() {
        val a = table()
        val b = table()
        a.record("x1", "active", now - 3)
        a.record("x2", "active", now - 1)
        b.record("x2", "active", now - 1)
        b.record("x1", "active", now - 3)
        assertEquals(a.activeDevice(), b.activeDevice())
    }

    @Test
    fun revokedMembersAreForgotten() {
        val t = table()
        t.record("aaa", "active", now)
        t.remove("aaa")
        assertNull(t.activeDevice())
    }
}
