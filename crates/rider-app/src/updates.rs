use eframe::egui;
use rider_core::update::{self, Release, Version};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const FIRST_CHECK: Duration = Duration::from_secs(20);
const EVERY: Duration = Duration::from_secs(3600);
const NO_AUTO_CHECK: &str = "NoUpdateCheck";

#[derive(Clone)]
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
    ctx: egui::Context,
    state: Arc<Mutex<State>>,
    auto_on: Arc<AtomicBool>,
    pub installed: bool,
    pub auto: bool,
}

impl Updater {
    pub fn new(ctx: &egui::Context) -> Self {
        if let Ok(entries) = std::fs::read_dir(download_dir()) {
            for e in entries.flatten() {
                let _ = std::fs::remove_file(e.path());
            }
        }
        let me = Self {
            ctx: ctx.clone(),
            state: Arc::new(Mutex::new(State::Idle)),
            auto_on: Arc::new(AtomicBool::new(!crate::sys::flag(NO_AUTO_CHECK))),
            installed: crate::sys::installed(),
            auto: !crate::sys::flag(NO_AUTO_CHECK),
        };
        if me.installed {
            let (state, ctx, auto_on) = (me.state.clone(), me.ctx.clone(), me.auto_on.clone());
            let _ = std::thread::Builder::new().name("update-check".into()).spawn(move || {
                std::thread::sleep(FIRST_CHECK);
                loop {
                    let idle = matches!(
                        state.lock().map(|s| s.clone()).unwrap_or(State::Idle),
                        State::Idle | State::UpToDate | State::Failed { .. }
                    );
                    if auto_on.load(Ordering::Relaxed) && idle {
                        run_check(&state, &ctx, false);
                    }
                    std::thread::sleep(EVERY);
                }
            });
        }
        me
    }

    pub fn state(&self) -> State {
        self.state.lock().map(|s| s.clone()).unwrap_or(State::Idle)
    }

    pub fn preview(&self, stage: &str) {
        let release = Release {
            version: Version::parse("9.9.0").unwrap_or_else(current),
            page: "https://example.com".into(),
            file_name: "rdr-setup-9.9.0.exe".into(),
            url: String::new(),
            size: 31_400_000,
            sha256: [0; 32],
        };
        let s = match stage {
            "downloading" => State::Downloading { release, done: 13_100_000 },
            "installing" => State::Installing(release),
            "failed" => State::Failed {
                error: "The download was interrupted (connection reset).".into(),
                release: Some(release),
            },
            _ => State::Available(release),
        };
        Self::set(&self.state, &self.ctx, s);
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
        self.auto_on.store(on, Ordering::Relaxed);
        if on { crate::sys::clear_flag(NO_AUTO_CHECK) } else { crate::sys::set_flag(NO_AUTO_CHECK) }
    }

    pub fn check(&mut self) {
        if self.busy() {
            return;
        }
        let (state, ctx) = (self.state.clone(), self.ctx.clone());
        std::thread::spawn(move || run_check(&state, &ctx, true));
    }

    pub fn install_offered(&mut self) {
        if let State::Available(r) = self.state() {
            self.install(r);
        }
    }

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
            let s = match launch(&file) {
                Ok(code) => fail(rider_core::trf!(
                    "The update did not finish (installer exit code {code}).",
                    "Yeniləmə başa çatmadı (quraşdırıcının çıxış kodu {code})."
                )),
                Err(e) => {
                    fail(rider_core::trf!("Cannot start the installer: {e:#}", "Quraşdırıcı başladıla bilmir: {e:#}"))
                }
            };
            let _ = std::fs::remove_file(&file);
            Self::set(&state, &ctx, s);
        });
    }
}

fn run_check(state: &Mutex<State>, ctx: &egui::Context, manual: bool) {
    Updater::set(state, ctx, State::Checking { manual });
    let s = match update::newer() {
        Ok(Some(r)) => {
            crate::tray::announce_update(&r.version.to_string());
            State::Available(r)
        }
        Ok(None) => State::UpToDate,
        Err(e) if manual => State::Failed { error: format!("{e:#}"), release: None },
        Err(_) => State::Idle,
    };
    Updater::set(state, ctx, s);
}

fn download_dir() -> std::path::PathBuf {
    std::env::temp_dir().join("RemoteDesktopRider").join("updates")
}

fn fetch(r: &Release, progress: &mut dyn FnMut(u64)) -> anyhow::Result<std::path::PathBuf> {
    let dir = download_dir();
    std::fs::create_dir_all(&dir)?;
    let file = dir.join(&r.file_name);
    let mut last = Instant::now();
    update::download(r, &file, &mut |done, total| {
        if done == total || last.elapsed() >= Duration::from_millis(100) {
            last = Instant::now();
            progress(done);
        }
    })?;
    Ok(file)
}

fn launch(file: &std::path::Path) -> anyhow::Result<i32> {
    let status = std::process::Command::new(file)
        .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/show=1"])
        .spawn()?
        .wait()?;
    Ok(status.code().unwrap_or(-1))
}

pub fn current() -> Version {
    Version::current()
}
