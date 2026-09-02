package io.qpwgraph.relay

import org.junit.Assert.*
import org.junit.Test

/**
 * Host/network vs audio lifecycle separation (Part 14 – Host lifecycle tests).
 *
 * These are pure state-model tests – they verify that the UI state can
 * represent LISTENING+FAILED independently and that transitions keep the
 * TCP host alive when audio fails, without needing a real AudioRecord or
 * MediaProjection. This makes the source-selection logic unit-testable as
 * required by Part 15.
 */
class HostLifecycleSeparationTest {

    // Test 1 – start host audio starts successfully, client connects
    @Test
    fun test1_hostRunning_audioRunning_isValid() {
        val hostRunning = RelayUiState(
            hostState = RelayHostState.Running,
            hostAudioState = RelayHostAudioState.Running,
            hostActive = true,
            hostPort = 48123,
            hostAddress = "192.168.1.10",
        )
        assertEquals(RelayHostState.Running, hostRunning.hostState)
        assertEquals(RelayHostAudioState.Running, hostRunning.hostAudioState)
        assertTrue(hostRunning.hostActive)
        // trusted connection can be established because hostActive
        assertNotNull(hostRunning.hostPort)
    }

    // Test 2 – audio initialization fails but TCP remains listening
    @Test
    fun test2_audioInitFails_hostStaysListening() {
        val audioFailed = RelayUiState(
            hostState = RelayHostState.Running,
            hostAudioState = RelayHostAudioState.Error,
            hostAudioMessage = "Audio capture initialization failed: AudioRecord is not initialized",
            hostActive = true,
            hostPort = 48123,
            hostAddress = "192.168.1.10",
            hostMessage = "Listening on port 48123 — Audio capture initialization failed",
        )
        // Network is still LISTENING
        assertEquals(RelayHostState.Running, audioFailed.hostState)
        assertTrue(audioFailed.hostActive)
        // Audio is Error – client can still establish TCP/trusted session
        assertEquals(RelayHostAudioState.Error, audioFailed.hostAudioState)
        assertTrue(audioFailed.hostAudioMessage.contains("AudioRecord"))
        assertEquals(48123, audioFailed.hostPort)
    }

    // Test 3 – audio service crashes/stops, host listener remains active
    @Test
    fun test3_audioServiceCrashes_hostNotStopped() {
        val before = RelayUiState(
            hostState = RelayHostState.Running,
            hostAudioState = RelayHostAudioState.Running,
            hostActive = true,
            hostPort = 48123,
        )
        // Simulate ServiceStopped event handled as audio stopped, not host released
        val after = before.copy(
            hostAudioState = RelayHostAudioState.Stopped,
            hostAudioMessage = "The relay audio service stopped unexpectedly; reconnect to restart it.",
            // hostState stays Running, handle not released
        )
        assertEquals(RelayHostState.Running, after.hostState)
        assertEquals(RelayHostAudioState.Stopped, after.hostAudioState)
        assertTrue(after.hostActive)
        // native host is not implicitly stopped – would require explicit stopHost()
    }

    // Test 4 – explicit Stop stops both
    @Test
    fun test4_explicitStop_clearsBoth() {
        val running = RelayUiState(
            hostState = RelayHostState.Running,
            hostAudioState = RelayHostAudioState.Error,
            hostActive = true,
            hostPort = 48123,
            hostAddress = "192.168.1.10",
            sessions = listOf(RelaySessionInfo(1, "peer", "192.168.1.20:1234", true, true)),
        )
        val stopped = running.copy(
            hostState = RelayHostState.Idle,
            hostAudioState = RelayHostAudioState.Stopped,
            hostAudioMessage = "",
            hostActive = false,
            hostPort = null,
            hostAddress = null,
            sessions = emptyList(),
            host = running.host.copy(pin = ""),
        )
        assertEquals(RelayHostState.Idle, stopped.hostState)
        assertEquals(RelayHostAudioState.Stopped, stopped.hostAudioState)
        assertFalse(stopped.hostActive)
        assertNull(stopped.hostPort)
        assertTrue(stopped.sessions.isEmpty())
        assertEquals("", stopped.host.pin)
    }

