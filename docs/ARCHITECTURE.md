# Architecture

How Remote Desktop Rider is put together. Keep this page in step with
structural changes (new crates, protocol versions, new kinds of streams).

## Crates

| Crate | Role |
|---|---|
| `rider-core` | Shared by everything: identity and pinned TLS, the session protocol (`proto`), rendezvous client, HTTPS tunnel client, clipboard, file transfer, sharp-text tiles, TCP tunnel rules (`forward`), pixel conversion, diagnostics, translations |
| `rider-host` | The controlled Windows computer: screen capture, H.264 pipeline, input injection, the red banner, sessions, the Windows service and its SYSTEM session agent |
| `rider-client` | The viewer: session window, network session, decoder thread, toolbar, side panel (chat, tunnels), command line |
| `rider-app` | The desktop app: computer list, pairing, settings, one-time support window, the host's session panel, tray |
| `rider-media` | Media Foundation H.264 encoder (hardware first) and decoder |
| `rider-ui` | egui views shared by the app and the viewer (chat, tunnels, theme, software renderer) |
| `rider-server` | Rendezvous (registration, pairing codes, NAT hole punching), UDP relay, HTTPS tunnel endpoint, diagnostics log |

## A session

One QUIC connection between viewer and host, direct or through the server's
UDP relay (or the HTTPS tunnel when UDP is blocked). Both sides pin each
other's certificate fingerprint.

| Channel | Carries |
|---|---|
| Control stream (bidirectional, first one opened) | `ClientMsg` / `HostMsg`: input, permissions, quality, chat text, monitors, cursor |
| Unidirectional stream per video frame | One H.264 frame each; abandoned when late |
| Datagrams | Mouse movement |
| Further bidirectional streams | Start with a `BulkHeader` (from `rider/6`): a file, a clipboard image, a sharp-text refresh, or a TCP tunnel |

The host enforces every permission; the viewer is only told what it may do.

## Protocol versions

Negotiated by ALPN. Each version keeps everything from the ones before.

| Paired sessions | One-time support | Adds |
|---|---|---|
| `rider/1` | | Screen, input, text clipboard |
| `rider/2` | `rider-support/1` | Quality choice |
| `rider/3` | `rider-support/2` | Remote cursor shapes, local pointer drawing |
| `rider/4` | `rider-support/3` | Access levels (`HostMsg::Permissions`) |
| `rider/5` | `rider-support/4` | Chat and files on extra streams |
| `rider/6` | `rider-support/5` | `BulkHeader` streams: clipboard images, sharp text when still |
| `rider/7` | `rider-support/6` | TCP tunnels |
| `rider/8` | `rider-support/7` | `HostMsg::Tunnels(bool)`: whether the host allows tunnels (the viewer shows the Tunnels button only then) |
| `rider/9` | | Restart: `ClientMsg::Restart`, `HostMsg::Restarting { seconds }`, `HostMsg::RestartCancelled`; `HostMsg::WakeAddresses` (wired MACs, for Wake-on-LAN) |

The rendezvous server speaks `rider-rv/1` and `rider-rv/2`; `rider-rv/2` adds
Wake-on-LAN (`PeerMsg::Wake`, `ServerMsg::Wake`, `ServerMsg::WakeSent`). The
server sends `ServerMsg::Wake` only to hosts that negotiated `rider-rv/2`, and a
viewer sends `PeerMsg::Wake` only to a server that did.

Policy: a release must keep working with peers from the last two minor
versions. Every version above is still accepted today.

## TCP tunnels

The viewer listens on `127.0.0.1` only (`--forward [LOCAL_PORT:]HOST:PORT`
or the toolbar's Tunnels panel). For each local connection it opens a stream
with `BulkHeader::Forward { target }`. The host:

1. accepts only Full access sessions, and only when its `tcp_tunnels` setting
   is `ask` or `allow` (default `off`; support sessions are always off). It
   tells `rider/8` viewers with `HostMsg::Tunnels`; `rider/7` viewers still
   see the button but get a refusal;
2. resolves the target and connects only to itself or private addresses
   (loopback, 10/8, 172.16/12, 192.168/16, link-local, IPv6 ULA and
   link-local), never the internet;
3. asks the person at the host once per address per session
   (`tunnel_request` event, `tunnel-allow` / `tunnel-deny` answer, denied
   after 60 s) when the setting is `ask`;
4. replies `ForwardReply::Open` or a `Refusal`, then copies bytes both ways.

## Restart

Only paired sessions with Full access (`rider/9`) may ask; support sessions
never can. The host:

1. drops the request from any other access level or a support session;
2. if someone is signed in, tells the session panel (`restart` event, 15 s)
   and the viewer (`HostMsg::Restarting`); the person at the host can press
   Cancel (`restart-cancel`), which the viewer hears as
   `HostMsg::RestartCancelled`. With nobody signed in it restarts at once;
3. restarts with `InitiateShutdownW` (no forced closing of apps, so unsaved
   work is never thrown away).

The viewer treats the next disconnect as expected and keeps reconnecting for
up to 10 minutes, long enough for Windows to restart and the service to come
back.

## Wake-on-LAN

1. In a paired session (`rider/9`, never a support session) the host sends the
   MAC addresses of its wired adapters; the viewer saves them with that
   computer in `client.toml`.
2. While remote access is on, the host arms its wired adapters to wake the
   computer (`powercfg /deviceenablewake`).
3. "Wake up" on an offline computer sends a magic packet on the viewer's own
   network and asks the server (`PeerMsg::Wake`). The server remembers, in
   memory only, the public address each computer was last online from and
   which devices it allowed (the oldest of at most 10,000 go first). If the
   asking device is one of them, it passes `ServerMsg::Wake` to every online
   `rider-rv/2` host behind the same public address, which broadcasts the
   magic packet on its own network.

A server restart forgets where sleeping computers were; they are known again
the next time they come online.

## Paths to the server

| Path | When |
|---|---|
| Direct UDP | Both sides reachable (LAN first, then hole punching) |
| UDP relay (`rider-server`, ports 47901-47999) | No direct path |
| HTTPS tunnel (WebSocket on `/rider-tunnel` behind nginx) | UDP blocked (corporate networks, VPNs) |
