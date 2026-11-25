# macOS support: plan

Goal: the same one app on a Mac, in both roles. A Mac can **control** other
computers (viewer) and **be controlled** (host), including quick connect by
one-time code. The wire protocol, rendezvous server, pairing, PIN and
end-to-end encryption are already platform-neutral, so a Mac interoperates
with Windows peers from day one.

## What is ready

| Part | State |
|---|---|
| `rider-core`: identity, pinned TLS, protocol, rendezvous, NAT traversal, diagnostics, i18n | portable; **built and tested on macOS in CI** (`macos` job) |
| `rider-server` | portable; built and tested on macOS in CI (Linux release binary as before) |
| Per-user data folder | `~/Library/Application Support/RemoteDesktopRider/<role>` (`rider_core::data_dir`) |
| Device name | `gethostname` (`mac-mini.local` → `mac-mini`) |
| LAN candidates for hole punching | `getifaddrs` on macOS and Linux |
| Clipboard | `arboard` (already cross-platform) |
| Interface language (EN/AZ) | `rider_core::i18n`, system language from `LANG`/`LC_ALL` |
| **Viewer and app (step 1, done)** | `rider-app` + `rider-client` build for macOS (universal, Metal) and Linux (`.deb`, OpenGL): OpenH264 software decoder (`rider-media/soft.rs`), toolbar labels with ab_glyph and egui's font, keys mapped to Windows scancodes (`rider-client/keymap.rs`), PINs in the Keychain (Linux: 0600 files), server set in Settings. Packaged by `packaging/macos/build.sh` (ad-hoc signed `.dmg`) and `packaging/linux/build-deb.sh` |
| Not yet portable | HTTPS tunnel (WinHTTP + system proxy), self-update (WinHTTP), tray / menu-bar icon, quality popup menu (cycles on click instead); see below |

## What needs a macOS implementation

Each Windows-only piece and its macOS counterpart:

| Windows module | Does | macOS counterpart |
|---|---|---|
| `rider-host/capture` (DXGI Desktop Duplication, GDI fallback) | screen frames | **ScreenCaptureKit** (`SCStream`, macOS 12.3+), BGRA or NV12 straight from the stream |
| `rider-media` encoder (Media Foundation H.264, QuickSync/NVENC/AMF) | low-latency H.264 | **VideoToolbox** `VTCompressionSession`: real-time, no B-frames (`AllowFrameReordering = false`), `AverageBitRate` changed live |
| `rider-media` decoder (MF) | H.264 → NV12 | **VideoToolbox** `VTDecompressionSession` |
| `rider-host/input` (`SendInput`) | mouse, keys, wheel | **CGEvent** (`CGEventCreateMouseEvent`, `CGEventCreateKeyboardEvent`, `CGEventPost`); needs a Windows scancode → macOS virtual keycode table |
| `rider-host/banner` (layered Win32 window, hidden from capture) | "being controlled" bar | borderless `NSPanel` at the status-bar level, `sharingType = .none` so capture does not see it |
| `rider-host/service` (Windows service + SYSTEM agent in the console session) | unattended access, login screen | **launchd**: a LaunchAgent per user session for capture/input. The login window needs a LaunchDaemon plus a pre-login agent (`LimitLoadToSessionType = LoginWindow`): phase 2 |
| `rider-host/system` (`SendSAS`, ACLs, elevation) | Ctrl+Alt+Del, locked-down state | no SAS on macOS; state in `/Library/Application Support/RemoteDesktopRider` (root, 0700); elevation through the installer / `SMAppService` |
| `rider-client/keyhook` (low-level keyboard hook) | Win keys to the remote | `CGEventTap` while the session window is focused (Cmd → Win mapping) |
| `rider-client/toolbar` (GDI-rendered sprite) | session toolbar | draw with the same softbuffer path (tiny-skia + a bundled font) instead of GDI, which makes it portable everywhere |
| `rider-client/app` (winit + softbuffer) | session window | already portable libraries; only the toolbar and keyhook above are Windows-only |
| `rider-app` (eframe on wgpu **DX12**) | app window | enable wgpu's **Metal** backend for macOS (`wgpu` feature `metal`) |
| `rider-app/tray` (Shell_NotifyIcon) | tray icon | `NSStatusItem` (or the `tray-icon` crate) |
| `rider-app/pins` (DPAPI) | remembered PINs | **Keychain** (`security-framework` crate) |
| `rider-app/sys` (UAC relaunch, Run key, service state) | Manage, autostart | `SMAppService` (login item, agents), `launchctl print` for state |
| `rider-core/tunnel`, `rider-core/update` (WinHTTP) | HTTPS fallback, downloads | a portable HTTP stack (`hyper` + `rustls` with `rustls-platform-verifier`, proxy from `CFNetworkCopySystemProxySettings`) |
| packaging (Inno Setup) | installer | signed **.app** in a notarized **.dmg** (Developer ID); updates via the same GitHub releases with a `.dmg` asset |

## Permissions the user must grant (host)

macOS asks once per app, in System Settings → Privacy & Security:

- **Screen Recording**: ScreenCaptureKit.
- **Accessibility**: posting mouse and keyboard events.
- **Input Monitoring**: the viewer's key tap (only to send Cmd/Option combinations to the remote).

The app must explain this up front ("This Mac can't be controlled until you allow…")
and link to the right pane (`x-apple.systempreferences:` URLs). Quick connect
needs the same permissions; without them it must say so before showing a code.

## Code structure

1. Keep the session logic platform-neutral: `rider-host/session.rs` talks to
   traits instead of Windows modules:
   `trait Capturer { fn monitors(); fn next_frame(); }`,
   `trait Encoder { fn encode(); fn set_bitrate(); fn request_keyframe(); }`,
   `trait Injector { fn send(InputEvent); }`, `trait Banner`.
   Windows implementations move under `platform/windows/`, macOS ones under
   `platform/macos/`, chosen with `#[cfg(target_os = …)]`.
2. `rider-media` gets `Encoder` / `Decoder` traits with `mf` (Windows) and
   `vt` (VideoToolbox) implementations.
3. Input on the wire stays in Windows scancodes (the protocol is already
   deployed); the macOS host translates to virtual keycodes, the macOS viewer
   translates its keycodes to scancodes.

## Order of work

1. **Viewer on macOS** (control Windows PCs from a Mac): portable toolbar,
   Keychain PINs, Metal app window, `.dmg`. **Done**, with the OpenH264
   software decoder; a VideoToolbox (hardware) decoder is a later speed-up.
   The same code ships for Debian/Ubuntu.
2. **Quick-connect host on macOS** (get help on a Mac): ScreenCaptureKit,
   VideoToolbox encoder, CGEvent input, banner, permission onboarding. No
   service needed: like on Windows it runs as the signed-in user.
3. **Unattended macOS host**: launchd agent/daemon, login window, auto-update.
4. CI: build and test the macOS app on `macos-latest`, sign and notarize on
   release (needs an Apple Developer ID; secrets in GitHub).
