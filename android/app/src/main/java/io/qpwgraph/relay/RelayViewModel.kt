package io.qpwgraph.relay

import android.app.Application
import android.content.Intent
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject

/**
 * Single state holder for both relay roles.
 *
 * Receiver (client) and Emitter (host) each own a native handle, an audio
 * foreground service, and a 100 ms polling job that drains native events.
 * Discovery owns a third handle and polls on a slower cadence because the
 * peer snapshot replaces the whole list every tick.
 */
class RelayViewModel(application: Application) : AndroidViewModel(application) {
    private val preferences = application.getSharedPreferences("relay", 0)
    private val mutableState = MutableStateFlow(
        RelayUiState(settings = loadSettings(), host = loadHostSettings()),
    )
    val state: StateFlow<RelayUiState> = mutableState.asStateFlow()
    private var clientHandle = 0L
    private var hostHandle = 0L
    private var discoveryHandle = 0L
    private var clientPolling: Job? = null
    private var hostPolling: Job? = null
    private var discoveryPolling: Job? = null
    private var usbPolling: Job? = null

    init {
        startUsbPolling()
    }

    private fun setState(transform: (RelayUiState) -> RelayUiState) {
        mutableState.value = transform(mutableState.value)
    }

    private fun text(id: Int, vararg args: Any): String =
        getApplication<Application>().getString(id, *args)

    fun setMode(mode: RelayMode) {
        setState { it.copy(mode = mode) }
    }

    // ------------------------------------------------------------------
    // Receiver (client)
    // ------------------------------------------------------------------

    fun update(settings: RelaySettings) {
        setState { it.copy(settings = settings) }
        preferences.edit()
            .putString("target", settings.target)
            .putString("pin", settings.pin)
            .putString("role", settings.role)
            .putString("codec", settings.codec)
            .putString("transport", settings.transport)
            .putString("device_name", settings.deviceName)
            .apply()
    }

    fun connect() {
        if (mutableState.value.connection == RelayConnectionState.Connecting) return
        val settings = mutableState.value.settings
        if (settings.target.isBlank() || settings.pin.isBlank()) {
            setState {
                it.copy(
                    connection = RelayConnectionState.Error,
                    message = text(R.string.relay_validation_missing_target),
                )
            }
            return
        }
        setState {
            it.copy(
                connection = RelayConnectionState.Connecting,
                message = text(R.string.relay_connecting),
            )
        }
        viewModelScope.launch(Dispatchers.IO) {
            try {
                clientHandle = NativeBridge.create(
                    settings.deviceName,
                    settings.role,
                    settings.codec,
                    settings.transport,
                    settings.sampleRate,
                    settings.channels,
                    settings.frameMs,
                )
                require(clientHandle != 0L) {
                    text(R.string.relay_error_native_create)
                }
                val response =
                    JSONObject(NativeBridge.connect(clientHandle, settings.target, settings.pin))
                if (response.optString("type") == "error") {
                    clientError(response.optString("message"))
                    return@launch
                }
                setState {
                    it.copy(
                        connection = RelayConnectionState.Connected,
                        message = text(R.string.relay_connected),
                    )
                }
                startService(RelayService.MODE_CLIENT, clientHandle, settings.role)
                startClientPolling()
            } catch (error: Exception) {
                clientError(error.message ?: text(R.string.relay_error_connect_failed))
            }
        }
    }

    /** Discovery tap-to-connect: adopt the peer address, then connect. */
    fun connectToPeer(address: String) {
        update(mutableState.value.settings.copy(target = address))
        setState { it.copy(mode = RelayMode.Receiver) }
        connect()
    }

    fun disconnect() {
        viewModelScope.launch(Dispatchers.IO) {
            stopClientPolling()
            stopService()
            if (clientHandle != 0L) {
                NativeBridge.disconnect(clientHandle)
                NativeBridge.release(clientHandle)
                clientHandle = 0L
            }
            setState {
                it.copy(
                    connection = RelayConnectionState.Disconnected,
                    sessionId = null,
                    hostName = "",
                    message = text(R.string.relay_disconnected),
                    rms = 0f,
                )
            }
        }
    }

