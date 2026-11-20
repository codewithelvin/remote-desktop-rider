//! Keys to Windows scancodes (set 1), which is what hosts expect on the wire.
//!
//! On Windows winit already reports scancodes. On macOS and Linux it reports
//! the platform's own key codes, so keys are translated by their position on
//! the keyboard (`KeyCode`), not by the character they type: the host's
//! keyboard layout decides the character, as when sitting at it.
//! Command (macOS) and Super (Linux) are the Windows key.

use winit::keyboard::KeyCode;

/// (scancode, extended) for a physical key, if the host has one.
pub fn scancode(key: KeyCode) -> Option<(u16, bool)> {
    use KeyCode::*;
    let (sc, ext) = match key {
        Escape => (0x01, false),
        Digit1 => (0x02, false),
        Digit2 => (0x03, false),
        Digit3 => (0x04, false),
        Digit4 => (0x05, false),
        Digit5 => (0x06, false),
        Digit6 => (0x07, false),
        Digit7 => (0x08, false),
        Digit8 => (0x09, false),
        Digit9 => (0x0A, false),
        Digit0 => (0x0B, false),
        Minus => (0x0C, false),
        Equal => (0x0D, false),
        Backspace => (0x0E, false),
        Tab => (0x0F, false),
        KeyQ => (0x10, false),
        KeyW => (0x11, false),
        KeyE => (0x12, false),
        KeyR => (0x13, false),
        KeyT => (0x14, false),
        KeyY => (0x15, false),
        KeyU => (0x16, false),
        KeyI => (0x17, false),
        KeyO => (0x18, false),
        KeyP => (0x19, false),
        BracketLeft => (0x1A, false),
        BracketRight => (0x1B, false),
        Enter => (0x1C, false),
        ControlLeft => (0x1D, false),
        KeyA => (0x1E, false),
        KeyS => (0x1F, false),
        KeyD => (0x20, false),
        KeyF => (0x21, false),
        KeyG => (0x22, false),
        KeyH => (0x23, false),
        KeyJ => (0x24, false),
        KeyK => (0x25, false),
        KeyL => (0x26, false),
        Semicolon => (0x27, false),
        Quote => (0x28, false),
        Backquote => (0x29, false),
        ShiftLeft => (0x2A, false),
        Backslash => (0x2B, false),
        KeyZ => (0x2C, false),
        KeyX => (0x2D, false),
        KeyC => (0x2E, false),
        KeyV => (0x2F, false),
        KeyB => (0x30, false),
        KeyN => (0x31, false),
        KeyM => (0x32, false),
        Comma => (0x33, false),
        Period => (0x34, false),
        Slash => (0x35, false),
        ShiftRight => (0x36, false),
        NumpadMultiply => (0x37, false),
        AltLeft => (0x38, false),
        Space => (0x39, false),
        CapsLock => (0x3A, false),
        F1 => (0x3B, false),
        F2 => (0x3C, false),
        F3 => (0x3D, false),
        F4 => (0x3E, false),
        F5 => (0x3F, false),
        F6 => (0x40, false),
        F7 => (0x41, false),
        F8 => (0x42, false),
        F9 => (0x43, false),
        F10 => (0x44, false),
        NumLock => (0x45, true),
        ScrollLock => (0x46, false),
        Numpad7 => (0x47, false),
        Numpad8 => (0x48, false),
        Numpad9 => (0x49, false),
        NumpadSubtract => (0x4A, false),
        Numpad4 => (0x4B, false),
        Numpad5 => (0x4C, false),
        Numpad6 => (0x4D, false),
        NumpadAdd => (0x4E, false),
        Numpad1 => (0x4F, false),
        Numpad2 => (0x50, false),
        Numpad3 => (0x51, false),
        Numpad0 => (0x52, false),
        NumpadDecimal => (0x53, false),
        IntlBackslash => (0x56, false),
        F11 => (0x57, false),
        F12 => (0x58, false),
        NumpadEqual => (0x59, false),
        F13 => (0x64, false),
        F14 => (0x65, false),
        F15 => (0x66, false),
        F16 => (0x67, false),
        F17 => (0x68, false),
        F18 => (0x69, false),
        F19 => (0x6A, false),
        IntlRo => (0x73, false),
        IntlYen => (0x7D, false),
        NumpadComma => (0x7E, false),
        NumpadEnter => (0x1C, true),
        ControlRight => (0x1D, true),
        NumpadDivide => (0x35, true),
        PrintScreen => (0x37, true),
        AltRight => (0x38, true),
        Pause => (0x45, false),
        Home => (0x47, true),
        ArrowUp => (0x48, true),
        PageUp => (0x49, true),
        ArrowLeft => (0x4B, true),
        ArrowRight => (0x4D, true),
        End => (0x4F, true),
        ArrowDown => (0x50, true),
        PageDown => (0x51, true),
        Insert => (0x52, true),
        Delete => (0x53, true),
        SuperLeft | Meta => (0x5B, true),
        SuperRight => (0x5C, true),
        ContextMenu => (0x5D, true),
        AudioVolumeMute => (0x20, true),
        AudioVolumeDown => (0x2E, true),
        AudioVolumeUp => (0x30, true),
        MediaPlayPause => (0x22, true),
        MediaTrackNext => (0x19, true),
        MediaTrackPrevious => (0x10, true),
        MediaStop => (0x24, true),
        _ => return None,
    };
    Some((sc, ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_keys() {
        assert_eq!(scancode(KeyCode::KeyA), Some((0x1E, false)));
        assert_eq!(scancode(KeyCode::Enter), Some((0x1C, false)));
        assert_eq!(scancode(KeyCode::NumpadEnter), Some((0x1C, true)));
        assert_eq!(scancode(KeyCode::ArrowLeft), Some((0x4B, true)));
        assert_eq!(scancode(KeyCode::SuperLeft), Some((0x5B, true)), "Command is the Windows key");
        assert_eq!(scancode(KeyCode::Fn), None);
    }

    /// Same answer as winit's own Windows table for every key we map.
    #[cfg(windows)]
    #[test]
    fn matches_windows_scancodes() {
        use winit::keyboard::PhysicalKey;
        use winit::platform::scancode::PhysicalKeyExtScancode;
        let mut checked = 0;
        let mut wrong = Vec::new();
        for sc in 1u32..0x80 {
            for ext in [false, true] {
                let raw = if ext { 0xE000 | sc } else { sc };
                let PhysicalKey::Code(code) = PhysicalKey::from_scancode(raw) else { continue };
                let Some(ours) = scancode(code) else { continue };
                // Pause and Print Screen have two codes on Windows (with Ctrl / Alt).
                if raw == 0x54 || code == KeyCode::Pause || code == KeyCode::Meta {
                    continue; // Windows reports these through special sequences
                }
                if ours != (sc as u16, ext) {
                    wrong.push(format!("{code:?}: ours {ours:x?}, Windows ({sc:#x}, {ext})"));
                }
                checked += 1;
            }
        }
        assert!(wrong.is_empty(), "{wrong:#?}");
        assert!(checked > 90, "checked {checked}");
    }
}
