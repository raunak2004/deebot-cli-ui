//! The Deebot app. The desktop binary and the Android library both start from [`run`].

mod app;
mod backend;
mod mapview;
mod platform;
mod theme;

use eframe::egui;

/// Open the window and run until it closes.
pub fn run(options: eframe::NativeOptions) -> eframe::Result {
    eframe::run_native("Deebot", options, Box::new(|cc| Ok(Box::new(app::DeebotApp::new(cc)))))
}

/// Window settings for a computer.
pub fn desktop_options() -> eframe::NativeOptions {
    let mut viewport = egui::ViewportBuilder::default().with_inner_size([720.0, 640.0]).with_title("Deebot");
    // The icon is built into the program; if it ever fails to load, the window just has no icon
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        viewport = viewport.with_icon(icon);
    }
    eframe::NativeOptions { viewport, ..Default::default() }
}

/// Android starts the app here instead of `main`.
#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(android_app: android_activity::AndroidApp) {
    // Android has no home directory, so keep the saved session in the app's private storage
    if let Some(dir) = android_app.internal_data_path() {
        deebot_core::session::set_data_dir(dir);
    }
    let _ = platform::ANDROID_APP.set(android_app.clone());
    let options = eframe::NativeOptions { android_app: Some(android_app), ..Default::default() };
    // Without its window the app has nothing to do, so a failure here simply ends it
    let _ = run(options);
}
