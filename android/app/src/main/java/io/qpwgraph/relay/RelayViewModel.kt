package io.qpwgraph.relay

import android.Manifest
import android.app.Application
import android.content.pm.PackageManager
import androidx.core.content.ContextCompat
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
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
import kotlinx.coroutines.withContext
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
    // Keep this name aligned with backup_rules.xml/data_extraction_rules.xml:
    // sharedpref/relay.xml is intentionally excluded from backup and transfer.
    private val preferences = application.getSharedPreferences(
        TrustedCredentialStore.PREFERENCES_NAME,
        0,
    )
    private val settings = RelaySettingsRepository(preferences)
    private val trustedStore = TrustedPeerRepository(TrustedCredentialStore(preferences))
    private val service = RelayServiceController(application)
    private val deviceId = settings.deviceId
    private val mutableState = MutableStateFlow(
        RelayUiState(settings = settings.loadSettings(), host = settings.loadHostSettings()),
    )
    val state: StateFlow<RelayUiState> = mutableState.asStateFlow()
    // The native handles and their teardown rules live in the controllers.
    // This class owns the UI state, the operation mutex that serializes the
    // two roles against each other, and nothing else about them.
    private val client = ClientController(viewModelScope, service)
    private val host = HostController(viewModelScope, service)
    private var usbPolling: Job? = null
    private var serviceEvents: Job? = null
    private val operationMutex = Mutex()
    private var usbWasPresent = false
    private var lastTrustedAutoAttemptAt = 0L
    private val trustedCandidateBackoff = TrustedCandidateBackoff()
    private val discovery = DiscoveryController(
        application = application,
        scope = viewModelScope,
        trusted = trustedStore,
    ) { snapshot -> onDiscoverySnapshot(snapshot) }

    init {
        settings.purgeLegacyPins()
        setState { it.copy(trustedPeers = trustedStore.summaries()) }
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

    /** Reading failures surface once, then degrade to an empty record. */
    private fun trustedPeers(): List<TrustedRelayPeer> =
        trustedStore.peers()
            .onFailure { error ->
                setState {
                    it.copy(message = error.message ?: text(R.string.relay_error_host_failed))
                }
            }
            .getOrDefault(emptyList())

    /**
     * Apply a store outcome to the UI: refresh the summaries on success,
     * surface the reason on failure, stay quiet when there was nothing to
     * store. Returns whether the credential is now persisted.
     */
    private fun applySaved(saved: TrustedPeerRepository.Saved): Boolean {
        when (saved) {
            TrustedPeerRepository.Saved.Stored ->
                setState { it.copy(trustedPeers = trustedStore.summaries()) }
            is TrustedPeerRepository.Saved.Failed ->
                setState { it.copy(message = saved.message) }
            TrustedPeerRepository.Saved.Skipped -> Unit
        }
        return saved.stored
    }

    private fun saveTrustedPeer(
        peerId: String,
        secret: String,
        name: String = "",
        address: String = "",
    ): Boolean = applySaved(trustedStore.save(peerId, secret, name, address))

    private fun rememberTrustedPeerFromJson(event: JSONObject) {
        applySaved(trustedStore.saveFrom(event))
    }

    private fun rememberTrustedPeerFromConnected(response: JSONObject) {
        response.optJSONObject("trusted_peer")?.let { rememberTrustedPeerFromJson(it) }
    }

    private fun rememberTrustedPeerFromNative() {
        client.trustedCredential()?.let { credential ->
            saveTrustedPeer(
                peerId = credential.optString("peer_id"),
                secret = credential.optString("secret"),
            )
        }
    }

    private fun removeStoredTrustedPeer(peerId: String): Boolean =
        trustedStore.remove(peerId)
            .onFailure { error ->
                setState {
                    it.copy(
                        message = error.message
                            ?: text(R.string.relay_error_trusted_revocation),
                    )
                }
            }
            .isSuccess

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

    fun update(updated: RelaySettings) {
        setState { it.copy(settings = updated) }
        settings.save(updated)
    }

    fun forgetTrustedPeer(peerId: String) {
        viewModelScope.launch(Dispatchers.IO) {
            operationMutex.withLock {
                // A false native result means the handle is stale/unknown,
                // not that revocation succeeded. Keep the encrypted record
                // until every live owner has acknowledged removal.
                val clientRevoked = !client.isOpen || client.removeTrustedPeer(peerId)
                val hostRevoked = !host.isOpen || host.removeTrustedPeer(peerId)
                if (!clientRevoked || !hostRevoked || !removeStoredTrustedPeer(peerId)) {
                    setState {
                        it.copy(message = text(R.string.relay_error_trusted_revocation))
                    }
                    return@withLock
                }
                setState { it.copy(trustedPeers = trustedStore.summaries()) }
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
        val trusted = trustedStore.peer(peer.id) ?: return
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
                    client.open(settings, deviceId, trustedStore.credentialsJson()) {
                        text(R.string.relay_error_native_create)
                    }
                    val response = if (trusted == null) {
                        client.connect(settings.target, settings.pin)
                    } else {
                        client.connectTrusted(settings.target, trusted)
                    }
                    if (response.optString("type") == "error") {
                        val message = response.optString("message")
                        if (response.optString("code") == "unknown_client_handle") {
                            client.forgetHandle()
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
                        rememberTrustedPeerFromNative()
                    } else {
                        trustedCandidateBackoff.clear(trusted.peerId, settings.target)
                    }
                    nativeConnected = true

                    // Do not publish Connected until the foreground audio
                    // service has initialized every requested worker.
                    service.start(
                        RelayService.MODE_CLIENT,
                        client.nativeHandle,
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
                        if (nativeConnected && !error.serviceWasAlreadyActive) {
                            service.stopAndWait()
                        }
                        if (nativeConnected) client.releaseNow()
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
        if (discovered != null && trustedStore.peer(discovered.id) != null) {
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
        client.startPolling(
            onEvents = ::consumeClientEvents,
            onStatus = ::applyClientStatus,
            onError = { error ->
                clientError(error.message ?: text(R.string.relay_error_connect_failed))
            },
        )
    }

    private suspend fun consumeClientEvents(raw: String): Boolean {
        // Native returns a JSON object for an invalidated handle, while a
        // healthy poll returns an array. Handle that shape explicitly so a
        // service that died before its ServiceStopped event is observed still
        // clears the stale ViewModel handle.
        RelayJson.pollError(raw)?.let { failure ->
            if (failure.unknownHandle) {
                operationMutex.withLock { client.forgetHandle() }
            }
            clientError(failure.message)
            return !failure.unknownHandle
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
                "trusted_peer_available" -> rememberTrustedPeerFromNative()
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
            if (client.isOpen &&
                mutableState.value.connection == RelayConnectionState.Disconnected
            ) {
                service.stopAndWait()
                client.releaseNow()
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

    private fun applyClientStatus(status: JSONObject) {
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
        settings.saveHost(host)
    }

    fun startHost() {
        if (mutableState.value.hostState == RelayHostState.Starting ||
            mutableState.value.hostState == RelayHostState.Running
        ) {
            return
        }
        val wanted = mutableState.value.host
        if (!hasMicrophonePermission()) {
            permissionDenied(host = true)
            return
        }
        if (wanted.pin.isBlank()) {
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
                    if (host.isOpen &&
                        (!host.preparedFor(wanted) ||
                            mutableState.value.hostState == RelayHostState.Error)
                    ) {
                        // A previous service/native failure may have left a
                        // valid Running host handle behind. Stop it through
                        // the state machine before replacing it; releasing a
                        // live handle would bypass the worker-safety fence.
                        service.stopAndWait()
                        if (!host.stopAndRelease()) {
                            throw IllegalStateException("previous relay host is still running")
                        }
                    }
                    host.open(
                        wanted,
                        deviceId,
                        trustedStore.credentialsJson(),
                    ) { text(R.string.relay_error_native_create) }
                    val response = host.start()
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
                    service.start(
                        RelayService.MODE_HOST,
                        host.nativeHandle,
                        "both",
                        audioGeometryForHostMode(
                            hostMode = true,
                            client = mutableState.value.settings,
                            host = wanted,
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
                        if (nativeStarted && !error.serviceWasAlreadyActive) {
                            service.stopAndWait()
                        }
                        if (nativeStarted) host.stopAndRelease()
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
            host.disconnectSession(sessionId)
        }
    }

    private fun startHostPolling() {
        host.startPolling(
            onEvents = ::consumeHostEvents,
            onStatus = ::applyHostStatus,
            onError = { error ->
                hostError(error.message ?: text(R.string.relay_error_host_failed))
            },
        )
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
                    val transactionId = event.optLong("transaction_id", -1L)
                    val peerId = event.optString("peer_id").ifBlank { event.optString("id") }
                    val previous = trustedStore.peer(peerId)
                    val secret = host.enrollmentSecret(transactionId)
                    val persisted = secret.isNotBlank() && saveTrustedPeer(
                        peerId = peerId,
                        secret = secret,
                        name = event.optString("name"),
                        address = event.optString("address"),
                    )
                    if (persisted) {
                        if (!host.acceptEnrollment(transactionId)) {
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
                        host.rejectEnrollment(
                            transactionId,
                            "trusted credential could not be durably persisted",
                        )
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

    private fun applyHostStatus(status: JSONObject) {
        if (status.optString("type") != "status") return
        val active = status.optBoolean("host_active")
        if (!active && mutableState.value.hostState == RelayHostState.Running) {
            // The service or native host may have stopped independently
            // (for example after the process lost its foreground service).
            // Reflect that transition and release the prepared handle so a
            // later explicit start imports the latest trusted credentials.
            if (!host.releaseInactive()) {
                hostError("native relay host became inactive but its handle is still owned")
                return
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
        setState {
            it.copy(
                sessions = RelayJson.sessions(status),
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

    /** Stop client-side native/audio state while the operation mutex is held. */
    private suspend fun stopClientLocked() {
        client.quiesceAndRelease()
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
        host.quiesceAndStop()
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
                    event.mode == RelayService.MODE_CLIENT && client.owns(event.handle)
                ) {
                    client.cancelPolling()
                    client.releaseNow()
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
                    host.owns(event.handle) &&
                    mutableState.value.hostState != RelayHostState.Idle
                ) {
                    host.cancelPolling()
                    host.stopAndRelease(event.handle)
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
                    if (host.owns(event.handle)) {
                        host.stopPollingAndWait()
                        // The service owns the worker threads that still use
                        // this native handle. Wait for its bounded teardown
                        // before stopping/releasing the host below.
                        service.stopAndWait()
                        host.stopAndRelease()
                        hostError(event.message)
                    }
                } else if (client.owns(event.handle)) {
                    // Do not invalidate a handle while the failed worker can
                    // still be inside a JNI audio call.
                    client.quiesceAndRelease()
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
            when (
                val started = discovery.start(mutableState.value.settings.deviceName) {
                    mutableState.value.discoveryActive
                }
            ) {
                is DiscoveryController.Started.Ok -> setState {
                    it.copy(
                        discoveryActive = true,
                        peers = emptyList(),
                        discoveryMessage = if (started.multicastAvailable) {
                            text(R.string.relay_discovery_started)
                        } else {
                            text(R.string.relay_discovery_multicast_unavailable)
                        },
                    )
                }

                is DiscoveryController.Started.Failed -> setState {
                    it.copy(
                        discoveryMessage = started.message
                            ?: text(R.string.relay_error_discovery_failed),
                    )
                }

                DiscoveryController.Started.AlreadyRunning -> Unit
            }
        }
    }

    fun stopDiscovery() {
        viewModelScope.launch(Dispatchers.IO) {
            discovery.stop()
            setState {
                it.copy(
                    discoveryActive = false,
                    peers = emptyList(),
                    discoveryMessage = text(R.string.relay_discovery_stopped),
                )
            }
        }
    }

    /**
     * Publish a discovery snapshot, keep a connected client supplied with it,
     * and decide whether it justifies an automatic trusted reconnect.
     *
     * Discovery has its own native engine on Android, so the connected client
     * has to be handed the snapshot explicitly: its resume worker can then
     * authenticate the same host at a new USB/Wi-Fi address instead of
     * retrying the original IP forever.
     */
    private fun onDiscoverySnapshot(snapshot: DiscoveryController.Snapshot) {
        if (snapshot is DiscoveryController.Snapshot.Failed) {
            // The poll failed outright, so there is no new peer list. Report
            // it and leave the last known peers standing.
            setState { it.copy(discoveryMessage = discoveryText(snapshot.message)) }
            return
        }
        val completed = snapshot as DiscoveryController.Snapshot.Peers
        // An error *event* does not invalidate the snapshot: report it and
        // still publish the peers this tick found.
        completed.message?.let { message ->
            setState { it.copy(discoveryMessage = discoveryText(message)) }
        }
        val peers = completed.peers
        client.updatePeers(RelayJson.discoveredPeersJson(peers))
        setState { it.copy(peers = peers) }
        autoConnectTrustedCandidate(peers)
    }

    private fun discoveryText(message: String): String =
        message.ifBlank { text(R.string.relay_error_discovery_failed) }

    /**
     * Pick the best trusted peer worth dialling automatically, if the UI is in
     * a state where connecting is appropriate and the candidate is not backed
     * off after a recent failure.
     */
    private fun autoConnectTrustedCandidate(peers: List<DiscoveredPeer>) {
        val current = mutableState.value
        val trustedRecords = trustedPeers()
        val now = android.os.SystemClock.elapsedRealtime()
        val candidate = peers
            .filter { peer ->
                peer.id.isNotBlank() && trustedRecords.any { it.peerId == peer.id } &&
                    trustedAutoConnectAllowed(current.settings, peer) &&
                    trustedCandidateAllowed(peer.id, peer.address, now)
            }
            .minWithOrNull(
                compareBy<DiscoveredPeer> { peer ->
                    trustedCandidateRank(
                        peer,
                        trustedRecords.firstOrNull { stored -> stored.peerId == peer.id },
                    )
                }.thenBy { it.address },
            )
            ?: return
        val microphoneReady = !clientNeedsMicrophone(current.settings.role) ||
            hasMicrophonePermission()
        if (
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
        val links = RelayJson.localLinks(NativeBridge.localLinks()) ?: return
        val usb = links.firstOrNull { it.kind == "usb" }?.let { UsbLinkInfo(it.name, it.addr) }
        val current = mutableState.value
        val usbAppeared = usb != null && !usbWasPresent
        val usbDisappeared = usb == null && usbWasPresent
        usbWasPresent = usb != null
        if (links != current.localLinks || usb != current.usbLink) {
            setState { it.copy(localLinks = links, usbLink = usb) }
        }
        if (usbDisappeared) {
            discovery.usbLinkLost()
            if (mutableState.value.discoveryActive) discovery.refreshNow()
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

    override fun onCleared() {
        client.cancelPolling()
        host.cancelPolling()
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
                        client.stopPollingAndWait()
                        host.stopPollingAndWait()
                        service.stopAndWait()
                        client.releaseNow()
                        if (host.isOpen) host.stopAndRelease()
                    }
                    discovery.release()
                }
            }
        }, "qpw-relay-viewmodel-cleanup").start()
        super.onCleared()
    }

    private companion object {
        const val POLL_INTERVAL_MS = 100L
        const val USB_LINK_POLL_INTERVAL_MS = 1_000L
        const val TRUSTED_AUTO_RETRY_INTERVAL_MS = 5_000L
    }
}
