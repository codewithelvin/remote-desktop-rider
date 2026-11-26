use anyhow::{Result, bail};

pub const MAIN_TITLE: &str = "Remote Desktop Rider";

#[derive(Debug)]
pub struct Tray;

#[expect(clippy::unused_self, reason = "same interface as the Windows tray")]
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
