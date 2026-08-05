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

class RelayViewModel(application: Application) : AndroidViewModel(application) {
    private val preferences = application.getSharedPreferences("relay", 0)
    private val mutableState = MutableStateFlow(RelayUiState(loadSettings()))
    val state: StateFlow<RelayUiState> = mutableState.asStateFlow()
    private var handle = 0L
    private var polling: Job? = null

    fun update(settings: RelaySettings) {
        mutableState.value = mutableState.value.copy(settings = settings)
        preferences.edit()
            .putString("target", settings.target)
            .putString("pin", settings.pin)
            .putString("role", settings.role)
            .putString("codec", settings.codec)
            .putString("transport", settings.transport)
            .putString("device_name", settings.deviceName)
            .apply()
    }

    private fun loadSettings(): RelaySettings = RelaySettings(
        target = preferences.getString("target", "") ?: "",
        pin = preferences.getString("pin", "") ?: "",
        role = preferences.getString("role", "emit") ?: "emit",
        codec = preferences.getString("codec", "opus") ?: "opus",
        transport = preferences.getString("transport", "auto") ?: "auto",
        deviceName = preferences.getString("device_name", "android-relay") ?: "android-relay",
    )

    fun connect() {
        if (mutableState.value.connection == RelayConnectionState.Connecting) return
        val settings = mutableState.value.settings
        if (settings.target.isBlank() || settings.pin.isBlank()) {
            mutableState.value = mutableState.value.copy(
                connection = RelayConnectionState.Error,
                message = "Enter a host address and pairing PIN.",
            )
            return
        }
        mutableState.value = mutableState.value.copy(
            connection = RelayConnectionState.Connecting,
            message = "Connecting…",
        )
        viewModelScope.launch(Dispatchers.IO) {
            try {
                handle = NativeBridge.create(
                    settings.deviceName,
                    settings.role,
                    settings.codec,
                    settings.transport,
                    settings.sampleRate,
                    settings.channels,
                    settings.frameMs,
                )
                require(handle != 0L) { "Could not create native relay client" }
                val response = JSONObject(NativeBridge.connect(handle, settings.target, settings.pin))
                if (response.optString("type") == "error") {
                    error(response.optString("message"))
                    return@launch
                }
                mutableState.value = mutableState.value.copy(
                    connection = RelayConnectionState.Connected,
                    message = "Connected",
                )
                startService(settings, handle)
                startPolling()
            } catch (error: Exception) {
                error(error.message ?: "Connection failed")
            }
        }
    }

    fun disconnect() {
        viewModelScope.launch(Dispatchers.IO) {
            stopPolling()
            getApplication<Application>().stopService(Intent(getApplication(), RelayService::class.java))
            handle = 0L
            mutableState.value = mutableState.value.copy(
                connection = RelayConnectionState.Disconnected,
                sessionId = null,
                hostName = "",
                message = "Disconnected",
            )
        }
    }

    private fun startPolling() {
        polling?.cancel()
        polling = viewModelScope.launch(Dispatchers.IO) {
            while (true) {
                if (handle != 0L) consumeEvents(NativeBridge.pollEvents(handle))
                delay(100)
            }
        }
    }

    private fun consumeEvents(raw: String) {
        val events = JSONArray(raw)
        for (index in 0 until events.length()) {
            val event = events.getJSONObject(index)
            when (event.optString("type")) {
                "connected" -> mutableState.value = mutableState.value.copy(
                    connection = RelayConnectionState.Connected,
                    hostName = event.optString("host"),
                    sessionId = event.optLong("session"),
                    message = "Connected",
                )
                "disconnected" -> mutableState.value = mutableState.value.copy(
                    connection = RelayConnectionState.Disconnected,
                    sessionId = null,
                    message = event.optString("message"),
                )
                "level" -> mutableState.value = mutableState.value.copy(
                    rms = event.optDouble("rms").toFloat().coerceIn(0f, 1f),
                )
                "error" -> error(event.optString("message"))
            }
        }
    }

    private fun error(message: String) {
        mutableState.value = mutableState.value.copy(
            connection = RelayConnectionState.Error,
            message = message,
        )
    }

    private fun startService(settings: RelaySettings, clientHandle: Long) {
        val intent = Intent(getApplication(), RelayService::class.java)
            .putExtra(RelayService.EXTRA_HANDLE, clientHandle)
            .putExtra(RelayService.EXTRA_ROLE, settings.role)
            .putExtra(RelayService.EXTRA_SAMPLE_RATE, settings.sampleRate)
            .putExtra(RelayService.EXTRA_CHANNELS, settings.channels)
            .putExtra(RelayService.EXTRA_FRAME_MS, settings.frameMs)
        getApplication<Application>().startForegroundService(intent)
    }

    private fun stopPolling() {
        polling?.cancel()
        polling = null
    }

    override fun onCleared() {
        stopPolling()
        getApplication<Application>().stopService(Intent(getApplication(), RelayService::class.java))
        handle = 0L
        super.onCleared()
    }
}
