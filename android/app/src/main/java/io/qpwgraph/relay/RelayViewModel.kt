package io.qpwgraph.relay

import android.Manifest
import android.app.Application
import android.content.Intent
import android.content.pm.PackageManager
import android.net.wifi.WifiManager
import androidx.core.content.ContextCompat
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import java.util.UUID

/**
 * Single state holder for both relay roles.
 *
 * Receiver (client) and Emitter (host) each own a native handle, an audio
 * foreground service, and a 100 ms polling job that drains native events.
 * Discovery owns a third handle and polls on a slower cadence because the
 * peer snapshot replaces the whole list every tick.
 */
class RelayViewModel(application: Application) : AndroidViewModel(application) {
    // Keep this name aligned with backup_rules.xml/data_extraction_rules.xml:
    // sharedpref/relay.xml is intentionally excluded from backup and transfer.
    private val preferences = application.getSharedPreferences(
        TrustedCredentialStore.PREFERENCES_NAME,
        0,
    )
    private val trustedCredentialStore = TrustedCredentialStore(preferences)
    /** Stable identity used by discovery and trusted handshakes. */
    private val deviceId = preferences.getString("device_id", null)
        ?: UUID.randomUUID().toString().also { generated ->
            // Identity is installation state. Commit it before exposing the
            // engine so a process death cannot accidentally create a new
            // advertised identity on the next start.
            check(preferences.edit().putString("device_id", generated).commit()) {
                "could not persist relay installation identity"
            }
        }
    private val mutableState = MutableStateFlow(
        RelayUiState(settings = loadSettings(), host = loadHostSettings()),
    )
    val state: StateFlow<RelayUiState> = mutableState.asStateFlow()
    @Volatile private var clientHandle = 0L
    @Volatile private var hostHandle = 0L
    // Keep the configuration used to prepare the current host so edits made
    // while it is stopped can be compared before a restart. The handle is
    // released on every stop so newly enrolled trusted peers are imported
    // into the next native engine instance.
    private var hostPreparedSettings: HostSettings? = null
    private var discoveryHandle = 0L
    private var clientPolling: Job? = null
    private var hostPolling: Job? = null
    private var discoveryPolling: Job? = null
    private var usbPolling: Job? = null
    private var serviceEvents: Job? = null
    private val operationMutex = Mutex()
    private val discoveryMutex = Mutex()
    private var multicastLock: WifiManager.MulticastLock? = null
    private var usbWasPresent = false
    private var lastTrustedAutoAttemptAt = 0L
    private val trustedCandidateBackoff = TrustedCandidateBackoff()

    init {
        // Pairing PINs are credentials, not app preferences. Remove values
        // written by older builds so upgrading does not leave a usable host
        // or client PIN in SharedPreferences.
        preferences.edit().remove("pin").remove("host_pin").apply()
        setState { it.copy(trustedPeers = trustedPeerSummaries()) }
        serviceEvents = viewModelScope.launch(Dispatchers.IO) {
            RelayServiceBridge.events.collect { event ->
                handleServiceEvent(event)
            }
        }
        startUsbPolling()
    }

    private fun setState(transform: (RelayUiState) -> RelayUiState) {
        mutableState.value = transform(mutableState.value)
    }

    private fun text(id: Int, vararg args: Any): String =
        getApplication<Application>().getString(id, *args)

    private fun hasMicrophonePermission(): Boolean = ContextCompat.checkSelfPermission(
        getApplication(),
        Manifest.permission.RECORD_AUDIO,
    ) == PackageManager.PERMISSION_GRANTED

    private fun createdHandle(raw: String): Long {
        val response = JSONObject(raw)
        if (response.optString("type") == "error") {
            throw IllegalStateException(response.optString("message"))
        }
        require(response.optString("type") == "created") {
            "native creation returned an unexpected response"
        }
        return response.optLong("handle").also {
            require(it != 0L) { text(R.string.relay_error_native_create) }
        }
    }

    private fun trustedPeers(): List<TrustedRelayPeer> {
        return runCatching { trustedCredentialStore.read() }
            .onFailure { error ->
                setState { it.copy(message = error.message ?: text(R.string.relay_error_host_failed)) }
            }
            .getOrDefault(emptyList())
    }

    private fun trustedPeer(peerId: String): TrustedRelayPeer? =
        trustedPeers().firstOrNull { it.peerId == peerId }

    private fun trustedPeerSummaries(): List<TrustedRelayPeerSummary> =
        trustedPeers().map { peer ->
            TrustedRelayPeerSummary(peer.peerId, peer.name, peer.address)
        }

    private fun saveTrustedPeer(
        peerId: String,
        secret: String,
        name: String = "",
        address: String = "",
    ): Boolean {
        if (peerId.isBlank() || secret.isBlank()) return false
        val old = trustedPeer(peerId)
        val merged = TrustedRelayPeer(
            peerId = peerId,
            secret = secret,
            name = name.ifBlank { old?.name.orEmpty() },
            address = address.ifBlank { old?.address.orEmpty() },
        )
        return try {
            // TrustedCredentialStore uses a fresh AES-GCM nonce for every
            // complete-record write and commits the encrypted record before
            // any legacy plaintext can be replaced.
            trustedCredentialStore.save(merged)
            setState { it.copy(trustedPeers = trustedPeerSummaries()) }
            true
        } catch (error: Exception) {
            setState { it.copy(message = error.message.orEmpty()) }
            false
        }
    }

    private fun rememberTrustedPeerFromJson(event: JSONObject) {
        val peerId = event.optString("peer_id").ifBlank { event.optString("id") }
        val secret = event.optString("secret")
        saveTrustedPeer(
            peerId = peerId,
            secret = secret,
            name = event.optString("name").ifBlank { event.optString("host") },
            address = event.optString("address"),
        )
    }

    private fun rememberTrustedPeerFromConnected(response: JSONObject) {
        response.optJSONObject("trusted_peer")?.let { rememberTrustedPeerFromJson(it) }
    }

