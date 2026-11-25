use anyhow::{Context, Result, bail};
use rider_core::company::{Computer, DirEdit, Directory, Floor, HostInfo, MAX_NAME, Place, Room, Whereabouts};
use rider_core::identity::{Fingerprint, fp_from_hex, fp_hex};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const SETUP_LIFETIME_SECS: u64 = 24 * 3600;
pub const MAX_COMPUTERS: usize = 5000;
const MAX_FLOORS: usize = 200;
const MAX_ROOMS_PER_FLOOR: usize = 200;

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[derive(Serialize, Deserialize, Default)]
struct Stored {
    name: String,
    admin: Option<String>,
    join_secret: String,
    next_id: u32,
    floors: Vec<Floor>,
    computers: Vec<StoredComputer>,
}

#[derive(Serialize, Deserialize, Clone)]
struct StoredComputer {
    fingerprint: String,
    info: HostInfo,
    room: Option<u32>,
    unattended_on: bool,
    last_seen: u64,
    joined: u64,
}

pub struct Company {
    dir: PathBuf,
    state: Mutex<Option<Stored>>,
    dirty: std::sync::atomic::AtomicBool,
    taken: std::sync::atomic::AtomicU64,
    written: Mutex<u64>,
    places: Mutex<HashMap<Fingerprint, Whereabouts>>,
}

fn random_hex(bytes: usize) -> Result<String> {
    let mut b = vec![0u8; bytes];
    getrandom::fill(&mut b).map_err(|e| anyhow::anyhow!("random: {e}"))?;
    Ok(hex::encode(b))
}

fn new_setup_code() -> Result<String> {
    let alphabet = rider_core::rendezvous::CODE_ALPHABET;
    let mut out = String::new();
    let mut byte = [0u8; 1];
    let mut n = 0;
    while n < 12 {
        getrandom::fill(&mut byte).map_err(|e| anyhow::anyhow!("random: {e}"))?;
        let limit = 256 - 256 % alphabet.len();
        if (byte[0] as usize) < limit {
            if n > 0 && n % 4 == 0 {
                out.push('-');
            }
            out.push(alphabet[byte[0] as usize % alphabet.len()] as char);
            n += 1;
        }
    }
    Ok(out)
}

fn clean_name(name: &str) -> Result<String> {
    let name: String = name.trim().chars().filter(|c| !c.is_control()).take(MAX_NAME).collect();
    if name.is_empty() {
        bail!("the name is empty");
    }
    Ok(name)
}

fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))
}

