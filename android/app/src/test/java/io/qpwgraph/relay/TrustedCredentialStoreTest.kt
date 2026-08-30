package io.qpwgraph.relay

import android.content.SharedPreferences
import java.lang.reflect.Proxy
import javax.crypto.spec.SecretKeySpec
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class TrustedCredentialStoreTest {
    private val secret = "ab".repeat(32)

    @Test
    fun legacy_plaintext_is_migrated_only_after_encrypted_commit() {
        val preferences = MemoryPreferences(
            JSONArray().put(
                JSONObject()
                    .put("peer_id", "desktop")
                    .put("name", "Studio")
                    .put("address", "192.168.1.20:48123")
                    .put("secret", secret),
            ).toString(),
        )
        val store = TrustedCredentialStore(preferences.proxy(), key(1))

        assertEquals(secret, store.read().single().secret)
        val migrated = preferences.value.orEmpty()
        assertFalse(migrated.contains("\"secret\""))
        assertTrue(migrated.contains("\"version\":${TrustedCredentialStore.FORMAT_VERSION}"))
        assertTrue(migrated.contains("\"ciphertext\""))
        assertEquals(secret, store.read().single().secret)
    }

    @Test
    fun interrupted_plaintext_migration_keeps_the_legacy_record_recoverable() {
        val legacy = JSONArray().put(
            JSONObject().put("peer_id", "desktop").put("secret", secret),
        ).toString()
        val preferences = MemoryPreferences(legacy, commitSucceeds = false)
        val store = TrustedCredentialStore(preferences.proxy(), key(2))

        assertThrows(CredentialStoreException::class.java) { store.read() }
        assertEquals(legacy, preferences.value)
        assertTrue(preferences.value.orEmpty().contains(secret))
    }

    @Test
    fun unknown_version_and_missing_or_wrong_key_fail_closed() {
        val writerPreferences = MemoryPreferences(null)
        val writer = TrustedCredentialStore(writerPreferences.proxy(), key(3))
        val record = writer.encodedRecordForTests("desktop", secret)
        val unknown = JSONArray().put(JSONObject(record.toString()).put("version", 99))
        val unknownStore = TrustedCredentialStore(
            MemoryPreferences(unknown.toString()).proxy(),
            key(3),
        )
        assertThrows(CredentialStoreException::class.java) { unknownStore.read() }

        val wrongKeyStore = TrustedCredentialStore(
            MemoryPreferences(JSONArray().put(record).toString()).proxy(),
            key(4),
        )
        assertThrows(CredentialStoreException::class.java) { wrongKeyStore.read() }

        val missingKeyStore = TrustedCredentialStore(
            MemoryPreferences(JSONArray().put(record).toString()).proxy(),
        )
        assertThrows(CredentialStoreException::class.java) { missingKeyStore.read() }
    }

    @Test
    fun record_peer_id_is_authenticated_as_aad() {
        val preferences = MemoryPreferences(null)
        val store = TrustedCredentialStore(preferences.proxy(), key(5))
        val record = store.encodedRecordForTests("desktop", secret)
        val tampered = JSONObject(record.toString()).put("peer_id", "attacker")
        val tamperedStore = TrustedCredentialStore(
            MemoryPreferences(JSONArray().put(tampered).toString()).proxy(),
            key(5),
        )
        assertThrows(CredentialStoreException::class.java) { tamperedStore.read() }
    }

    private fun key(seed: Int): SecretKeySpec =
        SecretKeySpec(ByteArray(32) { (it + seed).toByte() }, "AES")

    private class MemoryPreferences(
        initial: String?,
        var commitSucceeds: Boolean = true,
    ) {
        var value: String? = initial

        fun proxy(): SharedPreferences {
            var pending = value
            lateinit var editor: SharedPreferences.Editor
            editor = Proxy.newProxyInstance(
                SharedPreferences.Editor::class.java.classLoader,
                arrayOf(SharedPreferences.Editor::class.java),
            ) { _, method, args ->
                when (method.name) {
                    "putString" -> {
                        pending = args?.get(1) as String?
                        editor
                    }
                    "remove" -> {
                        pending = null
                        editor
                    }
                    "commit" -> if (commitSucceeds) {
                        value = pending
                        true
                    } else {
                        false
                    }
                    "apply" -> {
                        value = pending
                        Unit
                    }
                    else -> editor
                }
            } as SharedPreferences.Editor

            return Proxy.newProxyInstance(
                SharedPreferences::class.java.classLoader,
                arrayOf(SharedPreferences::class.java),
            ) { _, method, args ->
                when (method.name) {
                    "getString" -> if (args?.get(0) == TrustedCredentialStore.PREFERENCES_KEY) {
                        value
                    } else {
                        args?.getOrNull(1)
                    }
                    "edit" -> editor
                    "contains" -> args?.get(0) == TrustedCredentialStore.PREFERENCES_KEY &&
                        value != null
                    "getAll" -> emptyMap<String, Any>()
                    "getStringSet" -> null
                    "getBoolean", "getInt", "getLong", "getFloat" -> args?.getOrNull(1)
                    else -> Unit
                }
            } as SharedPreferences
        }
    }
}
