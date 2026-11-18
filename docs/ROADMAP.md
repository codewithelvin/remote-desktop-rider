# Development plan

## Target experience

One app, **Remote Desktop Rider**, installed on every PC. Example: PC A at
work, PC B at home.

**One-time setup, at PC A (work)**

1. Install the app, choose a PIN.
2. Switch on **"Allow remote access"**. PC A is then reachable after reboots,
   at the login screen and with nobody logged in.
3. Click **"Add a device"**. PC A waits up to 10 minutes.
4. On PC B click **"Add computer"** and pick or enter PC A.
5. Both screens show the same 6-digit code (e.g. `482 913`). Compare and
   click **Yes** on both. They now trust each other permanently.

**Daily use, at home (PC B)**

1. Open the app, click **"PC A (work)" → Connect**, type the PIN.
2. PC A's screen appears in a window; mouse, keyboard, clipboard and file drop
   work as if sitting there. Ctrl+Alt+End sends Ctrl+Alt+Del (login screen).
3. PC A shows a red "being controlled by PC B" bar; clicking it ends the session.

**The other way, at work (PC A → PC B):** same steps mirrored. Enable remote
access on PC B, add PC A once, then connect from PC A.

This must work between two different networks (home ↔ office), behind
routers and NAT, **with no VPN, port forwarding or router setup**.

## Status

| Area | State |
|---|---|
| QUIC transport, pinned mutual TLS, pairing code, PIN, banner | done |
| Video pipeline (DXGI → NV12 → H.264 MF, adaptive bitrate, frame deadlines) | done |
| Input, clipboard, multi-monitor, client→host file transfer | done |
| Unattended Windows service + SYSTEM session agent, login screen / UAC, Ctrl+Alt+End | done, verified on a clean Windows 11 VM |
| Static C runtime (runs on clean Windows without the VC++ redistributable) | done (found by VM test) |
| Connect across the internet (rendezvous / relay) | done; verified between VMs, real-NAT test open |
| One app with a window (no terminal) | **in progress**: app, tray and installer done; auto-update, session toolbar next |

## Milestone 1: Verify unattended access on a clean Windows (done, 2026-11-18)

A Vagrant VM (`test/vm/`, Windows 11 Enterprise eval on VirtualBox, no GPU
encoder) acted as PC A; this repo's client connected from the host machine.

- [x] Binary starts on a fresh Windows (bug found: needed the VC++ runtime, now statically linked)
- [x] `service install` elevated; service + SYSTEM agent in the console session
- [x] Pairing: both sides showed the same code; service paused and resumed around it
- [x] Desktop streams with the **software** H.264 encoder (bug found: an early
      FLUSH made it fail; video failures now end the session with a reason
      instead of a black window)
- [x] Lock screen (Win+L) captured; password typed remotely unlocks it
- [x] Ctrl+Alt+End → Windows security screen (bug found: `SendSAS` is ignored
      outside a service; the agent now asks the service via a SYSTEM-only event;
      numpad End also accepted)
- [x] Reboot: service back automatically; login screen with nobody logged on;
      remote login with mouse + keyboard
- [x] UAC prompt visible on the secure desktop; "Yes" clicked remotely; elevated
      console started
- [x] Upgrade in place (bug found: binary still locked by exiting processes; now
      waits/retries and renames aside as a fallback)
- [x] `service uninstall` leaves no service, process, binary, firewall rule or
      policy; pairings kept; state dir ACL = SYSTEM + Administrators only
- [ ] `set-pin` / `revoke` hot reload on the running service
- [ ] One unexplained failed login right after reboot (fast typing); did not
      reproduce. Watch for dropped keystrokes.
- [ ] Turn the manual steps into a scripted `test/vm/e2e.ps1`

## Milestone 2: Connect across the internet (rendezvous + relay)

Needed for the home ↔ work case: both routers block incoming connections.

```
 PC B (home) ──▶  rider-server (your VPS)  ◀── PC A (work)
       1. both keep an outgoing connection to the server
       2. B asks for A → server tells both each other's public address
       3. both send UDP at the same time → direct path through the NATs (hole punching)
       4. if that fails (strict NAT): traffic goes through the server's relay
```

- New crate `rider-server`: a small self-hosted binary (Linux and Windows,
  Docker image), one UDP port.
- Hosts register by fingerprint (signed with their key). Only devices paired
  with a host can ask the server to connect to it.
- UDP hole punching with QUIC on the same socket; IPv6 when available.
- Relay fallback forwards encrypted QUIC packets only. The server cannot read
  screens or keystrokes and cannot impersonate a host: keys are pinned at pairing.
- Pairing over the server too: the host shows a short one-time **pairing ID**
  (e.g. `RDR-7K3-QX9`) instead of needing its IP address.
- Client tries direct LAN → hole punch → relay, and shows which path is used.
- Server rate limiting and bans against scanning/abuse; health/metrics endpoint.

**Status: implemented.** Verified with `test/server-vm` (Ubuntu, systemd unit
from `deploy/`) and the Windows 11 VM:

