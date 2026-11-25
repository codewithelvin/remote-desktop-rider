use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Az,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::En, Lang::Az];

    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Az => "az",
        }
    }

    pub fn from_code(s: &str) -> Option<Lang> {
        match s.trim().to_ascii_lowercase().as_str() {
            "en" | "english" => Some(Lang::En),
            "az" | "azerbaijani" | "azərbaycan" => Some(Lang::Az),
            _ => None,
        }
    }

    pub fn native_name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Az => "Azərbaycan dili",
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn lang() -> Lang {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Lang::En,
        2 => Lang::Az,
        _ => {
            let l = load();
            set_current(l);
            l
        }
    }
}

pub fn set_current(lang: Lang) {
    CURRENT.store(if lang == Lang::Az { 2 } else { 1 }, Ordering::Relaxed);
}

pub fn tr(en: &'static str, az: &'static str) -> &'static str {
    match lang() {
        Lang::En => en,
        Lang::Az => az,
    }
}

#[macro_export]
macro_rules! trf {
    ($en:literal, $az:literal $(, $arg:expr)* $(,)?) => {
        match $crate::i18n::lang() {
            $crate::i18n::Lang::En => format!($en $(, $arg)*),
            $crate::i18n::Lang::Az => format!($az $(, $arg)*),
        }
    };
}

fn file() -> Option<std::path::PathBuf> {
    crate::app_dir().ok().map(|d| d.join("language"))
}

pub fn load() -> Lang {
    if let Some(l) = std::env::var("RIDER_LANG").ok().and_then(|s| Lang::from_code(&s)) {
        return l;
    }
    if let Some(l) = file().and_then(|f| std::fs::read_to_string(f).ok()).and_then(|s| Lang::from_code(&s)) {
        return l;
    }
    system()
}

pub fn save(lang: Lang) -> std::io::Result<()> {
    set_current(lang);
    let Some(f) = file() else { return Ok(()) };
    if let Some(dir) = f.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(f, lang.code())
}

#[cfg(windows)]
fn system() -> Lang {
    use windows::Win32::Globalization::GetUserDefaultLocaleName;
    let mut buf = [0u16; 85];
    let n = unsafe { GetUserDefaultLocaleName(&mut buf) };
    let name = String::from_utf16_lossy(&buf[..(n.max(1) as usize - 1)]);
    if name.to_ascii_lowercase().starts_with("az") { Lang::Az } else { Lang::En }
}

#[cfg(not(windows))]
fn system() -> Lang {
    let v = std::env::var("LC_ALL").or_else(|_| std::env::var("LANG")).unwrap_or_default();
    if v.to_ascii_lowercase().starts_with("az") { Lang::Az } else { Lang::En }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip() {
        for l in Lang::ALL {
            assert_eq!(Lang::from_code(l.code()), Some(l));
        }
        assert_eq!(Lang::from_code(" AZ\n"), Some(Lang::Az));
        assert_eq!(Lang::from_code("ru"), None);
    }

    #[test]
    fn picks_by_language() {
        set_current(Lang::Az);
        assert_eq!(tr("Connect", "Qoşul"), "Qoşul");
        let n = 3;
        assert_eq!(trf!("{n} min ago", "{n} dəq əvvəl"), "3 dəq əvvəl");
        set_current(Lang::En);
        assert_eq!(tr("Connect", "Qoşul"), "Connect");
    }
}
