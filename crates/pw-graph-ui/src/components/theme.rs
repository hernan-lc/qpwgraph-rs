//! Theme management and semantic design tokens for DOM-like components.

use egui::{Color32, Stroke};

/// Light or Dark appearance mode.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
}

impl ThemeMode {
    pub fn is_dark(self) -> bool {
        matches!(self, Self::Dark)
    }

    pub fn is_light(self) -> bool {
        matches!(self, Self::Light)
    }

    pub fn toggled(self) -> Self {
        match self {
            Self::Dark => Self::Light,
            Self::Light => Self::Dark,
        }
    }
}

/// Semantic design tokens for UI components.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ThemeToken {
    /// Window or page background.
    Background,
    /// Container or panel surface background.
    Surface,
    /// Hovered state background for cards or rows.
    SurfaceHover,
    /// Selected state background for cards or tabs.
    SurfaceSelected,
    /// Default border stroke.
    Border,
    /// Primary high-contrast text.
    TextPrimary,
    /// Secondary / muted label text.
    TextSecondary,
    /// Weak / hint text.
    TextWeak,
    /// Primary accent highlight.
    Accent,
    /// Connected / active status green.
    AccentConnected,
    /// Warning / amber status color.
    AccentWarning,
    /// Error / danger status color.
    AccentError,
}

/// Palette holding color assignments for every [`ThemeToken`].
#[derive(Clone, Debug, PartialEq)]
pub struct ThemePalette {
    pub background: Color32,
    pub surface: Color32,
    pub surface_hover: Color32,
    pub surface_selected: Color32,
    pub border: Color32,
    pub text_primary: Color32,
    pub text_secondary: Color32,
    pub text_weak: Color32,
    pub accent: Color32,
    pub accent_connected: Color32,
    pub accent_warning: Color32,
    pub accent_error: Color32,
}

impl ThemePalette {
    /// High-contrast dark palette.
    pub fn dark() -> Self {
        Self {
            background: Color32::from_rgb(25, 29, 36),
            surface: Color32::from_rgb(34, 40, 50),
            surface_hover: Color32::from_rgb(45, 54, 68),
            surface_selected: Color32::from_rgb(35, 86, 119),
            border: Color32::from_rgb(59, 70, 84),
            text_primary: Color32::from_rgb(240, 244, 250),
            text_secondary: Color32::from_rgb(215, 225, 238),
            text_weak: Color32::from_rgb(180, 195, 215),
            accent: Color32::from_rgb(96, 165, 250),
            accent_connected: Color32::from_rgb(96, 190, 130),
            accent_warning: Color32::from_rgb(239, 169, 82),
            accent_error: Color32::from_rgb(239, 90, 90),
        }
    }

    /// High-contrast light palette.
    pub fn light() -> Self {
        Self {
            background: Color32::from_rgb(245, 247, 250),
            surface: Color32::from_rgb(255, 255, 255),
            surface_hover: Color32::from_rgb(235, 240, 248),
            surface_selected: Color32::from_rgb(210, 230, 250),
            border: Color32::from_rgb(205, 215, 225),
            text_primary: Color32::from_rgb(20, 25, 32),
            text_secondary: Color32::from_rgb(50, 65, 85),
            text_weak: Color32::from_rgb(90, 105, 125),
            accent: Color32::from_rgb(37, 99, 235),
            accent_connected: Color32::from_rgb(22, 163, 74),
            accent_warning: Color32::from_rgb(217, 119, 6),
            accent_error: Color32::from_rgb(220, 38, 38),
        }
    }

    /// Resolves a token to a concrete [`Color32`].
    pub fn get(&self, token: ThemeToken) -> Color32 {
        match token {
            ThemeToken::Background => self.background,
            ThemeToken::Surface => self.surface,
            ThemeToken::SurfaceHover => self.surface_hover,
            ThemeToken::SurfaceSelected => self.surface_selected,
            ThemeToken::Border => self.border,
            ThemeToken::TextPrimary => self.text_primary,
            ThemeToken::TextSecondary => self.text_secondary,
            ThemeToken::TextWeak => self.text_weak,
            ThemeToken::Accent => self.accent,
            ThemeToken::AccentConnected => self.accent_connected,
            ThemeToken::AccentWarning => self.accent_warning,
            ThemeToken::AccentError => self.accent_error,
        }
    }
}

/// Active theme instance.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub mode: ThemeMode,
    pub palette: ThemePalette,
}

impl Default for Theme {
    fn default() -> Self {
        Self::new(ThemeMode::Dark)
    }
}

impl Theme {
    /// Creates a theme for the given mode.
    pub fn new(mode: ThemeMode) -> Self {
        let palette = match mode {
            ThemeMode::Dark => ThemePalette::dark(),
            ThemeMode::Light => ThemePalette::light(),
        };
        Self { mode, palette }
    }

    /// Resolves a design token to a concrete [`Color32`].
    pub fn color(&self, token: ThemeToken) -> Color32 {
        self.palette.get(token)
    }

    /// Helper returning default border stroke for this theme.
    pub fn border_stroke(&self) -> Stroke {
        Stroke::new(1.0_f32, self.color(ThemeToken::Border))
    }
}
