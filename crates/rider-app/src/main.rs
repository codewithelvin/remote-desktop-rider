//! Remote Desktop Rider desktop app.
//!
//! * `RemoteDesktopRider` - computers this device can control (no admin rights),
//!   with a tray icon; closing the window keeps it in the tray.
//! * `RemoteDesktopRider --tray` - the same, starting hidden (used at sign-in).
//! * `RemoteDesktopRider --manage` - this computer's remote-access settings;
//!   relaunches itself elevated (one UAC prompt).
//! * `RemoteDesktopRider --session-panel` - the chat window of a paired
//!   session, started by the host's service (see `panel`).
//!
//! Sessions run in `rider-client.exe` viewer windows and host changes go
//! through `rider-host.exe`, both installed next to this executable.
//!
//! macOS and Linux: the same app for controlling other computers (no tray,
//! no Manage yet); the `portable/` modules stand in for the Windows ones.

#![windows_subsystem = "windows"]

mod client;
#[cfg(windows)]
mod host;
#[cfg(windows)]
mod panel;
#[cfg_attr(not(windows), path = "portable/pins.rs")]
mod pins;
mod sys;
#[cfg_attr(not(windows), path = "portable/tray.rs")]
mod tray;
#[cfg_attr(not(windows), path = "portable/updates.rs")]
mod updates;
mod widgets;

use eframe::egui;

fn main() -> eframe::Result {
    let has = |flag: &str| std::env::args().any(|a| a == flag);
    #[cfg(windows)]
    if has("--session-panel") {
        return session_panel();
    }
    // Manage (this computer as a host) exists on Windows only.
    let manage = cfg!(windows) && has("--manage");
    if manage && !sys::is_elevated() && std::env::var_os("RIDER_PREVIEW").is_none() {
        // Started without rights (e.g. from a shortcut): ask once and hand over.
        let _ = sys::relaunch_elevated("--manage");
        return Ok(());
    }
    // RIDER_PREVIEW: a test copy next to an installed one (screenshots).
    if !manage && std::env::var_os("RIDER_PREVIEW").is_none() && !sys::first_instance() {
        // Already running (maybe only in the tray): show that one instead.
        tray::activate_other_instance();
        return Ok(());
    }
    if !manage {
        sys::default_autostart();
    }
    let hidden = !manage && has("--tray");
    #[cfg(windows)]
    let (title, size) =
        if manage { (host::window_title(), [700.0, 700.0]) } else { (tray::MAIN_TITLE, [860.0, 580.0]) };
    #[cfg(not(windows))]
    let (title, size) = (tray::MAIN_TITLE, [860.0, 580.0]);
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(title)
        .with_icon(egui::IconData { rgba: rider_core::icon::rgba(64), width: 64, height: 64 })
        .with_inner_size(size)
        .with_min_inner_size([620.0, 440.0])
        // Linux: matches the .desktop file, so docks show the app's icon and name.
        .with_app_id("remote-desktop-rider");
    if hidden {
        // eframe shows the window after its first frame: keep that off-screen.
        viewport = viewport.with_position([-32000.0, -32000.0]);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(
        title,
        options,
        Box::new(move |cc| {
            widgets::setup(&cc.egui_ctx);
            widgets::theme_override(&cc.egui_ctx);
            #[cfg(windows)]
            if manage {
                return Ok(Box::new(host::HostApp::new(&cc.egui_ctx)) as Box<dyn eframe::App>);
            }
            // The tray window must live on this (the event loop's) thread.
            let tray = tray::Tray::create().ok();
            Ok(Box::new(client::ClientApp::new(&cc.egui_ctx, tray, hidden)) as Box<dyn eframe::App>)
        }),
    )
}

/// The chat window of a paired session: its own small window, apart from
/// the app (which may be running too), never in the tray.
#[cfg(windows)]
fn session_panel() -> eframe::Result {
    let viewport = egui::ViewportBuilder::default()
        .with_title(panel::TITLE)
        .with_icon(egui::IconData { rgba: rider_core::icon::rgba(64), width: 64, height: 64 })
        .with_inner_size([380.0, 520.0])
        .with_min_inner_size([300.0, 320.0])
        // Never takes the keyboard from what the person is doing; hidden
        // until the panel shows it minimized (see `panel`).
        .with_active(false)
        .with_visible(false);
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(
        panel::TITLE,
        options,
        Box::new(|cc| {
            widgets::setup(&cc.egui_ctx);
            widgets::theme_override(&cc.egui_ctx);
            Ok(Box::new(panel::Panel::new(cc)) as Box<dyn eframe::App>)
        }),
    )
}
