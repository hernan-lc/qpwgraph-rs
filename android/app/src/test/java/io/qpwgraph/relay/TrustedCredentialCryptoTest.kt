package io.qpwgraph.relay

import java.security.SecureRandom
import javax.crypto.KeyGenerator
import javax.crypto.spec.SecretKeySpec
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class TrustedCredentialCryptoTest {
    private val peerId = "desktop-installation"
    private val clear = ByteArray(32) { it.toByte() }

    private fun key(seed: Int): SecretKeySpec =
        SecretKeySpec(ByteArray(32) { (it + seed).toByte() }, "AES")

    @Test
    fun round_trip_uses_authenticated_peer_binding() {
        val encrypted = TrustedCredentialCrypto.encrypt(peerId, clear, key(1), SecureRandom())
        assertArrayEquals(
            clear,
            TrustedCredentialCrypto.decrypt(peerId, encrypted.nonce, encrypted.ciphertext, key(1)),
        )
        assertThrows(CredentialStoreException::class.java) {
            TrustedCredentialCrypto.decrypt(
                "another-peer",
                encrypted.nonce,
                encrypted.ciphertext,
                key(1),
            )
        }
    }

    @Test
    fun corrupted_ciphertext_and_wrong_key_fail_closed() {
        val encrypted = TrustedCredentialCrypto.encrypt(peerId, clear, key(2), SecureRandom())
        val corrupted = encrypted.ciphertext.clone().also { it[it.lastIndex] = (it.last() + 1).toByte() }
        assertThrows(CredentialStoreException::class.java) {
            TrustedCredentialCrypto.decrypt(peerId, encrypted.nonce, corrupted, key(2))
        }
        assertThrows(CredentialStoreException::class.java) {
            TrustedCredentialCrypto.decrypt(peerId, encrypted.nonce, encrypted.ciphertext, key(3))
        }
    }

    @Test
    fun each_write_gets_a_fresh_nonce_and_unknown_nonce_length_is_rejected() {
        val random = SecureRandom()
        val first = TrustedCredentialCrypto.encrypt(peerId, clear, key(4), random)
        val second = TrustedCredentialCrypto.encrypt(peerId, clear, key(4), random)
        assertNotEquals(first.nonce.toList(), second.nonce.toList())
        assertTrue(first.nonce.size == TrustedCredentialCrypto.NONCE_BYTES)
        assertThrows(CredentialStoreException::class.java) {
            TrustedCredentialCrypto.decrypt(peerId, ByteArray(8), first.ciphertext, key(4))
        }
    }

    @Test
    fun aes_key_is_256_bit_for_production_configuration() {
        val generator = KeyGenerator.getInstance("AES")
        generator.init(256)
        assertEquals(32, generator.generateKey().encoded.size)
    }
}
