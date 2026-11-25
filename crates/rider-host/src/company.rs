//! Enterprise mode on a company computer (docs/ROADMAP.md, Milestone 7):
//! `rider-host enroll <token>` joins the company with the join token from
//! the company installer, and while registered with the server this host
//! keeps the admin's console up to date (`HostInfo`: name, who is signed
//! in, locked or not, version).

use crate::config::{CompanyLink, HostConfig};
use anyhow::{Context, Result};
use rider_core::company::{HostInfo, JoinToken};
use rider_core::identity::{Identity, fp_hex};
use rider_core::rendezvous::ServerRef;
use windows::Win32::System::RemoteDesktop::{
    WTS_SESSIONSTATE_LOCK, WTSFreeMemory, WTSGetActiveConsoleSessionId, WTSINFOEXW, WTSQuerySessionInformationW,
    WTSSessionInfoEx, WTSUserName,
};
use windows::core::PWSTR;

/// Join the company in `token`: pin its server and admin, then enroll.
pub fn enroll(cfg: &mut HostConfig, identity: &Identity, token: &str, unattended: bool) -> Result<String> {
    let token = JoinToken::decode(token)?;
    let server = ServerRef::new(&token.server, &token.server_fingerprint_hex())?;
    let link = CompanyLink { name: String::new(), admin: fp_hex(&token.admin), unattended };
    let info = info(cfg, Some(&link));
    let company = tokio::runtime::Runtime::new()?
        .block_on(rider_core::rendezvous::enroll(identity, &server, &token.secret, info))
        .context("joining the company")?;
    cfg.server = Some(server);
    cfg.company = Some(CompanyLink { name: company.clone(), ..link });
    cfg.save()?;
    Ok(company)
}

/// What the admin's console shows about this computer.
pub fn info(cfg: &HostConfig, company: Option<&CompanyLink>) -> HostInfo {
    let (user, locked) = console_user();
    HostInfo {
        name: cfg.name.clone(),
        user,
        locked,
        version: env!("CARGO_PKG_VERSION").into(),
        unattended: company.is_some_and(|c| c.unattended),
    }
}

/// Who is signed in at the console, and whether the session is locked.
pub fn console_user() -> (Option<String>, bool) {
    // SAFETY: WTS buffers are allocated by the system and freed once each;
    // the session-info buffer is read as the struct the call documents.
    unsafe {
        let session = WTSGetActiveConsoleSessionId();
        let mut buf = PWSTR::null();
        let mut len = 0u32;
        let user = if WTSQuerySessionInformationW(None, session, WTSUserName, &mut buf, &mut len).is_ok() {
            let name = buf.to_string().unwrap_or_default();
            WTSFreeMemory(buf.0.cast());
            Some(name).filter(|n| !n.is_empty())
        } else {
            None
        };
        let mut locked = false;
        let mut buf = PWSTR::null();
        if user.is_some() && WTSQuerySessionInformationW(None, session, WTSSessionInfoEx, &mut buf, &mut len).is_ok() {
            if len as usize >= std::mem::size_of::<WTSINFOEXW>() {
                let ex = &*(buf.0 as *const WTSINFOEXW);
                if ex.Level == 1 {
                    locked = ex.Data.WTSInfoExLevel1.SessionFlags as u32 == WTS_SESSIONSTATE_LOCK;
                }
            }
            WTSFreeMemory(buf.0.cast());
        }
        (user, locked)
    }
}
