package io.qpwgraph.relay

import android.content.SharedPreferences
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import org.json.JSONArray
import org.json.JSONObject
import java.security.KeyStore
import java.security.SecureRandom
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Small provider-independent AES-GCM core used by the Keystore-backed store.
 * Keeping this part independent makes the nonce/AAD/authentication contract
 * testable on the JVM; the production key still comes exclusively from
 * AndroidKeyStore.
 */
internal object TrustedCredentialCrypto {
    const val NONCE_BYTES = 12
    private const val TAG_BITS = 128

    data class EncryptedCredential(val nonce: ByteArray, val ciphertext: ByteArray)

    fun encrypt(
        peerId: String,
        clear: ByteArray,
        key: SecretKey,
        random: SecureRandom,
    ): EncryptedCredential {
        val nonce = ByteArray(NONCE_BYTES)
        random.nextBytes(nonce)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, key, GCMParameterSpec(TAG_BITS, nonce))
        cipher.updateAAD(peerId.toByteArray(Charsets.UTF_8))
        return EncryptedCredential(nonce, cipher.doFinal(clear))
    }

    fun decrypt(
        peerId: String,
        nonce: ByteArray,
        ciphertext: ByteArray,
        key: SecretKey,
    ): ByteArray {
        if (nonce.size != NONCE_BYTES) {
            throw CredentialStoreException("trusted credential nonce has an invalid length")
        }
        return try {
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(TAG_BITS, nonce))
            cipher.updateAAD(peerId.toByteArray(Charsets.UTF_8))
            cipher.doFinal(ciphertext)
        } catch (error: Exception) {
            throw CredentialStoreException("trusted credential authentication failed", error)
        }
    }
}

/**
 * Encrypted storage for the long-lived trusted relay bearer credentials.
 *
 * The preferences file is deliberately still excluded from backup/device
 * transfer (see backup_rules.xml and data_extraction_rules.xml), but backup
 * exclusion is defense in depth rather than the credential protection itself.
 * The value stored in relay.xml is ciphertext, never the raw 32-byte secret.
 */
