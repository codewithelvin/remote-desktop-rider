#!/usr/bin/env bash
# One-line install or upgrade of rider-server (Remote Desktop Rider's
# rendezvous + relay server) from the latest GitHub release:
#
#   curl -fsSL https://codewithelvin.github.io/remote-desktop-rider/install.sh | sudo bash -s -- [PUBLIC_IP] [options]
#
# Options:
#   PUBLIC_IP              address peers use to reach the relay ports. Detected
#                          when left out; give it behind cloud NAT to be sure.
#   --domain NAME          also set up the HTTPS fallback for networks that
#                          block UDP: nginx + a Let's Encrypt certificate for
#                          NAME (DNS must already point here; TCP 80/443 open).
#   --email ADDRESS        contact for Let's Encrypt (with --domain).
#   --version X.Y.Z        install this release instead of the latest.
#
# Safe to run again (upgrades in place). The server key in
# /var/lib/rider-server is kept, so the fingerprint never changes.
#
# Everything is wrapped in main() and only runs once the whole script has
# downloaded, so a cut-off download never runs half a script.
set -euo pipefail

REPO="codewithelvin/remote-desktop-rider"

say() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
die() {
  printf '\033[1;31merror:\033[0m %s\n' "$*" >&2
  exit 1
}

usage() {
  sed -n '2,20p' "$0" 2>/dev/null | sed 's/^# \{0,1\}//' || true
  echo "usage: install.sh [PUBLIC_IP] [--domain NAME --email ADDRESS] [--version X.Y.Z]"
}

is_private_ipv4() {
  case "$1" in
    10.* | 192.168.* | 127.* | 169.254.*) return 0 ;;
    172.1[6-9].* | 172.2[0-9].* | 172.3[01].*) return 0 ;;
    100.6[4-9].* | 100.[7-9][0-9].* | 100.1[01][0-9].* | 100.12[0-7].*) return 0 ;; # CGNAT
    *) return 1 ;;
  esac
}