    private fun startClientPolling() {
        clientPolling?.cancel()
        clientPolling = viewModelScope.launch(Dispatchers.IO) {
            while (true) {
                if (clientHandle != 0L) consumeClientEvents(NativeBridge.pollEvents(clientHandle))
                delay(POLL_INTERVAL_MS)
            }
        }
    }

    private fun consumeClientEvents(raw: String) {
        val events = JSONArray(raw)
        for (index in 0 until events.length()) {
            val event = events.getJSONObject(index)
            when (event.optString("type")) {
                "connected" -> setState {
                    it.copy(
                        connection = RelayConnectionState.Connected,
                        hostName = event.optString("host"),
                        sessionId = event.optLong("session"),
                        message = text(R.string.relay_connected),
                    )
                }

                "disconnected" -> setState {
                    it.copy(
                        connection = RelayConnectionState.Disconnected,
                        sessionId = null,
                        message = event.optString("message"),
                    )
                }

                "level" -> setState {
                    it.copy(rms = event.optDouble("rms").toFloat().coerceIn(0f, 1f))
                }

                "error" -> clientError(event.optString("message"))
            }
        }
    }

    private fun clientError(message: String) {
        setState {
            it.copy(connection = RelayConnectionState.Error, message = message)
        }
    }

    // ------------------------------------------------------------------
    // Emitter (host)
    // ------------------------------------------------------------------

    fun updateHost(host: HostSettings) {
        setState { it.copy(host = host) }
        preferences.edit()
            .putString("host_device_name", host.deviceName)
            .putString("host_pin", host.pin)
            .putInt("host_port", host.port)
            .putString("host_codec", host.codec)
            .putString("host_transport", host.transport)
            .apply()
    }

    fun startHost() {
        if (mutableState.value.hostState == RelayHostState.Starting ||
            mutableState.value.hostState == RelayHostState.Running
        ) {
            return
        }
        val host = mutableState.value.host
        if (host.pin.isBlank()) {
            setState {
                it.copy(
                    hostState = RelayHostState.Error,
                    hostMessage = text(R.string.relay_validation_missing_pin),
                )
            }
            return
        }
        setState {
            it.copy(
                hostState = RelayHostState.Starting,
                hostMessage = text(R.string.relay_host_starting),
            )
        }
        viewModelScope.launch(Dispatchers.IO) {
            try {
                hostHandle = NativeBridge.hostCreate(
                    host.deviceName,
                    host.pin,
                    host.port,
                    host.codec,
                    host.transport,
                    host.sampleRate,
                    host.channels,
                    host.frameMs,
                )
                require(hostHandle != 0L) {
                    text(R.string.relay_error_native_create)
                }
                val response = JSONObject(NativeBridge.hostStart(hostHandle))
                if (response.optString("type") != "host_started") {
                    hostError(response.optString("message"))
                    return@launch
                }
                val port = response.optInt("port")
                setState {
                    it.copy(
                        hostState = RelayHostState.Running,
                        hostPort = port,
                        hostMessage = text(R.string.relay_listening, port),
                    )
                }
                startService(RelayService.MODE_HOST, hostHandle, "both")
                startHostPolling()
            } catch (error: Exception) {
                hostError(error.message ?: text(R.string.relay_error_host_failed))
            }
        }
    }

    fun stopHost() {
        viewModelScope.launch(Dispatchers.IO) {
            stopHostPolling()
            stopService()
            if (hostHandle != 0L) {
                NativeBridge.hostStop(hostHandle)
                NativeBridge.hostRelease(hostHandle)
                hostHandle = 0L
            }
            setState {
                it.copy(
                    hostState = RelayHostState.Idle,
                    hostPort = null,
                    hostMessage = text(R.string.relay_host_stopped),
                    hostRms = 0f,
                    sessions = emptyList(),
                )
            }
        }
    }

    fun disconnectSession(sessionId: Long) {
        viewModelScope.launch(Dispatchers.IO) {
            if (hostHandle != 0L) {
                NativeBridge.hostDisconnectSession(hostHandle, sessionId)
            }
        }
    }

