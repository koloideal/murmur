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

        fun fromHex(hex: String): FamilyKey = FamilyKey(hex.chunked(2).map { it.toInt(16).toByte() }.toByteArray())
        fun random(): FamilyKey = FamilyKey(ByteArray(KEY_LEN).also(SecureRandom()::nextBytes))
    }
}
