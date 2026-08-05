package io.qpwgraph.relay

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExposedDropdownMenuBox
import androidx.compose.material3.ExposedDropdownMenuDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Tab
import androidx.compose.material3.TabRow
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel

/** Link options offered in the UI. USB is deliberately absent: the relay
 * auto-detects an active USB tether and prefers it under `auto`. */
private val LINK_OPTIONS = listOf("auto", "wifi", "bluetooth", "lan")
private val LINK_DISPLAY = mapOf(
    "auto" to "Auto",
    "wifi" to "Wi-Fi",
    "bluetooth" to "Bluetooth PAN",
    "lan" to "LAN",
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
    var permissionRequested by remember { mutableStateOf(false) }
    val permissionLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestMultiplePermissions(),
    ) { permissions ->
        val granted = permissions[Manifest.permission.RECORD_AUDIO] == true
        if (granted) viewModel.connect()
    }

    fun connectWithPermission() {
        val permissions = buildList {
            add(Manifest.permission.RECORD_AUDIO)
            if (Build.VERSION.SDK_INT >= 33) add(Manifest.permission.POST_NOTIFICATIONS)
        }
        val missing = permissions.filter {
            ContextCompat.checkSelfPermission(context, it) != PackageManager.PERMISSION_GRANTED
        }
        if (missing.isEmpty()) viewModel.connect()
        else if (!permissionRequested) {
            permissionRequested = true
            permissionLauncher.launch(missing.toTypedArray())
        }
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
                RelayMode.Receiver -> ReceiverTab(state, viewModel, ::connectWithPermission)
                RelayMode.Emitter -> EmitterTab(state, viewModel)
                RelayMode.Discover -> DiscoverTab(state, viewModel)
            }
        }
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
    }
}

@Composable
private fun ReceiverTab(
    state: RelayUiState,
    viewModel: RelayViewModel,
    connectWithPermission: () -> Unit,
) {
    OutlinedTextField(
        value = state.settings.target,
        onValueChange = { viewModel.update(state.settings.copy(target = it)) },
        label = { Text("Host address") },
        placeholder = { Text("192.168.1.20:48123") },
        modifier = Modifier.fillMaxWidth(),
        singleLine = true,
    )
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
            if (state.message.isNotBlank()) Text(state.message)
            Text("Level: ${(state.rms * 100).toInt()}%")
        }
    }
}

@Composable
private fun EmitterTab(state: RelayUiState, viewModel: RelayViewModel) {
    OutlinedTextField(
        value = state.host.deviceName,
        onValueChange = { viewModel.updateHost(state.host.copy(deviceName = it)) },
        label = { Text("Device name") },
        modifier = Modifier.fillMaxWidth(),
        singleLine = true,
    )
    OutlinedTextField(
        value = state.host.pin,
        onValueChange = { viewModel.updateHost(state.host.copy(pin = it)) },
        label = { Text("Pairing PIN") },
        modifier = Modifier.fillMaxWidth(),
        singleLine = true,
    )
    OutlinedTextField(
        value = state.host.port.toString(),
        onValueChange = { value ->
            value.toIntOrNull()?.let { viewModel.updateHost(state.host.copy(port = it)) }
        },
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
            modifier = Modifier.weight(1f),
        )
        DropdownField(
            label = "Link",
            value = state.host.transport,
            options = LINK_OPTIONS,
            display = LINK_DISPLAY,
            onSelected = { viewModel.updateHost(state.host.copy(transport = it)) },
            modifier = Modifier.weight(1f),
        )
    }
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        if (state.hostState == RelayHostState.Running) {
            Button(onClick = viewModel::stopHost, modifier = Modifier.weight(1f)) {
                Text("Stop host")
            }
        } else {
            Button(onClick = viewModel::startHost, modifier = Modifier.weight(1f)) {
                Text("Start host")
            }
        }
    }
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp)) {
            Text("Status", style = MaterialTheme.typography.titleMedium)
            Spacer(Modifier.height(6.dp))
            Text(state.hostState.name.lowercase())
            if (state.hostPort != null) Text("Listening on port ${state.hostPort}")
            if (state.hostMessage.isNotBlank()) Text(state.hostMessage)
            Text("Level: ${(state.hostRms * 100).toInt()}%")
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
                        Text("${session.name} — ${session.address}")
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
private fun DiscoverTab(state: RelayUiState, viewModel: RelayViewModel) {
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
            Button(onClick = { viewModel.connectToPeer(peer.address) }) {
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
    modifier: Modifier = Modifier,
    display: Map<String, String> = emptyMap(),
) {
    var expanded by remember { mutableStateOf(false) }
    ExposedDropdownMenuBox(
        expanded = expanded,
        onExpandedChange = { expanded = !expanded },
        modifier = modifier,
    ) {
        OutlinedTextField(
            value = display[value] ?: value,
            onValueChange = {},
            readOnly = true,
            label = { Text(label) },
            trailingIcon = { ExposedDropdownMenuDefaults.TrailingIcon(expanded) },
            modifier = Modifier.menuAnchor().fillMaxWidth(),
        )
        ExposedDropdownMenu(expanded = expanded, onDismissRequest = { expanded = false }) {
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
