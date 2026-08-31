package io.qpwgraph.relay

import android.content.SharedPreferences
import java.util.UUID

/**
 * Everything the relay keeps in `SharedPreferences`: the installation
 * identity, the client settings and the host settings.
 *
 * Credentials never pass through here. PINs are deliberately not persisted —
 * a client enters one per pairing session (or scans the host's QR code), and
 * an Android host's PIN lives only in the in-memory UI state. Trusted
 * credentials belong to [TrustedCredentialStore], which encrypts them.
 *
 * Keep [TrustedCredentialStore.PREFERENCES_NAME] aligned with
 * `backup_rules.xml` / `data_extraction_rules.xml`: `sharedpref/relay.xml` is
 * intentionally excluded from backup and transfer.
 */
class RelaySettingsRepository(private val preferences: SharedPreferences) {

    /**
     * Stable identity used by discovery and trusted handshakes.
     *
     * Identity is installation state, so a generated one is committed before
     * it is returned: a process death must not be able to leave the engine
     * advertising an identity that was never written.
     */
    val deviceId: String by lazy {
        preferences.getString("device_id", null)
            ?: UUID.randomUUID().toString().also { generated ->
                check(preferences.edit().putString("device_id", generated).commit()) {
                    "could not persist relay installation identity"
                }
            }
    }

    /**
     * Remove PINs written by older builds. They are credentials, not app
     * preferences, so upgrading must not leave a usable one behind.
     */
    fun purgeLegacyPins() {
        preferences.edit().remove("pin").remove("host_pin").apply()
    }

    fun loadSettings(): RelaySettings = RelaySettings(
        target = preferences.getString("target", "") ?: "",
        pin = "",
        role = preferences.getString("role", "emit") ?: "emit",
        codec = preferences.getString("codec", "opus") ?: "opus",
        transport = migrateTransport(preferences.getString("transport", "auto") ?: "auto"),
        deviceName = preferences.getString("device_name", "android-relay") ?: "android-relay",
        autoConnectTrusted = preferences.getBoolean("auto_connect_trusted", true),
        autoConnectTrustedWifi = preferences.getBoolean("auto_connect_trusted_wifi", false),
    )

    fun save(settings: RelaySettings) {
        preferences.edit()
            .putString("target", settings.target)
            .putString("role", settings.role)
            .putString("codec", settings.codec)
            .putString("transport", settings.transport)
            .putString("device_name", settings.deviceName)
            .putBoolean("auto_connect_trusted", settings.autoConnectTrusted)
            .putBoolean("auto_connect_trusted_wifi", settings.autoConnectTrustedWifi)
            .apply()
    }

    fun loadHostSettings(): HostSettings = HostSettings(
        deviceName = preferences.getString("host_device_name", "android-relay")
            ?: "android-relay",
        pin = "",
        port = preferences.getInt("host_port", DEFAULT_HOST_PORT),
        codec = preferences.getString("host_codec", "opus") ?: "opus",
        transport = migrateTransport(preferences.getString("host_transport", "auto") ?: "auto"),
    )

    fun saveHost(host: HostSettings) {
        preferences.edit()
            .putString("host_device_name", host.deviceName)
            .putInt("host_port", host.port)
            .putString("host_codec", host.codec)
            .putString("host_transport", host.transport)
            .apply()
    }

    private companion object {
        /** USB is auto-detected now; legacy explicit selections fall back. */
        fun migrateTransport(value: String): String = if (value == "usb") "auto" else value
    }
}
