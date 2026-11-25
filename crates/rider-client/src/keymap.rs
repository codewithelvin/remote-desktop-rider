use winit::keyboard::KeyCode;

const STANDARD: &[(KeyCode, u16)] = &[
    (KeyCode::Escape, 0x01),
    (KeyCode::Digit1, 0x02),
    (KeyCode::Digit2, 0x03),
    (KeyCode::Digit3, 0x04),
    (KeyCode::Digit4, 0x05),
    (KeyCode::Digit5, 0x06),
    (KeyCode::Digit6, 0x07),
    (KeyCode::Digit7, 0x08),
    (KeyCode::Digit8, 0x09),
    (KeyCode::Digit9, 0x0A),
    (KeyCode::Digit0, 0x0B),
    (KeyCode::Minus, 0x0C),
    (KeyCode::Equal, 0x0D),
    (KeyCode::Backspace, 0x0E),
    (KeyCode::Tab, 0x0F),
    (KeyCode::KeyQ, 0x10),
    (KeyCode::KeyW, 0x11),
    (KeyCode::KeyE, 0x12),
    (KeyCode::KeyR, 0x13),
    (KeyCode::KeyT, 0x14),
    (KeyCode::KeyY, 0x15),
    (KeyCode::KeyU, 0x16),
    (KeyCode::KeyI, 0x17),
    (KeyCode::KeyO, 0x18),
    (KeyCode::KeyP, 0x19),
    (KeyCode::BracketLeft, 0x1A),
    (KeyCode::BracketRight, 0x1B),
    (KeyCode::Enter, 0x1C),
    (KeyCode::ControlLeft, 0x1D),
    (KeyCode::KeyA, 0x1E),
    (KeyCode::KeyS, 0x1F),
    (KeyCode::KeyD, 0x20),
    (KeyCode::KeyF, 0x21),
    (KeyCode::KeyG, 0x22),
    (KeyCode::KeyH, 0x23),
    (KeyCode::KeyJ, 0x24),
    (KeyCode::KeyK, 0x25),
    (KeyCode::KeyL, 0x26),
    (KeyCode::Semicolon, 0x27),
    (KeyCode::Quote, 0x28),
    (KeyCode::Backquote, 0x29),
    (KeyCode::ShiftLeft, 0x2A),
    (KeyCode::Backslash, 0x2B),
    (KeyCode::KeyZ, 0x2C),
    (KeyCode::KeyX, 0x2D),
    (KeyCode::KeyC, 0x2E),
    (KeyCode::KeyV, 0x2F),
    (KeyCode::KeyB, 0x30),
    (KeyCode::KeyN, 0x31),
    (KeyCode::KeyM, 0x32),
    (KeyCode::Comma, 0x33),
    (KeyCode::Period, 0x34),
    (KeyCode::Slash, 0x35),
    (KeyCode::ShiftRight, 0x36),
    (KeyCode::NumpadMultiply, 0x37),
    (KeyCode::AltLeft, 0x38),
    (KeyCode::Space, 0x39),
    (KeyCode::CapsLock, 0x3A),
    (KeyCode::F1, 0x3B),
    (KeyCode::F2, 0x3C),
    (KeyCode::F3, 0x3D),
    (KeyCode::F4, 0x3E),
    (KeyCode::F5, 0x3F),
    (KeyCode::F6, 0x40),
    (KeyCode::F7, 0x41),
    (KeyCode::F8, 0x42),
    (KeyCode::F9, 0x43),
    (KeyCode::F10, 0x44),
    (KeyCode::ScrollLock, 0x46),
    (KeyCode::Numpad7, 0x47),
    (KeyCode::Numpad8, 0x48),
    (KeyCode::Numpad9, 0x49),
    (KeyCode::NumpadSubtract, 0x4A),
    (KeyCode::Numpad4, 0x4B),
    (KeyCode::Numpad5, 0x4C),
    (KeyCode::Numpad6, 0x4D),
    (KeyCode::NumpadAdd, 0x4E),
    (KeyCode::Numpad1, 0x4F),
    (KeyCode::Numpad2, 0x50),
    (KeyCode::Numpad3, 0x51),
    (KeyCode::Numpad0, 0x52),
    (KeyCode::NumpadDecimal, 0x53),
    (KeyCode::IntlBackslash, 0x56),
    (KeyCode::F11, 0x57),
    (KeyCode::F12, 0x58),
    (KeyCode::NumpadEqual, 0x59),
    (KeyCode::F13, 0x64),
    (KeyCode::F14, 0x65),
    (KeyCode::F15, 0x66),
    (KeyCode::F16, 0x67),
    (KeyCode::F17, 0x68),
    (KeyCode::F18, 0x69),
    (KeyCode::F19, 0x6A),
    (KeyCode::IntlRo, 0x73),
    (KeyCode::IntlYen, 0x7D),
    (KeyCode::NumpadComma, 0x7E),
    (KeyCode::Pause, 0x45),
];

const EXTENDED: &[(KeyCode, u16)] = &[
    (KeyCode::NumLock, 0x45),
    (KeyCode::NumpadEnter, 0x1C),
    (KeyCode::ControlRight, 0x1D),
    (KeyCode::NumpadDivide, 0x35),
    (KeyCode::PrintScreen, 0x37),
    (KeyCode::AltRight, 0x38),
    (KeyCode::Home, 0x47),
    (KeyCode::ArrowUp, 0x48),
    (KeyCode::PageUp, 0x49),
    (KeyCode::ArrowLeft, 0x4B),
    (KeyCode::ArrowRight, 0x4D),
    (KeyCode::End, 0x4F),
    (KeyCode::ArrowDown, 0x50),
    (KeyCode::PageDown, 0x51),
    (KeyCode::Insert, 0x52),
    (KeyCode::Delete, 0x53),
    (KeyCode::SuperLeft, 0x5B),
    (KeyCode::Meta, 0x5B),
    (KeyCode::SuperRight, 0x5C),
    (KeyCode::ContextMenu, 0x5D),
    (KeyCode::AudioVolumeMute, 0x20),
    (KeyCode::AudioVolumeDown, 0x2E),
    (KeyCode::AudioVolumeUp, 0x30),
    (KeyCode::MediaPlayPause, 0x22),
    (KeyCode::MediaTrackNext, 0x19),
    (KeyCode::MediaTrackPrevious, 0x10),
    (KeyCode::MediaStop, 0x24),
];

pub fn scancode(key: KeyCode) -> Option<(u16, bool)> {
    let find = |table: &[(KeyCode, u16)]| table.iter().find(|(k, _)| *k == key).map(|&(_, sc)| sc);
    find(STANDARD).map(|sc| (sc, false)).or_else(|| find(EXTENDED).map(|sc| (sc, true)))
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
                if raw == 0x54 || code == KeyCode::Pause || code == KeyCode::Meta {
                    continue;
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
