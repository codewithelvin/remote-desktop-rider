//! Updates from the project's GitHub releases (public, no server of ours).
//!
//! `latest()` reads the newest release; `download()` fetches its installer and
//! checks it against the SHA-256 GitHub publishes for the file. HTTP goes
//! through WinHTTP, so it uses the system's proxy settings and certificate
//! store (company networks) and adds no TLS stack to the binaries. Only HTTPS
//! is used, and only release files of this repository are accepted.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fmt;
use std::path::Path;

pub const REPO: &str = "codewithelvin/remote-desktop-rider";
/// Installer files are named `rdr-setup-<version>.exe`.
const ASSET_PREFIX: &str = "rdr-setup-";
/// Larger than any real installer; guards against a runaway download.
const MAX_SIZE: u64 = 200 << 20;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    /// `1.2.3` or `v1.2.3`; anything after a `-` or `+` is ignored.
    pub fn parse(s: &str) -> Option<Self> {
        let core = s.trim().trim_start_matches('v').split(['-', '+']).next()?;
        let mut it = core.split('.').map(|p| p.parse::<u32>().ok());
        let v = Self(it.next()??, it.next()??, it.next().unwrap_or(Some(0))?);
        it.next().is_none().then_some(v)
    }

    /// The version of this build (CI stamps it into Cargo.toml).
    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Self(0, 0, 0))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: Version,
    /// Release page, for "what's new".
    pub page: String,
    pub file_name: String,
    pub url: String,
    pub size: u64,
    pub sha256: [u8; 32],
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    /// `sha256:<hex>`, set by GitHub for uploaded files.
    #[serde(default)]
    digest: Option<String>,
}

fn sha_from_hex(s: &str) -> Option<[u8; 32]> {
    hex::decode(s.trim()).ok()?.try_into().ok()
}

/// The release from the GitHub API's JSON.
fn parse_release(json: &str) -> Result<Release> {
    let r: ApiRelease = serde_json::from_str(json).context("unexpected reply from GitHub")?;
    if r.draft || r.prerelease {
        bail!("the latest release is not final");
    }
    let version = Version::parse(&r.tag_name).with_context(|| format!("unexpected release tag {:?}", r.tag_name))?;
    let asset = r
        .assets
        .iter()
        .find(|a| a.name.starts_with(ASSET_PREFIX) && a.name.ends_with(".exe"))
        .context("the release has no installer")?;
    // Only this repository's release files: nothing the JSON could redirect elsewhere.
    let prefix = format!("https://github.com/{REPO}/releases/download/");
    if !asset.browser_download_url.starts_with(&prefix) {
        bail!("unexpected download address {}", asset.browser_download_url);
    }
    if asset.size == 0 || asset.size > MAX_SIZE {
        bail!("unexpected installer size {}", asset.size);
    }
    // GitHub's digest of the uploaded file; older releases: the SHA-256 that
    // CI writes into the release notes.
    let sha256 = asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
        .and_then(sha_from_hex)
        .or_else(|| {
            r.body.as_deref()?.split(|c: char| !c.is_ascii_hexdigit()).find(|w| w.len() == 64).and_then(sha_from_hex)
        })
        .context("the release does not say the installer's SHA-256")?;
    Ok(Release {
        version,
        page: r.html_url,
        file_name: asset.name.clone(),
        url: asset.browser_download_url.clone(),
        size: asset.size,
        sha256,
    })
}

/// The newest published release.
#[cfg(windows)]
pub fn latest() -> Result<Release> {
    let mut body = Vec::new();
    http::get(
        &format!("https://api.github.com/repos/{REPO}/releases/latest"),
        "application/vnd.github+json",
        1 << 20,
        &mut |chunk| {
            body.extend_from_slice(chunk);
            Ok(())
        },
    )
    .context("cannot reach GitHub")?;
    parse_release(std::str::from_utf8(&body).context("unexpected reply from GitHub")?)
}

/// The latest release, if it is newer than this build.
#[cfg(windows)]
pub fn newer() -> Result<Option<Release>> {
    let r = latest()?;
    Ok((r.version > Version::current()).then_some(r))
}

