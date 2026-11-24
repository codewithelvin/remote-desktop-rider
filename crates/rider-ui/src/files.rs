//! The chat's file actions: pick files to send, keep a received file
//! ("Save as…"), open one. Dialogs block until closed; callers run them on
//! a thread of their own so the session keeps drawing.

use rider_core::i18n::tr;
use std::path::{Path, PathBuf};

/// Files to send (none if the dialog was cancelled).
pub fn pick_files() -> Vec<PathBuf> {
    rfd::FileDialog::new().set_title(tr("Send files", "Fayl göndər")).pick_files().unwrap_or_default()
}

/// Ask where to keep `from` (offered as `name`, in Downloads) and copy it
/// there. The copy keeps the "downloaded from the internet" mark.
pub fn save_copy(from: &Path, name: &str) -> std::io::Result<Option<PathBuf>> {
    let mut dialog = rfd::FileDialog::new().set_title(tr("Save as", "Yadda saxla")).set_file_name(name);
    if let Some(dir) = downloads() {
        dialog = dialog.set_directory(dir);
    }
    let Some(to) = dialog.save_file() else { return Ok(None) };
    std::fs::copy(from, &to)?;
    Ok(Some(to))
}

/// Open with the program the system uses for it (Windows checks the
/// internet mark first, as for any download).
pub fn open(path: &Path) -> std::io::Result<()> {
    open::that_detached(path)
}

fn downloads() -> Option<PathBuf> {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })?;
    let d = PathBuf::from(home).join("Downloads");
    d.is_dir().then_some(d)
}
