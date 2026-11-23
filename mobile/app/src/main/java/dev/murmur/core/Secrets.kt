package dev.murmur.core

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import androidx.core.content.edit
import kotlinx.serialization.decodeFromString
import kotlinx.serialization.encodeToString
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
class Secrets(private val ctx: Context) {

    private val prefs = ctx.getSharedPreferences("murmur", Context.MODE_PRIVATE)
    private val cipher = KeystoneCipher()

    init {
        // Cold start already provisioned → dashboard may show immediately.
        if (provisioned) settledFlow.value = true
        mutedFlow.value = mutedApps
        removedFlow.value = removed
        hideTextFlow.value = hideNotificationText
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

    /** Family kind — "serverful" | "serverless", chosen at family
     *  creation and carried by the invite. Explicit, never inferred
     *  from serverUrl: old installs default to "serverful". */
    var familyKind: String
        get() = prefs.getString(KEY_FAMILY_KIND, "serverful") ?: "serverful"
        set(v) = prefs.edit { putString(KEY_FAMILY_KIND, v) }

    /** Display name for this device. Defaults to the system device name
     *  ("device_name" is OEM-set, e.g. "Galaxy S26 Ultra"), then the
     *  build model. Editable from the dashboard; the service pushes it
     *  to the server with RenameDevice. */
    var deviceName: String
        get() = (prefs.getString(KEY_NAME, null) ?: systemDeviceName().also { deviceName = it })
            // The side-by-side debug install is a second family member
            // on the same handset — mark it even when the name was
            // cached before the suffix existed.
            .let { if (ctx.packageName.endsWith(".debug") && !it.endsWith("·dev")) "$it·dev" else it }
        set(v) = prefs.edit { putString(KEY_NAME, v) }

    private fun systemDeviceName(): String {
        val sys = runCatching {
            android.provider.Settings.Global.getString(
                ctx.contentResolver, "device_name",
            )
        }.getOrNull()
        val base = sys?.takeIf { it.isNotBlank() } ?: android.os.Build.MODEL
        // The side-by-side debug install is a second family member on
        // the same handset — mark it so the device list reads sanely.
        return if (ctx.packageName.endsWith(".debug")) "$base·dev" else base
    }

    /** Full local logout: prefs + the Keystore key material. */
    fun clear() {
        prefs.edit { clear() }
        KeystoneCipher.deleteKey()
        settledFlow.value = false
        removedFlow.value = false
    }

    fun save(
        deviceId: String,
        authTokenHex: String,
        familyKeyBytes: ByteArray,
        serverUrl: String,
        keyEpoch: Long,
        familyName: String? = null,
        familyKind: String = "serverful",
    ) {
        prefs.edit {
            putLong(KEY_EPOCH, keyEpoch)
            putString(KEY_DEVICE, deviceId)
            putString(KEY_URL, serverUrl)
            putString(KEY_FAMILY_KIND, familyKind)
            putString(KEY_TOKEN, cipher.seal(authTokenHex.encodeToByteArray()))
            putString(KEY_FAMILY, cipher.seal(familyKeyBytes))
            if (familyName != null) putString(KEY_FAMILY_NAME, familyName)
        }
    }

    fun authTokenHex(): String? =
        prefs.getString(KEY_TOKEN, null)?.let { cipher.open(it)?.decodeToString() }

    val familyName: String?
        get() = prefs.getString(KEY_FAMILY_NAME, null)

    fun familyKey(): FamilyKey? =
        prefs.getString(KEY_FAMILY, null)?.let { cipher.open(it)?.let(FamilyKey::fromBytes) }

    /** Apps whose notifications never leave the phone. */
    var mutedApps: Set<String>
        get() = prefs.getStringSet(KEY_MUTED, emptySet()) ?: emptySet()
        set(v) {
            prefs.edit { putStringSet(KEY_MUTED, v) }
            mutedFlow.value = v
        }

    /** Laptops get the app + title only, never the message text. */
    var hideNotificationText: Boolean
        get() = prefs.getBoolean(KEY_HIDE_TEXT, false)
        set(v) {
            prefs.edit { putBoolean(KEY_HIDE_TEXT, v) }
            hideTextFlow.value = v
        }

    /** Long-term identity key, created on first use, Keystore-sealed. */
    fun identity(): IdentityKey {
        prefs.getString(KEY_IDENTITY, null)?.let { cipher.open(it) }?.let { return IdentityKey(it) }
        return IdentityKey.generate().also {
            prefs.edit { putString(KEY_IDENTITY, cipher.seal(it.secret)) }
        }
    }

    /** The relay said this phone is revoked/unknown — sticky until the
     *  user re-pairs (which [clear]s everything). */
    var removed: Boolean
        get() = prefs.getBoolean(KEY_REMOVED, false)
        set(v) {
            prefs.edit { putBoolean(KEY_REMOVED, v) }
            removedFlow.value = v
        }

    val keyEpoch: Long get() = prefs.getLong(KEY_EPOCH, 0)
    val keyRotator: String? get() = prefs.getString(KEY_ROTATOR, null)

    /** Adopt a rotated family key (KeyRotate). */
    fun rotateFamilyKey(newKey: ByteArray, epoch: Long, rotator: String, revoked: List<String>) {
        prefs.edit {
            putString(KEY_FAMILY, cipher.seal(newKey))
            putLong(KEY_EPOCH, epoch)
            putString(KEY_ROTATOR, rotator)
            putStringSet(KEY_REVOKED, revokedIds + revoked)
        }
        roster = roster - revoked.toSet()
    }

    val revokedIds: Set<String> get() = prefs.getStringSet(KEY_REVOKED, emptySet()) ?: emptySet()

    /** Pinned identities of the other members: id → (pubkey b64, kind). */
    var roster: Map<String, Pair<String, String>>
        get() = prefs.getString(KEY_ROSTER, null)?.let { s ->
            runCatching {
                protoJson.decodeFromString<Map<String, List<String>>>(s)
                    .mapValues { (_, v) -> v[0] to v[1] }
            }.getOrNull()
        } ?: emptyMap()
        set(v) = prefs.edit {
            putString(
                KEY_ROSTER,
                protoJson.encodeToString(v.mapValues { (_, p) -> listOf(p.first, p.second) }),
            )
        }

    companion object {
        /** UI gate: true once the dashboard may be shown. Set on cold
         *  start when already provisioned, or by the pairing card when
         *  its welcome animation finishes — not by [save] itself, so the
         *  burst gets its moment before the swap. */
        val settledFlow = kotlinx.coroutines.flow.MutableStateFlow(false)

        val removedFlow = kotlinx.coroutines.flow.MutableStateFlow(false)
        val hideTextFlow = kotlinx.coroutines.flow.MutableStateFlow(false)

        /** Muted-apps set, observable by the notifications card. */
        val mutedFlow = kotlinx.coroutines.flow.MutableStateFlow<Set<String>>(emptySet())

        private const val KEY_DEVICE = "device_id"
        private const val KEY_TOKEN = "auth_token_enc"
        private const val KEY_FAMILY = "family_key_enc"
        private const val KEY_FAMILY_NAME = "family_name"
        private const val KEY_URL = "server_url"
        private const val KEY_FAMILY_KIND = "family_kind"
        private const val KEY_NAME = "device_name"
        private const val KEY_MUTED = "notif_muted"
        private const val KEY_HIDE_TEXT = "notif_hide_text"
        private const val KEY_IDENTITY = "identity_enc"
        private const val KEY_EPOCH = "key_epoch"
        private const val KEY_ROTATOR = "key_rotator"
        private const val KEY_REVOKED = "revoked_ids"
        private const val KEY_ROSTER = "roster"
        private const val KEY_REMOVED = "removed"
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

    companion object {
        private const val ALIAS = "murmur-secrets"
        private const val GCM_IV_LEN = 12

        fun deleteKey() {
            runCatching {
                KeyStore.getInstance("AndroidKeyStore").apply {
                    load(null)
                    deleteEntry(ALIAS)
                }
            }
        }
    }
}