impl Company {
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join("company.json");
        let state = match std::fs::read_to_string(&path) {
            Ok(text) => Some(serde_json::from_str(&text).with_context(|| format!("reading {}", path.display()))?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        Ok(Self::with(dir.to_owned(), state))
    }

    #[cfg(test)]
    pub fn none() -> Self {
        Self::with(std::env::temp_dir().join("rider-no-company"), None)
    }

    fn with(dir: PathBuf, state: Option<Stored>) -> Self {
        Self {
            dir,
            state: Mutex::new(state),
            dirty: std::sync::atomic::AtomicBool::new(false),
            taken: std::sync::atomic::AtomicU64::new(0),
            written: Mutex::new(0),
            places: Mutex::new(HashMap::new()),
        }
    }

    pub fn init(dir: &Path, name: &str) -> Result<String> {
        let path = dir.join("company.json");
        if path.exists() {
            bail!("{} exists already: this server has a company", path.display());
        }
        let stored = Stored { name: clean_name(name)?, join_secret: random_hex(16)?, next_id: 1, ..Default::default() };
        write_atomic(&path, &serde_json::to_string_pretty(&stored)?)?;
        Self::new_setup_code(dir)
    }

    pub fn new_setup_code(dir: &Path) -> Result<String> {
        if !dir.join("company.json").exists() {
            bail!("this server has no company: run `rider-server company init --name <name>` first");
        }
        let code = new_setup_code()?;
        write_atomic(&dir.join("company-setup"), &format!("{code} {}\n", now() + SETUP_LIFETIME_SECS))?;
        Ok(code)
    }

    pub fn summary(&self) -> Option<(String, Option<String>, usize)> {
        let s = self.state.lock().unwrap();
        s.as_ref().map(|s| (s.name.clone(), s.admin.clone(), s.computers.len()))
    }

    fn save(&self, s: &Stored) -> Result<()> {
        let snapshot = self.snapshot(s)?;
        self.write(snapshot)
    }

    fn save_later(&self) {
        self.dirty.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn flush(&self) -> Result<()> {
        if !self.dirty.swap(false, std::sync::atomic::Ordering::Relaxed) {
            return Ok(());
        }
        let snapshot = {
            let guard = self.state.lock().unwrap();
            let Some(s) = guard.as_ref() else { return Ok(()) };
            self.snapshot(s)?
        };
        self.write(snapshot)
    }

    fn snapshot(&self, s: &Stored) -> Result<(u64, String)> {
        let n = self.taken.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        Ok((n, serde_json::to_string(s)?))
    }

    fn write(&self, (n, text): (u64, String)) -> Result<()> {
        let mut written = self.written.lock().unwrap();
        if n < *written {
            return Ok(());
        }
        write_atomic(&self.dir.join("company.json"), &text)?;
        *written = n;
        Ok(())
    }

    pub fn is_admin(&self, fp: &Fingerprint) -> bool {
        let hex = fp_hex(fp);
        self.state.lock().unwrap().as_ref().is_some_and(|s| s.admin.as_deref() == Some(hex.as_str()))
    }

    pub fn claim(&self, fp: &Fingerprint, code: &str) -> Result<(String, String)> {
        let setup = self.dir.join("company-setup");
        let text = std::fs::read_to_string(&setup).unwrap_or_default();
        let mut parts = text.split_whitespace();
        let (want, expiry) = (parts.next().unwrap_or(""), parts.next().and_then(|e| e.parse::<u64>().ok()));
        let normalize =
            |c: &str| c.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_uppercase();
        let valid = !want.is_empty() && expiry.is_some_and(|e| now() < e) && normalize(want) == normalize(code);
        if !valid {
            bail!("this setup code is not valid: it may have expired (24 hours) or been used");
        }
        let mut guard = self.state.lock().unwrap();
        if guard.is_none() {
            *guard = Self::load(&self.dir)?.state.into_inner().unwrap();
        }
        let s = guard.as_mut().context("this server has no company")?;
        s.admin = Some(fp_hex(fp));
        self.save(s)?;
        let _ = std::fs::remove_file(&setup);
        Ok((s.name.clone(), s.join_secret.clone()))
    }

    pub fn directory(&self, online: &HashSet<Fingerprint>) -> Option<Directory> {
        let s = self.state.lock().unwrap();
        let s = s.as_ref()?;
        let computers = s
            .computers
            .iter()
            .filter_map(|c| {
                let fingerprint = fp_from_hex(&c.fingerprint).ok()?;
                Some(Computer {
                    online: online.contains(&fingerprint),
                    fingerprint,
                    info: c.info.clone(),
                    room: c.room,
                    unattended_on: c.unattended_on,
                    last_seen: c.last_seen,
                    joined: c.joined,
                })
            })
            .collect();
        Some(Directory { company: s.name.clone(), floors: s.floors.clone(), computers })
    }

    pub fn edit(&self, edit: DirEdit) -> Result<()> {
        let mut guard = self.state.lock().unwrap();
        let s = guard.as_mut().context("this server has no company")?;
        apply(s, edit)?;
        self.save(s)
    }

    pub fn enroll(&self, fp: &Fingerprint, secret: &str, info: HostInfo) -> Result<String> {
        let mut guard = self.state.lock().unwrap();
        let s = guard.as_mut().context("this server has no company")?;
        if !constant_time_eq(secret.as_bytes(), s.join_secret.as_bytes()) {
            bail!("this join token is not for this server's company");
        }
        let hex = fp_hex(fp);
        let info = bounded(info);
        match s.computers.iter_mut().find(|c| c.fingerprint == hex) {
            Some(c) => {
                if c.info != info {
                    c.info = info;
                    self.save_later();
                }
                return Ok(s.name.clone());
            }
            None => {
                if s.computers.len() >= MAX_COMPUTERS {
                    bail!("the company has the most computers it can have ({MAX_COMPUTERS})");
                }
                s.computers.push(StoredComputer {
                    fingerprint: hex,
                    info,
                    room: None,
                    unattended_on: false,
                    last_seen: now(),
                    joined: now(),
                });
            }
        }
        self.save(s)?;
        Ok(s.name.clone())
    }

    #[cfg(test)]
    pub fn fill(&self, n: u32, info: &HostInfo) {
        let mut guard = self.state.lock().unwrap();
        let s = guard.as_mut().unwrap();
        for i in 0..n {
            let mut fp = [0u8; 32];
            fp[..4].copy_from_slice(&i.to_le_bytes());
            fp[31] = 7;
            s.computers.push(StoredComputer {
                fingerprint: fp_hex(&fp),
                info: bounded(info.clone()),
                room: None,
                unattended_on: false,
                last_seen: now(),
                joined: now(),
            });
        }
        self.save(s).unwrap();
    }

    pub fn unattended_on(&self, fp: &Fingerprint) -> bool {
        let hex = fp_hex(fp);
        let s = self.state.lock().unwrap();
        s.as_ref().and_then(|s| s.computers.iter().find(|c| c.fingerprint == hex)).is_some_and(|c| c.unattended_on)
    }

    pub fn update_info(&self, fp: &Fingerprint, info: HostInfo) {
        let hex = fp_hex(fp);
        let mut guard = self.state.lock().unwrap();
        let Some(s) = guard.as_mut() else { return };
        let Some(c) = s.computers.iter_mut().find(|c| c.fingerprint == hex) else { return };
        let info = bounded(info);
        if c.info != info {
            c.info = info;
            c.last_seen = now();
            self.save_later();
        }
    }

    pub fn takes_place(&self, fp: &Fingerprint) -> bool {
        let hex = fp_hex(fp);
        let s = self.state.lock().unwrap();
        let version =
            s.as_ref().and_then(|s| s.computers.iter().find(|c| c.fingerprint == hex)).map(|c| c.info.version.clone());
        version.is_some_and(|v| at_least(&v, (0, 6)))
    }

    pub fn set_place(&self, fp: &Fingerprint, mut place: Whereabouts) {
        let hex = fp_hex(fp);
        let enrolled =
            self.state.lock().unwrap().as_ref().is_some_and(|s| s.computers.iter().any(|c| c.fingerprint == hex));
        if enrolled {
            place.via = cut(&place.via, 64);
            self.places.lock().unwrap().insert(*fp, place);
        }
    }

    pub fn places(&self, online: &HashSet<Fingerprint>) -> Vec<(Fingerprint, Whereabouts)> {
        let places = self.places.lock().unwrap();
        places
            .iter()
            .filter(|(fp, w)| online.contains(*fp) && w.place != Place::Unknown)
            .map(|(fp, w)| (*fp, w.clone()))
            .collect()
    }

    pub fn seen(&self, fp: &Fingerprint) {
        self.places.lock().unwrap().remove(fp);
        let hex = fp_hex(fp);
        let mut guard = self.state.lock().unwrap();
        let Some(s) = guard.as_mut() else { return };
        if let Some(c) = s.computers.iter_mut().find(|c| c.fingerprint == hex) {
            c.last_seen = now();
            self.save_later();
        }
    }
}

fn cut(s: &str, max: usize) -> String {
    let mut out = String::new();
    for c in s.chars().filter(|c| !c.is_control()) {
        if out.len() + c.len_utf8() > max {
            break;
        }
        out.push(c);
    }
    out
}

fn at_least(version: &str, (major, minor): (u32, u32)) -> bool {
    let mut parts = version.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    let v = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    v >= (major, minor)
}

fn bounded(mut info: HostInfo) -> HostInfo {
    let cut = |s: &str, max: usize| {
        let mut out = String::new();
        for c in s.chars().filter(|c| !c.is_control()) {
            if out.len() + c.len_utf8() > max {
                break;
            }
            out.push(c);
        }
        out
    };
    info.name = cut(&info.name, 128);
    info.user = info.user.map(|u| cut(&u, 128)).filter(|u| !u.is_empty());
    info.version = cut(&info.version, 32);
    info
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn apply(s: &mut Stored, edit: DirEdit) -> Result<()> {
    let room_exists = |s: &Stored, id: u32| s.floors.iter().any(|f| f.rooms.iter().any(|r| r.id == id));
    match edit {
        DirEdit::AddFloor { name } => {
            if s.floors.len() >= MAX_FLOORS {
                bail!("too many floors");
            }
            let id = next_id(s);
            s.floors.push(Floor { id, name: clean_name(&name)?, rooms: Vec::new() });
        }
        DirEdit::AddRoom { floor, name } => {
            let name = clean_name(&name)?;
            let id = next_id(s);
            let f = s.floors.iter_mut().find(|f| f.id == floor).context("no such floor")?;
            if f.rooms.len() >= MAX_ROOMS_PER_FLOOR {
                bail!("too many rooms on this floor");
            }
            f.rooms.push(Room { id, name });
        }
        DirEdit::Rename { id, name } => {
            let name = clean_name(&name)?;
            if let Some(f) = s.floors.iter_mut().find(|f| f.id == id) {
                f.name = name;
            } else {
                let r = s.floors.iter_mut().flat_map(|f| f.rooms.iter_mut()).find(|r| r.id == id);
                r.context("no such floor or room")?.name = name;
            }
        }
        DirEdit::Delete { id } => {
            if let Some(i) = s.floors.iter().position(|f| f.id == id) {
                if !s.floors[i].rooms.is_empty() {
                    bail!("the floor still has rooms");
                }
                s.floors.remove(i);
            } else {
                if s.computers.iter().any(|c| c.room == Some(id)) {
                    bail!("the room still has computers");
                }
                let f = s.floors.iter_mut().find(|f| f.rooms.iter().any(|r| r.id == id)).context("no such room")?;
                f.rooms.retain(|r| r.id != id);
            }
        }
        DirEdit::Place { computers, room } => {
            if let Some(r) = room
                && !room_exists(s, r)
            {
                bail!("no such room");
            }
            if computers.is_empty() || computers.len() > MAX_COMPUTERS {
                bail!("no computers to move");
            }
            let wanted: HashSet<String> = computers.iter().map(fp_hex).collect();
            let mut moved = 0;
            for c in s.computers.iter_mut().filter(|c| wanted.contains(&c.fingerprint)) {
                c.room = room;
                moved += 1;
            }
            if moved == 0 {
                bail!("no such computer");
            }
        }
        DirEdit::Unattended { computer, on } => computer_mut(s, &computer)?.unattended_on = on,
        DirEdit::Forget { computer } => {
            let hex = fp_hex(&computer);
            s.computers.retain(|c| c.fingerprint != hex);
        }
    }
    Ok(())
}

fn next_id(s: &mut Stored) -> u32 {
    s.next_id = s.next_id.max(1);
    let id = s.next_id;
    s.next_id += 1;
    id
}

fn computer_mut<'a>(s: &'a mut Stored, fp: &Fingerprint) -> Result<&'a mut StoredComputer> {
    let hex = fp_hex(fp);
    s.computers.iter_mut().find(|c| c.fingerprint == hex).context("no such computer")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rider-company-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn info(name: &str) -> HostInfo {
        HostInfo { name: name.into(), version: "0.5.0".into(), ..Default::default() }
    }

    #[test]
    fn setup_code_makes_one_admin() {
        let d = dir("claim");
        let code = Company::init(&d, "Northwind").unwrap();
        let c = Company::load(&d).unwrap();
        assert!(c.claim(&[1; 32], "WRONG-CODE-XXXX").is_err());
        let (name, secret) = c.claim(&[1; 32], &code.to_lowercase().replace('-', " ")).unwrap();
        assert_eq!(name, "Northwind");
        assert_eq!(secret.len(), 32);
        assert!(c.is_admin(&[1; 32]));
        assert!(c.claim(&[2; 32], &code).is_err());
        let code = Company::new_setup_code(&d).unwrap();
        c.claim(&[2; 32], &code).unwrap();
        assert!(c.is_admin(&[2; 32]) && !c.is_admin(&[1; 32]));
        assert!(Company::load(&d).unwrap().is_admin(&[2; 32]));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn computers_join_and_are_placed() {
        let d = dir("place");
        Company::init(&d, "Northwind").unwrap();
        let c = Company::load(&d).unwrap();
        let secret = c.claim(&[1; 32], &std::fs::read_to_string(d.join("company-setup")).unwrap()[..14]).unwrap().1;
        assert!(c.enroll(&[5; 32], "nope", info("PC-1")).is_err());
        c.enroll(&[5; 32], &secret, info("PC-1")).unwrap();
        c.enroll(&[6; 32], &secret, info("PC-2")).unwrap();
        c.enroll(&[5; 32], &secret, info("PC-1b")).unwrap();

        c.edit(DirEdit::AddFloor { name: "Floor 3".into() }).unwrap();
        let floor = c.directory(&HashSet::new()).unwrap().floors[0].id;
        c.edit(DirEdit::AddRoom { floor, name: "310".into() }).unwrap();
        let room = c.directory(&HashSet::new()).unwrap().floors[0].rooms[0].id;
        c.edit(DirEdit::Place { computers: vec![[5; 32]], room: Some(room) }).unwrap();
        assert!(c.edit(DirEdit::Place { computers: vec![[5; 32]], room: Some(999) }).is_err());
        assert!(c.edit(DirEdit::Delete { id: room }).is_err(), "a room with computers stays");
        assert!(c.edit(DirEdit::Delete { id: floor }).is_err(), "a floor with rooms stays");

        let online: HashSet<_> = [[5u8; 32]].into();
        let dirx = c.directory(&online).unwrap();
        assert_eq!(dirx.computers.len(), 2);
        assert_eq!(dirx.room_counts(Some(room)), (1, 1));
        assert_eq!(dirx.room_counts(None), (0, 1));
        assert_eq!(dirx.computers[0].info.name, "PC-1b");

        c.edit(DirEdit::Forget { computer: [6; 32] }).unwrap();
        c.edit(DirEdit::Place { computers: vec![[5; 32]], room: None }).unwrap();
        c.edit(DirEdit::Delete { id: room }).unwrap();
        c.edit(DirEdit::Delete { id: floor }).unwrap();
        let reloaded = Company::load(&d).unwrap().directory(&HashSet::new()).unwrap();
        assert_eq!((reloaded.floors.len(), reloaded.computers.len()), (0, 1));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn places_of_new_computers_only() {
        let d = dir("whereabouts");
        Company::init(&d, "Northwind").unwrap();
        let c = Company::load(&d).unwrap();
        let secret = c.claim(&[1; 32], &std::fs::read_to_string(d.join("company-setup")).unwrap()[..14]).unwrap().1;
        let version = |v: &str| HostInfo { name: "PC".into(), version: v.into(), ..Default::default() };
        c.enroll(&[5; 32], &secret, version("0.5.0")).unwrap();
        c.enroll(&[6; 32], &secret, version("0.6.0")).unwrap();
        c.enroll(&[7; 32], &secret, version("1.0.2")).unwrap();
        assert!(!c.takes_place(&[5; 32]) && c.takes_place(&[6; 32]) && c.takes_place(&[7; 32]));
        assert!(!c.takes_place(&[8; 32]));
        let remote = Whereabouts { place: Place::Remote, via: "PANGP Virtual Ethernet Adapter".into() };
        c.set_place(&[6; 32], remote.clone());
        c.set_place(&[7; 32], Whereabouts { place: Place::Unknown, via: String::new() });
        c.set_place(&[8; 32], remote.clone());
        let online: HashSet<Fingerprint> = [[6; 32], [7; 32], [8; 32]].into();
        assert_eq!(c.places(&online), vec![([6; 32], remote)]);
        c.seen(&[6; 32]);
        assert!(c.places(&online).is_empty());
    }

    #[test]
    fn batched_writes_keep_the_newest() {
        let d = std::env::temp_dir().join(format!("rider-company-batch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Company::init(&d, "Batch").unwrap();
        let c = Company::load(&d).unwrap();
        let code = Company::new_setup_code(&d).unwrap();
        let (_, secret) = c.claim(&[1; 32], &code).unwrap();
        c.enroll(&[5; 32], &secret, HostInfo { name: "A".into(), ..Default::default() }).unwrap();
        let name_on_disk =
            || Company::load(&d).unwrap().directory(&HashSet::new()).unwrap().computers[0].info.name.clone();

        c.update_info(&[5; 32], HostInfo { name: "B".into(), ..Default::default() });
        assert_eq!(name_on_disk(), "A");
        c.flush().unwrap();
        assert_eq!(name_on_disk(), "B");
        c.flush().unwrap();

        let older = {
            let g = c.state.lock().unwrap();
            c.snapshot(g.as_ref().unwrap()).unwrap()
        };
        c.update_info(&[5; 32], HostInfo { name: "C".into(), ..Default::default() });
        let newer = {
            let g = c.state.lock().unwrap();
            c.snapshot(g.as_ref().unwrap()).unwrap()
        };
        c.write(newer).unwrap();
        c.write(older).unwrap();
        assert_eq!(name_on_disk(), "C");

        c.flush().unwrap();
        let before = std::fs::metadata(d.join("company.json")).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        c.enroll(&[5; 32], &secret, HostInfo { name: "C".into(), ..Default::default() }).unwrap();
        c.flush().unwrap();
        assert_eq!(std::fs::metadata(d.join("company.json")).unwrap().modified().unwrap(), before);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn list_of_5000_fits_one_message() {
        let long = "\u{1F600}".repeat(500);
        let via = cut(&long, 64);
        let info =
            HostInfo { name: long.clone(), user: Some(long.clone()), version: long, locked: true, unattended: true };
        let info = bounded(info);
        assert!(info.name.len() <= 128 && info.user.as_ref().unwrap().len() <= 128 && info.version.len() <= 32);
        let computers = (0..MAX_COMPUTERS as u32)
            .map(|i| {
                let mut fingerprint = [0u8; 32];
                fingerprint[..4].copy_from_slice(&i.to_le_bytes());
                Computer {
                    fingerprint,
                    info: info.clone(),
                    room: Some(u32::MAX),
                    unattended_on: true,
                    online: true,
                    last_seen: u64::MAX,
                    joined: u64::MAX,
                }
            })
            .collect();
        let dir = Directory { company: "x".repeat(MAX_NAME), floors: Vec::new(), computers };
        let places = dir
            .computers
            .iter()
            .map(|c| (c.fingerprint, Whereabouts { place: Place::Remote, via: via.clone() }))
            .collect();
        let msg = rider_core::rendezvous::ServerMsg::Directory2 { directory: dir, places };
        let size = postcard::to_allocvec(&msg).unwrap().len();
        eprintln!("the worst list of {MAX_COMPUTERS}: {size} bytes");
        assert!(size < 4 * 1024 * 1024 * 3 / 4, "{size} bytes: too close to the 4 MB message limit");
    }

    #[test]
    fn names_are_cleaned() {
        assert_eq!(clean_name("  Room\u{7}  310 ").unwrap(), "Room  310");
        assert!(clean_name("   ").is_err());
        assert_eq!(clean_name(&"x".repeat(200)).unwrap().len(), MAX_NAME);
    }
}
