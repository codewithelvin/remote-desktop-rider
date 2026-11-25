use anyhow::{Result, bail};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, HLOCAL, LocalFree};
use windows::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_BINARY, RRF_RT_REG_BINARY, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::core::{HSTRING, PCWSTR, w};

const KEY: PCWSTR = w!(r"Software\RemoteDesktopRider\Pins");

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
}

fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    unsafe {
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(out.pbData.cast())));
        v
    }
}

pub fn save(fingerprint: &str, pin: &str) -> Result<()> {
    let mut out = CRYPT_INTEGER_BLOB::default();
    let entropy = blob(fingerprint.as_bytes());
    unsafe {
        CryptProtectData(
            &blob(pin.as_bytes()),
            w!("Remote Desktop Rider PIN"),
            Some(&entropy),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )?;
    }
    let data = take(out);
    let name = HSTRING::from(fingerprint);
    unsafe {
        RegSetKeyValueW(HKEY_CURRENT_USER, KEY, &name, REG_BINARY.0, Some(data.as_ptr().cast()), data.len() as u32)
    }
    .ok()?;
    Ok(())
}

pub fn load(fingerprint: &str) -> Option<String> {
    let name = HSTRING::from(fingerprint);
    let mut len = 0u32;
    let data = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, KEY, &name, RRF_RT_REG_BINARY, None, None, Some(&mut len)).ok().ok()?;
        let mut buf = vec![0u8; len as usize];
        RegGetValueW(
            HKEY_CURRENT_USER,
            KEY,
            &name,
            RRF_RT_REG_BINARY,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        )
        .ok()
        .ok()?;
        buf.truncate(len as usize);
        buf
    };
    let mut out = CRYPT_INTEGER_BLOB::default();
    let entropy = blob(fingerprint.as_bytes());
    unsafe {
        CryptUnprotectData(&blob(&data), None, Some(&entropy), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out).ok()?;
    }
    String::from_utf8(take(out)).ok()
}

pub fn forget(fingerprint: &str) -> Result<()> {
    let name = HSTRING::from(fingerprint);
    match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, KEY, &name) } {
        ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
        e => bail!("cannot forget the saved PIN: {e:?}"),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn round_trip() {
        let fp = "test-0000-remote-desktop-rider-pins";
        super::save(fp, "secret PIN ü").unwrap();
        assert_eq!(super::load(fp).as_deref(), Some("secret PIN ü"));
        super::forget(fp).unwrap();
        assert_eq!(super::load(fp), None);
        super::forget(fp).unwrap();
    }
}
