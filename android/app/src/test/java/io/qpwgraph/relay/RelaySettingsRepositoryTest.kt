package io.qpwgraph.relay

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class RelaySettingsRepositoryTest {

    @Test
    fun a_generated_installation_identity_is_committed_before_it_is_used() {
        val preferences = FakePreferences()
        val identity = RelaySettingsRepository(preferences.proxy()).deviceId

        assertTrue(identity.isNotBlank())
        assertEquals(identity, preferences["device_id"])
    }

    /**
     * Identity is installation state. If the write fails there is no safe
     * value to advertise, so the repository must fail loudly rather than hand
     * back an identity the next process start would not recognise.
     */
    @Test
    fun a_refused_identity_write_is_not_silently_accepted() {
        val preferences = FakePreferences(commitSucceeds = false)
        assertThrows(IllegalStateException::class.java) {
            RelaySettingsRepository(preferences.proxy()).deviceId
        }
    }

    @Test
    fun an_existing_identity_is_reused_and_never_rewritten() {
        val preferences = FakePreferences(mutableMapOf("device_id" to "stable-id"))
        val repository = RelaySettingsRepository(preferences.proxy())

        assertEquals("stable-id", repository.deviceId)
        assertEquals("stable-id", repository.deviceId)
    }

    /** PINs are credentials; an upgrade must not leave a usable one behind. */
    @Test
    fun legacy_pins_are_purged() {
        val preferences = FakePreferences(
            mutableMapOf("pin" to "123456", "host_pin" to "654321", "target" to "host:1"),
        )
        RelaySettingsRepository(preferences.proxy()).purgeLegacyPins()

        assertNull(preferences["pin"])
        assertNull(preferences["host_pin"])
        assertEquals("host:1", preferences["target"])
    }

    @Test
    fun settings_round_trip_without_persisting_the_pin() {
        val preferences = FakePreferences()
        val repository = RelaySettingsRepository(preferences.proxy())
        val saved = RelaySettings(
            target = "192.168.1.20:48123",
            pin = "123456",
            role = "receive",
            codec = "pcm",
            transport = "tcp",
            deviceName = "pixel",
            autoConnectTrusted = false,
            autoConnectTrustedWifi = true,
        )

        repository.save(saved)
        val loaded = repository.loadSettings()

        assertEquals(saved.copy(pin = ""), loaded)
        assertNull(preferences["pin"])
    }

    @Test
    fun host_settings_round_trip_without_persisting_the_pin() {
        val preferences = FakePreferences()
        val repository = RelaySettingsRepository(preferences.proxy())
        val saved = HostSettings(
            deviceName = "studio",
            pin = "998877",
            port = 49000,
            codec = "pcm",
            transport = "tcp",
        )

        repository.saveHost(saved)
        val loaded = repository.loadHostSettings()

        assertEquals(saved.copy(pin = ""), loaded)
        assertNull(preferences["host_pin"])
    }

    /** USB is auto-detected now; an old explicit selection must not stick. */
    @Test
    fun a_stored_usb_transport_migrates_to_auto_on_both_sides() {
        val preferences = FakePreferences(
            mutableMapOf("transport" to "usb", "host_transport" to "usb"),
        )
        val repository = RelaySettingsRepository(preferences.proxy())

        assertEquals("auto", repository.loadSettings().transport)
        assertEquals("auto", repository.loadHostSettings().transport)
    }

    @Test
    fun defaults_apply_to_an_empty_installation() {
        val repository = RelaySettingsRepository(FakePreferences().proxy())
        val settings = repository.loadSettings()

        assertEquals("", settings.target)
        assertEquals("emit", settings.role)
        assertEquals("opus", settings.codec)
        assertEquals("auto", settings.transport)
        assertEquals("android-relay", settings.deviceName)
        assertTrue(settings.autoConnectTrusted)
        assertFalse(settings.autoConnectTrustedWifi)
        assertEquals(DEFAULT_HOST_PORT, repository.loadHostSettings().port)
    }

    @Test
    fun two_installations_do_not_share_an_identity() {
        val first = RelaySettingsRepository(FakePreferences().proxy()).deviceId
        val second = RelaySettingsRepository(FakePreferences().proxy()).deviceId
        assertNotEquals(first, second)
    }
}
