//! Remote Desktop Rider desktop app.
//!
//! * `RemoteDesktopRider` - computers this device can control (no admin rights),
//!   with a tray icon; closing the window keeps it in the tray.
//! * `RemoteDesktopRider --tray` - the same, starting hidden (used at sign-in).
//! * `RemoteDesktopRider --manage` - this computer's remote-access settings;
//!   relaunches itself elevated (one UAC prompt).
//!
//! Sessions run in `rider-client.exe` viewer windows and host changes go
//! through `rider-host.exe`, both installed next to this executable.

#![windows_subsystem = "windows"]

mod client;
mod host;
mod pins;
mod sys;
mod tray;
mod updates;
mod widgets;

use eframe::egui;

fn main() -> eframe::Result {
    let has = |flag: &str| std::env::args().any(|a| a == flag);
    let manage = has("--manage");
    if manage && !sys::is_elevated() {
        // Started without rights (e.g. from a shortcut): ask once and hand over.
        let _ = sys::relaunch_elevated("--manage");
        return Ok(());
    }
    if !manage && !sys::first_instance() {
        // Already running (maybe only in the tray): show that one instead.
        tray::activate_other_instance();
        return Ok(());
    }
    if !manage {
        sys::default_autostart();
    }
    let hidden = !manage && has("--tray");
    let (title, size) = if manage {
        ("Remote Desktop Rider: this computer (administrator)", [680.0, 680.0])
    } else {
        (tray::MAIN_TITLE, [860.0, 580.0])
    };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(title)
        .with_icon(egui::IconData { rgba: rider_core::icon::rgba(64), width: 64, height: 64 })
        .with_inner_size(size)
        .with_min_inner_size([620.0, 440.0]);
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
            Ok(if manage {
                Box::new(host::HostApp::new(&cc.egui_ctx)) as Box<dyn eframe::App>
            } else {
                // The tray window must live on this (the event loop's) thread.
                let tray = tray::Tray::create().ok();
                Box::new(client::ClientApp::new(&cc.egui_ctx, tray, hidden))
            })
        }),
    )
}
