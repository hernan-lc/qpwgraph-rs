package io.qpwgraph.relay

/** Settings for connecting to a relay host as a receiver. */
data class RelaySettings(
    val target: String = "",
    // Pairing credentials are entered for the current session and are not
    // persisted or supplied as an insecure app-wide default.
    val pin: String = "",
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
    // The Android API requires an explicit caller-owned pairing PIN.
    val pin: String = "",
    // Fixed default port: desktop USB probing scans for hosts on 48123, so
    // an ephemeral port would make this host undiscoverable over USB.
    val port: Int = DEFAULT_HOST_PORT,
    val codec: String = "opus",
    val transport: String = "auto",
    val sampleRate: Int = 48_000,
    val channels: Int = 1,
    val frameMs: Int = 20,
)

const val DEFAULT_HOST_PORT = 48123

/** Android currently exposes mono PCM on both platform audio endpoints. */
const val ANDROID_AUDIO_CHANNELS = 1
const val PCM16_BYTES_PER_SAMPLE = 2

/** Audio geometry copied into the foreground service start request. */
data class AudioGeometry(
    val sampleRate: Int,
    val channels: Int,
    val frameMs: Int,
)

/** Select the geometry belonging to the operation being started. */
fun audioGeometryForHostMode(
    hostMode: Boolean,
    client: RelaySettings,
    host: HostSettings,
): AudioGeometry = if (hostMode) {
    AudioGeometry(host.sampleRate, host.channels, host.frameMs)
} else {
    AudioGeometry(client.sampleRate, client.channels, client.frameMs)
}

fun clientRoleEmits(role: String): Boolean = when (role.lowercase()) {
    "emit", "both" -> true
    else -> false
}

fun clientRoleReceives(role: String): Boolean = when (role.lowercase()) {
    "receive", "both" -> true
    else -> false
}

fun clientNeedsMicrophone(role: String): Boolean = clientRoleEmits(role)

/** Number of interleaved PCM frames in one configured relay quantum. */
fun audioFrameCount(sampleRate: Int, frameMs: Int): Int {
    require(sampleRate > 0 && frameMs > 0) { "audio geometry must be positive" }
    val frames = sampleRate.toLong() * frameMs.toLong() / 1000L
    require(frames in 1..Int.MAX_VALUE) { "audio frame count is out of range" }
    return frames.toInt()
}

/** Byte count expected by AudioRecord/AudioTrack for PCM16 interleaved data. */
fun pcm16BufferBytes(frames: Int, channels: Int = ANDROID_AUDIO_CHANNELS): Int {
    require(frames > 0 && channels > 0) { "PCM geometry must be positive" }
    val bytes = frames.toLong() * channels.toLong() * PCM16_BYTES_PER_SAMPLE
    require(bytes <= Int.MAX_VALUE) { "PCM buffer is too large" }
    return bytes.toInt()
}

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
    Discover,
}

/** An active USB tether link, auto-detected by the native layer. */
data class UsbLinkInfo(
    val name: String,
    val addr: String,
)

/** One usable local IPv4 link, ranked best-first by the native layer. */
data class LocalLinkInfo(
    val name: String,
    val addr: String,
    val kind: String,
)

/**
 * Parse a scanned QR payload into `(target, pin)`. Accepts the app's own
 * `qpw-relay://host:port?pin=123456` URI as well as a plain `host:port`
 * string, so any generic QR carrying the address still works.
 */
fun parseRelayQr(raw: String): Pair<String, String?>? {
    val text = raw.trim()
    if (text.isEmpty()) return null
    if (text.startsWith("qpw-relay://")) {
        val rest = text.removePrefix("qpw-relay://")
        val parts = rest.split('?', limit = 2)
        val target = parts[0].trimEnd('/')
        if (target.isEmpty()) return null
        val pin = parts.getOrNull(1)
            ?.split('&')
            ?.firstOrNull { it.startsWith("pin=") }
            ?.removePrefix("pin=")
            ?.takeIf { it.isNotBlank() }
        return target to pin
    }
    if (Regex("""^[\w.\-]+:\d+$""").matches(text)) return text to null
    return null
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
    val hostAddress: String? = null,
    val hostMessage: String = "",
    val hostRms: Float = 0f,
    val sessions: List<RelaySessionInfo> = emptyList(),
    // Discovery section (shared by both modes).
    val discoveryActive: Boolean = false,
    val peers: List<DiscoveredPeer> = emptyList(),
    val discoveryMessage: String = "",
    // Auto-detected USB tether link, when one is up.
    val usbLink: UsbLinkInfo? = null,
    // All usable local links, best-first; shown with the host port.
    val localLinks: List<LocalLinkInfo> = emptyList(),
    // Selected tab.
    val mode: RelayMode = RelayMode.Receiver,
)
