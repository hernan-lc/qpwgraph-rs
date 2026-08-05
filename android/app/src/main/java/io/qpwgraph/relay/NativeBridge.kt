package io.qpwgraph.relay

/**
 * JNI surface of `crates/pw-graph-relay-android`.
 *
 * Keep this class in the `io.qpwgraph.relay` package with exactly this name:
 * the native symbol names (`Java_io_qpwgraph_relay_NativeBridge_*`) are
 * mangled from it. All interchange is JSON strings; every call returns
 * `{"type":"error","message":...}` on failure.
 */
internal object NativeBridge {
    init {
        System.loadLibrary("pw_graph_relay_android")
    }

    // Receiver (client) --------------------------------------------------
    external fun create(
        deviceName: String,
        role: String,
        codec: String,
        transport: String,
        sampleRate: Int,
        channels: Int,
        frameMs: Int,
    ): Long

    external fun connect(handle: Long, target: String, pin: String): String
    external fun disconnect(handle: Long): Boolean
    external fun pollEvents(handle: Long): String
    external fun pushCapture(handle: Long, samples: FloatArray): Int
    external fun pullPlayback(handle: Long, output: FloatArray): Int
    external fun release(handle: Long)

    // Emitter (host) -----------------------------------------------------
    external fun hostCreate(
        deviceName: String,
        pin: String,
        port: Int,
        codec: String,
        transport: String,
        sampleRate: Int,
        channels: Int,
        frameMs: Int,
    ): Long

    external fun hostStart(handle: Long): String
    external fun hostStop(handle: Long): String
    external fun hostPollEvents(handle: Long): String
    external fun hostStatus(handle: Long): String
    external fun hostDisconnectSession(handle: Long, sessionId: Long): String
    external fun hostPushCapture(handle: Long, samples: FloatArray): Int
    external fun hostPullPlayback(handle: Long, output: FloatArray): Int
    external fun hostRelease(handle: Long)

    // Discovery ----------------------------------------------------------
    external fun discoveryCreate(deviceName: String): Long
    external fun discoveryStart(handle: Long): String
    external fun discoveryStop(handle: Long): String
    external fun discoveryPeers(handle: Long): String
    external fun discoveryPollEvents(handle: Long): String
    external fun discoveryRelease(handle: Long)

    // Link detection -----------------------------------------------------
    /** JSON snapshot of the active USB tether link, or `{"type":"none"}`. */
    external fun usbLink(): String
}