- [x] Host registers and re-registers after a server restart (backoff, the same UDP port as sessions)
- [x] Pairing by code (`rider-client pair sf6-ck6`, any case), same verification code on both sides
- [x] Connect via the server picks the direct path (RTT 4 ms on the VM network)
- [x] Forced relay (`RIDER_FORCE_RELAY=1`) streams; relay adds 0.5 ms median (p99 3.9 ms, tcpdump)
- [x] Relay ignores datagrams without the session token (unit test)
- [x] Wrong code and unpaired devices get the same generic refusal
- [x] New pairing reaches the running service's server registration without restart
- [x] Server down: client tries the known direct address after 3 s

Still open:

- [ ] Test across two real NATs (home ↔ work, mobile hotspot) and a symmetric NAT
- [ ] IPv6 candidates (currently IPv4 only)
- [ ] Docker image; health/metrics endpoint; persistent bans
- [ ] Unused relay ports stay reserved for 30 s after a direct path wins; release them early
- [ ] Occasional ~700-900 ms latency spikes with the VM's software encoder (seen on direct and
      relayed paths alike: encoder/host-side, belongs to Milestone 4)

## Milestone 3: One app with a window

Merge `rider-host` and `rider-client` into one installer and one tray app
(the CLI stays for administrators and scripts).

- **Computers list:** name, online/offline (via the server), last connected,
  **Connect** button.
- **Add computer:** enter a pairing ID, show the 6-digit code, Yes/No.
- **This computer:** "Allow remote access" switch (installs/removes the
  service; one UAC prompt), set/change PIN, paired devices with **Remove**,
  "Add a device" (shows pairing ID + code).
- Session toolbar: monitors, fullscreen, Ctrl+Alt+Del, send file, disconnect,
  connection quality (path, RTT, bitrate).
- Host side: incoming-session notification and the red banner (exists).
- Installer (MSI or signed setup.exe), Start-menu entry, auto-start tray,
  auto-update from your server; code signing so SmartScreen does not warn.

**Part 1 (done):** `RemoteDesktopRider.exe` (egui on DirectX 12 / WARP).

- [x] Computers list with online dots (new server `Status` query that answers
      only for hosts that allow the asking device), last connected, Connect with
      PIN (viewer runs as a separate `rider-client` process), Remove
- [x] Add computer: pairing code or address, verification code, Yes/No
- [x] Settings: this device, server address + fingerprint
- [x] This computer: remote access state; "Manage" relaunches elevated (one UAC
      prompt) and drives `rider-host --json`: remote access on/off, PIN, devices
      with Remove, Add a device (code + verification), server
- [x] Host applies `set-server` live; `rider-client` is now a library + CLI
- [x] Production server `rdm.codex.az` (51.210.43.4) deployed with `deploy/install.sh`;
      direct and relayed sessions verified from the internet (relay adds ~0 ms
      over the 2x path latency)
- [x] Click-through test of every dialog, both sides (pairing by code, PIN,
      wrong PIN, Remove, Manage with UAC driven through a Rider session)
- [x] `rdm.codex.az` DNS switched to DNS only (UDP works); devices use the name, not the IP

**Part 2:**

- [x] Redesigned UI (sidebar, cards, Segoe icons), app icon + version resource
- [x] Tray icon (open, status, manage, quit); closing keeps it in the tray;
      start at sign-in (on by default for installed copies); single instance
- [x] Installer (Inno Setup, `packaging/windows`): all three programs in
      Program Files, Start menu, uninstall entry, firewall rules for
      `rider-host`/`rider-client`, stops and restarts the service and app on
      upgrade; tested install, upgrade over a service install and uninstall
- [x] GitHub Actions publishes every push to `main` as the next version
      (`v<major>.<minor>.<n>`) with the installer on the Releases page
- [ ] Auto-update from the GitHub releases (public, no server needed)
- [x] Session toolbar in the viewer (top edge): full screen, Ctrl+Alt+Del,
      one button per remote monitor, disconnect
- [x] "Remember PIN on this device" (DPAPI, per Windows user): Connect opens
      the session directly; a PIN that stopped working is forgotten
- [x] Win key combinations, Alt+Tab, Alt+Esc, Ctrl+Esc go to the remote
      computer (low-level keyboard hook while the session window is in front)
- [x] Bilinear scaling in the viewer (text stays readable at any window size);
      app icon on the session window and `rider-client.exe`
- [ ] All remote monitors at once in one window (needs host-side compositing)
- [ ] Code signing (needs a certificate) so SmartScreen does not warn

**Later:** pair through the running service instead of pausing it, so a device
can be added to a computer you are controlling remotely (today "Add a device"
ends remote sessions while it waits).

## Milestone 4: Performance on poor links

- GPU path: D3D11 BGRA→NV12 on the host, DXVA decode + D3D presentation on the
  client (removes ~5–10 ms of CPU conversion per frame).
- FEC for keyframes on lossy links; intra-refresh instead of full keyframes.
- Network emulation tests (latency 50–300 ms, 1–10 % loss, 1–5 Mbps) with
  latency/fps measurements kept in CI history.

## Milestone 5: More features

- Host→client file transfer and a file browser.
- Cursor shape sync.
- Audio.
- Session log / audit on the host (who connected, when, from where).
