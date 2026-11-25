use rider_core::proto::{CursorShape, HostMsg};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PCWSTR;

const POLL: Duration = Duration::from_millis(40);
const DESKTOP_RECHECK: Duration = Duration::from_millis(250);

pub fn spawn(out: UnboundedSender<HostMsg>, stop: Arc<AtomicBool>) {
    let spawned = std::thread::Builder::new().name("rider-cursor".into()).spawn(move || {
        let known = standard_cursors();
        let mut last = None;
        let mut checked = Instant::now() - DESKTOP_RECHECK;
        while !stop.load(Ordering::Relaxed) {
            if checked.elapsed() >= DESKTOP_RECHECK {
                crate::system::follow_input_desktop();
                checked = Instant::now();
            }
            if let Some(shape) = current(&known)
                && last != Some(shape)
            {
                last = Some(shape);
                if out.send(HostMsg::Cursor { shape: shape as u8 }).is_err() {
                    break;
                }
            }
            std::thread::sleep(POLL);
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("cursor shapes unavailable: {e}");
    }
}

fn standard_cursors() -> Vec<(HCURSOR, CursorShape)> {
    use CursorShape::*;
    let table: [(PCWSTR, CursorShape); 15] = [
        (IDC_ARROW, Arrow),
        (IDC_UPARROW, Arrow),
        (IDC_IBEAM, Text),
        (IDC_WAIT, Wait),
        (IDC_APPSTARTING, Progress),
        (IDC_CROSS, Crosshair),
        (IDC_HAND, Hand),
        (IDC_SIZEWE, ResizeWE),
        (IDC_SIZENS, ResizeNS),
        (IDC_SIZENWSE, ResizeNWSE),
        (IDC_SIZENESW, ResizeNESW),
        (IDC_SIZEALL, Move),
        (IDC_NO, NotAllowed),
        (IDC_HELP, Help),
        (IDC_PIN, Hand),
    ];
    table.into_iter().filter_map(|(id, shape)| unsafe { LoadCursorW(None, id) }.ok().map(|h| (h, shape))).collect()
}

fn current(known: &[(HCURSOR, CursorShape)]) -> Option<CursorShape> {
    let mut info = CURSORINFO { cbSize: size_of::<CURSORINFO>() as u32, ..Default::default() };
    unsafe { GetCursorInfo(&mut info) }.ok()?;
    Some(known.iter().find(|(h, _)| *h == info.hCursor).map_or(CursorShape::Arrow, |(_, s)| *s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_this_computers_cursor() {
        let known = standard_cursors();
        assert_eq!(known.len(), 15, "every system cursor loads");
        assert!(known.iter().any(|&(_, s)| s == CursorShape::ResizeWE));
        if let Some(shape) = current(&known) {
            assert_eq!(CursorShape::from_u8(shape as u8), shape);
        }
    }
}
