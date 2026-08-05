package io.qpwgraph.relay

/** Settings for connecting to a relay host as a receiver. */
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

/** Settings for broadcasting this device's audio as a relay host. */
data class HostSettings(
    val deviceName: String = "android-relay",
    val pin: String = "123456",
    val port: Int = 0,
    val codec: String = "opus",
    val transport: String = "auto",
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

enum class RelayHostState {
    Idle,
    Starting,
    Running,
    Error,
}

/** Which role this device takes on the relay. */
enum class RelayMode {
    Receiver,
    Emitter,
}

/** A relay host seen on the local network during discovery. */
data class DiscoveredPeer(
    val name: String,
    val address: String,
)

/** One live session on the local host. */
data class RelaySessionInfo(
    val id: Long,
    val name: String,
    val address: String,
    val sending: Boolean,
    val receiving: Boolean,
)

data class RelayUiState(
    // Receiver (client) section.
    val settings: RelaySettings = RelaySettings(),
    val connection: RelayConnectionState = RelayConnectionState.Disconnected,
    val hostName: String = "",
    val sessionId: Long? = null,
    val message: String = "",
    val rms: Float = 0f,
    // Emitter (host) section.
    val host: HostSettings = HostSettings(),
    val hostState: RelayHostState = RelayHostState.Idle,
    val hostActive: Boolean = false,
    val hostPort: Int? = null,
    val hostMessage: String = "",
    val hostRms: Float = 0f,
    val sessions: List<RelaySessionInfo> = emptyList(),
    // Discovery section (shared by both modes).
    val discoveryActive: Boolean = false,
    val peers: List<DiscoveredPeer> = emptyList(),
    val discoveryMessage: String = "",
    // Selected tab.
    val mode: RelayMode = RelayMode.Receiver,
)