internal class TrustedCredentialStore(
    private val preferences: SharedPreferences,
    // JVM tests inject a generated AES key so migration and fail-closed
    // record handling can be exercised without pretending a software key is
    // an Android Keystore key. Production callers leave this null.
    private val keyOverrideForTests: SecretKey? = null,
) {
    companion object {
        /** Must stay in sync with AndroidManifest backup exclusions. */
        const val PREFERENCES_NAME = "relay"
        const val PREFERENCES_KEY = "trusted_peers"
        const val FORMAT_VERSION = 1
        const val MAX_TRUSTED_PEERS = 256
        private const val KEY_ALIAS = "qpwgraph.relay.trusted.aes256gcm.v1"
    }

    private val random = SecureRandom()

    fun read(): List<TrustedRelayPeer> {
        val raw = preferences.getString(PREFERENCES_KEY, null).orEmpty()
        if (raw.isBlank()) return emptyList()
        val array = JSONArray(raw)
        if (array.length() > MAX_TRUSTED_PEERS) {
            throw CredentialStoreException("too many trusted relay credentials")
        }
        val peers = ArrayList<TrustedRelayPeer>(array.length())
        var migrated = false
        for (index in 0 until array.length()) {
            val item = array.optJSONObject(index)
                ?: throw CredentialStoreException("trusted credential record is malformed")
            val peerId = item.optString("peer_id").trim()
            if (peerId.isEmpty()) {
                throw CredentialStoreException("trusted credential peer id is empty")
            }
            val name = item.optString("name")
            val address = item.optString("address")
            val secret = if (item.has("secret")) {
                // One-time migration for records written by older releases.
                // Do not remove this field until the complete encrypted array
                // has been committed successfully.
                migrated = true
                val legacy = item.optString("secret").trim()
                validateSecret(legacy)
                legacy
            } else {
                val version = if (item.has("version")) item.optInt("version", -1) else -1
                if (version != FORMAT_VERSION) {
                    throw CredentialStoreException("unknown trusted credential format version")
                }
                val nonce = decode(item.optString("nonce"), "trusted credential nonce")
                val ciphertext = decode(item.optString("ciphertext"), "trusted credential ciphertext")
                val clear = decrypt(peerId, nonce, ciphertext)
                try {
                    if (clear.size != 32) {
                        throw CredentialStoreException(
                            "trusted credential has an invalid authenticated length",
                        )
                    }
                    bytesToHex(clear)
                } finally {
                    clear.fill(0)
                }
            }
            peers += TrustedRelayPeer(peerId, secret, name, address)
        }
        if (migrated) {
            // A failed commit leaves the old JSON, including its plaintext,
            // recoverable for a later retry. No partial migration is allowed.
            if (!writeEncrypted(peers)) {
                throw CredentialStoreException("trusted credential migration could not be committed")
            }
        }
        return peers
    }

    fun save(peer: TrustedRelayPeer) {
        validateSecret(peer.secret)
        val peers = read().toMutableList()
        val index = peers.indexOfFirst { it.peerId == peer.peerId }
        if (index >= 0) peers[index] = peer else peers += peer
        if (!writeEncrypted(peers)) {
            throw CredentialStoreException("trusted credential write could not be committed")
        }
    }

    fun remove(peerId: String): Boolean {
        val peers = read().toMutableList()
        val removed = peers.removeAll { it.peerId == peerId }
        if (removed && !writeEncrypted(peers)) {
            throw CredentialStoreException("trusted credential removal could not be committed")
        }
        return removed
    }

    /** Test helper that exercises the same authenticated record encoder. */
    internal fun encodedRecordForTests(peerId: String, secret: String): JSONObject {
        validateSecret(secret)
        val clear = hexToBytes(secret)
        return try {
            val encrypted = encrypt(peerId, clear)
            JSONObject()
                .put("peer_id", peerId)
                .put("version", FORMAT_VERSION)
                .put("nonce", encoded(encrypted.nonce))
                .put("ciphertext", encoded(encrypted.ciphertext))
        } finally {
            clear.fill(0)
        }
    }

    private fun writeEncrypted(peers: List<TrustedRelayPeer>): Boolean {
        if (peers.size > MAX_TRUSTED_PEERS) {
            throw CredentialStoreException("too many trusted relay credentials")
        }
        val array = JSONArray()
        peers.forEach { peer ->
            validateSecret(peer.secret)
            val clear = hexToBytes(peer.secret)
            try {
                val encrypted = encrypt(peer.peerId, clear)
                array.put(
                    JSONObject()
                        .put("peer_id", peer.peerId)
                        .put("name", peer.name)
                        .put("address", peer.address)
                        .put("version", FORMAT_VERSION)
                        .put("nonce", encoded(encrypted.nonce))
                        .put("ciphertext", encoded(encrypted.ciphertext)),
                )
            } finally {
                clear.fill(0)
            }
        }
        return preferences.edit().putString(PREFERENCES_KEY, array.toString()).commit()
    }

    private fun keyForEncrypt(): SecretKey {
        keyOverrideForTests?.let { return it }
        val store = keyStore()
        val existing = store.getKey(KEY_ALIAS, null) as? SecretKey
        if (existing != null) return existing
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        generator.init(
            KeyGenParameterSpec.Builder(
                KEY_ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            )
                .setKeySize(256)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .build(),
        )
        return generator.generateKey()
    }

    private fun keyForDecrypt(): SecretKey {
        keyOverrideForTests?.let { return it }
        return keyStore().getKey(KEY_ALIAS, null) as? SecretKey
            ?: throw CredentialStoreException("trusted credential Keystore key is missing")
    }

    private fun encrypt(
        peerId: String,
        clear: ByteArray,
    ): TrustedCredentialCrypto.EncryptedCredential {
        return TrustedCredentialCrypto.encrypt(peerId, clear, keyForEncrypt(), random)
    }

    private fun decrypt(peerId: String, nonce: ByteArray, ciphertext: ByteArray): ByteArray {
        return TrustedCredentialCrypto.decrypt(peerId, nonce, ciphertext, keyForDecrypt())
    }

    private fun keyStore(): KeyStore {
        return try {
            KeyStore.getInstance("AndroidKeyStore").also { it.load(null) }
        } catch (error: Exception) {
            throw CredentialStoreException("Android Keystore is unavailable", error)
        }
    }

    private fun encoded(bytes: ByteArray): String = Base64.encodeToString(bytes, Base64.NO_WRAP)

    private fun decode(value: String, field: String): ByteArray {
        if (value.isBlank()) throw CredentialStoreException("$field is missing")
        return try {
            Base64.decode(value, Base64.DEFAULT)
        } catch (error: IllegalArgumentException) {
            throw CredentialStoreException("$field is malformed", error)
        }
    }

    private fun validateSecret(secret: String) {
        if (secret.length != 64 || secret.any { it !in "0123456789abcdefABCDEF" }) {
            throw CredentialStoreException("trusted relay secret must be exactly 32 bytes")
        }
    }

    private fun hexToBytes(value: String): ByteArray = ByteArray(32) { index ->
        value.substring(index * 2, index * 2 + 2).toInt(16).toByte()
    }

    private fun bytesToHex(value: ByteArray): String = buildString(value.size * 2) {
        value.forEach { append("%02x".format(it.toInt() and 0xff)) }
    }

}

internal class CredentialStoreException(message: String, cause: Throwable? = null) : Exception(message, cause)
