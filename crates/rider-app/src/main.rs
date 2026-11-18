//! Remote Desktop Rider desktop app.
//!
//! * `RemoteDesktopRider` - computers this device can control (no admin rights).
//! * `RemoteDesktopRider --manage` - this computer's remote-access settings;
//!   relaunches itself elevated (one UAC prompt).
//!
//! Sessions run in `rider-client.exe` viewer windows and host changes go
//! through `rider-host.exe`, both installed next to this executable.

#![windows_subsystem = "windows"]

mod client;
mod host;
mod sys;
mod widgets;

use eframe::egui;

fn main() -> eframe::Result {
    let manage = std::env::args().any(|a| a == "--manage");
    if manage && !sys::is_elevated() {
        // Started without rights (e.g. from a shortcut): ask once and hand over.
        let _ = sys::relaunch_elevated("--manage");
        return Ok(());
    }
    let (title, size) = if manage {
        ("Remote Desktop Rider: this computer (administrator)", [640.0, 680.0])
    } else {
        ("Remote Desktop Rider", [720.0, 500.0])
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size(size)
            .with_min_inner_size([520.0, 380.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Remote Desktop Rider",
        options,
        Box::new(move |cc| {
            widgets::style(&cc.egui_ctx);
            Ok(if manage {
                Box::new(host::HostApp::new(&cc.egui_ctx)) as Box<dyn eframe::App>
            } else {
                Box::new(client::ClientApp::new(&cc.egui_ctx))
            })
        }),
    )
}
