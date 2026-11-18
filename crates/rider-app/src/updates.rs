//! "Update available": checks GitHub releases in the background (at start and
//! every few hours, unless turned off), downloads the installer with progress,
//! verifies it and runs it silently. The installer asks for elevation itself,
//! closes this app, installs, and reopens it with its window (`/show=1`).
//!
//! Only the installed copy updates itself: a build run from elsewhere (e.g.
//! `target\`) would be replaced by nothing.

use eframe::egui;
use rider_core::update::{self, Release, Version};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::core::w;

/// First check shortly after start, then this often.
const FIRST_CHECK: Duration = Duration::from_secs(20);
const EVERY: Duration = Duration::from_secs(6 * 3600);
/// Registry flag (HKCU) set when automatic checks are turned off.
const NO_AUTO_CHECK: windows::core::PCWSTR = w!("NoUpdateCheck");

#[derive(Clone)]
pub enum State {
    Idle,
    Checking {
        manual: bool,
    },
    UpToDate,
    Available(Release),
    Downloading {
        release: Release,
        done: u64,
    },
    /// The installer is running (it will close this app).
    Installing(Release),
    Failed {
        error: String,
        release: Option<Release>,
    },
}

pub struct Updater {
    ctx: egui::Context,
    state: Arc<Mutex<State>>,
    started: Instant,
    last_check: Option<Instant>,
    pub installed: bool,
    pub auto: bool,
}

impl Updater {
    pub fn new(ctx: &egui::Context) -> Self {
        Self {
            ctx: ctx.clone(),
            state: Arc::new(Mutex::new(State::Idle)),
            started: Instant::now(),
            last_check: None,
            installed: crate::sys::installed(),
            auto: !crate::sys::flag(NO_AUTO_CHECK),
        }
    }

    pub fn state(&self) -> State {
        self.state.lock().map(|s| s.clone()).unwrap_or(State::Idle)
    }

    fn set(state: &Mutex<State>, ctx: &egui::Context, s: State) {
        if let Ok(mut g) = state.lock() {
            *g = s;
        }
        ctx.request_repaint();
    }

    fn busy(&self) -> bool {
        matches!(self.state(), State::Checking { .. } | State::Downloading { .. } | State::Installing(_))
    }

    pub fn set_auto(&mut self, on: bool) {
        self.auto = on;
        if on { crate::sys::clear_flag(NO_AUTO_CHECK) } else { crate::sys::set_flag(NO_AUTO_CHECK) }
    }

    /// Call every frame: starts the periodic check when due.
    pub fn tick(&mut self) {
        if !self.installed || !self.auto || self.busy() {
            return;
        }
        let due = match self.last_check {
            None => self.started.elapsed() >= FIRST_CHECK,
            Some(t) => t.elapsed() >= EVERY,
        };
        // A found update stays offered; checking again would only repeat it.
        if due && !matches!(self.state(), State::Available(_)) {
            self.check(false);
        }
    }

    pub fn check(&mut self, manual: bool) {
        if self.busy() {
            return;
        }
        self.last_check = Some(Instant::now());
        let (state, ctx) = (self.state.clone(), self.ctx.clone());
        Self::set(&state, &ctx, State::Checking { manual });
        std::thread::spawn(move || {
            let s = match update::newer() {
                Ok(Some(r)) => State::Available(r),
                Ok(None) => State::UpToDate,
                // Background checks fail quietly (offline, proxy...); a manual one says why.
                Err(e) if manual => State::Failed { error: format!("{e:#}"), release: None },
                Err(_) => State::Idle,
            };
            Self::set(&state, &ctx, s);
        });
    }

    /// Download, verify and start the installer of `release`.
    pub fn install(&mut self, release: Release) {
        if self.busy() || !self.installed {
            return;
        }
        let (state, ctx) = (self.state.clone(), self.ctx.clone());
        Self::set(&state, &ctx, State::Downloading { release: release.clone(), done: 0 });
        std::thread::spawn(move || {
            let fail = |error: String| State::Failed { error, release: Some(release.clone()) };
            let file = match fetch(&release, &mut |done| {
                Self::set(&state, &ctx, State::Downloading { release: release.clone(), done });
            }) {
                Ok(f) => f,
                Err(e) => return Self::set(&state, &ctx, fail(format!("{e:#}"))),
            };
            Self::set(&state, &ctx, State::Installing(release.clone()));
            // On success the installer closes this app before it exits, so
            // getting an answer here means it stopped early (e.g. the UAC
            // prompt was declined).
            let s = match launch(&file) {
                Ok(code) => fail(format!("The update did not finish (installer exit code {code}).")),
                Err(e) => fail(format!("Cannot start the installer: {e:#}")),
            };
            let _ = std::fs::remove_file(&file);
            Self::set(&state, &ctx, s);
        });
    }
}

/// Download and verify the installer into the user's temp folder.
fn fetch(r: &Release, progress: &mut dyn FnMut(u64)) -> anyhow::Result<std::path::PathBuf> {
    let dir = std::env::temp_dir().join("RemoteDesktopRider");
    std::fs::create_dir_all(&dir)?;
    let file = dir.join(&r.file_name);
    let mut last = Instant::now();
    update::download(r, &file, &mut |done, total| {
        // About ten updates a second is plenty for a progress bar.
        if done == total || last.elapsed() >= Duration::from_millis(100) {
            last = Instant::now();
            progress(done);
        }
    })?;
    Ok(file)
}

/// Run the installer silently; it elevates itself (one UAC prompt).
fn launch(file: &std::path::Path) -> anyhow::Result<i32> {
    let status = std::process::Command::new(file)
        .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/show=1"])
        .spawn()?
        .wait()?;
    Ok(status.code().unwrap_or(-1))
}

/// The version this app reports.
pub fn current() -> Version {
    Version::current()
}
