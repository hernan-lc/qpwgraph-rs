package io.qpwgraph.relay

import android.Manifest
import android.app.Activity
import android.content.Context
import android.content.pm.PackageManager
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExposedDropdownMenuBox
import androidx.compose.material3.ExposedDropdownMenuDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Tab
import androidx.compose.material3.TabRow
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.Switch
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.google.mlkit.vision.barcode.BarcodeScannerOptions
import com.google.mlkit.vision.barcode.BarcodeScanning
import com.google.mlkit.vision.barcode.common.Barcode
import com.google.mlkit.vision.common.InputImage
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

/** Link options offered in the UI. USB is auto-detected; ADB is an explicit
 * localhost TCP-forwarding mode and requires `adb reverse`/`adb forward`. */
private val LINK_OPTIONS = listOf("auto", "wifi", "bluetooth", "lan", "adb")
private val LINK_DISPLAY = mapOf(
    "auto" to "Auto",
    "wifi" to "Wi-Fi",
    "bluetooth" to "Bluetooth PAN",
    "lan" to "LAN",
    "adb" to "ADB forwarding",
)

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent { RelayApp() }
    }
}

@Composable
private fun RelayApp(viewModel: RelayViewModel = viewModel()) {
    val context = LocalContext.current
    val state by viewModel.state.collectAsStateWithLifecycle()
    var showScanner by remember { mutableStateOf(false) }
    var pendingPermissionAction by remember { mutableStateOf<(() -> Unit)?>(null) }
    var pendingMicrophonePermission by remember { mutableStateOf(false) }
    var pendingHostAction by remember { mutableStateOf(false) }
    val permissionLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestMultiplePermissions(),
    ) { permissions ->
        val microphoneGranted = permissions[Manifest.permission.RECORD_AUDIO] != false
        val action = pendingPermissionAction
        val needsMicrophone = pendingMicrophonePermission
        val hostAction = pendingHostAction
        pendingPermissionAction = null
        pendingMicrophonePermission = false
        pendingHostAction = false
        if (needsMicrophone && !microphoneGranted) {
            viewModel.permissionDenied(hostAction)
        } else {
            // Notification permission is intentionally non-fatal: foreground
            // service startup is still attempted on API 33+ when the user
            // declines notifications.
            action?.invoke()
        }
    }
    val mediaProjectionLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.StartActivityForResult(),
    ) { result ->
        viewModel.onMediaProjectionResult(result.resultCode, result.data)
        // After consent, retry host start if we have the audio permission already.
        val hasAudio = ContextCompat.checkSelfPermission(context, Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED
        if (result.resultCode == Activity.RESULT_OK && hasAudio) {
            viewModel.startHost()
        }
    }
    val cameraPermissionLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { granted ->
        if (granted) showScanner = true
    }

    fun runWithServicePermissions(
        requiresMicrophone: Boolean,
        host: Boolean,
        action: () -> Unit,
    ) {
        val permissions = buildList {
            if (requiresMicrophone) add(Manifest.permission.RECORD_AUDIO)
            if (Build.VERSION.SDK_INT >= 33) add(Manifest.permission.POST_NOTIFICATIONS)
        }
        val missing = permissions.filter {
            ContextCompat.checkSelfPermission(context, it) != PackageManager.PERMISSION_GRANTED
        }
        if (missing.isEmpty()) action()
        else {
            pendingPermissionAction = action
            pendingMicrophonePermission = requiresMicrophone
            pendingHostAction = host
            permissionLauncher.launch(missing.toTypedArray())
        }
    }

    fun openScanner() {
        val granted = ContextCompat.checkSelfPermission(
            context,
            Manifest.permission.CAMERA,
        ) == PackageManager.PERMISSION_GRANTED
        if (granted) showScanner = true
        else cameraPermissionLauncher.launch(Manifest.permission.CAMERA)
    }

    MaterialTheme {
        Column(
            modifier = Modifier
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .padding(20.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text("qpwgraph Relay", style = MaterialTheme.typography.headlineMedium)
            Text("Use your Android device as a relay microphone, speaker, or both.")
            RelayTabs(mode = state.mode, onSelected = viewModel::setMode)
            UsbStatus(link = state.usbLink)
            when (state.mode) {
                RelayMode.Receiver -> ReceiverTab(
                    state,
                    viewModel,
                    connectWithPermission = {
                        runWithServicePermissions(
                            clientNeedsMicrophone(state.settings.role),
                            host = false,
                            action = viewModel::connect,
                        )
                    },
                    openScanner = ::openScanner,
                )
                RelayMode.Emitter -> EmitterTab(
                    state,
                    viewModel,
                    startHost = {
                        val source = state.host.captureSource
                        if (source == CaptureSource.DEVICE_PLAYBACK) {
                            // Playback capture requires RECORD_AUDIO + MediaProjection consent.
                            // RECORD_AUDIO is still required but does NOT imply microphone use.
                            runWithServicePermissions(
                                requiresMicrophone = true,
                                host = true,
                                action = {
                                    if (viewModel.hasMediaProjectionConsent()) {
                                        viewModel.startHost()
                                    } else {
                                        val mgr = context.getSystemService(Context.MEDIA_PROJECTION_SERVICE) as MediaProjectionManager
                                        mediaProjectionLauncher.launch(mgr.createScreenCaptureIntent())
                                    }
                                },
                            )
                        } else {
                            runWithServicePermissions(
                                requiresMicrophone = true,
                                host = true,
                                action = viewModel::startHost,
                            )
                        }
                    },
                    requestPlaybackConsent = {
                        val mgr = context.getSystemService(Context.MEDIA_PROJECTION_SERVICE) as MediaProjectionManager
                        mediaProjectionLauncher.launch(mgr.createScreenCaptureIntent())
                    },
                )
                RelayMode.Discover -> DiscoverTab(
                    state,
                    viewModel,
                    connectToPeer = { address ->
                        runWithServicePermissions(
                            clientNeedsMicrophone(state.settings.role),
                            host = false,
                            action = { viewModel.connectToPeer(address) },
                        )
                    },
                )
            }
            TrustedDevicesCard(state, viewModel)
        }
    }
    if (showScanner) {
        QrScannerDialog(
            onDetected = { value ->
                showScanner = false
                viewModel.applyScannedQr(value)
            },
            onDismiss = { showScanner = false },
        )
    }
}

@Composable
private fun RelayTabs(mode: RelayMode, onSelected: (RelayMode) -> Unit) {
    val tabs = listOf(
        "Receiver" to RelayMode.Receiver,
        "Emitter" to RelayMode.Emitter,
        "Discover" to RelayMode.Discover,
    )
    TabRow(selectedTabIndex = tabs.indexOfFirst { it.second == mode }.coerceAtLeast(0)) {
        tabs.forEach { (label, tabMode) ->
            Tab(
                selected = mode == tabMode,
                onClick = { onSelected(tabMode) },
                text = { Text(label) },
            )
        }
    }
}

@Composable
private fun UsbStatus(link: UsbLinkInfo?) {
    if (link != null) {
        Text(
            stringResource(R.string.relay_usb_detected, link.name, link.addr),
            style = MaterialTheme.typography.bodySmall,
        )
    } else {
        Text(
            "No USB tether network detected. For an ADB-only cable, select ADB forwarding and configure adb reverse/forward; otherwise use Wi-Fi/LAN.",
            style = MaterialTheme.typography.bodySmall,
        )
    }
}

/** Camera preview that decodes QR codes and reports the first payload. */
// `ImageProxy.image` is CameraX's experimental accessor; ML Kit's
// `InputImage.fromMediaImage` is the documented consumer for it. Opt in
// explicitly rather than silencing the lint.
@androidx.annotation.OptIn(androidx.camera.core.ExperimentalGetImage::class)
@Composable
private fun QrScannerDialog(onDetected: (String) -> Unit, onDismiss: () -> Unit) {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    val executor = remember { Executors.newSingleThreadExecutor() }
    val scanner = remember {
        BarcodeScanning.getClient(
            BarcodeScannerOptions.Builder()
                .setBarcodeFormats(Barcode.FORMAT_QR_CODE)
                .build(),
        )
    }
    val detected = remember { AtomicBoolean(false) }
    var cameraProvider by remember { mutableStateOf<ProcessCameraProvider?>(null) }
    DisposableEffect(Unit) {
        onDispose {
            cameraProvider?.unbindAll()
            executor.shutdown()
            scanner.close()
        }
    }
    Dialog(
        onDismissRequest = onDismiss,
        properties = DialogProperties(usePlatformDefaultWidth = false),
    ) {
        Box(
            modifier = Modifier
                .size(320.dp)
                .clip(RoundedCornerShape(16.dp)),
        ) {
            AndroidView(
                factory = { ctx ->
                    PreviewView(ctx).also { previewView ->
                        val future = ProcessCameraProvider.getInstance(ctx)
                        future.addListener({
                            val provider = future.get()
                            cameraProvider = provider
                            val preview = Preview.Builder().build().also { built ->
                                built.setSurfaceProvider(previewView.surfaceProvider)
                            }
                            val analysis = ImageAnalysis.Builder()
                                .setBackpressureStrategy(
                                    ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST,
                                )
                                .build()
                            analysis.setAnalyzer(executor) { proxy ->
                                val image = proxy.image
                                if (image == null || detected.get()) {
                                    proxy.close()
                                    return@setAnalyzer
                                }
                                val input = InputImage.fromMediaImage(
                                    image,
                                    proxy.imageInfo.rotationDegrees,
                                )
                                scanner.process(input)
                                    .addOnSuccessListener { codes ->
                                        val value = codes.firstNotNullOfOrNull { it.rawValue }
                                        if (value != null && detected.compareAndSet(false, true)) {
                                            onDetected(value)
                                        }
                                    }
                                    .addOnCompleteListener { proxy.close() }
                            }
                            provider.unbindAll()
                            provider.bindToLifecycle(
                                lifecycleOwner,
                                CameraSelector.DEFAULT_BACK_CAMERA,
                                preview,
                                analysis,
                            )
                        }, ContextCompat.getMainExecutor(ctx))
                    }
                },
                modifier = Modifier.fillMaxSize(),
            )
            TextButton(
                onClick = onDismiss,
                modifier = Modifier.align(Alignment.BottomCenter),
            ) {
                Text("Cancel")
            }
        }
    }
}

@Composable
private fun ReceiverTab(
    state: RelayUiState,
    viewModel: RelayViewModel,
    connectWithPermission: () -> Unit,
    openScanner: () -> Unit,
) {
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        OutlinedTextField(
            value = state.settings.target,
            onValueChange = { viewModel.update(state.settings.copy(target = it)) },
            label = { Text("Host address") },
            placeholder = { Text("192.168.1.20:48123") },
            modifier = Modifier.weight(1f),
            singleLine = true,
        )
        OutlinedButton(onClick = openScanner, modifier = Modifier.padding(top = 8.dp)) {
            Text("Scan QR")
        }
    }
    OutlinedTextField(
        value = state.settings.pin,
        onValueChange = { viewModel.update(state.settings.copy(pin = it)) },
        label = { Text("Pairing PIN") },
        modifier = Modifier.fillMaxWidth(),
        singleLine = true,
    )
    DropdownField(
        label = "Role",
        value = state.settings.role,
        options = listOf("emit", "receive", "both"),
        display = mapOf(
            "emit" to "Emit microphone",
            "receive" to "Receive playback",
            "both" to "Both",
        ),
        onSelected = { viewModel.update(state.settings.copy(role = it)) },
    )
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        DropdownField(
            label = "Codec",
            value = state.settings.codec,
            options = listOf("opus", "pcm"),
            onSelected = { viewModel.update(state.settings.copy(codec = it)) },
            modifier = Modifier.weight(1f),
        )
        DropdownField(
            label = "Link",
            value = state.settings.transport,
            options = LINK_OPTIONS,
            display = LINK_DISPLAY,
            onSelected = { viewModel.update(state.settings.copy(transport = it)) },
            modifier = Modifier.weight(1f),
        )
    }
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp)) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text("Automatically reconnect trusted devices")
                Switch(
                    checked = state.settings.autoConnectTrusted,
                    onCheckedChange = { enabled ->
                        viewModel.update(state.settings.copy(autoConnectTrusted = enabled))
                    },
                )
            }
            if (state.settings.autoConnectTrusted) {
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.SpaceBetween,
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text("Allow trusted Wi-Fi reconnect")
                    Switch(
                        checked = state.settings.autoConnectTrustedWifi,
                        onCheckedChange = { enabled ->
                            viewModel.update(
                                state.settings.copy(autoConnectTrustedWifi = enabled),
                            )
                        },
                    )
                }
                Text(
                    "USB tethering is enabled by default; LAN, Wi-Fi, and ADB remain opt-in.",
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        }
    }
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        if (state.connection == RelayConnectionState.Connected ||
            state.connection == RelayConnectionState.Connecting
        ) {
            Button(onClick = viewModel::disconnect, modifier = Modifier.weight(1f)) {
                Text("Disconnect")
            }
        } else {
            Button(onClick = connectWithPermission, modifier = Modifier.weight(1f)) {
                Text("Connect")
            }
        }
    }
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp)) {
            Text("Status", style = MaterialTheme.typography.titleMedium)
            Spacer(Modifier.height(6.dp))
            Text(state.connection.name.lowercase().replace('_', ' '))
            if (state.hostName.isNotBlank()) Text("Host: ${state.hostName}")
            if (state.sessionId != null) Text("Session: ${state.sessionId}")
            if (state.transport.isNotBlank()) {
                Text(
                    "Connected via ${state.link.ifBlank { "unknown link" }} " +
                        "(${state.transport})",
                )
            }
            if (state.audioChannelState == "reconnecting") Text("Reconnecting audio")
            if (state.message.isNotBlank()) Text(state.message)
            Text("Level: ${(state.rms * 100).toInt()}%")
        }
    }
}

