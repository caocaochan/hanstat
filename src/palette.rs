//! Colours of text and marks that read on both themes. The dark-theme
//! colours are the bright ones hanstat always used; on the light theme each
//! has a darker counterpart with a contrast of at least 4.5:1 (WCAG AA for
//! text) against egui's light panels (248, 248, 248) and white. `App::ui`
//! sets the theme once per frame with [`set_dark`].

use eframe::egui::Color32;
use std::sync::atomic::{AtomicBool, Ordering};

static DARK: AtomicBool = AtomicBool::new(true);

/// Whether the frame being drawn uses the dark theme.
pub(crate) fn set_dark(dark: bool) {
    DARK.store(dark, Ordering::Relaxed);
}

/// `dark` on the dark theme, `light` on the light one.
pub(crate) fn pick(dark: Color32, light: Color32) -> Color32 {
    if DARK.load(Ordering::Relaxed) {
        dark
    } else {
        light
    }
}

/// Warnings and hints that need attention (amber).
pub(crate) fn warn() -> Color32 {
    pick(
        Color32::from_rgb(220, 170, 60),
        Color32::from_rgb(160, 95, 0),
    )
}

/// Errors (red).
pub(crate) fn error() -> Color32 {
    pick(
        Color32::from_rgb(230, 100, 100),
        Color32::from_rgb(195, 40, 40),
    )
}

/// Success: a done update, a recommended word, a book read (green).
pub(crate) fn good() -> Color32 {
    pick(
        Color32::from_rgb(90, 175, 100),
        Color32::from_rgb(28, 125, 55),
    )
}

/// Neutral marks in the Library's charts (blue).
pub(crate) fn accent() -> Color32 {
    pick(
        Color32::from_rgb(90, 150, 220),
        Color32::from_rgb(35, 95, 190),
    )
}
