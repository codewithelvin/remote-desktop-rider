# Enterprise mode: a guide for IT

Enterprise mode lets one admin see and reach all of a company's computers,
organised by floor and room (for example 3rd floor > room 310). The admin
asks to connect; the person at the computer accepts or declines. Computers
that nobody sits at (servers, kiosks) can be set up so the admin connects
without asking.

Employees keep the normal app. Only the admin's app shows the company.

## What you need

- A server for the company: Linux with a public or company-wide address,
  running `rider-server` (UDP 47900 and 47901-47999 open). Install it with:

  ```sh
  curl -fsSL https://codewithelvin.github.io/remote-desktop-rider/install.sh | sudo bash
  ```

- The Windows installer, `rdr-setup-<version>.exe`, from the
  [releases page](https://github.com/codewithelvin/remote-desktop-rider/releases).
- For many computers: a Windows domain (Active Directory) and Group Policy,
  explained below.

## 1. Create the company on the server

```sh
sudo rider-server --state-dir /var/lib/rider-server company init --name "Your Company"
```

It prints an **admin setup code**, valid for 24 hours and for one use. For a
new one later: `company setup-code`. `rider-server --state-dir
/var/lib/rider-server company` shows the company, its admin and how many
computers it has.

## 2. The admin signs in

On the admin's computer, install the app as usual, then:

1. **Settings > Server**: the company server's address and fingerprint
   (`sudo rider-server --state-dir /var/lib/rider-server info` shows both).
2. **Settings > Enterprise mode > Sign in**, enter the setup code.

The app now has an **Enterprise** section. Its **Copy join token** button
gives the token that computers join with (it starts with `rdr1.`). Keep it
inside the company: any computer that has it can join the list.

## 3. Install on the company's computers

### A few computers, by hand

Run the installer from an administrator command prompt:

```bat
rdr-setup-0.4.3.exe /VERYSILENT /SUPPRESSMSGBOXES /ENROLL=rdr1.0123...
```

Add `/UNATTENDED` for computers nobody sits at (see below). The computer
appears in the admin's console under **Unassigned** within seconds.

### Many computers: Group Policy

**Group Policy** is the Windows domain's way of applying settings to many
computers at once. A **GPO** (Group Policy Object) is one named set of such
settings. You link it to a folder of computers in Active Directory (an OU,
organisational unit), and every computer in that folder applies it. One of
the things a GPO can do is run a script each time a computer starts, before
anyone signs in. That script installs Remote Desktop Rider.

1. **Put the files on a share the computers can read.** `NETLOGON` on your
   domain controller works without any setup: every computer can read it.
   Make a folder there and copy two files into it:
   - `rdr-setup.exe` (the installer, renamed without the version),
   - `rdr-startup.ps1` (from `packaging/windows/enterprise/` in this
     repository).

   For example `\\yourdomain.local\NETLOGON\rdr\`.

2. **Create the GPO.** Open **Group Policy Management** (on the domain
   controller, or any PC with the admin tools). Right-click the OU that holds
   the company's computers > **Create a GPO in this domain, and Link it
   here...**, name it "Remote Desktop Rider".

3. **Add the startup script.** Right-click the new GPO > **Edit**, then go to
   **Computer Configuration > Policies > Windows Settings > Scripts
   (Startup/Shutdown) > Startup**, tab **PowerShell Scripts > Add...**:
   - Script name: `\\yourdomain.local\NETLOGON\rdr\rdr-startup.ps1`
   - Script parameters:
     `-Installer \\yourdomain.local\NETLOGON\rdr\rdr-setup.exe -Token rdr1.0123...`
     (add ` -Unattended` for a GPO of computers nobody sits at)

4. **Recommended:** in the same GPO, **Computer Configuration > Policies >
   Administrative Templates > System > Logon > Always wait for the network at
   computer startup and logon: Enabled**, so the share can be reached.

5. **Restart a test computer** (or run `gpupdate /force` on it, then restart).
   It installs the app, turns on remote access and joins the company. Check
   that it shows up in the console, then let the other computers restart in
   their own time.

The script runs at every start. When everything is in place it does nothing
(a second or two). It writes what it did to `C:\Windows\Temp\rdr-startup.log`.

## 4. Updates

The app updates itself, like any Remote Desktop Rider installation. To roll a
version out yourself, put the newer installer on the share as `rdr-setup.exe`:
each computer installs it at its next start. The script never installs an
older version over a newer one.

## 5. Arrange the computers

In the admin's console, new computers wait under **Unassigned**. Add floors
and rooms (**+ Floor**, then **+ Room** on a floor), and drag computers onto
rooms. Right-click a computer for more: move it, switch unattended access,
or remove it from the list.

### Needs attention

**Needs attention** at the top of the tree lists the computers to look at,
each with the reason:

- **Not seen for N days**: the computer has not checked in for 14 days or
  more (switched off, moved, or the app removed).
- **Outdated**: it runs an older version (0.8 while the newest is 0.9) than the
  newest one among your computers and your own app. Computers update
  themselves, so this usually means its updates are failing. Smaller gaps
  (0.9.0 against 0.9.1) do not count.

A computer leaves the list by itself once it is back or updated.

### In the office or remote

A laptop taken home still reaches the company's server over the company VPN
(GlobalProtect, AnyConnect, ...), so you can help it there too. The console
marks it **Remote**, and the header counts how many are ("6 of 7 computers
online, 2 remote"); hover over a computer to see what it went by.

Each computer works this out itself every 15 seconds, from the networks
Windows sees:

- **In the office**: it finds its Windows domain on its own Wi-Fi or
  Ethernet (Windows calls it a "Domain network"), even with the VPN
  connected too.
- **Remote**: it reaches the domain only through a VPN's adapter (for
  GlobalProtect, "PANGP Virtual Ethernet Adapter"), or not at all.
- Computers that are not joined to a domain and use no VPN show nothing.

To check one computer, run `rider-host info` there: the `Place:` line says
what it would report. Needs version 0.6 on the server, the computers and the
admin's app; older ones keep working and show nothing.

## Connecting

Double-click a computer (or right-click > **Connect**). On that computer the
signed-in person sees who is asking, with **Accept** and **Decline**. Nothing
happens unless they accept; with no answer in 60 seconds it counts as
declined. After they accept, the same window is the session's chat, and they
can end the session at any time. If nobody is signed in, the connection is
refused.

### Unattended computers

The admin can connect without anyone accepting only when **both** are true:

- the computer was installed with `/UNATTENDED` (or the script with
  `-Unattended`), so the company decided this per group of computers, and
- the admin switched **Unattended** on for that computer in the console
  (right-click).

Either one alone is not enough.

## Taking a computer out

On the computer, as administrator: `"C:\Program Files\RemoteDesktopRider\rider-host.exe" leave-company`.
Its admin can no longer connect. Then remove it from the list in the console
(right-click > **Remove from the list**). Uninstalling the app also ends
remote access on that computer. Take it out of the GPO's OU too, or the
script installs and joins it again at the next start.

## When something does not work

| Where | What |
| --- | --- |
| `C:\Windows\Temp\rdr-startup.log` | What the startup script did at each start |
| `C:\ProgramData\RemoteDesktopRider\host\enroll.log` | The result of joining, from the installer |
| `rider-host.exe info` (in `C:\Program Files\RemoteDesktopRider`) | The computer's server, company, remote access |
| `journalctl -u rider-server` | The server's side |

Common causes: the share is not readable by computer accounts (use
NETLOGON, or give **Domain Computers** read access), the computers cannot
reach the server on UDP 47900, or the token was copied incompletely.
