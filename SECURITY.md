# Security policy

Remote Desktop Rider gives one computer control of another, so security
problems are taken seriously and fixed first.

## Supported versions

Only the [latest release](https://github.com/codewithelvin/remote-desktop-rider/releases/latest)
gets security fixes. Installed copies update themselves (hourly check,
SHA-256 verified), so a fix reaches everyone within hours of its release.

## Reporting a vulnerability

**Please do not open a public issue for a security problem.**

Report it privately on GitHub instead:
[Security → Report a vulnerability](https://github.com/codewithelvin/remote-desktop-rider/security/advisories/new).
Only the maintainer sees the report.

Please include:

- the version (`rider-host.exe info` or *Settings* in the app) and Windows,
  macOS or Linux version;
- what an attacker can do, and from where (same network, the internet, a
  paired device, the relay server, an enterprise admin);
- the steps to reproduce it, or a proof of concept.

What happens next:

- an answer within **3 days**;
- a fix, or a plan for one, within **14 days** for serious problems;
- a release with the fix, then a public advisory that credits you (unless you
  prefer to stay unnamed).

## What is in scope

- Connecting to or controlling a computer without its PIN, pairing, the
  person's **Allow**/**Accept**, or the enterprise admin's rights.
- Reading or changing session traffic (video, input, clipboard, chat, files),
  including by the rendezvous/relay server, which must never be able to.
- Taking over a computer through the host service (it runs as SYSTEM) or the
  installer and updater.
- The enterprise join token, the admin's setup code, and the company list.

## Design

How devices are trusted, how sessions are encrypted and what the server can
and can't see is described in the README under
[Security](README.md#security) and [How it connects](README.md#how-it-connects).
