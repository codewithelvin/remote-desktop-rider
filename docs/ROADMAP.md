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
- [x] Production server (e.g. `rdm.example.com`) deployed with `deploy/install.sh`;
      direct and relayed sessions verified from the internet (relay adds ~0 ms
      over the 2x path latency)
- [x] Click-through test of every dialog, both sides (pairing by code, PIN,
      wrong PIN, Remove, Manage with UAC driven through a Rider session)
- [x] Server DNS switched to DNS only (UDP works); devices use the name, not the IP

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
- [x] Auto-update from the GitHub releases (public, no server needed): the app
      offers new versions (one click, one UAC prompt, reopens itself); the
      service installs them unattended when no session is active and reopens
      the tray app; downloads over WinHTTP (system proxy), SHA-256 verified,
      SYSTEM's copy kept in an admin-only folder
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

- [x] Cursor shape sync.
- Host→client file transfer: now part of chat (Milestone 6).
- Audio.
- Session log / audit on the host (who connected, when, from where).

## Milestone 6: What other remote desktop apps have (from Remoteer, 2026-11-24)

Compared with Remoteer (Windows, macOS and browser; WebRTC). Small items
first. Voice and video calls are not planned.

| # | Feature | Size | State |
|---|---|---|---|
| 1 | Access levels: Full access / Standard / View only, changeable during the session | small | **done** for one-time sessions (unattended: with the session panel, #2) |
| 2 | Chat with file transfer both ways, nothing kept after the session | medium | **done** (one-time and paired sessions) |
| 3 | Clipboard with images (files go through chat) | small | planned |
| 4 | Sharp text when the screen is still (a lossless refresh after motion stops) | medium | planned |
| 5 | TCP tunnels through the session (reach a port on the remote network) | medium | planned |
| 6 | Browser client (phones included) | large | later |
| 7 | macOS as the controlled computer | large | later |

### 1. Access levels

The person at the controlled computer decides what the other side may do:

| Level | See the screen | Mouse and keyboard, Ctrl+Alt+Del, lock | Clipboard | Send files to this computer |
|---|---|---|---|---|
| **Full access** | yes | yes | yes | yes |
| **Standard** | yes | yes | yes | no |
| **View only** | yes | no | no | no |

- Chosen when allowing a one-time session (Full access preselected), changed
  at any time while it runs. Paired, PIN-protected sessions start with Full
  access; the host-side session panel (chat, below) can lower it.
- The **host enforces** it: input, clipboard and files are dropped there, so a
  modified viewer gains nothing. When control is taken away, keys and mouse
  buttons the viewer was holding are released.
- The viewer is told (`HostMsg::Permissions`, protocol `rider/4`) and shows
  it: a "View only" badge in the toolbar, Ctrl+Alt+Del hidden, a notice when
  the level changes, nothing sent that would be dropped.

### 2. Chat with files

Goals: polished, always where you expect it, and **nothing survives the
session** on either computer.

**Viewer side: a panel inside the session window**, not a separate window
(a separate window falls behind a full-screen session and gets lost).

- A **Chat** button on the toolbar with an unread count. The panel slides in
  on the right (about 340 px); the remote picture shrinks to the rest of the
  window, so the panel never covers the remote screen and works in full
  screen. Closing it gives the space back.
- A message arriving while the panel is closed shows in the title bar (name
  + first line), the toolbar slides in for a few seconds and its **Chat**
  button lights up with the unread count. `Ctrl+Alt+Shift+C` opens and
  closes the panel.
- Keyboard: while the message box has focus, keys type there instead of going
  to the remote computer (Esc or a click on the picture gives them back).
- Files: the attach button, or drag files onto the panel. Dropping a file on
  the picture sends it through chat too (it shows up in the conversation).
- Drawn with egui (the same widgets and look as the app), rendered on the CPU
  into the viewer's frame buffer next to the video: no GPU dependency, the
  same on Windows, macOS and Linux.

**Controlled side: a session panel**, a small window of the app running as
the signed-in user (never as SYSTEM: file dialogs and file access must have
the user's rights, not the service's).

- One-time sessions: while someone is connected, the app's whole window is
  the session: who is connected, the access level (a dropdown) and **End
  session** at the top, the chat below. A message arriving while the app is
  in the background flashes its taskbar button and shows a notification.
- Paired (unattended) sessions, as built: the service agent starts
  `RemoteDesktopRider --session-panel` in the console session with the
  signed-in user's token and environment, linked by two anonymous pipes
  (only those two handles are passed on). It waits minimized in the
  taskbar (it would sit in the remote picture otherwise) and comes up,
  without taking the keyboard, when the viewer writes or sends a file.
  Header: who is connected and **End session**; closing it only minimizes
  it. Nobody signed in, or the window closed: the viewer is told
  (`ChatReady(false)`). Files are opened and written with the user's
  rights, never the agent's.
- Paired sessions are the owner's own devices: received files go to
  `Downloads\Rider` and are kept (as before chat); one-time sessions keep
  nothing.
- Later maybe: a **Chat** part on the red banner.

**Messages and files**

- Text up to 4000 characters, sent on the control stream; files on their own
  stream each (any size, progress and cancel on both sides). Each message
  shows time and sender; files show name, size, progress, then **Open** and
  **Save as…**
- Files are only allowed in both directions while the access level includes
  files (Full access); the person at the computer can always send.

**Nothing survives the session**

- Messages live only in memory in the two processes; never written to disk,
  never logged (logs record counts, not text or file names), never via the
  server (the session is end-to-end encrypted).
- Received files go to a private per-session folder in the user's temp
  directory, marked as downloaded from the internet (Mark of the Web, so
  Office and SmartScreen treat them with care). The folder is deleted when
  the session ends; leftovers from a crash are deleted at the next start.
  **Save as…** is the only way to keep a file, and it is the user's choice.
- Drag-and-drop to `Downloads\Rider` (which kept files) is replaced by chat.

**Protocol (`rider/5`, `rider-support/4`):** `ClientMsg::Chat` /
`HostMsg::Chat` (text), `HostMsg::ChatReady` (whether someone at the host
sees the chat: the viewer shows the Chat button only then), file streams in
both directions (the host also opens streams), and `FileResult` answers with
a reason. Without someone at the host (unattended, until the session panel),
files dropped on the window still go to `Downloads\Rider`. Control messages a peer does not know are
skipped (from `rider/4` on), so later additions cannot break a session.

## Milestone 7: Enterprise mode (design agreed 2026-11-25; released in 0.6.0)

One admin looks after a company's computers (1000+) from the same app.
The core (sessions, video, chat, security) is unchanged; what is new is a
company list on the server and a console for it.

**Two kinds of install, same app**

- Employee computers stay **plain clients**: no console, nothing new to learn.
- The admin turns on **Enterprise mode** in the app by **signing in to the
  company server** with a **one-time setup code** (printed by
  `rider-server company init` on the server; no password, no license key).
  That app's own key becomes the admin; the Enterprise section appears.
- **One admin** to start with (no roles or scopes yet).

**How computers join**

- The company's computers are domain-joined, so IT pushes RDR to them with
  Group Policy: a ready **startup script** runs the normal installer
  silently with the company's join token
  (`rdr-setup.exe /VERYSILENT /ENROLL=<token>`); no MSI. Updates then come
  through the app's own updater.
- The admin downloads a **company installer** from Enterprise mode: the
  normal installer with the company's join token inside. Pushed by Group
  Policy or run by hand, it installs RDR and the PC joins the company list
  by itself.
- A new computer appears under **Unassigned**. The admin places every
  computer **by hand** (drag to a room). No automatic placement from AD,
  names or subnets.

**What the admin sees**

- Left: a tree **Floor → Room** (e.g. Floor 3 → Room 310), each with
  online/total counts; plus search (computer, user, IP) and **Problems**
  (outdated, service stopped, not seen for days).
- Right: the room's computers as tiles: name, signed-in user, status
  (online, offline + last seen, locked, nobody signed in, in a session,
  waiting for an answer, declined). **No screen thumbnails**: the admin
  never sees a screen before the employee accepts.

**Connecting**

- Admin clicks **Connect**; the tile shows "waiting". The employee gets an
  **Accept / Decline** prompt naming the admin; no answer in ~60 s counts
  as declined. Accepted: a normal session with chat.
- **Unattended** (connect without asking, e.g. servers, kiosks): only when
  **both** agree: the computer was installed as unattended-capable (a
  separate company installer / policy value) **and** the admin turns it on
  for that computer in the console.

**Trust:** the join token carries the server's fingerprint and the admin
app's key fingerprint, so a computer pins its admin at install time: a
compromised server cannot swap in another admin.

**Phases**

1. **Built.** Company on the server (`rider-server company init`,
   `company setup-code`), admin sign-in with the setup code, Enterprise
   section in the app (Settings > Enterprise mode > Sign in).
2. **Built.** Joining: join token (`rdr1.…`), `rider-host enroll <token>
   [--unattended]` / `leave-company`; computers appear in Unassigned with
   name, signed-in user, locked, version, online / last seen.
3. **Built** (Problems list still to do). Console: floors and rooms with
   counts, tiles, search, drag a tile onto a room, right-click menu.
4. **Built.** Connect: double-click a tile (`rider-client company-connect`).
   The host skips the PIN for the pinned admin and asks the signed-in user
   in the chat window (Accept / Decline, 60 s, closing = Decline); after
   Accept the window is the session's chat. Nobody signed in: refused.
   Unattended only when the installer flag AND the admin's switch (sent by
   the server to the computer at once) agree. Admin commands without the
   app: `rider-client company sign-in|token|list|unattended`.
5. **Built.** Company installer and Group Policy: `rdr-setup.exe
   /VERYSILENT /ENROLL=<token> [/UNATTENDED]` joins and turns remote access
   on (a company computer needs no PIN);
   `packaging/windows/enterprise/rdr-startup.ps1` as a GPO startup script
   installs, updates (never downgrades) and joins, and does nothing once
   all is in place. Guide for IT: `docs/ENTERPRISE.md`.
6. **Built.** Office or remote: every 15 s a company computer reports
   whether its Windows domain is on its own network (office) or only
   through a VPN adapter such as GlobalProtect's (remote); the console
   marks remote computers and counts them (`PeerMsg::Place`, sent only
   after the server's `CompanyPolicy2`; the app asks `Directory2`).

**Open questions for later:** audit log of connections, floor-plan view,
bulk actions (message a room, push an update), more admins with scopes.
