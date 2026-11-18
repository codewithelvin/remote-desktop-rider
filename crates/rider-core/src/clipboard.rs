//! Two-way text clipboard sync. A background thread polls the local
//! clipboard and applies remote updates. It remembers the last value either
//! way, so a remote update is never echoed back.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

const MAX_TEXT: usize = 1024 * 1024;
const POLL: Duration = Duration::from_millis(300);

/// `local_changes` receives text copied on this machine; `remote_updates`
/// delivers text copied on the peer, to be placed on this machine's clipboard.
pub fn spawn(local_changes: UnboundedSender<String>, remote_updates: Receiver<String>, stop: Arc<AtomicBool>) {
    let _ = std::thread::Builder::new().name("rider-clipboard".into()).spawn(move || {
        let mut cb = match arboard::Clipboard::new() {
            Ok(cb) => cb,
            Err(e) => {
                tracing::warn!("clipboard unavailable: {e}");
                return;
            }
        };
        // Do not push whatever happened to be on the clipboard at connect time.
        let mut last = cb.get_text().unwrap_or_default();
        while !stop.load(Ordering::Relaxed) {
            while let Ok(text) = remote_updates.try_recv() {
                if text != last && cb.set_text(text.clone()).is_ok() {
                    last = text;
                }
            }
            if let Ok(text) = cb.get_text()
                && text != last
            {
                last = text.clone();
                if text.len() <= MAX_TEXT && local_changes.send(text).is_err() {
                    return;
                }
            }
            std::thread::sleep(POLL);
        }
    });
}
