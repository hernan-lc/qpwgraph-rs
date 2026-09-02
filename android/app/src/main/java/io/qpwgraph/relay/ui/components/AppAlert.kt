package io.qpwgraph.relay.ui.components

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Error
import androidx.compose.material.icons.filled.Info
import androidx.compose.material.icons.filled.Warning
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import io.qpwgraph.relay.R
import io.qpwgraph.relay.ui.theme.ErrorContainer
import io.qpwgraph.relay.ui.theme.ErrorRed
import io.qpwgraph.relay.ui.theme.InfoBlue
import io.qpwgraph.relay.ui.theme.InfoContainer
import io.qpwgraph.relay.ui.theme.SuccessContainer
import io.qpwgraph.relay.ui.theme.SuccessGreen
import io.qpwgraph.relay.ui.theme.WarningAmber
import io.qpwgraph.relay.ui.theme.WarningContainer

enum class AlertSeverity {
    Info,
    Success,
    Warning,
    Error,
}

private data class AlertStyle(
    val icon: ImageVector,
    val containerColor: androidx.compose.ui.graphics.Color,
    val contentColor: androidx.compose.ui.graphics.Color,
    val iconColor: androidx.compose.ui.graphics.Color,
)

/**
 * Material 3 alert / banner component.
 *
 * Replaces ad-hoc `Text(message)` error rendering in the previous layout.
 * Severity maps to distinct container/icon colors, supports an optional
 * title, dismiss action, and an optional primary action (e.g. "Retry").
 *
 * Accessible: role and contentDescription reflect severity.
 */
@Composable
fun AppAlert(
    message: String,
    severity: AlertSeverity = AlertSeverity.Info,
    modifier: Modifier = Modifier,
    title: String? = null,
    dismissible: Boolean = false,
    onDismiss: (() -> Unit)? = null,
    actionLabel: String? = null,
    onAction: (() -> Unit)? = null,
) {
    if (message.isBlank() && title.isNullOrBlank()) return

    val style = when (severity) {
        AlertSeverity.Info -> AlertStyle(Icons.Filled.Info, InfoContainer, InfoBlue, InfoBlue)
        AlertSeverity.Success -> AlertStyle(Icons.Filled.CheckCircle, SuccessContainer, SuccessGreen, SuccessGreen)
        AlertSeverity.Warning -> AlertStyle(Icons.Filled.Warning, WarningContainer, WarningAmber, WarningAmber)
        AlertSeverity.Error -> AlertStyle(Icons.Filled.Error, ErrorContainer, ErrorRed, ErrorRed)
    }

    val severityLabel = when (severity) {
        AlertSeverity.Info -> stringResource(R.string.alert_info)
        AlertSeverity.Success -> stringResource(R.string.alert_success)
        AlertSeverity.Warning -> stringResource(R.string.alert_warning)
        AlertSeverity.Error -> stringResource(R.string.alert_error)
    }

    Card(
        modifier = modifier
            .fillMaxWidth()
            .semantics { contentDescription = "$severityLabel: $message" },
        colors = CardDefaults.cardColors(
            containerColor = style.containerColor,
            contentColor = style.contentColor,
        ),
        elevation = CardDefaults.cardElevation(defaultElevation = 0.dp),
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(12.dp),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
            verticalAlignment = Alignment.Top,
        ) {
            Icon(
                imageVector = style.icon,
                contentDescription = severityLabel,
                tint = style.iconColor,
                modifier = Modifier
                    .padding(top = 2.dp)
                    .size(20.dp),
            )
            Column(
                modifier = Modifier.weight(1f),
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                if (!title.isNullOrBlank()) {
                    Text(
                        text = title,
                        style = MaterialTheme.typography.titleSmall,
                        color = style.contentColor,
                    )
                }
                if (message.isNotBlank()) {
                    Text(
                        text = message,
                        style = MaterialTheme.typography.bodyMedium,
                        color = style.contentColor,
                    )
                }
                if (actionLabel != null && onAction != null) {
                    TextButton(onClick = onAction) {
                        Text(actionLabel, color = style.contentColor)
                    }
                }
            }
            if (dismissible && onDismiss != null) {
                IconButton(
                    onClick = onDismiss,
                    modifier = Modifier.size(24.dp),
                ) {
                    Icon(
                        imageVector = Icons.Filled.Close,
                        contentDescription = stringResource(R.string.alert_dismiss),
                        tint = style.contentColor,
                        modifier = Modifier.size(18.dp),
                    )
                }
            }
        }
    }
}

/**
 * Convenience for inline field errors (compact, error severity).
 */
@Composable
fun FieldError(text: String, modifier: Modifier = Modifier) {
    if (text.isBlank()) return
    AppAlert(message = text, severity = AlertSeverity.Error, modifier = modifier)
}
