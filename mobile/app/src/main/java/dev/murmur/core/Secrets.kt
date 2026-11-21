package dev.murmur.core

import android.content.Context
import androidx.core.content.edit

/**
 * Provisioning material received during pairing: device id, auth token,
 * family key. App-private storage — the Android sandbox is the protection.
 * TODO(security): wrap family key with Android Keystore before release.
 */
class Secrets(context: Context) {

    private val prefs = context.getSharedPreferences("murmur", Context.MODE_PRIVATE)

    val provisioned: Boolean
        get() = prefs.contains(KEY_FAMILY)

    var deviceId: String?
        get() = prefs.getString(KEY_DEVICE, null)
        private set(v) = prefs.edit { putString(KEY_DEVICE, v) }

    var serverUrl: String
        get() = prefs.getString(KEY_URL, "wss://murmur.example.org/murmur")!!
        set(v) = prefs.edit { putString(KEY_URL, v) }

    fun save(deviceId: String, authTokenHex: String, familyKeyHex: String) {
        prefs.edit {
            putString(KEY_DEVICE, deviceId)
            putString(KEY_TOKEN, authTokenHex)
            putString(KEY_FAMILY, familyKeyHex)
        }
    }

    fun authTokenHex(): String? = prefs.getString(KEY_TOKEN, null)

    fun familyKey(): FamilyKey? =
        prefs.getString(KEY_FAMILY, null)?.let(FamilyKey::fromHex)

    /** App-package filter for notifications: "pkg1,pkg2" allowlist. */
    var notificationAllowlist: Set<String>
        get() = prefs.getStringSet(KEY_ALLOW, emptySet()) ?: emptySet()
        set(v) = prefs.edit { putStringSet(KEY_ALLOW, v) }

    private companion object {
        const val KEY_DEVICE = "device_id"
        const val KEY_TOKEN = "auth_token_hex"
        const val KEY_FAMILY = "family_key_hex"
        const val KEY_URL = "server_url"
        const val KEY_ALLOW = "notif_allowlist"
    }
}