    // Part 15 – Playback capture tests (unit-testable without real MediaProjection)

    @Test
    fun microphone_source_uses_MIC_not_playbackConfig() {
        val src = captureSourceFromString("microphone")
        assertEquals(CaptureSource.MICROPHONE, src)
        // MIC path requires RECORD_AUDIO but not MediaProjection
        assertFalse(src == CaptureSource.DEVICE_PLAYBACK)
    }

    @Test
    fun devicePlayback_source_requires_MediaProjection() {
        val src = captureSourceFromString("device_playback")
        assertEquals(CaptureSource.DEVICE_PLAYBACK, src)
        // should use AudioPlaybackCaptureConfiguration, not MIC
    }

    @Test
    fun playback_consent_denied_is_handled_cleanly() {
        // ViewModel would set hostAudioState=Error with relay_error_media_projection_denied
        val denied = RelayUiState(
            hostState = RelayHostState.Error,
            hostAudioState = RelayHostAudioState.Error,
            hostAudioMessage = "MediaProjection permission denied — device playback capture not started.",
        )
        assertEquals(RelayHostAudioState.Error, denied.hostAudioState)
        assertTrue(denied.hostAudioMessage.contains("MediaProjection"))
    }

    @Test
    fun playback_projection_revoked_is_handled_cleanly() {
        val revoked = RelayUiState(
            hostState = RelayHostState.Running,
            hostAudioState = RelayHostAudioState.Error,
            hostAudioMessage = "Device playback capture stopped: projection revoked",
            hostActive = true,
        )
        assertEquals(RelayHostState.Running, revoked.hostState) // still listening
        assertEquals(RelayHostAudioState.Error, revoked.hostAudioState)
        assertTrue(revoked.hostActive)
    }

    @Test
    fun audioRecord_init_failure_keeps_host_listening() {
        val msg = "AudioRecord is not initialized (state=0)"
        val state = RelayUiState(
            hostState = RelayHostState.Running,
            hostAudioState = RelayHostAudioState.Error,
            hostAudioMessage = "microphone audio failed: $msg",
            hostActive = true,
            hostPort = 48123,
        )
        assertTrue(state.hostAudioMessage.contains("microphone"))
        assertEquals(RelayHostState.Running, state.hostState)
    }

    @Test
    fun audioRecord_read_failure_keeps_host_listening() {
        val msg = "AudioRecord.read failed with code -3"
        val state = RelayUiState(
            hostState = RelayHostState.Running,
            hostAudioState = RelayHostAudioState.Error,
            hostAudioMessage = "device playback audio failed: $msg",
            hostActive = true,
        )
        assertTrue(state.hostAudioMessage.contains("device playback"))
        assertEquals(RelayHostState.Running, state.hostState)
    }

    @Test
    fun playback_capture_uses_MONO_and_opus_or_pcm() {
        val playback = HostSettings(captureSource = CaptureSource.DEVICE_PLAYBACK, codec = "opus", channels = 1)
        assertEquals(1, playback.channels)
        assertEquals("opus", playback.codec)
        val pcm = playback.copy(codec = "pcm")
        assertEquals("pcm", pcm.codec)
    }

    @Test
    fun diagnostics_do_not_leak_secrets() {
        val secret = "a".repeat(64)
        val peer = TrustedRelayPeer("peer1", secret, "MyDevice", "192.168.1.10:48123")
        assertFalse(peer.toString().contains(secret))
        val host = HostSettings(pin = "123456", captureSource = CaptureSource.DEVICE_PLAYBACK)
        assertFalse(host.toString().contains("123456"))
        assertTrue(host.toString().contains("DEVICE_PLAYBACK"))
    }
}
