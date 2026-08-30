package io.qpwgraph.relay

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class RelayModelTest {
    @Test
    fun microphone_permission_matrix_matches_audio_direction() {
        assertTrue(clientNeedsMicrophone("emit"))
        assertTrue(clientNeedsMicrophone("both"))
        assertFalse(clientNeedsMicrophone("receive"))
    }

    @Test
    fun pcm_buffer_size_uses_frames_channels_and_bytes_per_sample() {
        assertEquals(480 * 1 * 2, pcm16BufferBytes(480, 1))
        assertEquals(480 * 2 * 2, pcm16BufferBytes(480, 2))
        assertEquals(480, audioFrameCount(48_000, 10))
    }

    @Test
    fun android_host_default_is_the_usb_discovery_port_and_mono() {
        assertEquals(48_123, DEFAULT_HOST_PORT)
        assertEquals(1, HostSettings().channels)
        assertEquals(1, RelaySettings().channels)
    }

    @Test
    fun service_geometry_uses_host_settings_for_host_and_client_settings_for_client() {
        val client = RelaySettings(sampleRate = 16_000, channels = 1, frameMs = 60)
        val host = HostSettings(sampleRate = 48_000, channels = 1, frameMs = 5)
        assertEquals(AudioGeometry(48_000, 1, 5), audioGeometryForHostMode(true, client, host))
        assertEquals(AudioGeometry(16_000, 1, 60), audioGeometryForHostMode(false, client, host))
    }
}