    private fun startHostPolling() {
        hostPolling?.cancel()
        hostPolling = viewModelScope.launch(Dispatchers.IO) {
            while (true) {
                if (hostHandle != 0L) {
                    consumeHostEvents(NativeBridge.hostPollEvents(hostHandle))
                    refreshHostStatus()
                }
                delay(POLL_INTERVAL_MS)
            }
        }
    }

    private fun consumeHostEvents(raw: String) {
        val events = JSONArray(raw)
        for (index in 0 until events.length()) {
            val event = events.getJSONObject(index)
            when (event.optString("type")) {
                "connected" -> setState {
                    it.copy(
                        hostMessage = text(R.string.relay_session_connected, event.optString("host")),
                    )
                }

                "disconnected" -> setState {
                    it.copy(hostMessage = event.optString("message"))
                }

                "level" -> setState {
                    it.copy(hostRms = event.optDouble("rms").toFloat().coerceIn(0f, 1f))
                }

                "error" -> hostError(event.optString("message"))
            }
        }
    }

    private fun refreshHostStatus() {
        val status = JSONObject(NativeBridge.hostStatus(hostHandle))
        if (status.optString("type") != "status") return
        val sessionsJson = status.optJSONArray("sessions") ?: JSONArray()
        val sessions = (0 until sessionsJson.length()).map { index ->
            val session = sessionsJson.getJSONObject(index)
            RelaySessionInfo(
                id = session.optLong("id"),
                name = session.optString("name"),
                address = session.optString("address"),
                sending = session.optBoolean("sending"),
                receiving = session.optBoolean("receiving"),
            )
        }
        setState {
            it.copy(
                sessions = sessions,
                hostActive = status.optBoolean("host_active"),
            )
        }
    }

    private fun hostError(message: String) {
        setState {
            it.copy(hostState = RelayHostState.Error, hostMessage = message)
        }
    }

    // ------------------------------------------------------------------
    // Discovery
    // ------------------------------------------------------------------

    fun startDiscovery() {
        if (mutableState.value.discoveryActive) return
        viewModelScope.launch(Dispatchers.IO) {
            if (discoveryHandle == 0L) {
                discoveryHandle = NativeBridge.discoveryCreate(
                    mutableState.value.settings.deviceName,
                )
            }
            if (discoveryHandle == 0L) {
                setState {
                    it.copy(discoveryMessage = text(R.string.relay_error_discovery_failed))
                }
                return@launch
            }
            val response = JSONObject(NativeBridge.discoveryStart(discoveryHandle))
            if (response.optString("type") != "discovery_started") {
                setState { it.copy(discoveryMessage = response.optString("message")) }
                return@launch
            }
            setState {
                it.copy(
                    discoveryActive = true,
                    peers = emptyList(),
                    discoveryMessage = text(R.string.relay_discovery_started),
                )
            }
            startDiscoveryPolling()
        }
    }

    fun stopDiscovery() {
        viewModelScope.launch(Dispatchers.IO) {
            stopDiscoveryPolling()
            if (discoveryHandle != 0L) {
                NativeBridge.discoveryStop(discoveryHandle)
            }
            setState {
                it.copy(
                    discoveryActive = false,
                    discoveryMessage = text(R.string.relay_discovery_stopped),
                )
            }
        }
    }

    private fun startDiscoveryPolling() {
        discoveryPolling?.cancel()
        discoveryPolling = viewModelScope.launch(Dispatchers.IO) {
            while (true) {
                if (discoveryHandle != 0L) {
                    refreshPeers()
                }
                delay(DISCOVERY_POLL_INTERVAL_MS)
            }
        }
    }

    /** Replace the whole peer list from the native snapshot each tick. */
    private fun refreshPeers() {
        val peersJson = JSONArray(NativeBridge.discoveryPeers(discoveryHandle))
        val peers = (0 until peersJson.length()).map { index ->
            val peer = peersJson.getJSONObject(index)
            DiscoveredPeer(
                name = peer.optString("name"),
                address = peer.optString("address"),
            )
        }
        setState { it.copy(peers = peers) }
    }

    // ------------------------------------------------------------------
    // USB tether auto-detection
    // ------------------------------------------------------------------

