package io.qpwgraph.relay.ui.theme

import android.os.Build
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.platform.LocalContext

private val LightColors = lightColorScheme(
    primary = BrandBlue,
    onPrimary = androidx.compose.ui.graphics.Color.White,
    primaryContainer = BrandBlueContainer,
    onPrimaryContainer = BrandBlueDark,
    secondary = BrandBlueDark,
    surface = SurfaceLight,
    surfaceVariant = SurfaceVariantLight,
    onSurface = OnSurfaceLight,
    error = ErrorRed,
    onError = androidx.compose.ui.graphics.Color.White,
    errorContainer = ErrorContainer,
    onErrorContainer = ErrorRed,
)

private val DarkColors = darkColorScheme(
    primary = BrandBlueLight,
    onPrimary = BrandBlueDark,
    primaryContainer = BrandBlueDark,
    onPrimaryContainer = BrandBlueLight,
    secondary = BrandBlueLight,
    error = ErrorContainer,
    onError = ErrorRed,
)

@Composable
fun QpwRelayTheme(
    darkTheme: Boolean = isSystemInDarkTheme(),
    dynamicColor: Boolean = false,
    content: @Composable () -> Unit,
) {
    val context = LocalContext.current
    val colorScheme = when {
        dynamicColor && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S -> {
            if (darkTheme) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
        }
        darkTheme -> DarkColors
        else -> LightColors
    }
    MaterialTheme(
        colorScheme = colorScheme,
        typography = RelayTypography,
        content = content,
    )
}