valid_ipv4() {
  local o='(0|[1-9][0-9]{0,2})' # no leading zeros: rider-server would reject them
  [[ "$1" =~ ^$o\.$o\.$o\.$o$ ]] || return 1
  local octet
  for octet in "${BASH_REMATCH[@]:1}"; do
    ((10#$octet <= 255)) || return 1
  done
}

# The public IPv4 peers will see: this machine's own address when it has a
# public one, otherwise ask an echo service (the machine is behind NAT).
detect_public_ip() {
  local src=""
  if command -v ip >/dev/null; then
    src=$(ip -4 route get 1.1.1.1 2>/dev/null | sed -n 's/.* src \([0-9.]*\).*/\1/p' | head -1)
  fi
  if [ -n "$src" ] && ! is_private_ipv4 "$src"; then
    echo "$src"
    return
  fi
  local url ip
  for url in https://api.ipify.org https://ifconfig.me/ip https://icanhazip.com; do
    ip=$(curl -4 -fsS --max-time 5 "$url" 2>/dev/null | tr -d '[:space:]') || continue
    if valid_ipv4 "$ip"; then
      echo "$ip"
      return
    fi
  done
}

# Expected SHA-256 of a release file: from the release's SHA256SUMS file, or
# (older releases) from the hash printed in the release notes.
expected_sha256() {
  local tag="$1" file="$2" sums body
  if sums=$(curl -fsSL "https://github.com/$REPO/releases/download/$tag/SHA256SUMS" 2>/dev/null); then
    awk -v f="$file" '$2 == f || $2 == "*" f { print $1 }' <<<"$sums" | head -1
    return
  fi
  body=$(curl -fsSL -H "Accept: application/vnd.github+json" "https://api.github.com/repos/$REPO/releases/tags/$tag" 2>/dev/null) || return 0
  grep -o "${file//./\\.}\*\*, SHA-256: \`[0-9a-f]\{64\}\`" <<<"$body" | grep -o '[0-9a-f]\{64\}' | head -1
}

setup_tunnel() {
  local domain="$1" email="$2" tmpl="$3"
  command -v apt-get >/dev/null || die "--domain sets up nginx with apt (Debian/Ubuntu). Elsewhere follow deploy/nginx-rider-tunnel.conf by hand."
  say "Setting up the HTTPS fallback for $domain (nginx + Let's Encrypt)"
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq
  apt-get install -y -qq nginx certbot python3-certbot-nginx >/dev/null

  local site=/etc/nginx/sites-available/rider-tunnel
  if [ -f "$site" ] && grep -q "server_name $domain;" "$site"; then
    say "nginx site for $domain already present, keeping it"
  else
    sed "s/rider\.example\.com/$domain/g" "$tmpl" >"$site"
    ln -sf "$site" /etc/nginx/sites-enabled/rider-tunnel
  fi
  nginx -t -q
  systemctl reload nginx || systemctl restart nginx

  if command -v ufw >/dev/null && ufw status | grep -q "Status: active"; then
    ufw allow 80/tcp comment 'rider-server certificate (Let'\''s Encrypt)' >/dev/null
    ufw allow 443/tcp comment 'rider-server HTTPS tunnel' >/dev/null
    echo "ufw: opened TCP 80 and 443"
  fi

  local contact=(--register-unsafely-without-email)
  [ -n "$email" ] && contact=(-m "$email")
  certbot --nginx -d "$domain" --non-interactive --agree-tos "${contact[@]}" --keep-until-expiring \
    || die "certbot could not get a certificate for $domain: check that its DNS points to this server and TCP 80 is open, then run this again"
  nginx -t -q && systemctl reload nginx
  say "HTTPS fallback ready on https://$domain/rider-tunnel"
}

main() {
  local public_ip="" domain="" email="" version=""
  while [ $# -gt 0 ]; do
    case "$1" in
      --domain) domain="${2:-}"; shift 2 || die "--domain needs a name" ;;
      --domain=*) domain="${1#*=}"; shift ;;
      --email) email="${2:-}"; shift 2 || die "--email needs an address" ;;
      --email=*) email="${1#*=}"; shift ;;
      --version) version="${2:-}"; shift 2 || die "--version needs a value" ;;
      --version=*) version="${1#*=}"; shift ;;
      -h | --help) usage; exit 0 ;;
      -*) usage >&2; die "unknown option $1" ;;
      *)
        [ -z "$public_ip" ] || die "unexpected argument $1"
        public_ip="$1"
        shift
        ;;
    esac
  done
  [ -z "$public_ip" ] || valid_ipv4 "$public_ip" || die "$public_ip is not an IPv4 address"
  [ -z "$domain" ] || [[ "$domain" =~ ^[A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?$ ]] || die "$domain is not a domain name"
  version="${version#v}"

  [ "$(id -u)" -eq 0 ] || die "run as root: curl -fsSL <url> | sudo bash -s -- [PUBLIC_IP]"
  [ "$(uname -s)" = Linux ] || die "rider-server packages are for Linux"
  case "$(uname -m)" in
    x86_64 | amd64) ;;
    *) die "release binaries are for x86_64; on $(uname -m) build from source: cargo build --release -p rider-server" ;;
  esac
  command -v systemctl >/dev/null && [ -d /run/systemd/system ] || die "systemd is required"
  local tool
  for tool in curl sha256sum install sed awk; do
    command -v "$tool" >/dev/null || die "$tool is required"
  done

  local tag
  if [ -n "$version" ]; then
    tag="v$version"
  else
    say "Finding the latest release"
    tag=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest" | sed 's|.*/tag/||')
    [[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "could not find the latest release (got '$tag')"
    version="${tag#v}"
  fi

  local work
  work=$(mktemp -d)
  trap 'rm -rf "$work"' EXIT
  local bin="rider-server-$version-linux-x86_64"
  say "Downloading rider-server $version"
  curl -fSL --progress-bar --retry 3 -o "$work/rider-server" "https://github.com/$REPO/releases/download/$tag/$bin" \
    || die "download failed: is $tag a release with a Linux server?"
  local raw="https://raw.githubusercontent.com/$REPO/$tag/deploy"
  local f
  for f in install.sh rider-server.service nginx-rider-tunnel.conf; do
    curl -fsSL --retry 3 -o "$work/$f" "$raw/$f" || die "cannot download $f for $tag"
  done

  local want got
  want=$(expected_sha256 "$tag" "$bin")
  [ -n "$want" ] || die "no published SHA-256 for $bin; refusing to install an unverified binary"
  got=$(sha256sum "$work/rider-server" | awk '{print $1}')
  [ "$want" = "$got" ] || die "SHA-256 mismatch for $bin (expected $want, got $got)"
  say "SHA-256 verified"
  chmod +x "$work/rider-server"

  if [ -z "$public_ip" ]; then
    public_ip=$(detect_public_ip || true)
    if [ -n "$public_ip" ]; then
      say "Public IP: $public_ip (detected; pass it as the first argument to override)"
    else
      warn "could not detect the public IP; relayed sessions may fail behind NAT. Run again with it: ... | sudo bash -s -- <public-ip>"
    fi
  fi

  say "Installing"
  bash "$work/install.sh" ${public_ip:+"$public_ip"}

  if [ -n "$domain" ]; then
    setup_tunnel "$domain" "$email" "$work/nginx-rider-tunnel.conf"
  fi

  local fp
  fp=$(/usr/local/bin/rider-server --state-dir /var/lib/rider-server info 2>/dev/null | sed -n 's/^Server fingerprint: //p')
  local addr="${domain:-${public_ip:-<this server address>}}"
  echo
  say "Done. On every computer, open This computer → Manage → Server and enter:"
  echo "      Address:      $addr"
  echo "      Fingerprint:  ${fp:-(run: sudo rider-server --state-dir /var/lib/rider-server info)}"
  echo
  echo "  Firewall / cloud security group: allow UDP 47900 and 47901-47999${domain:+, TCP 80 and 443}."
  echo "  Logs: journalctl -u rider-server -f"
}

main "$@"
