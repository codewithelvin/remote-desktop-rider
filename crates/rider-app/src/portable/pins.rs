use anyhow::Result;

#[cfg(target_os = "macos")]
mod store {
    use anyhow::Result;
    use security_framework::passwords::{delete_generic_password, get_generic_password, set_generic_password};

    const SERVICE: &str = "Remote Desktop Rider PIN";

    pub fn save(fingerprint: &str, pin: &str) -> Result<()> {
        set_generic_password(SERVICE, fingerprint, pin.as_bytes())?;
        Ok(())
    }

    pub fn load(fingerprint: &str) -> Option<String> {
        String::from_utf8(get_generic_password(SERVICE, fingerprint).ok()?).ok()
    }

    pub fn forget(fingerprint: &str) -> Result<()> {
        match delete_generic_password(SERVICE, fingerprint) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == -25300 => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod store {
    use anyhow::{Context, Result};
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    use std::path::PathBuf;

    fn file(fingerprint: &str) -> Result<PathBuf> {
        anyhow::ensure!(fingerprint.chars().all(|c| c.is_ascii_hexdigit()), "invalid fingerprint");
        Ok(rider_core::app_dir()?.join("pins").join(fingerprint))
    }

    pub fn save(fingerprint: &str, pin: &str) -> Result<()> {
        let path = file(fingerprint)?;
        let dir = path.parent().context("no folder")?;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        let tmp = path.with_extension("tmp");
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        f.write_all(pin.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn load(fingerprint: &str) -> Option<String> {
        std::fs::read_to_string(file(fingerprint).ok()?).ok()
    }

    pub fn forget(fingerprint: &str) -> Result<()> {
        match std::fs::remove_file(file(fingerprint)?) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }
}

pub fn save(fingerprint: &str, pin: &str) -> Result<()> {
    store::save(fingerprint, pin)
}

pub fn load(fingerprint: &str) -> Option<String> {
    store::load(fingerprint)
}

pub fn forget(fingerprint: &str) -> Result<()> {
    store::forget(fingerprint)
}
