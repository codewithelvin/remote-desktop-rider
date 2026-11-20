//! No tray (menu-bar) icon on macOS and Linux yet: closing the window quits
//! the app, as most apps there do. Same interface as the Windows tray.

use anyhow::{Result, bail};

/// Title of the main window.
pub const MAIN_TITLE: &str = "Remote Desktop Rider";

#[derive(Debug)]
pub struct Tray;

impl Tray {
    pub fn create() -> Result<Self> {
        bail!("no tray icon on this system")
    }

    pub fn notify(&self, _title: &str, _text: &str) {}
}

pub fn take_install_request() -> bool {
    false
}

pub fn activate_other_instance() -> bool {
    false
}

pub fn hide_main() {}

pub fn hide_main_centered() {}