@Composable
private fun TrustedDevicesCard(state: RelayUiState, viewModel: RelayViewModel) {
    if (state.trustedPeers.isEmpty()) return
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp)) {
            Text("Trusted devices", style = MaterialTheme.typography.titleMedium)
            Spacer(Modifier.height(6.dp))
            state.trustedPeers.forEach { peer ->
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.SpaceBetween,
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Column(modifier = Modifier.weight(1f)) {
                        Text(peer.name.ifBlank { peer.peerId })
                        if (peer.address.isNotBlank()) {
                            Text(peer.address, style = MaterialTheme.typography.bodySmall)
                        }
                    }
                    TextButton(onClick = { viewModel.forgetTrustedPeer(peer.peerId) }) {
                        Text("Forget")
                    }
                }
            }
        }
    }
}

@Composable
private fun EmitterTab(
    state: RelayUiState,
    viewModel: RelayViewModel,
    startHost: () -> Unit,
    requestPlaybackConsent: () -> Unit,
) {
    val hostEditable = state.hostState != RelayHostState.Starting &&
        state.hostState != RelayHostState.Running
    OutlinedTextField(
        value = state.host.deviceName,
        onValueChange = { viewModel.updateHost(state.host.copy(deviceName = it)) },
        enabled = hostEditable,
        label = { Text("Device name") },
        modifier = Modifier.fillMaxWidth(),
        singleLine = true,
    )
    OutlinedTextField(
        value = state.host.pin,
        onValueChange = { viewModel.updateHost(state.host.copy(pin = it)) },
        enabled = hostEditable,
        label = { Text("Pairing PIN") },
        modifier = Modifier.fillMaxWidth(),
        singleLine = true,
    )
    OutlinedTextField(
        value = state.host.port.toString(),
        onValueChange = { value ->
            value.toIntOrNull()?.let { viewModel.updateHost(state.host.copy(port = it)) }
        },
        enabled = hostEditable,
        label = { Text("Control port") },
        modifier = Modifier.fillMaxWidth(),
        singleLine = true,
    )
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        DropdownField(
            label = "Codec",
            value = state.host.codec,
            options = listOf("opus", "pcm"),
            onSelected = { viewModel.updateHost(state.host.copy(codec = it)) },
            enabled = hostEditable,
            modifier = Modifier.weight(1f),
        )
        DropdownField(
            label = "Link",
            value = state.host.transport,
            options = LINK_OPTIONS,
            display = LINK_DISPLAY,
            onSelected = { viewModel.updateHost(state.host.copy(transport = it)) },
            enabled = hostEditable,
            modifier = Modifier.weight(1f),
        )
    }
    DropdownField(
        label = "Capture source",
        value = state.host.captureSource.name.lowercase(),
        options = listOf("microphone", "device_playback"),
        display = mapOf("microphone" to "Microphone", "device_playback" to "Device playback"),
        onSelected = { viewModel.setHostCaptureSource(captureSourceFromString(it)) },
        enabled = hostEditable,
    )
    if (state.host.captureSource == CaptureSource.DEVICE_PLAYBACK) {
        Card(modifier = Modifier.fillMaxWidth()) {
            Column(modifier = Modifier.padding(12.dp)) {
                Text("Device playback capture requires Android audio-recording permission, but captures device playback instead of the physical microphone. You will be asked to allow screen/audio capture.", style = MaterialTheme.typography.bodySmall)
                if (!viewModel.hasMediaProjectionConsent() && hostEditable) {
                    Spacer(Modifier.height(6.dp))
                    OutlinedButton(onClick = requestPlaybackConsent) { Text("Grant capture consent") }
                }
            }
        }
    }
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        if (state.hostState == RelayHostState.Running) {
            Button(onClick = viewModel::stopHost, modifier = Modifier.weight(1f)) {
                Text("Stop host")
            }
        } else {
            Button(onClick = startHost, modifier = Modifier.weight(1f)) {
                Text("Start host")
            }
        }
    }
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp)) {
            Text("Status", style = MaterialTheme.typography.titleMedium)
            Spacer(Modifier.height(6.dp))
            Text("Host: ${state.hostState.name.lowercase()}  •  Audio: ${state.hostAudioState.name.lowercase()}")
            if (state.hostPort != null) Text("Listening on port ${state.hostPort}")
            if (state.hostMessage.isNotBlank()) Text(state.hostMessage)
            if (state.hostAudioMessage.isNotBlank() && state.hostAudioMessage != state.hostMessage) Text("Audio: ${state.hostAudioMessage}")
            Text("Level: ${(state.hostRms * 100).toInt()}%")
            Text("Capture: ${state.host.captureSource.name.lowercase()}", style = MaterialTheme.typography.bodySmall)
        }
    }
    val hostPort = state.hostPort
    val hostAddress = state.hostAddress
    if (state.hostState == RelayHostState.Running &&
        hostPort != null &&
        hostAddress != null
    ) {
        Card(modifier = Modifier.fillMaxWidth()) {
            Column(modifier = Modifier.padding(16.dp)) {
                Text("Reachable at", style = MaterialTheme.typography.titleMedium)
                Spacer(Modifier.height(6.dp))
                Text("$hostAddress:$hostPort")
            }
        }
    }
    if (state.sessions.isNotEmpty()) {
        Card(modifier = Modifier.fillMaxWidth()) {
            Column(modifier = Modifier.padding(16.dp)) {
                Text("Active sessions", style = MaterialTheme.typography.titleMedium)
                Spacer(Modifier.height(6.dp))
                state.sessions.forEach { session ->
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.SpaceBetween,
                    ) {
                        Column(modifier = Modifier.weight(1f)) {
                            Text("${session.name} — ${session.address}")
                            if (session.transport.isNotBlank()) {
                                Text(
                                    "${session.link.ifBlank { "unknown link" }} / " +
                                        session.transport +
                                        if (session.audioChannelState == "reconnecting") {
                                            " — reconnecting audio"
                                        } else {
                                            ""
                                        },
                                    style = MaterialTheme.typography.bodySmall,
                                )
                            }
                        }
                        TextButton(onClick = { viewModel.disconnectSession(session.id) }) {
                            Text("Disconnect")
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun DiscoverTab(
    state: RelayUiState,
    viewModel: RelayViewModel,
    connectToPeer: (String) -> Unit,
) {
    Button(
        onClick = {
            if (state.discoveryActive) viewModel.stopDiscovery() else viewModel.startDiscovery()
        },
        modifier = Modifier.fillMaxWidth(),
    ) {
        Text(if (state.discoveryActive) "Stop discovery" else "Start discovery")
    }
    if (state.discoveryMessage.isNotBlank()) {
        Text(state.discoveryMessage, style = MaterialTheme.typography.bodySmall)
    }
    if (state.peers.isEmpty()) {
        Text(
            "No relay hosts found yet. Keep discovery running while the host advertises; USB tethers are scanned automatically.",
            style = MaterialTheme.typography.bodySmall,
        )
    }
    state.peers.forEach { peer ->
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            Text("${peer.name} — ${peer.address}")
            Button(onClick = { connectToPeer(peer.address) }) {
                Text("Connect")
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun DropdownField(
    label: String,
    value: String,
    options: List<String>,
    onSelected: (String) -> Unit,
    enabled: Boolean = true,
    modifier: Modifier = Modifier,
    display: Map<String, String> = emptyMap(),
) {
    var expanded by remember { mutableStateOf(false) }
    ExposedDropdownMenuBox(
        expanded = expanded,
        onExpandedChange = { if (enabled) expanded = !expanded },
        modifier = modifier,
    ) {
        OutlinedTextField(
            value = display[value] ?: value,
            onValueChange = {},
            readOnly = true,
            enabled = enabled,
            label = { Text(label) },
            trailingIcon = { ExposedDropdownMenuDefaults.TrailingIcon(expanded) },
            modifier = Modifier.menuAnchor().fillMaxWidth(),
        )
        ExposedDropdownMenu(
            expanded = expanded && enabled,
            onDismissRequest = { expanded = false },
        ) {
            options.forEach { option ->
                DropdownMenuItem(
                    text = { Text(display[option] ?: option) },
                    onClick = {
                        onSelected(option)
                        expanded = false
                    },
                )
            }
        }
    }
}