    private fun rememberTrustedPeerFromNative(handle: Long) {
        runCatching { JSONObject(NativeBridge.clientTrustedPeer(handle)) }
            .getOrNull()
            ?.takeIf { it.optString("type") == "trusted_peer" }
            ?.let { credential ->
                saveTrustedPeer(
                    peerId = credential.optString("peer_id"),
                    secret = credential.optString("secret"),
                )
            }
    }

    private fun removeStoredTrustedPeer(peerId: String): Boolean {
        return runCatching { trustedCredentialStore.remove(peerId) }
            .onFailure { error ->
                setState {
                    it.copy(
                        message = error.message
                            ?: text(R.string.relay_error_trusted_revocation),
                    )
                }
            }
            .isSuccess
    }

    private fun trustedPeersJson(): String {
        val array = JSONArray()
        trustedPeers().forEach { peer ->
            array.put(
                JSONObject()
                    .put("peer_id", peer.peerId)
                    .put("secret", peer.secret),
            )
        }
        return array.toString()
    }

    private fun discoveredPeersJson(peers: List<DiscoveredPeer>): String {
        val array = JSONArray()
        peers.forEach { peer ->
            array.put(
                JSONObject()
                    .put("id", peer.id)
                    .put("name", peer.name)
                    .put("address", peer.address)
                    .put("link", peer.link),
            )
        }
        return array.toString()
    }

    fun setMode(mode: RelayMode) {
        setState { it.copy(mode = mode) }
    }

    /** Called by the Activity when a required runtime permission was denied. */
    fun permissionDenied(host: Boolean) {
        if (host) {
            setState {
                it.copy(
                    hostState = RelayHostState.Error,
                    hostPort = null,
                    hostActive = false,
                    hostAddress = null,
                    hostMessage = text(R.string.relay_error_microphone_permission),
                )
            }
        } else {
            setState {
                it.copy(
                    connection = RelayConnectionState.Error,
                    sessionId = null,
                    hostName = "",
                    transport = "",
                    link = "",
                    audioChannelState = "",
                    message = text(R.string.relay_error_microphone_permission),
                )
            }
        }
    }

    // ------------------------------------------------------------------
    // Receiver (client)
    // ------------------------------------------------------------------

