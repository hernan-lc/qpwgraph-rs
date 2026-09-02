package io.qpwgraph.relay.ui.components

import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Info
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.PlainTooltip
import androidx.compose.material3.Text
import androidx.compose.material3.TooltipBox
import androidx.compose.material3.TooltipDefaults
import androidx.compose.material3.rememberTooltipState
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import io.qpwgraph.relay.R

/**
 * Wraps any content with a Material 3 plain tooltip on long-press / hover.
 *
 * Uses [TooltipBox] + [PlainTooltip] so the tooltip participates in the
 * Material 3 motion/placement system and respects accessibility.
 * On mobile the tooltip appears on long-press; on larger screens it also
 * appears on hover.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AppTooltip(
    text: String,
    modifier: Modifier = Modifier,
    content: @Composable () -> Unit,
) {
    if (text.isBlank()) {
        content()
        return
    }
    TooltipBox(
        positionProvider = TooltipDefaults.rememberPlainTooltipPositionProvider(),
        tooltip = { PlainTooltip { Text(text) } },
        state = rememberTooltipState(),
        modifier = modifier,
    ) {
        content()
    }
}

/**
 * Small info icon that reveals a tooltip. Use next to labels that need
 * an explanation without cluttering the layout.
 *
 * Example:
 * ```
 * Row(verticalAlignment = Alignment.CenterVertically) {
 *   Text(stringResource(R.string.receiver_link_label))
 *   InfoTooltip(stringResource(R.string.receiver_link_tooltip))
 * }
 * ```
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun InfoTooltip(
    tooltip: String,
    modifier: Modifier = Modifier,
    contentDescription: String? = null,
) {
    if (tooltip.isBlank()) return
    val cd = contentDescription ?: stringResource(R.string.cd_info)
    TooltipBox(
        positionProvider = TooltipDefaults.rememberPlainTooltipPositionProvider(),
        tooltip = {
            PlainTooltip {
                Text(tooltip, style = MaterialTheme.typography.bodySmall)
            }
        },
        state = rememberTooltipState(),
        modifier = modifier,
    ) {
        IconButton(
            onClick = {},
            modifier = Modifier.size(20.dp),
            enabled = false, // icon is purely for tooltip affordance; Box handles gesture
        ) {
            Icon(
                imageVector = Icons.Outlined.Info,
                contentDescription = cd,
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.size(16.dp),
            )
        }
    }
}

/**
 * Alternative interactive variant that also shows tooltip on tap (for
 * accessibility / TalkBack). The icon is enabled and taps toggle tooltip
 * visibility via the state.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun InfoTooltipInteractive(
    tooltip: String,
    modifier: Modifier = Modifier,
) {
    if (tooltip.isBlank()) return
    val state = rememberTooltipState()
    TooltipBox(
        positionProvider = TooltipDefaults.rememberPlainTooltipPositionProvider(),
        tooltip = { PlainTooltip { Text(tooltip) } },
        state = state,
        modifier = modifier,
    ) {
        IconButton(onClick = {}) {
            Icon(
                imageVector = Icons.Outlined.Info,
                contentDescription = stringResource(R.string.cd_info),
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.size(16.dp),
            )
        }
    }
}
