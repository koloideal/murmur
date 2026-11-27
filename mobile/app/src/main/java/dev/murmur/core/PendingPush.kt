package dev.murmur.core

import android.content.Context

/**
 * Shares that arrived while the service was down. One SharedPreferences
 * file with a string set — the service drains it once a transport is
 * live, so a share to a sleeping phone is queued, not lost.
 */
object PendingPush {
    private const val PREFS = "murmur_pending"
    private const val KEY = "texts"

    fun enqueue(ctx: Context, text: String) {
        val p = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        p.edit().putStringSet(KEY, p.getStringSet(KEY, emptySet())!! + text).apply()
    }

    /** Everything queued — returned and cleared atomically. */
    fun drain(ctx: Context): List<String> {
        val p = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val out = p.getStringSet(KEY, emptySet())!!.toList()
        if (out.isNotEmpty()) p.edit().remove(KEY).apply()
        return out
    }
}
