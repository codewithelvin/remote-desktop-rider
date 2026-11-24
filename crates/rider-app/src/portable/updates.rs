//! macOS and Linux: no self-update yet; Settings links to the releases page
//! instead. Same interface as the Windows updater, which never leaves Idle here.

use eframe::egui;

#[derive(Clone)]
pub struct Release {
    pub version: String,
    pub page: String,
    pub size: u64,
}

#[derive(Clone)]
#[allow(dead_code)] // the variants the shared UI matches on
pub enum State {
    Idle,
    Checking { manual: bool },
    UpToDate,
    Available(Release),
    Downloading { release: Release, done: u64 },
    Installing(Release),
    Failed { error: String, release: Option<Release> },
}

pub struct Updater {
    pub installed: bool,
    pub auto: bool,
}

impl Updater {
    pub fn new(_ctx: &egui::Context) -> Self {
        Self { installed: false, auto: false }
    }

    pub fn state(&self) -> State {
        State::Idle
    }

    pub fn set_auto(&mut self, on: bool) {
        self.auto = on;
    }

    pub fn check(&mut self) {}

    pub fn install(&mut self, _release: Release) {}

    pub fn install_offered(&mut self) {}

    /// `RIDER_PREVIEW=update-*`: nothing to show (no self-update here).
    pub fn preview(&self, _stage: &str) {}
}

pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