    fun update(settings: RelaySettings) {
        setState { it.copy(settings = settings) }
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

    fun forgetTrustedPeer(peerId: String) {
        viewModelScope.launch(Dispatchers.IO) {
            operationMutex.withLock {
                // A false native result means the handle is stale/unknown,
                // not that revocation succeeded. Keep the encrypted record
                // until every live owner has acknowledged removal.
                val clientRevoked = clientHandle == 0L ||
                    runCatching { NativeBridge.removeTrustedPeer(clientHandle, peerId) }
                        .getOrDefault(false)
                val hostRevoked = hostHandle == 0L ||
                    runCatching { NativeBridge.hostRemoveTrustedPeer(hostHandle, peerId) }
                        .getOrDefault(false)
                if (!clientRevoked || !hostRevoked || !removeStoredTrustedPeer(peerId)) {
                    setState {
                        it.copy(message = text(R.string.relay_error_trusted_revocation))
                    }
                    return@withLock
                }
                setState { it.copy(trustedPeers = trustedPeerSummaries()) }
            }
        }
    }

    fun connect() {
        val settings = mutableState.value.settings
        if (clientNeedsMicrophone(settings.role) && !hasMicrophonePermission()) {
            permissionDenied(host = false)
            return
        }
        if (settings.target.isBlank()) {
            setState {
                it.copy(
                    connection = RelayConnectionState.Error,
                    message = text(R.string.relay_validation_missing_target),
                )
            }
            return
        }
        if (settings.pin.isBlank()) {
            setState {
                it.copy(
                    connection = RelayConnectionState.Error,
                    message = text(R.string.relay_validation_missing_pin),
                )
            }
            return
        }
        connectInternal(null)
    }

    /** Connect to a discovered peer with its previously enrolled credential. */
    fun connectToTrustedPeer(peer: DiscoveredPeer) {
        // This is an explicit user action, so it may retry this candidate
        // immediately even while automatic reconnect has it backed off.
        val trusted = trustedPeer(peer.id) ?: return
        update(mutableState.value.settings.copy(target = peer.address))
        setState { it.copy(mode = RelayMode.Receiver) }
        connectInternal(trusted)
    }

    private fun connectInternal(trusted: TrustedRelayPeer?) {
        if (mutableState.value.connection == RelayConnectionState.Connecting) return
        val settings = mutableState.value.settings
        if (clientNeedsMicrophone(settings.role) && !hasMicrophonePermission()) {
            permissionDenied(host = false)
            return
        }
        if (settings.target.isBlank()) {
            setState {
                it.copy(
                    connection = RelayConnectionState.Error,
                    message = text(R.string.relay_validation_missing_target),
                )
            }
            return
        }
        if (trusted == null && settings.pin.isBlank()) {
            setState {
                it.copy(
                    connection = RelayConnectionState.Error,
                    message = text(R.string.relay_validation_missing_pin),
                )
            }
            return
        }
        setState {
            it.copy(
                connection = RelayConnectionState.Connecting,
                message = if (trusted == null) {
                    text(R.string.relay_connecting)
                } else {
                    text(R.string.relay_trusted_connecting)
                },
            )
        }
        viewModelScope.launch(Dispatchers.IO) {
            operationMutex.withLock {
                var nativeConnected = false
                try {
                    // RelayService has one audio-pump instance. Stop the host
                    // before connecting a client so no live worker can observe
                    // a different mode or native handle.
                    if (mutableState.value.hostState == RelayHostState.Running ||
                        mutableState.value.hostState == RelayHostState.Starting
                    ) {
                        stopHostLocked()
                    }
                    if (clientHandle == 0L) {
                        clientHandle = createdHandle(
                            NativeBridge.create(
                                settings.deviceName,
                                deviceId,
                                trustedPeersJson(),
                                settings.role,
                                settings.codec,
                                settings.transport,
                                settings.sampleRate,
                                settings.channels,
                                settings.frameMs,
                            ),
                        )
                    }
                    val response = JSONObject(
                        if (trusted == null) {
                            NativeBridge.connect(clientHandle, settings.target, settings.pin)
                        } else {
                            NativeBridge.connectTrusted(
                                clientHandle,
                                settings.target,
                                trusted.peerId,
                                trusted.secret,
                            )
                        },
                    )
                    if (response.optString("type") == "error") {
                        val message = response.optString("message")
                        if (response.optString("code") == "unknown_client_handle") {
                            clientHandle = 0L
                        }
                        if (trusted != null) {
                            noteTrustedCandidateFailure(trusted.peerId, settings.target)
                        }
                        clientError(message)
                        return@withLock
                    }
                    require(response.optString("type") == "connected") {
                        "native connection returned an unexpected response"
                    }
                    val session = response.optLong("session")
                    require(session != 0L) { "native connection returned no session id" }
                    val host = response.optString("host").ifBlank { "Unknown host" }
                    // The credential accessor is deliberately separate from
                    // normal connection/status JSON. It returns a secret only
                    // to this persistence path after the host acknowledged it.
                    if (trusted == null) {
                        rememberTrustedPeerFromNative(clientHandle)
                    } else {
                        trustedCandidateBackoff.clear(trusted.peerId, settings.target)
                    }
                    nativeConnected = true

                    // Do not publish Connected until the foreground audio
                    // service has initialized every requested worker.
                    startService(
                        RelayService.MODE_CLIENT,
                        clientHandle,
                        settings.role,
                        audioGeometryForHostMode(
                            hostMode = false,
                            client = settings,
                            host = mutableState.value.host,
                        ),
                    )
                    setState {
                        it.copy(
                            connection = RelayConnectionState.Connected,
                            hostName = host,
                            sessionId = session,
                            message = text(R.string.relay_connected),
                        )
                    }
                    startClientPolling()
                } catch (error: Exception) {
                    // Stop the platform workers before invalidating the native
                    // handle they are polling. The service owns the same
                    // handle and must be quiescent before disconnect/release.
                    withContext(NonCancellable) {
                        if (nativeConnected &&
                            (error as? RelayServiceStartException)?.serviceWasAlreadyActive != true
                        ) {
                            stopServiceAndWait()
                        }
                        if (nativeConnected && clientHandle != 0L) {
                            runCatching { NativeBridge.disconnect(clientHandle) }
                            runCatching { NativeBridge.release(clientHandle) }
                            clientHandle = 0L
                        }
                    }
                    if (trusted != null) {
                        noteTrustedCandidateFailure(trusted.peerId, settings.target)
                    }
                    clientError(error.message ?: text(R.string.relay_error_connect_failed))
                }
            }
        }
    }

    /** Discovery tap-to-connect: adopt the peer address, then connect. */
    fun connectToPeer(address: String) {
        val discovered = mutableState.value.peers.firstOrNull { it.address == address }
        if (discovered != null && trustedPeer(discovered.id) != null) {
            connectToTrustedPeer(discovered)
            return
        }
        update(mutableState.value.settings.copy(target = address))
        setState { it.copy(mode = RelayMode.Receiver) }
        connect()
    }

    fun disconnect() {
        viewModelScope.launch(Dispatchers.IO) {
            operationMutex.withLock {
                stopClientLocked()
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
    }

    private fun startClientPolling() {
        clientPolling?.cancel()
        clientPolling = viewModelScope.launch(Dispatchers.IO) {
            while (isActive) {
                if (clientHandle != 0L) {
                    try {
                        if (!consumeClientEvents(NativeBridge.pollEvents(clientHandle))) {
                            break
                        }
                        refreshClientStatus()
                    } catch (error: Exception) {
                        clientError(error.message ?: text(R.string.relay_error_connect_failed))
                    }
                }
                delay(POLL_INTERVAL_MS)
            }
        }
    }

    private suspend fun consumeClientEvents(raw: String): Boolean {
        // Native returns a JSON object for an invalidated handle, while a
        // healthy poll returns an array. Handle that shape explicitly so a
        // service that died before its ServiceStopped event is observed still
        // clears the stale ViewModel handle.
        if (raw.trimStart().startsWith("{")) {
            val response = JSONObject(raw)
            if (response.optString("type") == "error") {
                val message = response.optString("message")
                val unknownHandle = response.optString("code") == "unknown_client_handle"
                if (unknownHandle) {
                    operationMutex.withLock { clientHandle = 0L }
                }
                clientError(message)
                return !unknownHandle
            }
        }
        val events = JSONArray(raw)
        var sessionLost = false
        for (index in 0 until events.length()) {
            val event = events.getJSONObject(index)
            when (event.optString("type")) {
                "connected" -> {
                    rememberTrustedPeerFromConnected(event)
                    setState {
                        it.copy(
                            connection = RelayConnectionState.Connected,
                            hostName = event.optString("host"),
                            sessionId = event.optLong("session"),
                            message = text(R.string.relay_connected),
                        )
                    }
                }

                "disconnected" -> {
                    sessionLost = true
                    setState {
                        it.copy(
                            connection = RelayConnectionState.Disconnected,
                        sessionId = null,
                        hostName = "",
                        transport = "",
                        link = "",
                        audioChannelState = "",
                        message = event.optString("message"),
                    )
                    }
                }

                "level" -> setState {
                    it.copy(rms = event.optDouble("rms").toFloat().coerceIn(0f, 1f))
                }

                "trusted_peer" -> rememberTrustedPeerFromJson(event)
                "trusted_peer_available" -> rememberTrustedPeerFromNative(clientHandle)
                "error" -> clientError(event.optString("message"))
            }
        }
        if (!sessionLost || mutableState.value.connection != RelayConnectionState.Disconnected) {
            return true
        }

        // A native SessionLost leaves the SDK client object in its registry
        // until the embedding releases it. Releasing only the UI state would
        // make a later trusted auto-connect reuse a Connected native handle
        // and receive "client is already connected" forever. Quiesce the
        // foreground service first, then retire the native handle. This runs
        // on the polling coroutine, so it must not cancel-and-join itself.
        operationMutex.withLock {
            if (clientHandle != 0L &&
                mutableState.value.connection == RelayConnectionState.Disconnected
            ) {
                stopServiceAndWait()
                val handle = clientHandle
                clientHandle = 0L
                runCatching { NativeBridge.disconnect(handle) }
                runCatching { NativeBridge.release(handle) }
            }
        }
        return false
    }

    private fun clientError(message: String) {
        setState {
            it.copy(
                connection = RelayConnectionState.Error,
                sessionId = null,
                hostName = "",
                rms = 0f,
                transport = "",
                link = "",
                audioChannelState = "",
                message = message,
            )
        }
    }

    private fun refreshClientStatus() {
        val handle = clientHandle
        if (handle == 0L) return
        val status = runCatching { JSONObject(NativeBridge.clientStatus(handle)) }.getOrNull()
            ?: return
        if (status.optString("type") == "error") return
        val session = status.optJSONArray("sessions")?.let { sessions ->
            if (sessions.length() > 0) sessions.optJSONObject(0) else null
        }
        setState {
            it.copy(
                transport = session?.optString("transport").orEmpty(),
                link = session?.optString("link").orEmpty(),
                audioChannelState = session?.optString("audio_channel_state").orEmpty(),
            )
        }
    }

    // ------------------------------------------------------------------
    // Emitter (host)
    // ------------------------------------------------------------------

    fun updateHost(host: HostSettings) {
        // The native host and its QR/PIN describe one immutable hosting
        // session. Do not let text-field edits make the UI advertise a
        // different PIN, port, or geometry while that session is live.
        if (mutableState.value.hostState == RelayHostState.Starting ||
            mutableState.value.hostState == RelayHostState.Running
        ) {
            return
        }
        setState { it.copy(host = host) }
        preferences.edit()
            .putString("host_device_name", host.deviceName)
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
        if (!hasMicrophonePermission()) {
            permissionDenied(host = true)
            return
        }
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
            operationMutex.withLock {
                var nativeStarted = false
                try {
                    // The Android app intentionally runs one relay mode at a
                    // time. This also makes service stop/start ordering
                    // deterministic when switching from Receiver to Emitter.
                    if (mutableState.value.connection == RelayConnectionState.Connected ||
                        mutableState.value.connection == RelayConnectionState.Connecting
                    ) {
                        stopClientLocked()
                    }
                    if (hostHandle != 0L &&
                        (hostPreparedSettings != host ||
                            mutableState.value.hostState == RelayHostState.Error)
                    ) {
                        // A previous service/native failure may have left a
                        // valid Running host handle behind. Stop it through
                        // the state machine before replacing it; releasing a
                        // live handle would bypass the worker-safety fence.
                        stopServiceAndWait()
                        if (!stopAndReleaseHost(hostHandle)) {
                            throw IllegalStateException("previous relay host is still running")
                        }
                    }
                    if (hostHandle == 0L) {
                        hostHandle = createdHandle(
                            NativeBridge.hostCreate(
                                host.deviceName,
                                deviceId,
                                trustedPeersJson(),
                                host.pin,
                                host.port,
                                host.codec,
                                host.transport,
                                host.sampleRate,
                                host.channels,
                                host.frameMs,
                            ),
                        )
                        hostPreparedSettings = host
                    }
                    val response = JSONObject(NativeBridge.hostStart(hostHandle))
                    if (response.optString("type") != "host_started") {
                        hostError(response.optString("message"))
                        return@withLock
                    }
                    val port = response.optInt("port")
                    val address = response.optString("address")
                        .takeIf { it.isNotBlank() }
                    nativeStarted = true

                    // A successful socket bind is not enough: publish Running
                    // only after the platform audio workers are available.
                    startService(
                        RelayService.MODE_HOST,
                        hostHandle,
                        "both",
                        audioGeometryForHostMode(
                            hostMode = true,
                            client = mutableState.value.settings,
                            host = host,
                        ),
                    )
                    setState {
                        it.copy(
                            hostState = RelayHostState.Running,
                            hostPort = port,
                            hostActive = true,
                            hostAddress = address,
                            hostMessage = text(R.string.relay_listening, port),
                        )
                    }
                    startHostPolling()
                } catch (error: Exception) {
                    // The audio service may still be using the native host;
                    // stop it before closing the listener/handle.
                    withContext(NonCancellable) {
                        if (nativeStarted &&
                            (error as? RelayServiceStartException)?.serviceWasAlreadyActive != true
                        ) {
                            stopServiceAndWait()
                        }
                        if (nativeStarted && hostHandle != 0L) {
                            stopAndReleaseHost(hostHandle)
                        }
                    }
                    hostError(error.message ?: text(R.string.relay_error_host_failed))
                }
            }
        }
    }

    fun stopHost() {
        viewModelScope.launch(Dispatchers.IO) {
            operationMutex.withLock {
                try {
                    stopHostLocked()
                    setState {
                        it.copy(
                            hostState = RelayHostState.Idle,
                            hostPort = null,
                            hostActive = false,
                            hostAddress = null,
                            hostMessage = text(R.string.relay_host_stopped),
                            hostRms = 0f,
                            sessions = emptyList(),
                            // A host PIN is fresh for one hosting session.
                            host = it.host.copy(pin = ""),
                        )
                    }
                } catch (error: Exception) {
                    hostError(error.message ?: text(R.string.relay_error_host_failed))
                }
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
            while (isActive) {
                if (hostHandle != 0L) {
                    try {
                        consumeHostEvents(NativeBridge.hostPollEvents(hostHandle))
                        refreshHostStatus()
                    } catch (error: Exception) {
                        hostError(error.message ?: text(R.string.relay_error_host_failed))
                    }
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

                "trusted_peer" -> rememberTrustedPeerFromJson(event)

                "trusted_enrollment_requested" -> {
                    val handle = hostHandle
                    val transactionId = event.optLong("transaction_id", -1L)
                    val peerId = event.optString("peer_id").ifBlank { event.optString("id") }
                    val previous = trustedPeer(peerId)
                    val secretResponse = runCatching {
                        JSONObject(
                            NativeBridge.hostTrustedEnrollmentSecret(handle, transactionId),
                        )
                    }.getOrNull()
                    val secret = secretResponse
                        ?.takeIf { it.optString("type") == "trusted_enrollment_secret" }
                        ?.optString("secret")
                        .orEmpty()
                    val persisted = secret.isNotBlank() && saveTrustedPeer(
                        peerId = peerId,
                        secret = secret,
                        name = event.optString("name"),
                        address = event.optString("address"),
                    )
                    if (persisted) {
                        val accepted = runCatching {
                            NativeBridge.hostAcceptTrustedEnrollment(handle, transactionId)
                        }.getOrDefault(false)
                        if (!accepted) {
                            // The host did not commit/ack the transaction.
                            // Restore the previous encrypted record so a
                            // failed rotation cannot strand either side.
                            if (previous != null) {
                                saveTrustedPeer(
                                    previous.peerId,
                                    previous.secret,
                                    previous.name,
                                    previous.address,
                                )
                            } else {
                                removeStoredTrustedPeer(peerId)
                            }
                            setState {
                                it.copy(message = text(R.string.relay_error_trusted_enrollment))
                            }
                        }
                    } else {
                        runCatching {
                            NativeBridge.hostRejectTrustedEnrollment(
                                handle,
                                transactionId,
                                "trusted credential could not be durably persisted",
                            )
                        }
                    }
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
        val active = status.optBoolean("host_active")
        if (!active && mutableState.value.hostState == RelayHostState.Running) {
            // The service or native host may have stopped independently
            // (for example after the process lost its foreground service).
            // Reflect that transition and release the prepared handle so a
            // later explicit start imports the latest trusted credentials.
            if (hostHandle != 0L) {
                val released = runCatching { NativeBridge.hostRelease(hostHandle) }
                    .getOrDefault(false)
                if (!released) {
                    hostError("native relay host became inactive but its handle is still owned")
                    return
                }
                hostHandle = 0L
                hostPreparedSettings = null
            }
            setState {
                it.copy(
                    hostState = RelayHostState.Idle,
                    hostPort = null,
                    hostActive = false,
                    hostAddress = null,
                    sessions = emptyList(),
                    host = it.host.copy(pin = ""),
                )
            }
            return
        }
        val sessionsJson = status.optJSONArray("sessions") ?: JSONArray()
        val sessions = (0 until sessionsJson.length()).map { index ->
            val session = sessionsJson.getJSONObject(index)
            RelaySessionInfo(
                id = session.optLong("id"),
                name = session.optString("name"),
                address = session.optString("address"),
                sending = session.optBoolean("sending"),
                receiving = session.optBoolean("receiving"),
                transport = session.optString("transport"),
                link = session.optString("link"),
                controlState = session.optString("control_state"),
                audioChannelState = session.optString("audio_channel_state"),
                trusted = session.optBoolean("trusted"),
            )
        }
        setState {
            it.copy(
                sessions = sessions,
                hostActive = active,
                hostPort = status.optInt("port").takeIf { port -> port > 0 },
                hostAddress = status.optString("address").takeIf { address -> address.isNotBlank() },
            )
        }
    }

    private fun hostError(message: String) {
        setState {
            it.copy(
                hostState = RelayHostState.Error,
                hostPort = null,
                hostActive = false,
                hostAddress = null,
                sessions = emptyList(),
                host = it.host.copy(pin = ""),
                hostMessage = message,
            )
        }
    }

    /**
     * Stop a host only after its platform workers are quiescent, and release
     * the native handle only when native confirms it is no longer active.
     * Keeping a still-running handle is safer than losing ownership of it;
     * the next serialized operation can retry the stop.
     */
    private fun stopAndReleaseHost(handle: Long): Boolean {
        val stopped = runCatching {
            JSONObject(NativeBridge.hostStop(handle)).optString("type") == "host_stopped"
        }.getOrDefault(false)
        val inactive = runCatching {
            !JSONObject(NativeBridge.hostStatus(handle)).optBoolean("host_active", true)
        }.getOrDefault(false)
        if (!stopped && !inactive) return false
        // hostRelease returns false when a stale caller races a still-running
        // native host. Treat that as a failed release, rather than confusing
        // "the JNI call returned" with "ownership was actually removed".
        val released = runCatching { NativeBridge.hostRelease(handle) }
            .getOrDefault(false)
        if (!released) return false
        if (hostHandle == handle) {
            hostHandle = 0L
            hostPreparedSettings = null
        }
        return true
    }

    /** Stop client-side native/audio state while the operation mutex is held. */
    private suspend fun stopClientLocked() {
        stopClientPollingAndWait()
        stopServiceAndWait()
        if (clientHandle != 0L) {
            runCatching { NativeBridge.disconnect(clientHandle) }
            runCatching { NativeBridge.release(clientHandle) }
            clientHandle = 0L
        }
        setState {
                it.copy(
                    connection = RelayConnectionState.Disconnected,
                    sessionId = null,
                    hostName = "",
                    rms = 0f,
                    transport = "",
                    link = "",
                    audioChannelState = "",
                )
        }
    }

    /** Stop host-side native/audio state while the operation mutex is held. */
    private suspend fun stopHostLocked() {
        stopHostPollingAndWait()
        stopServiceAndWait()
        if (hostHandle != 0L) {
            val handle = hostHandle
            val response = JSONObject(NativeBridge.hostStop(handle))
            if (response.optString("type") == "error") {
                throw IllegalStateException(response.optString("message"))
            }
            // Recreate the prepared host next time so credentials enrolled by
            // a client during this run are read from private preferences.
            if (!NativeBridge.hostRelease(handle)) {
                throw IllegalStateException("native relay host handle is still active")
            }
            hostHandle = 0L
            hostPreparedSettings = null
        }
        setState {
            it.copy(
                hostState = RelayHostState.Idle,
                hostPort = null,
                hostActive = false,
                hostAddress = null,
                sessions = emptyList(),
                host = it.host.copy(pin = ""),
            )
        }
    }

    private suspend fun handleServiceEvent(event: RelayServiceEvent) {
        operationMutex.withLock {
            when (event) {
                is RelayServiceEvent.ServiceStopped -> if (
                    event.mode == RelayService.MODE_CLIENT &&
                    event.handle == clientHandle
                ) {
                    clientPolling?.cancel()
                    clientPolling = null
                    val handle = clientHandle
                    clientHandle = 0L
                    if (handle != 0L) {
                        runCatching { NativeBridge.disconnect(handle) }
                        runCatching { NativeBridge.release(handle) }
                    }
                    setState {
                        it.copy(
                            connection = RelayConnectionState.Error,
                            sessionId = null,
                            hostName = "",
                            rms = 0f,
                            transport = "",
                            link = "",
                            audioChannelState = "",
                            message = text(R.string.relay_error_audio_service_stopped),
                        )
                    }
                } else if (
                    event.mode == RelayService.MODE_HOST &&
                    event.handle == hostHandle &&
                    mutableState.value.hostState != RelayHostState.Idle
                ) {
                    hostPolling?.cancel()
                    hostPolling = null
                    stopAndReleaseHost(event.handle)
                    setState {
                        it.copy(
                            hostState = RelayHostState.Error,
                            hostActive = false,
                            hostPort = null,
                            hostAddress = null,
                            hostMessage = text(R.string.relay_error_audio_service_stopped),
                        )
                    }
                }
                is RelayServiceEvent.AudioFailure -> if (event.mode == RelayService.MODE_HOST) {
                    if (event.handle == hostHandle) {
                        stopHostPollingAndWait()
                        // The service owns the worker threads that still use
                        // this native handle. Wait for its bounded teardown
                        // before stopping/releasing the host below.
                        stopServiceAndWait()
                        val handle = hostHandle
                        stopAndReleaseHost(handle)
                        hostError(event.message)
                    }
                } else if (event.handle == clientHandle) {
                    stopClientPollingAndWait()
                    // Do not invalidate a handle while the failed worker can
                    // still be inside a JNI audio call.
                    stopServiceAndWait()
                    runCatching { NativeBridge.disconnect(clientHandle) }
                    runCatching { NativeBridge.release(clientHandle) }
                    clientHandle = 0L
                    clientError(event.message)
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Discovery
    // ------------------------------------------------------------------

    fun startDiscovery() {
        if (mutableState.value.discoveryActive) return
        viewModelScope.launch(Dispatchers.IO) {
            discoveryMutex.withLock {
                if (mutableState.value.discoveryActive) return@withLock
                try {
                    val multicastAvailable = acquireMulticastLock()
                    if (discoveryHandle == 0L) {
                        discoveryHandle = createdHandle(
                            NativeBridge.discoveryCreate(
                                mutableState.value.settings.deviceName,
                            ),
                        )
                    }
                    val response = JSONObject(NativeBridge.discoveryStart(discoveryHandle))
                    if (response.optString("type") != "discovery_started") {
                        releaseMulticastLock()
                        setState { it.copy(discoveryMessage = response.optString("message")) }
                        return@withLock
                    }
                    setState {
                        it.copy(
                            discoveryActive = true,
                            peers = emptyList(),
                            discoveryMessage = if (multicastAvailable) {
                                text(R.string.relay_discovery_started)
                            } else {
                                text(R.string.relay_discovery_multicast_unavailable)
                            },
                        )
                    }
                    startDiscoveryPolling()
                } catch (error: Exception) {
                    releaseMulticastLock()
                    setState {
                        it.copy(
                            discoveryMessage = error.message
                                ?: text(R.string.relay_error_discovery_failed),
                        )
                    }
                }
            }
        }
    }

    fun stopDiscovery() {
        viewModelScope.launch(Dispatchers.IO) {
            discoveryMutex.withLock {
                stopDiscoveryPollingAndWait()
                if (discoveryHandle != 0L) {
                    NativeBridge.discoveryStop(discoveryHandle)
                }
                releaseMulticastLock()
                setState {
                    it.copy(
                        discoveryActive = false,
                        peers = emptyList(),
                        discoveryMessage = text(R.string.relay_discovery_stopped),
                    )
                }
            }
        }
    }

    private fun startDiscoveryPolling() {
        discoveryPolling?.cancel()
        discoveryPolling = viewModelScope.launch(Dispatchers.IO) {
            while (isActive) {
                if (discoveryHandle != 0L) {
                    refreshPeers()
                }
                delay(DISCOVERY_POLL_INTERVAL_MS)
            }
        }
    }

    /** Replace the whole peer list from the native snapshot each tick. */
    private fun refreshPeers() {
        try {
            consumeDiscoveryEvents(NativeBridge.discoveryPollEvents(discoveryHandle))
            val peersJson = JSONArray(NativeBridge.discoveryPeers(discoveryHandle))
            val discovered = (0 until peersJson.length()).map { index ->
                val peer = peersJson.getJSONObject(index)
                DiscoveredPeer(
                    id = peer.optString("id"),
                    name = peer.optString("name"),
                    address = peer.optString("address"),
                    link = peer.optString("link"),
                )
            }
            val peers = discovered.toMutableList()
            // Keep the durable last-known address in the bounded candidate
            // set even when a same-ID advertisement arrives first. The
            // address is only a dial candidate; the trusted proof still has
            // to authenticate the stable identity.
            for (stored in trustedPeers()) {
                if (stored.address.isNotBlank() &&
                    peers.none { it.id == stored.peerId && it.address == stored.address }
                ) {
                    peers += DiscoveredPeer(
                        id = stored.peerId,
                        name = stored.name,
                        address = stored.address,
                    )
                }
            }
            val activeClient = clientHandle
            if (activeClient != 0L) {
                // Discovery has its own native engine on Android. Keep the
                // connected client supplied with that snapshot so its resume
                // worker can authenticate the same host at a new USB/Wi-Fi
                // address instead of retrying the original IP forever.
                NativeBridge.updateClientPeers(activeClient, discoveredPeersJson(peers))
            }
            setState { it.copy(peers = peers) }
            val current = mutableState.value
            val trustedRecords = trustedPeers()
            val now = android.os.SystemClock.elapsedRealtime()
            val candidate = peers
                .filter { peer ->
                peer.id.isNotBlank() && trustedRecords.any { it.peerId == peer.id } &&
                    trustedAutoConnectAllowed(current.settings, peer) &&
                    trustedCandidateAllowed(peer.id, peer.address, now)
                }
                .minWithOrNull(compareBy<DiscoveredPeer> {
                    trustedCandidateRank(it, trustedRecords.firstOrNull { stored -> stored.peerId == it.id })
                }
                    .thenBy { it.address })
            val microphoneReady = !clientNeedsMicrophone(current.settings.role) ||
                hasMicrophonePermission()
            if (
                candidate != null &&
                microphoneReady &&
                (current.connection == RelayConnectionState.Disconnected ||
                    current.connection == RelayConnectionState.Error) &&
                current.hostState != RelayHostState.Starting &&
                current.hostState != RelayHostState.Running &&
                now - lastTrustedAutoAttemptAt >= TRUSTED_AUTO_RETRY_INTERVAL_MS
            ) {
                lastTrustedAutoAttemptAt = now
                connectToTrustedPeer(candidate)
            }
        } catch (error: Exception) {
            setState { it.copy(discoveryMessage = error.message ?: text(R.string.relay_error_discovery_failed)) }
        }
    }

    private fun trustedCandidateAllowed(peerId: String, address: String, now: Long): Boolean =
        trustedCandidateBackoff.allowed(peerId, address, now)

    private fun noteTrustedCandidateFailure(peerId: String, address: String) {
        trustedCandidateBackoff.noteFailure(
            peerId,
            address,
            android.os.SystemClock.elapsedRealtime(),
        )
    }

    private fun consumeDiscoveryEvents(raw: String) {
        try {
            val events = JSONArray(raw)
            for (index in 0 until events.length()) {
                val event = events.getJSONObject(index)
                if (event.optString("type") == "error") {
                    setState {
                        it.copy(
                            discoveryMessage = event.optString("message")
                                .ifBlank { text(R.string.relay_error_discovery_failed) },
                        )
                    }
                }
            }
        } catch (error: Exception) {
            setState {
                it.copy(
                    discoveryMessage = error.message
                        ?: text(R.string.relay_error_discovery_failed),
                )
            }
        }
    }

    /** Multicast is needed only by mDNS; direct USB probes remain usable if
     * Android refuses the lock or the device is on a non-Wi-Fi transport. */
    private fun acquireMulticastLock(): Boolean {
        if (multicastLock?.isHeld == true) return true
        val wifi = getApplication<Application>()
            .getSystemService(WifiManager::class.java)
            ?: return false
        var lock: WifiManager.MulticastLock? = null
        try {
            lock = wifi.createMulticastLock("qpwgraph-relay-discovery")
            lock.setReferenceCounted(false)
            lock.acquire()
            multicastLock = lock
            return true
        } catch (_: RuntimeException) {
            if (lock?.isHeld == true) runCatching { lock?.release() }
            multicastLock = null
            return false
        }
    }

    private fun releaseMulticastLock() {
        multicastLock?.let { lock ->
            if (lock.isHeld) runCatching { lock.release() }
        }
        multicastLock = null
    }

    // ------------------------------------------------------------------
    // Local link detection (USB tether + host addresses)
    // ------------------------------------------------------------------

    /**
     * Poll the native layer for local links. The one-second fallback is a
     * bounded link watcher for Android devices where no public RNDIS/NCM
     * callback is available. A newly visible USB link starts discovery so a
     * tethered host appears without requiring the user to revisit the tab.
     */
    private fun startUsbPolling() {
        usbPolling?.cancel()
        usbPolling = viewModelScope.launch(Dispatchers.IO) {
            while (isActive) {
                refreshLinks()
                delay(USB_LINK_POLL_INTERVAL_MS)
            }
        }
    }

    private fun refreshLinks() {
        val response = try {
            JSONObject(NativeBridge.localLinks())
        } catch (_: Exception) {
            return
        }
        if (response.optString("type") != "links") return
        val linksJson = response.optJSONArray("links") ?: return
        val links = (0 until linksJson.length()).map { index ->
            val link = linksJson.getJSONObject(index)
            LocalLinkInfo(
                name = link.optString("name"),
                addr = link.optString("addr"),
                kind = link.optString("kind"),
            )
        }
        val usb = links.firstOrNull { it.kind == "usb" }?.let { UsbLinkInfo(it.name, it.addr) }
        val current = mutableState.value
        val usbAppeared = usb != null && !usbWasPresent
        val usbDisappeared = usb == null && usbWasPresent
        usbWasPresent = usb != null
        if (links != current.localLinks || usb != current.usbLink) {
            setState { it.copy(localLinks = links, usbLink = usb) }
        }
        if (usbDisappeared && discoveryHandle != 0L) {
            runCatching { NativeBridge.discoveryUsbLinkLost(discoveryHandle) }
            if (mutableState.value.discoveryActive) refreshPeers()
        }
        if (usbAppeared && !mutableState.value.discoveryActive) startDiscovery()
    }

    /** Fill target (and PIN, when the QR carries one) from a scanned code. */
    fun applyScannedQr(raw: String) {
        val parsed = parseRelayQr(raw)
        if (parsed == null) {
            setState { it.copy(message = text(R.string.relay_qr_invalid)) }
            return
        }
        val (target, pin) = parsed
        val settings = mutableState.value.settings
        update(settings.copy(target = target, pin = pin ?: settings.pin))
        setState {
            it.copy(message = text(R.string.relay_qr_applied, target))
        }
    }

    // ------------------------------------------------------------------
    // Shared plumbing
    // ------------------------------------------------------------------

    private suspend fun startService(
        mode: String,
        handle: Long,
        role: String,
        geometry: AudioGeometry,
    ) {
        // HostSettings and RelaySettings intentionally remain unrelated data
        // classes. Select the primitive geometry explicitly so Kotlin does not
        // infer an unsafe common type. The caller supplies the operation's
        // immutable settings snapshot so a concurrent UI edit cannot make the
        // service geometry disagree with the native handle it is pumping.
        val token = UUID.randomUUID().toString()
        val ready = RelayServiceBridge.registerStart(token)
        val intent = Intent(getApplication(), RelayService::class.java)
            .putExtra(RelayService.EXTRA_MODE, mode)
            .putExtra(RelayService.EXTRA_HANDLE, handle)
            .putExtra(RelayService.EXTRA_ROLE, role)
            .putExtra(RelayService.EXTRA_SAMPLE_RATE, geometry.sampleRate)
            .putExtra(RelayService.EXTRA_CHANNELS, geometry.channels)
            .putExtra(RelayService.EXTRA_FRAME_MS, geometry.frameMs)
            .putExtra(RelayService.EXTRA_START_TOKEN, token)
        try {
            getApplication<Application>().startForegroundService(intent)
            val result = withTimeoutOrNull(SERVICE_START_TIMEOUT_MS) { ready.await() }
                ?: throw IllegalStateException("relay audio service did not become ready")
            if (!result.started) {
                val alreadyActive = result.message.contains("already active")
                throw RelayServiceStartException(result.message, alreadyActive)
            }
        } catch (error: Exception) {
            withContext(NonCancellable) {
                RelayServiceBridge.cancelStart(token)
                if ((error as? RelayServiceStartException)?.serviceWasAlreadyActive != true) {
                    stopServiceAndWait()
                }
            }
            throw error
        }
    }

    /**
     * Quiesce the platform workers before touching the native handle they use.
     * Every caller is on Dispatchers.IO (or the cleanup thread in
     * onCleared), so waiting here cannot freeze the Activity. Deliberately do
     * not time this wait out: invalidating a handle while a worker is still in
     * JNI is a much worse failure than keeping this background cleanup pending
     * until Service.onDestroy has finished its bounded worker joins.
     */
    private suspend fun stopServiceAndWait() {
        val waiter = RelayServiceBridge.registerStopWaiter()
        val stopped = getApplication<Application>()
            .stopService(Intent(getApplication(), RelayService::class.java))
        if (!stopped) waiter.complete(Unit)
        try {
            waiter.await()
        } finally {
            // A cancelled lifecycle cleanup must not leave a completed or
            // forever-pending waiter retained by the service bridge.
            RelayServiceBridge.unregisterStopWaiter(waiter)
        }
    }

    private suspend fun stopClientPollingAndWait() {
        clientPolling?.cancelAndJoin()
        clientPolling = null
    }

    private suspend fun stopHostPollingAndWait() {
        hostPolling?.cancelAndJoin()
        hostPolling = null
    }

    private suspend fun stopDiscoveryPollingAndWait() {
        discoveryPolling?.cancelAndJoin()
        discoveryPolling = null
    }

    private fun loadSettings(): RelaySettings = RelaySettings(
        target = preferences.getString("target", "") ?: "",
        // PINs are deliberately not persisted; the caller enters one for
        // each pairing session (or scans the current host QR code).
        pin = "",
        role = preferences.getString("role", "emit") ?: "emit",
        codec = preferences.getString("codec", "opus") ?: "opus",
        transport = migrateTransport(preferences.getString("transport", "auto") ?: "auto"),
            deviceName = preferences.getString("device_name", "android-relay") ?: "android-relay",
        autoConnectTrusted = preferences.getBoolean("auto_connect_trusted", true),
        autoConnectTrustedWifi = preferences.getBoolean("auto_connect_trusted_wifi", false),
    )

    private fun loadHostSettings(): HostSettings = HostSettings(
        deviceName = preferences.getString("host_device_name", "android-relay")
            ?: "android-relay",
        // Android hosts use an explicitly entered caller-owned PIN. It is
        // retained only in the in-memory UI state until this ViewModel ends.
        pin = "",
        port = preferences.getInt("host_port", DEFAULT_HOST_PORT),
        codec = preferences.getString("host_codec", "opus") ?: "opus",
        transport = migrateTransport(preferences.getString("host_transport", "auto") ?: "auto"),
    )

    /** USB is auto-detected now; legacy explicit selections fall back to auto. */
    private fun migrateTransport(value: String): String =
        if (value == "usb") "auto" else value

    override fun onCleared() {
        clientPolling?.cancel()
        hostPolling?.cancel()
        discoveryPolling?.cancel()
        serviceEvents?.cancel()
        serviceEvents = null
        usbPolling?.cancel()
        usbPolling = null
        // Retire the in-memory host credential with the ViewModel as well as
        // on an explicit stop. It is never serialized to preferences.
        setState { it.copy(host = it.host.copy(pin = "")) }
        // Service workers use these same native handles. Serialize cleanup
        // with in-flight connect/host/discovery operations, then stop and
        // await the service before invalidating them. This runs away from the
        // lifecycle/main thread; service destruction is idempotent with the
        // cleanup below, which also covers a service that never started.
        Thread({
            runCatching {
                runBlocking {
                    operationMutex.withLock {
                        stopClientPollingAndWait()
                        stopHostPollingAndWait()
                        stopServiceAndWait()
                        val client = clientHandle
                        clientHandle = 0L
                        if (client != 0L) {
                            runCatching { NativeBridge.disconnect(client) }
                            runCatching { NativeBridge.release(client) }
                        }
                        val host = hostHandle
                        if (host != 0L) stopAndReleaseHost(host)
                    }
                    discoveryMutex.withLock {
                        stopDiscoveryPollingAndWait()
                        val discovery = discoveryHandle
                        discoveryHandle = 0L
                        if (discovery != 0L) {
                            runCatching { NativeBridge.discoveryRelease(discovery) }
                        }
                        releaseMulticastLock()
                    }
                }
            }
        }, "qpw-relay-viewmodel-cleanup").start()
        super.onCleared()
    }

    private companion object {
        const val POLL_INTERVAL_MS = 100L
        const val DISCOVERY_POLL_INTERVAL_MS = 250L
        const val USB_LINK_POLL_INTERVAL_MS = 1_000L
        const val SERVICE_START_TIMEOUT_MS = 10_000L
        const val TRUSTED_AUTO_RETRY_INTERVAL_MS = 5_000L
    }
}
