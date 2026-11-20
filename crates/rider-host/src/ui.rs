//! Machine-readable mode (`--json`) used by the desktop app, which runs
//! `rider-host` elevated as a child process: events go to stdout as one JSON
//! object per line; answers come back as lines on stdin (`y`, `n`, `quit`).
//! Closing stdin counts as `quit`, so a crashed app never leaves the host
//! in pairing mode or the service paused.

use std::sync::{Arc, OnceLock};
use tokio::sync::{Mutex, Notify, mpsc};

static JSON: OnceLock<()> = OnceLock::new();
static ANSWERS: OnceLock<Mutex<mpsc::UnboundedReceiver<bool>>> = OnceLock::new();

pub fn enable() {
    let _ = JSON.set(());
}

pub fn json() -> bool {
    JSON.get().is_some()
}

/// Print one event line (only in JSON mode).
pub fn emit(event: serde_json::Value) {
    if json() {
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{event}");
        let _ = out.flush();
    }
}

/// Start reading stdin. The returned notify fires on `quit` or end of input.
pub fn start_input() -> Arc<Notify> {
    let quit = Arc::new(Notify::new());
    let (tx, rx) = mpsc::unbounded_channel();
    let _ = ANSWERS.set(Mutex::new(rx));
    let q = quit.clone();
    std::thread::Builder::new()
        .name("ui-stdin".into())
        .spawn(move || {
            let mut line = String::new();
            loop {
                line.clear();
                match std::io::stdin().read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    // Some callers (.NET) start the stream with a UTF-8 byte order mark.
                    Ok(_) => match line.trim().trim_start_matches('\u{feff}') {
                        "y" | "yes" => _ = tx.send(true),
                        "n" | "no" => _ = tx.send(false),
                        "quit" => break,
                        _ => {}
                    },
                }
            }
            q.notify_one();
        })
        .expect("spawn stdin thread");
    quit
}

/// Wait for the operator's yes/no answer (false if input closed).
pub async fn answer() -> bool {
    match ANSWERS.get() {
        Some(rx) => rx.lock().await.recv().await.unwrap_or(false),
        None => false,
    }
}
