package dev.murmur.core

/**
 * Duplicate-frame filter for the mesh — frames arrive over both the LAN
 * early-copy path and the relay; the duplicate collapses here. Pure and
 * Android-free so the JVM tests can pin the exact acceptance matrix
 * against murmurd's `Seen` (the Rust twin must agree bit-for-bit or the
 * mesh splits).
 *
 * [lastSeq] keeps the check correct across a peer restart: its seq
 * resets to 1, and a frame far below the high-water mark is fresh, not
 * a duplicate — so the sender's window resets.
 */
class Dedupe(
    private val restartGap: Long = RESTART_GAP,
    private val maxEntries: Int = SEEN_MAX,
) {
    private val seenSeq = LinkedHashSet<Pair<String, Long>>()
    private val lastSeq = HashMap<String, Long>()

    /** True exactly once per (from, seq) — the first sighting. */
    @Synchronized
    fun accept(from: String, seq: Long): Boolean {
        val last = lastSeq[from] ?: 0L
        if (seq <= last && last - seq > restartGap) {
            // seq jumped far backwards — the peer restarted; its stored
            // entries predate the restart and would swallow fresh frames.
            seenSeq.removeIf { it.first == from }
            lastSeq[from] = seq
        } else if (seq > last) {
            lastSeq[from] = seq
        }
        if (!seenSeq.add(from to seq)) return false
        if (seenSeq.size > maxEntries) seenSeq.remove(seenSeq.first())
        return true
    }

    companion object {
        const val SEEN_MAX = 4096
        const val RESTART_GAP = 64L
    }
}
