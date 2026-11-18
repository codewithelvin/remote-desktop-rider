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

All host commands need an **elevated** terminal (Run as administrator): the
host's private key and trust store live in `%ProgramData%\RemoteDesktopRider`,
readable only by SYSTEM and Administrators.

On the **host** (the machine to be controlled):

```powershell
rider-host set-pin             # once
rider-host service install     # unattended access: login screen, UAC, after reboot
rider-host pair                # once per new client device (pauses the service meanwhile)
```

`service install` copies the binary to `%ProgramFiles%\RemoteDesktopRider`, so
only administrators can replace what runs as SYSTEM. It also adds a firewall
rule for UDP 47800, registers auto-restart on failure, and allows Ctrl+Alt+Del
from services. `rider-host service status|stop|start|uninstall` manage it, and
`set-pin`, `revoke` and `clients` apply to the running service immediately.
Logs: `%ProgramData%\RemoteDesktopRider\host\logs`.

Without the service, `rider-host` (or `rider-host run --pair`) serves from the
terminal until Ctrl+C.

On the **client**:

```powershell
rider-client pair 192.168.1.20 --name office   # compare the code with the host
rider-client connect office                     # asks for the PIN
```

In the viewer:

| Shortcut | Action |
|---|---|
| `Ctrl+Alt+End` | Ctrl+Alt+Del on the host (login / lock screen) |
| `Ctrl+Alt+Shift+M` | next monitor |
| `Ctrl+Alt+Shift+F` | toggle fullscreen |
| `Ctrl+Alt+Shift+Q` | disconnect |
| drag a file onto the window | send it to the host user's `Downloads\Rider` |

### Across the internet (home ↔ work)

Without a server, the client needs the host's address, so it only works on the
same network, over a VPN or through a port forward. To connect from anywhere,
run the small `rider-server` on any machine both sides can reach (a cheap VPS):

```sh
rider-server run                 # UDP 47900 + relay ports 47901-47999
rider-server info                # prints the fingerprint for the commands below
```

`deploy/rider-server.service` is a hardened systemd unit for it. Then, once per device:

```powershell
rider-host   set-server vps.example.com <server-fingerprint>   # host; restart the service after
rider-client set-server vps.example.com <server-fingerprint>
```

`rider-host pair` now also shows a short code (e.g. `SF6-CK6`); on the client,
`rider-client pair SF6-CK6` works from any network. `connect` asks the server
for the host's addresses, punches through both NATs and uses a direct path
when one works (LAN first), or the server's relay when it doesn't. The viewer
prints which path it chose. If the server is down, a known direct address is
still tried.

The server only introduces devices. Sessions stay end-to-end encrypted with
the keys pinned at pairing, so the server can't watch or take over a session,
and it can't impersonate a host (the verification code would differ when
pairing by code). It answers the same "offline or not allowed" to everyone who
isn't paired with a host and rate-limits requests per IP.

Text clipboard syncs both ways automatically. The title bar shows fps, RTT,
estimated capture-to-screen latency, decode/encode time and the active encoder.

## How unattended access works

A service running in session 0 can't see the screen. The `RemoteDesktopRider`
service (LocalSystem) therefore launches `rider-host agent` **as SYSTEM inside
the active console session** and relaunches it whenever that session changes
(logon, logoff, user switch) or the agent exits. The agent runs in a
kill-on-close job object, so it never outlives the service. Its capture and
input threads follow the *input desktop*, which is how they reach the login
screen, lock screen and UAC prompts. Ctrl+Alt+Del is generated with `SendSAS`.

## Layout

```
crates/
  rider-core    identity, pinned TLS, pairing code, protocol, clipboard, pixel conversion
  rider-media   Media Foundation H.264 encoder (hardware first) and decoder
  rider-host    capture (DXGI, GDI fallback), video pipeline, input injection, banner,
                sessions, Windows service + SYSTEM session agent
  rider-client  CLI, network session, in-order decoder thread, viewer window
  rider-server  rendezvous (registration, pairing codes, NAT hole punching) and UDP relay
deploy/         systemd unit for rider-server
test/           Vagrant VMs: Windows 11 host (vm/), Ubuntu rider-server (server-vm/)
```

Build: `cargo build --release` (needs Rust stable and the MSVC build tools).

## Roadmap

See [docs/ROADMAP.md](docs/ROADMAP.md) for the full plan, including the target
end-user experience. In short:

1. ~~Verify unattended access on a clean Windows VM~~: done (`test/vm/`).
2. ~~Connect across the internet: self-hosted rendezvous server, NAT hole punching,
   end-to-end encrypted relay fallback~~: done, tested between VMs; real NATs next.
3. One app with a window: computer list, Connect button, "Allow remote access",
   pairing by ID and code, installer.
4. Performance on poor links: GPU conversion/decoding, FEC, intra-refresh.
5. Host→client files, cursor shapes, Win-key capture, audio, audit log.