    /**
     * Poll the native layer for an active USB tether. USB is never a
     * transport choice: `auto` prefers it, the UI only reports the link.
     */
    private fun startUsbPolling() {
        usbPolling?.cancel()
        usbPolling = viewModelScope.launch(Dispatchers.IO) {
            while (true) {
                refreshUsbLink()
                delay(USB_LINK_POLL_INTERVAL_MS)
            }
        }
    }

    private fun refreshUsbLink() {
        val response = try {
            JSONObject(NativeBridge.usbLink())
        } catch (_: Exception) {
            return
        }
        val link = if (response.optString("type") == "usb_link") {
            UsbLinkInfo(
                name = response.optString("name"),
                addr = response.optString("addr"),
            )
        } else {
            null
        }
        if (link != mutableState.value.usbLink) {
            setState { it.copy(usbLink = link) }
        }
    }

    // ------------------------------------------------------------------
    // Shared plumbing
    // ------------------------------------------------------------------

    private fun startService(mode: String, handle: Long, role: String) {
        val current = mutableState.value
        val intent = Intent(getApplication(), RelayService::class.java)
            .putExtra(RelayService.EXTRA_MODE, mode)
            .putExtra(RelayService.EXTRA_HANDLE, handle)
            .putExtra(RelayService.EXTRA_ROLE, role)
            .putExtra(RelayService.EXTRA_SAMPLE_RATE, current.settings.sampleRate)
            .putExtra(RelayService.EXTRA_CHANNELS, current.settings.channels)
            .putExtra(RelayService.EXTRA_FRAME_MS, current.settings.frameMs)
        getApplication<Application>().startForegroundService(intent)
    }

    private fun stopService() {
        getApplication<Application>()
            .stopService(Intent(getApplication(), RelayService::class.java))
    }

    private fun stopClientPolling() {
        clientPolling?.cancel()
        clientPolling = null
    }

    private fun stopHostPolling() {
        hostPolling?.cancel()
        hostPolling = null
    }

    private fun stopDiscoveryPolling() {
        discoveryPolling?.cancel()
        discoveryPolling = null
    }

    private fun loadSettings(): RelaySettings = RelaySettings(
        target = preferences.getString("target", "") ?: "",
        pin = preferences.getString("pin", "123456") ?: "123456",
        role = preferences.getString("role", "emit") ?: "emit",
        codec = preferences.getString("codec", "opus") ?: "opus",
        transport = migrateTransport(preferences.getString("transport", "auto") ?: "auto"),
        deviceName = preferences.getString("device_name", "android-relay") ?: "android-relay",
    )

    private fun loadHostSettings(): HostSettings = HostSettings(
        deviceName = preferences.getString("host_device_name", "android-relay")
            ?: "android-relay",
        pin = preferences.getString("host_pin", "123456") ?: "123456",
        port = preferences.getInt("host_port", DEFAULT_HOST_PORT),
        codec = preferences.getString("host_codec", "opus") ?: "opus",
        transport = migrateTransport(preferences.getString("host_transport", "auto") ?: "auto"),
    )

    /** USB is auto-detected now; legacy explicit selections fall back to auto. */
    private fun migrateTransport(value: String): String =
        if (value == "usb") "auto" else value

    override fun onCleared() {
        stopClientPolling()
        stopHostPolling()
        stopDiscoveryPolling()
        usbPolling?.cancel()
        usbPolling = null
        stopService()
        if (clientHandle != 0L) {
            NativeBridge.release(clientHandle)
            clientHandle = 0L
        }
        if (hostHandle != 0L) {
            NativeBridge.hostStop(hostHandle)
            NativeBridge.hostRelease(hostHandle)
            hostHandle = 0L
        }
        if (discoveryHandle != 0L) {
            NativeBridge.discoveryRelease(discoveryHandle)
            discoveryHandle = 0L
        }
        super.onCleared()
    }

    private companion object {
        const val POLL_INTERVAL_MS = 100L
        const val DISCOVERY_POLL_INTERVAL_MS = 250L
        const val USB_LINK_POLL_INTERVAL_MS = 1_000L
    }
}
