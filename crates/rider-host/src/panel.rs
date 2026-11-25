use crate::ui::{self, ChatCmd};
use anyhow::{Context, Result, bail};
use std::io::{BufRead, BufReader, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::time::Duration;
use tokio::sync::mpsc;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, HANDLE_FLAGS, SetHandleInformation};
use windows::Win32::Security::SECURITY_ATTRIBUTES;
use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::RemoteDesktop::{WTSGetActiveConsoleSessionId, WTSQueryUserToken};
use windows::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
    InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOEXW, STARTUPINFOW, TerminateProcess,
    UpdateProcThreadAttribute, WaitForSingleObject,
};
use windows::core::PWSTR;

const CLOSE_GRACE: Duration = Duration::from_secs(3);

pub struct Panel {
    lines: std::sync::mpsc::Sender<String>,
    process: Option<OwnedHandle>,
}

impl Panel {
    pub fn launch(device: &str, files: bool) -> Result<(Self, mpsc::UnboundedReceiver<ChatCmd>)> {
        let exe = std::env::current_exe()?.with_file_name("RemoteDesktopRider.exe");
        if !exe.exists() {
            bail!("{} is missing", exe.display());
        }
        let (process, to_window, from_window) = spawn(&exe)?;

        let (lines, rx) = std::sync::mpsc::channel::<String>();
        std::thread::Builder::new().name("panel-out".into()).spawn(move || {
            let mut pipe = to_window;
            for line in rx {
                if writeln!(pipe, "{line}").and_then(|()| pipe.flush()).is_err() {
                    break;
                }
            }
        })?;

        let (tx, cmds) = mpsc::unbounded_channel();
        std::thread::Builder::new().name("panel-in".into()).spawn(move || {
            let mut input = BufReader::new(from_window);
            let mut bytes = Vec::new();
            loop {
                bytes.clear();
                match input.read_until(b'\n', &mut bytes) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if let Some(cmd) = parse(&String::from_utf8_lossy(&bytes))
                            && tx.send(cmd).is_err()
                        {
                            break;
                        }
                    }
                }
            }
        })?;

        let panel = Self { lines, process: Some(process) };
        panel.emit(serde_json::json!({ "event": "session", "device": device, "files": files }));
        Ok((panel, cmds))
    }

    pub fn ask(&self, company: &str, seconds: u64) {
        self.emit(serde_json::json!({ "event": "request", "company": company, "seconds": seconds }));
    }

    pub fn emit(&self, event: serde_json::Value) {
        let _ = self.lines.send(event.to_string());
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        let (lines, _) = std::sync::mpsc::channel();
        drop(std::mem::replace(&mut self.lines, lines));
        if let Some(process) = self.process.take() {
            std::thread::spawn(move || {
                let h = HANDLE(process.as_raw_handle());
                unsafe {
                    if WaitForSingleObject(h, CLOSE_GRACE.as_millis() as u32).0 != 0 {
                        let _ = TerminateProcess(h, 0);
                    }
                }
            });
        }
    }
}

fn parse(line: &str) -> Option<ChatCmd> {
    match ui::split_line(line) {
        ("end", "") => Some(ChatCmd::End),
        ("accept", "") => Some(ChatCmd::Answer(true)),
        ("decline", "") => Some(ChatCmd::Answer(false)),
        (word, arg) => ui::parse_chat(word, arg),
    }
}

fn spawn(exe: &Path) -> Result<(OwnedHandle, std::fs::File, std::fs::File)> {
    let mut token = HANDLE::default();
    if unsafe { WTSQueryUserToken(WTSGetActiveConsoleSessionId(), &mut token) }.is_err() {
        if crate::system::is_system_profile() {
            bail!("nobody is signed in");
        }
        return spawn_here(exe);
    }
    let token = unsafe { OwnedHandle::from_raw_handle(token.0) };
    spawn_as(HANDLE(token.as_raw_handle()), exe)
}