/// Download the installer to `dest`, checking size and SHA-256 before it
/// appears there (a `.part` file is used meanwhile). `progress(done, total)`.
#[cfg(windows)]
pub fn download(r: &Release, dest: &Path, progress: &mut dyn FnMut(u64, u64)) -> Result<()> {
    use std::io::Write;
    let part = dest.with_extension("part");
    let mut file = std::fs::File::create(&part).with_context(|| format!("cannot write {}", part.display()))?;
    let mut hash = Sha256::new();
    let mut done = 0u64;
    let result = http::get(&r.url, "application/octet-stream", r.size, &mut |chunk| {
        file.write_all(chunk)?;
        hash.update(chunk);
        done += chunk.len() as u64;
        progress(done, r.size);
        Ok(())
    })
    .context("download failed")
    .and_then(|()| {
        file.sync_all()?;
        if done != r.size {
            bail!("download incomplete ({done} of {} bytes)", r.size);
        }
        if hash.finalize().as_slice() != r.sha256 {
            bail!("the downloaded installer does not match its published SHA-256");
        }
        Ok(())
    });
    drop(file);
    if let Err(e) = result {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    std::fs::rename(&part, dest).with_context(|| format!("cannot write {}", dest.display()))
}

/// Minimal HTTPS GET over WinHTTP.
#[cfg(windows)]
pub(crate) mod http {
    use anyhow::{Context, Result, bail};
    use std::ffi::c_void;
    use windows::Win32::Networking::WinHttp::{
        WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_ADDREQ_FLAG_ADD, WINHTTP_FLAG_SECURE,
        WINHTTP_QUERY_CONTENT_LENGTH, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE, WinHttpAddRequestHeaders,
        WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpReadData,
        WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts,
    };
    use windows::core::{HSTRING, PCWSTR, w};

    struct Handle(*mut c_void);

    impl Handle {
        fn new(h: *mut c_void, what: &str) -> Result<Self> {
            if h.is_null() {
                return Err(windows::core::Error::from_thread()).context(what.to_owned());
            }
            Ok(Self(h))
        }
    }

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: a handle WinHTTP gave us, closed once.
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }

    fn query_number(req: &Handle, info: u32) -> Option<u64> {
        let mut v = 0u32;
        let mut len = size_of::<u32>() as u32;
        // SAFETY: `v` is a u32 out-parameter of the stated length.
        unsafe {
            WinHttpQueryHeaders(
                req.0,
                info | WINHTTP_QUERY_FLAG_NUMBER,
                PCWSTR::null(),
                Some((&raw mut v).cast()),
                &mut len,
                std::ptr::null_mut(),
            )
        }
        .ok()?;
        Some(v as u64)
    }

    /// GET `url` (https only), following redirects; `sink(chunk)` gets
    /// the body. Fails on a non-200 status or more than `max` bytes.
    pub fn get(url: &str, accept: &str, max: u64, sink: &mut dyn FnMut(&[u8]) -> Result<()>) -> Result<()> {
        let rest = url.strip_prefix("https://").context("only https addresses are used")?;
        let (host, path) = rest.split_once('/').map_or((rest, "/".to_owned()), |(h, p)| (h, format!("/{p}")));
        let agent = HSTRING::from(format!("RemoteDesktopRider/{}", env!("CARGO_PKG_VERSION")));
        // SAFETY: plain WinHTTP calls on handles owned by the guards below;
        // buffers outlive the calls that use them.
        unsafe {
            let session = Handle::new(
                WinHttpOpen(&agent, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0),
                "WinHttpOpen",
            )?;
            // Resolve, connect, send, receive (ms).
            WinHttpSetTimeouts(session.0, 15_000, 15_000, 30_000, 30_000)?;
            let conn = Handle::new(WinHttpConnect(session.0, &HSTRING::from(host), 443, 0), "connect")?;
            let req = Handle::new(
                WinHttpOpenRequest(
                    conn.0,
                    w!("GET"),
                    &HSTRING::from(path),
                    PCWSTR::null(),
                    PCWSTR::null(),
                    std::ptr::null(),
                    WINHTTP_FLAG_SECURE,
                ),
                "request",
            )?;
            let headers: Vec<u16> = format!("Accept: {accept}\r\n").encode_utf16().collect();
            WinHttpAddRequestHeaders(req.0, &headers, WINHTTP_ADDREQ_FLAG_ADD)?;
            WinHttpSendRequest(req.0, None, None, 0, 0, 0).context("cannot connect")?;
            WinHttpReceiveResponse(req.0, std::ptr::null_mut()).context("no answer")?;
            let status = query_number(&req, WINHTTP_QUERY_STATUS_CODE).unwrap_or(0);
            if status != 200 {
                bail!("server answered HTTP {status}");
            }
            let total = query_number(&req, WINHTTP_QUERY_CONTENT_LENGTH);
            if total.is_some_and(|t| t > max) {
                bail!("reply too large");
            }
            let mut buf = vec![0u8; 64 << 10];
            let mut got = 0u64;
            loop {
                let mut n = 0u32;
                WinHttpReadData(req.0, buf.as_mut_ptr().cast(), buf.len() as u32, &mut n).context("connection lost")?;
                if n == 0 {
                    return Ok(());
                }
                got += n as u64;
                if got > max {
                    bail!("reply too large");
                }
                sink(&buf[..n as usize])?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "9bbd1b21aec245f7b8083e39db4b3d8f5c01cfa1a8b5f9eb02d3cb35a8b6f0c1";

    fn json(tag: &str, url: &str, digest: Option<&str>, body: &str) -> String {
        let digest = digest.map_or("null".to_owned(), |d| format!("\"{d}\""));
        format!(
            r#"{{"tag_name":"{tag}","html_url":"https://github.com/{REPO}/releases/tag/{tag}","body":"{body}",
               "draft":false,"prerelease":false,
               "assets":[{{"name":"notes.txt","browser_download_url":"https://github.com/{REPO}/releases/download/{tag}/notes.txt","size":10}},
                         {{"name":"rdr-setup-0.1.2.exe","browser_download_url":"{url}","size":9437184,"digest":{digest}}}]}}"#
        )
    }

    #[test]
    fn versions() {
        assert_eq!(Version::parse("v0.1.12"), Some(Version(0, 1, 12)));
        assert_eq!(Version::parse("1.2"), Some(Version(1, 2, 0)));
        assert_eq!(Version::parse("1.2.3-beta"), Some(Version(1, 2, 3)));
        assert_eq!(Version::parse("1.2.3.4"), None);
        assert_eq!(Version::parse("x"), None);
        assert!(Version(0, 1, 10) > Version(0, 1, 9));
        assert!(Version(0, 2, 0) > Version(0, 1, 99));
        assert_eq!(Version(0, 1, 3).to_string(), "0.1.3");
    }

    #[test]
    fn reads_a_release() {
        let url = format!("https://github.com/{REPO}/releases/download/v0.1.2/rdr-setup-0.1.2.exe");
        let r = parse_release(&json("v0.1.2", &url, Some(&format!("sha256:{SHA}")), "")).unwrap();
        assert_eq!(r.version, Version(0, 1, 2));
        assert_eq!(r.file_name, "rdr-setup-0.1.2.exe");
        assert_eq!(r.url, url);
        assert_eq!(hex::encode(r.sha256), SHA);
        // No digest: the SHA-256 from the release notes.
        let r = parse_release(&json("v0.1.2", &url, None, &format!("SHA-256: `{SHA}`"))).unwrap();
        assert_eq!(hex::encode(r.sha256), SHA);
    }

    #[test]
    fn rejects_foreign_or_unverifiable_files() {
        let evil = "https://example.com/rdr-setup-0.1.2.exe";
        assert!(parse_release(&json("v0.1.2", evil, Some(&format!("sha256:{SHA}")), "")).is_err());
        let other_repo = "https://github.com/someone/else/releases/download/v0.1.2/rdr-setup-0.1.2.exe";
        assert!(parse_release(&json("v0.1.2", other_repo, Some(&format!("sha256:{SHA}")), "")).is_err());
        let url = format!("https://github.com/{REPO}/releases/download/v0.1.2/rdr-setup-0.1.2.exe");
        assert!(parse_release(&json("v0.1.2", &url, None, "no hash here")).is_err());
    }

    #[test]
    #[ignore = "network: downloads the latest installer from GitHub"]
    fn live_latest_and_download() {
        let r = latest().unwrap();
        println!("latest {} {} ({} bytes)", r.version, r.file_name, r.size);
        let dest = std::env::temp_dir().join(format!("rider-update-test-{}", r.file_name));
        let mut last = 0;
        download(&r, &dest, &mut |done, total| last = done * 100 / total).unwrap();
        assert_eq!(last, 100);
        assert_eq!(std::fs::metadata(&dest).unwrap().len(), r.size);
        std::fs::remove_file(dest).unwrap();
    }
}
