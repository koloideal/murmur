package dev.murmur.core

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import androidx.core.content.edit
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Provisioning material: device id, auth token, family key, server url.
 * Token and family key are stored AES-GCM-encrypted under an Android
 * Keystore key that never leaves the TEE — a stolen prefs file is useless
 * off-device. Non-secret fields stay in plain SharedPreferences.
 */
class Secrets(context: Context) {

    private val prefs = context.getSharedPreferences("murmur", Context.MODE_PRIVATE)
    private val cipher = KeystoneCipher()

    init {
        // Cold start already provisioned → dashboard may show immediately.
        if (provisioned) settledFlow.value = true
    }

    val provisioned: Boolean
        get() = prefs.contains(KEY_FAMILY) && prefs.contains(KEY_TOKEN) &&
            prefs.contains(KEY_DEVICE)

    var deviceId: String?
        get() = prefs.getString(KEY_DEVICE, null)
        private set(v) = prefs.edit { putString(KEY_DEVICE, v) }

    var serverUrl: String
        get() = prefs.getString(KEY_URL, "") ?: ""
        set(v) = prefs.edit { putString(KEY_URL, v) }

    fun save(deviceId: String, authTokenHex: String, familyKeyBytes: ByteArray, serverUrl: String) {
        prefs.edit {
            putString(KEY_DEVICE, deviceId)
            putString(KEY_URL, serverUrl)
            putString(KEY_TOKEN, cipher.seal(authTokenHex.encodeToByteArray()))
            putString(KEY_FAMILY, cipher.seal(familyKeyBytes))
        }
    }

    fun authTokenHex(): String? =
        prefs.getString(KEY_TOKEN, null)?.let { cipher.open(it)?.decodeToString() }

    fun familyKey(): FamilyKey? =
        prefs.getString(KEY_FAMILY, null)?.let { cipher.open(it)?.let(FamilyKey::fromBytes) }

    /** App-package filter for notifications: allowlist, empty = all. */
    var notificationAllowlist: Set<String>
        get() = prefs.getStringSet(KEY_ALLOW, emptySet()) ?: emptySet()
        set(v) = prefs.edit { putStringSet(KEY_ALLOW, v) }

    /** Rotate the family key in place (KeyRotate broadcast). */
    fun rotateFamilyKey(newKey: ByteArray) {
        prefs.edit { putString(KEY_FAMILY, cipher.seal(newKey)) }
    }

    companion object {
        /** UI gate: true once the dashboard may be shown. Set on cold
         *  start when already provisioned, or by the pairing card when
         *  its welcome animation finishes — not by [save] itself, so the
         *  burst gets its moment before the swap. */
        val settledFlow = kotlinx.coroutines.flow.MutableStateFlow(false)

        private const val KEY_DEVICE = "device_id"
        private const val KEY_TOKEN = "auth_token_enc"
        private const val KEY_FAMILY = "family_key_enc"
        private const val KEY_URL = "server_url"
        private const val KEY_ALLOW = "notif_allowlist"
    }
}

/** AES/GCM under a hardware-backed key; stored blob = b64(iv ‖ ct ‖ tag). */
private class KeystoneCipher {

    private val key: SecretKey by lazy {
        val ks = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (ks.getEntry(ALIAS, null) as? KeyStore.SecretKeyEntry)?.secretKey
            ?: generateKey()
    }

    private fun generateKey(): SecretKey {
        val gen = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        gen.init(
            KeyGenParameterSpec.Builder(
                ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build(),
        )
        return gen.generateKey()
    }

    fun seal(plaintext: ByteArray): String {
        val c = Cipher.getInstance("AES/GCM/NoPadding")
        c.init(Cipher.ENCRYPT_MODE, key)
        val ct = c.doFinal(plaintext)
        return Base64.encodeToString(c.iv + ct, Base64.NO_WRAP)
    }

    fun open(b64: String): ByteArray? = runCatching {
        val blob = Base64.decode(b64, Base64.NO_WRAP)
        val c = Cipher.getInstance("AES/GCM/NoPadding")
        c.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, blob.copyOfRange(0, GCM_IV_LEN)))
        c.doFinal(blob.copyOfRange(GCM_IV_LEN, blob.size))
    }.getOrNull()

    private companion object {
        const val ALIAS = "murmur-secrets"
        const val GCM_IV_LEN = 12
    }
}
