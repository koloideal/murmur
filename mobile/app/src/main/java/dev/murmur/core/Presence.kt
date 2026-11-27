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

    /** The laptop clipboard traffic follows — mirrors murmurd's
     *  elect_clip_target exactly (same inputs → same winner on every
     *  node, so precisely one laptop applies a push). NOT a strict
     *  `active` filter: the user's real flow is phone-in-hand with the
     *  laptop idling untouched, and dropping everything in that state
     *  was the "clipboard dies in both directions" bug. Tiers: active
     *  beats idle beats locked; inside a tier freshest ts wins, ties
     *  go to the smaller id. Anything silent past the stale cutoff is
     *  dead — a crashed laptop must not route clips to a ghost. */
    @Synchronized
    fun activeDevice(): String? {
        val cutoff = clockS() - staleAfterS
        fun rank(state: String) = when (state) {
            "active" -> 2
            "idle" -> 1
            else -> 0 // locked — last resort, never a drop while alive
        }
        return entries
            .filterValues { (_, ts) -> ts >= cutoff }
            .maxWithOrNull(
                compareBy<Map.Entry<String, Pair<String, Long>>> { rank(it.value.first) }
                    .thenBy { it.value.second }
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
