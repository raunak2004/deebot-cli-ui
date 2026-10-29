//! Things that differ between a computer and an Android phone.

use eframe::egui;

/// The running Android activity, kept so the UI can ask how much of the screen the system bars cover.
#[cfg(target_os = "android")]
pub static ANDROID_APP: std::sync::OnceLock<android_activity::AndroidApp> = std::sync::OnceLock::new();

/// Space to keep clear at the top and bottom of the screen, in points: (status bar, gesture bar).
/// A computer window has none.
pub fn safe_insets(ctx: &egui::Context) -> (f32, f32) {
    #[cfg(target_os = "android")]
    {
        // Used when the system doesn't say how big its bars are
        const DEFAULT_TOP: f32 = 44.0;
        const MAX_TOP: f32 = 56.0;
        const DEFAULT_BOTTOM: f32 = 18.0;
        const MAX_BOTTOM: f32 = 40.0;

        let ppp = ctx.pixels_per_point().max(0.1);
        let (mut top, mut bottom) = (0.0, 0.0);
        if let Some(app) = ANDROID_APP.get() {
            let visible = app.content_rect();
            top = visible.top.max(0) as f32 / ppp;
            // Anything below the visible area is covered by the gesture bar or the keyboard
            bottom = (ctx.viewport_rect().height() - visible.bottom as f32 / ppp).max(0.0);
        }
        // Whatever the system reports, a status bar plus camera cutout never needs more than this
        let top = if top >= 1.0 { top.clamp(DEFAULT_TOP, MAX_TOP) } else { DEFAULT_TOP };
        let bottom = if bottom >= 1.0 { bottom.min(MAX_BOTTOM) } else { DEFAULT_BOTTOM };
        (top, bottom)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = ctx;
        (0.0, 0.0)
    }
}
