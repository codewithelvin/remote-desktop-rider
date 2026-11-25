use anyhow::{Context, Result, bail};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

const LOCK: &str = ".lock";

pub struct SessionFiles {
    dir: PathBuf,
    lock: Option<File>,
}

pub fn sessions_root(base: &Path) -> PathBuf {
    base.join("RemoteDesktopRider").join("sessions")
}

impl SessionFiles {
    pub fn create_in(base: &Path) -> Result<Self> {
        let root = sessions_root(base);
        clean_stale(&root);
        std::fs::create_dir_all(&root).with_context(|| format!("creating {}", root.display()))?;
        let mut id = [0u8; 8];
        getrandom::fill(&mut id).map_err(|e| anyhow::anyhow!("random: {e}"))?;
        let dir = root.join(hex::encode(id));
        std::fs::create_dir(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let lock = OpenOptions::new().write(true).create_new(true).open(dir.join(LOCK))?;
        lock.try_lock().map_err(|e| anyhow::anyhow!("locking the session folder: {e}"))?;
        Ok(Self { dir, lock: Some(lock) })
    }

    pub fn create() -> Result<Self> {
        Self::create_in(&std::env::temp_dir())
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn incoming(&self, name: &str) -> Result<(PathBuf, File)> {
        let (stem, ext) = match name.rfind('.') {
            Some(i) if i > 0 => (&name[..i], &name[i..]),
            _ => (name, ""),
        };
        for i in 1..1000 {
            let path = if i == 1 { self.dir.join(name) } else { self.dir.join(format!("{stem} ({i}){ext}")) };
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(f) => {
                    mark_from_internet(&path);
                    return Ok((path, f));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e).with_context(|| format!("creating {}", path.display())),
            }
        }
        bail!("too many files named {name}")
    }
}

impl Drop for SessionFiles {
    fn drop(&mut self) {
        drop(self.lock.take());
        if let Err(e) = std::fs::remove_dir_all(&self.dir) {
            tracing::debug!("session files not removed yet: {e}");
        }
    }
}

pub fn clean_stale(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for e in entries.flatten() {
        let dir = e.path();
        if !dir.is_dir() {
            continue;
        }
        let free = match OpenOptions::new().write(true).open(dir.join(LOCK)) {
            Ok(f) => f.try_lock().is_ok(),
            Err(e) => e.kind() == std::io::ErrorKind::NotFound,
        };
        if free {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

fn mark_from_internet(path: &Path) {
    if cfg!(windows) {
        let mut stream = path.as_os_str().to_owned();
        stream.push(":Zone.Identifier");
        let _ = std::fs::write(PathBuf::from(stream), "[ZoneTransfer]\r\nZoneId=3\r\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn base(name: &str) -> PathBuf {
        let b = std::env::temp_dir().join(format!("rider-session-files-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&b);
        b
    }

    #[test]
    fn files_go_away_with_the_session() {
        let b = base("drop");
        let s = SessionFiles::create_in(&b).unwrap();
        let (p1, mut f) = s.incoming("report.pdf").unwrap();
        f.write_all(b"x").unwrap();
        drop(f);
        let (p2, _) = s.incoming("report.pdf").unwrap();
        assert_eq!(p1.file_name().unwrap(), "report.pdf");
        assert_eq!(p2.file_name().unwrap(), "report (2).pdf");
        if cfg!(windows) {
            let mut z = p1.as_os_str().to_owned();
            z.push(":Zone.Identifier");
            assert!(std::fs::read_to_string(PathBuf::from(z)).unwrap().contains("ZoneId=3"));
        }
        let dir = s.dir().to_owned();
        drop(s);
        assert!(!dir.exists(), "deleted when the session ends");
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn leftovers_are_cleaned_but_live_sessions_kept() {
        let b = base("stale");
        let live = SessionFiles::create_in(&b).unwrap();
        let crashed = sessions_root(&b).join("deadbeef");
        std::fs::create_dir_all(&crashed).unwrap();
        std::fs::write(crashed.join(LOCK), "").unwrap();
        std::fs::write(crashed.join("secret.txt"), "left behind").unwrap();
        let next = SessionFiles::create_in(&b).unwrap();
        assert!(!crashed.exists(), "a dead session's files are removed");
        assert!(live.dir().exists(), "a running session's folder is kept");
        drop(next);
        drop(live);
        let _ = std::fs::remove_dir_all(&b);
    }
}
