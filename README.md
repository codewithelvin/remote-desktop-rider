# Remote Desktop Rider

Fast, secure 1:1 remote desktop for Windows, built to stay responsive on
**bad networks**: high latency, packet loss and low bandwidth.

## Why it stays fast

| Problem on bad links | What Rider does |
|---|---|
| TCP head-of-line blocking (one lost packet freezes everything) | **QUIC over UDP**. Each video frame travels on its own stream, and mouse motion goes as unreliable datagrams. |
| Retransmitting stale pixels | Frames that are not delivered within 500 ms are **abandoned**. The client asks for a fresh keyframe instead of waiting. |
| Queues building up (bufferbloat) | **At most 3 frames in flight**. If the network is busy, capture is skipped, so the next frame is always *now*. |
| Random loss collapsing throughput | **BBR** congestion control plus a delay-based **adaptive bitrate** (300 kbps–20 Mbps, changed live without keyframes). |
| Encoder/decoder buffering | H.264 in **low-latency mode**: no B-frames, CBR, hardware encoder (QuickSync / NVENC / AMF) with software fallback. |
| Laggy pointer | The **cursor is drawn locally** on the client, so it never waits for the network. |
| Wasted bandwidth on static screens | **Zero traffic** when nothing changes, then a few refinement frames sharpen the image. |

## Security model

- Every device has its own **Ed25519 identity key**. There is no CA and no central
  server that could be compromised to impersonate a host.
- **One-time pairing:** the host opens a pairing window (`rider-host pair`). Both
  screens show a 6-digit **verification code** derived from both keys *and* the
  live TLS session. The two people compare the codes; a man-in-the-middle would
  produce different codes.
- After pairing, both sides **pin** each other's key. The host rejects unpaired
  devices **during the TLS handshake**, before any application code runs. The
  client refuses a host whose key changed.
- Every session also requires the host **PIN** (Argon2id-hashed, exponential
  back-off, locked for 15 minutes after 10 failures). A stolen client laptop is
  not enough on its own.
- While someone is connected, the host shows a **red banner**. The banner is
  hidden from the stream. Clicking it ends the session immediately.

## Usage

On the **host** (the machine to be controlled):

```powershell
rider-host set-pin           # once
rider-host pair              # once per new client device; then Ctrl+C
rider-host                   # normal operation: paired devices + PIN
```

Allow `rider-host.exe` through Windows Firewall (UDP 47800) when prompted.

On the **client**:

```powershell
rider-client pair 192.168.1.20 --name office   # compare the code with the host
rider-client connect office                     # asks for the PIN
```

In the viewer:

| Shortcut | Action |
|---|---|
| `Ctrl+Alt+Shift+M` | next monitor |
| `Ctrl+Alt+Shift+F` | toggle fullscreen |
| `Ctrl+Alt+Shift+Q` | disconnect |
| drag a file onto the window | send it to the host (`Downloads\Rider`) |

Text clipboard syncs both ways automatically. The title bar shows fps, RTT,
estimated capture-to-screen latency, decode/encode time and the active encoder.

## Layout

```
crates/
  rider-core    identity, pinned TLS, pairing code, protocol, clipboard, pixel conversion
  rider-media   Media Foundation H.264 encoder (hardware first) and decoder
  rider-host    capture (DXGI, GDI fallback), video pipeline, input injection, banner, sessions
  rider-client  CLI, network session, in-order decoder thread, viewer window
```

Build: `cargo build --release` (needs Rust stable and the MSVC build tools).

## Roadmap

1. **Unattended service.** A Windows service that runs the agent in the active
   session as SYSTEM, so it works on the login screen, UAC prompts and after
   reboots. It also moves the config to `%ProgramData%` with a SYSTEM-only ACL.
2. **Reachability.** A self-hosted rendezvous server with UDP hole punching, plus
   an end-to-end encrypted relay as fallback. The relay cannot read or MITM
   sessions because the keys are pinned.
3. **GPU path:** D3D11 BGRA→NV12 on the host, DXVA decode and D3D presentation on
   the client. This removes ~5–10 ms of CPU conversion per frame.
4. FEC for keyframes on lossy links; intra-refresh instead of keyframes.
5. Host→client file transfer, cursor shape sync, a low-level keyboard hook for
   Win/Alt+Tab, audio.
