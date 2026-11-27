package dev.murmur.core

/**
 * Presence table + active-laptop election — the pure-Kotlin mirror of
 * murmurd's `elect_active`. No Android deps: JVM-testable. The phone
 * never broadcasts presence itself; it only tracks laptops to know
 * whose clipboard to apply and who commands route to.
 */
class PresenceTable(
    private val staleAfterS: Long = PRESENCE_STALE_S,
    private val clockS: () -> Long = { System.currentTimeMillis() / 1000 },
) {
    private val entries = mutableMapOf<String, Pair<String, Long>>()

    @Synchronized
    fun record(from: String, state: String, ts: Long) {
        entries[from] = state to ts
    }

    @Synchronized
    fun remove(id: String) {
        entries.remove(id)
    }

    /** Active laptop = newest `active` presence; ties → smaller id.
     *  Anything silent past the stale cutoff is dead — a crashed
     *  "active" laptop must not win forever or clips route to a ghost. */
    @Synchronized
    fun activeDevice(): String? {
        val cutoff = clockS() - staleAfterS
        return entries
            .filterValues { (state, ts) -> state == "active" && ts >= cutoff }
            .maxWithOrNull(
                compareBy<Map.Entry<String, Pair<String, Long>>> { it.value.second }
                    .thenByDescending { it.key },
            )
            ?.key
    }

    companion object {
        /** 2.5× the 60s presence heartbeat — mirrors murmurd's
         *  PRESENCE_STALE. Older entries lose the election. */
        const val PRESENCE_STALE_S = 150L
    }
}
