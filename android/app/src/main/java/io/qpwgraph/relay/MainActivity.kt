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
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel

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
                display = mapOf("emit" to "Emit microphone", "receive" to "Receive playback", "both" to "Both"),
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
                    options = listOf("auto", "usb", "wifi", "bluetooth", "lan"),
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
                    Button(onClick = ::connectWithPermission, modifier = Modifier.weight(1f)) {
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
