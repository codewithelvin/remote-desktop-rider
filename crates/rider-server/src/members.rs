use anyhow::{Context, Result, bail};
use rider_core::identity::{Fingerprint, fp_from_hex, fp_hex};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const INVITE_LIFETIME: Duration = Duration::from_secs(600);
const REREAD: Duration = Duration::from_secs(1);

pub struct Members {
    dir: PathBuf,
    pub invite_only: bool,
    cache: Mutex<Option<(Instant, HashMap<Fingerprint, u64>)>>,
    write: Mutex<()>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))
}

impl Members {
    pub fn new(dir: &Path, invite_only: bool) -> Self {
        Self { dir: dir.to_owned(), invite_only, cache: Mutex::default(), write: Mutex::default() }
    }

    fn members_path(&self) -> PathBuf {
        self.dir.join("members")
    }

    fn invites_path(&self) -> PathBuf {
        self.dir.join("invites")
    }

    pub fn list(&self) -> HashMap<Fingerprint, u64> {
        let mut cache = self.cache.lock().unwrap();
        if let Some((read, list)) = cache.as_ref()
            && read.elapsed() < REREAD
        {
            return list.clone();
        }
        let list = self.read();
        *cache = Some((Instant::now(), list.clone()));
        list
    }

    fn read(&self) -> HashMap<Fingerprint, u64> {
        std::fs::read_to_string(self.members_path())
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let mut it = l.split_whitespace();
                Some((fp_from_hex(it.next()?).ok()?, it.next().and_then(|t| t.parse().ok()).unwrap_or(0)))
            })
            .collect()
    }

    pub fn contains(&self, fp: &Fingerprint) -> bool {
        self.list().contains_key(fp)
    }

    pub fn admitted(&self, fp: &Fingerprint) -> bool {
        !self.invite_only || self.contains(fp)
    }

    fn save(&self, list: &HashMap<Fingerprint, u64>) -> Result<()> {
        let mut lines: Vec<_> = list.iter().map(|(fp, t)| format!("{} {t}\n", fp_hex(fp))).collect();
        lines.sort();
        write_atomic(&self.members_path(), &lines.concat())?;
        *self.cache.lock().unwrap() = Some((Instant::now(), list.clone()));
        Ok(())
    }

    pub fn add(&self, fp: Fingerprint) -> Result<()> {
        let _w = self.write.lock().unwrap();
        let mut list = self.read();
        if list.contains_key(&fp) {
            return Ok(());
        }
        list.insert(fp, now());
        self.save(&list)
    }

    pub fn remove(&self, prefix: &str) -> Result<usize> {
        let prefix: String = prefix.chars().filter(|c| *c != '-').collect::<String>().to_ascii_lowercase();
        if prefix.len() < 8 {
            bail!("give at least the first 8 characters of the fingerprint");
        }
        let _w = self.write.lock().unwrap();
        let mut list = self.read();
        let before = list.len();
        list.retain(|fp, _| !fp_hex(fp).starts_with(&prefix));
        let removed = before - list.len();
        if removed > 0 {
            self.save(&list)?;
        }
        Ok(removed)
    }

    fn invites(&self) -> HashMap<String, u64> {
        let now = now();
        std::fs::read_to_string(self.invites_path())
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let (code, until) = l.split_once(' ')?;
                Some((code.to_owned(), until.trim().parse().ok()?))
            })
            .filter(|(_, until)| *until > now)
            .collect()
    }

    fn save_invites(&self, invites: &HashMap<String, u64>) -> Result<()> {
        let text = invites.iter().fold(String::new(), |mut s, (c, t)| {
            let _ = writeln!(s, "{c} {t}");
            s
        });
        write_atomic(&self.invites_path(), &text)
    }

    pub fn is_invite(&self, code: &str) -> bool {
        self.invites().contains_key(code)
    }

    pub fn create_invite(&self) -> Result<String> {
        let _w = self.write.lock().unwrap();
        let mut invites = self.invites();
        let code = loop {
            let c = crate::new_code().context("no randomness available")?;
            if !invites.contains_key(&c) {
                break c;
            }
        };
        invites.insert(code.clone(), now() + INVITE_LIFETIME.as_secs());
        self.save_invites(&invites)?;
        Ok(code)
    }

    pub fn join(&self, fp: Fingerprint, code: &str) -> Result<()> {
        {
            let _w = self.write.lock().unwrap();
            let mut invites = self.invites();
            if invites.remove(code).is_none() {
                bail!(rider_core::rendezvous::refusal::BAD_INVITE);
            }
            self.save_invites(&invites)?;
        }
        self.add(fp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rider-members-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn invite_admits_once() {
        let d = dir("invite");
        let m = Members::new(&d, true);
        let (a, b) = ([1u8; 32], [2u8; 32]);
        assert!(!m.admitted(&a));
        let code = m.create_invite().unwrap();
        let server = Members::new(&d, true);
        assert!(server.is_invite(&code), "told apart from a pairing code");
        server.join(a, &code).unwrap();
        assert!(!server.is_invite(&code));
        assert!(server.admitted(&a));
        assert!(server.join(b, &code).is_err(), "one use only");
        assert!(!server.admitted(&b));
        assert!(server.join(b, "XXX-XXX").is_err());
        assert_eq!(m.remove(&fp_hex(&a)[..12]).unwrap(), 1);
        std::thread::sleep(REREAD + Duration::from_millis(100));
        assert!(!server.admitted(&a));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn open_server_admits_everyone() {
        let d = dir("open");
        let m = Members::new(&d, false);
        assert!(m.admitted(&[9u8; 32]));
        assert!(!m.contains(&[9u8; 32]));
        m.add([9u8; 32]).unwrap();
        assert!(m.contains(&[9u8; 32]));
        let _ = std::fs::remove_dir_all(&d);
    }
}
