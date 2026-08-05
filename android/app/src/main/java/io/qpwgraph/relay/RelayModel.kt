package io.qpwgraph.relay

data class RelaySettings(
    val target: String = "",
    val pin: String = "123456",
    val role: String = "emit",
    val codec: String = "opus",
    val transport: String = "auto",
    val deviceName: String = "android-relay",
    val sampleRate: Int = 48_000,
    val channels: Int = 1,
    val frameMs: Int = 20,
)

enum class RelayConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Error,
}

data class RelayUiState(
    val settings: RelaySettings = RelaySettings(),
    val connection: RelayConnectionState = RelayConnectionState.Disconnected,
    val hostName: String = "",
    val sessionId: Long? = null,
    val message: String = "",
    val rms: Float = 0f,
)
