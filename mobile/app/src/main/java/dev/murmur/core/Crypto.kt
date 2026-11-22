package dev.murmur.core

import com.goterl.lazysodium.LazySodiumAndroid
import com.goterl.lazysodium.SodiumAndroid
import com.goterl.lazysodium.utils.Key
import java.security.SecureRandom

/**
 * XChaCha20-Poly1305 under the family key — wire-identical to
 * murmur_proto::FamilyKey: sealed = nonce(24) || ciphertext.
 */
class FamilyKey(private val keyBytes: ByteArray) {

    private val sodium = LazySodiumAndroid(SodiumAndroid())
    private val key = Key.fromBytes(keyBytes)

    fun seal(plaintext: ByteArray): ByteArray {
        val nonce = ByteArray(NONCE_LEN).also(SecureRandom()::nextBytes)
        val ct = ByteArray(plaintext.size + MAC_LEN)
        check(
            sodium.cryptoAeadXChaCha20Poly1305IetfEncrypt(
                ct, null, plaintext, plaintext.size.toLong(),
                null, 0L, null, nonce, key.asBytes
            )
        ) { "seal failed" }
        return nonce + ct
    }

    fun open(sealed: ByteArray): ByteArray {
        require(sealed.size >= NONCE_LEN + MAC_LEN) { "ciphertext too short" }
        val nonce = sealed.copyOfRange(0, NONCE_LEN)
        val ct = sealed.copyOfRange(NONCE_LEN, sealed.size)
        val pt = ByteArray(ct.size - MAC_LEN)
        check(
            sodium.cryptoAeadXChaCha20Poly1305IetfDecrypt(
                pt, null, null, ct, ct.size.toLong(),
                null, 0L, nonce, key.asBytes
            )
        ) { "open failed: bad key or tampered data" }
        return pt
    }

    companion object {
        const val KEY_LEN = 32
        const val NONCE_LEN = 24
        const val MAC_LEN = 16

        fun fromBytes(bytes: ByteArray): FamilyKey {
            require(bytes.size == KEY_LEN) { "family key must be 32 bytes" }
            return FamilyKey(bytes)
        }

        fun fromHex(hex: String): FamilyKey = fromBytes(hexToBytes(hex))
        fun random(): FamilyKey = FamilyKey(ByteArray(KEY_LEN).also(SecureRandom()::nextBytes))
    }
}

/** Ephemeral X25519 keypair for the pairing handshake — mirrors
 *  murmur_proto::PairKey. `kx` keypairs are Curve25519, DH-compatible. */
class PairKey(private val sodium: LazySodiumAndroid) {

    private val pair = sodium.cryptoKxKeypair()

    val public: ByteArray get() = pair.publicKey.asBytes
    val publicB64: String
        get() = android.util.Base64.encodeToString(public, android.util.Base64.NO_WRAP)

    /** DH result — used both as SAS input and as the PairData AEAD key. */
    fun dh(peerPublic: ByteArray): ByteArray {
        val shared = ByteArray(32)
        check(sodium.cryptoScalarMult(shared, pair.secretKey.asBytes, peerPublic)) {
            "DH failed"
        }
        return shared
    }

    fun dhB64(peerB64: String): ByteArray =
        dh(android.util.Base64.decode(peerB64, android.util.Base64.NO_WRAP))
}

/** Six digits from SHA-256(sorted pkA||pkB || sorted idA||idB) — byte-for-byte
 *  murmur_proto::sas_code. Both sides compute the same value; a MITM gets
 *  different codes. `idA`/`idB` are the raw 8 device/session id bytes. */
fun sasCode(pubA: ByteArray, pubB: ByteArray, idA: ByteArray, idB: ByteArray): String {
    val (loPub, hiPub) = if (pubA.contentCompareTo(pubB) > 0) pubB to pubA else pubA to pubB
    val (loId, hiId) = if (idA.contentCompareTo(idB) > 0) idB to idA else idA to idB
    val digest = java.security.MessageDigest.getInstance("SHA-256").apply {
        update(loPub); update(hiPub); update(loId); update(hiId)
    }.digest()
    val n = (java.nio.ByteBuffer.wrap(digest, 0, 4).int.toLong() and 0xFFFFFFFFL) % 1_000_000
    return "%06d".format(n)
}

/** Long-term X25519 identity key — rotations are sealed to it. Mirrors
 *  murmur_proto::rekey: wrap key = SHA-256("murmur/rekey/v1" ‖ DH),
 *  sealed plaintext = epoch (u64 BE) ‖ new family key. */
class IdentityKey(val secret: ByteArray) {

    private val sodium = LazySodiumAndroid(SodiumAndroid())

    val public: ByteArray = ByteArray(32).also {
        check(sodium.cryptoScalarMultBase(it, secret)) { "identity pubkey failed" }
    }
    val publicB64: String
        get() = android.util.Base64.encodeToString(public, android.util.Base64.NO_WRAP)

    /** Open a rotation entry sealed for us; null if it isn't ours,
     *  is tampered, or claims a different epoch. */
    fun openRekey(rotatorPubB64: String, epoch: Long, sealedB64: String): ByteArray? =
        runCatching {
            val peer = android.util.Base64.decode(rotatorPubB64, android.util.Base64.NO_WRAP)
            val dh = ByteArray(32)
            check(sodium.cryptoScalarMult(dh, secret, peer))
            val wrap = java.security.MessageDigest.getInstance("SHA-256").apply {
                update("murmur/rekey/v1".toByteArray()); update(dh)
            }.digest()
            val pt = FamilyKey.fromBytes(wrap).open(
                android.util.Base64.decode(sealedB64, android.util.Base64.NO_WRAP),
            )
            require(pt.size == 40)
            require(java.nio.ByteBuffer.wrap(pt, 0, 8).long == epoch)
            pt.copyOfRange(8, 40)
        }.getOrNull()

    companion object {
        fun generate() = IdentityKey(ByteArray(32).also(SecureRandom()::nextBytes))
    }
}

fun hexToBytes(hex: String): ByteArray =
    hex.chunked(2).map { it.toInt(16).toByte() }.toByteArray()

/** Pair session id from a typed `XXXX-XXXX` code — byte-for-byte
 *  murmur_proto::DeviceId::from_code (SHA-256 of the normalized code,
 *  first 8 bytes, hex). */
fun sessionFromCode(code: String): String {
    val norm = code.replace("-", "").uppercase()
    val digest = java.security.MessageDigest.getInstance("SHA-256")
        .digest(norm.toByteArray(Charsets.UTF_8))
    return digest.take(8).joinToString("") { "%02x".format(it) }
}

private fun ByteArray.contentCompareTo(other: ByteArray): Int {
    for (i in 0 until minOf(size, other.size)) {
        val d = (this[i].toInt() and 0xFF) - (other[i].toInt() and 0xFF)
        if (d != 0) return d
    }
    return size - other.size
}