fn spawn_here(exe: &Path) -> Result<(OwnedHandle, std::fs::File, std::fs::File)> {
    use std::process::Stdio;
    let mut child = std::process::Command::new(exe)
        .arg("--session-panel")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("starting {}", exe.display()))?;
    let input = OwnedHandle::from(child.stdin.take().context("no stdin")?);
    let output = OwnedHandle::from(child.stdout.take().context("no stdout")?);
    let process = OwnedHandle::from(child);
    Ok((process, input.into(), output.into()))
}

fn pipe(child_reads: bool) -> Result<(OwnedHandle, OwnedHandle)> {
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        bInheritHandle: true.into(),
        ..Default::default()
    };
    let (mut r, mut w) = (HANDLE::default(), HANDLE::default());
    let (r, w) = unsafe {
        CreatePipe(&mut r, &mut w, Some(&sa), 0)?;
        (OwnedHandle::from_raw_handle(r.0), OwnedHandle::from_raw_handle(w.0))
    };
    let ours = if child_reads { &w } else { &r };
    unsafe { SetHandleInformation(HANDLE(ours.as_raw_handle()), HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0))? };
    Ok((r, w))
}

fn spawn_as(token: HANDLE, exe: &Path) -> Result<(OwnedHandle, std::fs::File, std::fs::File)> {
    let (in_r, in_w) = pipe(true)?;
    let (out_r, out_w) = pipe(false)?;
    let inherit = [HANDLE(in_r.as_raw_handle()), HANDLE(out_w.as_raw_handle())];
    let mut desktop: Vec<u16> = "winsta0\\default\0".encode_utf16().collect();
    let mut cmd: Vec<u16> = format!("\"{}\" --session-panel\0", exe.display()).encode_utf16().collect();

    unsafe {
        let mut size = 0usize;
        let _ = InitializeProcThreadAttributeList(None, 1, None, &mut size);
        let mut buf = vec![0u64; size.div_ceil(8)];
        let list = LPPROC_THREAD_ATTRIBUTE_LIST(buf.as_mut_ptr().cast());
        InitializeProcThreadAttributeList(Some(list), 1, None, &mut size)?;
        let mut env: *mut core::ffi::c_void = std::ptr::null_mut();
        let result = (|| {
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                Some(inherit.as_ptr().cast()),
                std::mem::size_of_val(&inherit),
                None,
                None,
            )?;
            CreateEnvironmentBlock(&mut env, Some(token), false)?;
            let si = STARTUPINFOEXW {
                StartupInfo: STARTUPINFOW {
                    cb: std::mem::size_of::<STARTUPINFOEXW>() as u32,
                    lpDesktop: PWSTR(desktop.as_mut_ptr()),
                    dwFlags: STARTF_USESTDHANDLES,
                    hStdInput: inherit[0],
                    hStdOutput: inherit[1],
                    ..Default::default()
                },
                lpAttributeList: list,
            };
            let mut pi = PROCESS_INFORMATION::default();
            CreateProcessAsUserW(
                Some(token),
                None,
                Some(PWSTR(cmd.as_mut_ptr())),
                None,
                None,
                true,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                Some(env),
                None,
                &si.StartupInfo,
                &mut pi,
            )?;
            let _ = CloseHandle(pi.hThread);
            anyhow::Ok(OwnedHandle::from_raw_handle(pi.hProcess.0))
        })();
        if !env.is_null() {
            let _ = DestroyEnvironmentBlock(env);
        }
        DeleteProcThreadAttributeList(list);
        let process = result.context("starting the chat window as the signed-in user")?;
        drop((in_r, out_w));
        Ok((process, in_w.into(), out_r.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_from_the_window() {
        assert_eq!(parse("end\n"), Some(ChatCmd::End));
        assert_eq!(parse(r#"chat "hi""#), Some(ChatCmd::Text("hi".into())));
        assert_eq!(parse("cancel-file 3"), Some(ChatCmd::Cancel(3)));
        assert_eq!(parse("end now"), None);
        assert_eq!(parse("quit"), None);
        assert_eq!(parse("accept\r\n"), Some(ChatCmd::Answer(true)));
        assert_eq!(parse("decline"), Some(ChatCmd::Answer(false)));
    }
}
