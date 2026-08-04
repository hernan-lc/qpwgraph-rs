package io.qpwgraph.relay

internal object NativeBridge {
    init {
        System.loadLibrary("pw_graph_relay_android")
    }

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
}
