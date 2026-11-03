#!/usr/bin/env bash
#
# WireFox panel installer: installs wirefox-panel and the WireFox server (the
# core) on a Linux server with systemd. The panel then runs the core as its
# own systemd service.
#
#   bash <(curl -fsSL https://raw.githubusercontent.com/ParsaKSH/WireFox/main/scripts/install-panel.sh)

set -euo pipefail

usage() {
    cat <<'EOF'
Usage: install-panel.sh [options]

  --version TAG      install this release (default: the latest)
  --mirror URL       download release files from URL/<tag>/<file> instead of GitHub
  --port N           web UI port (default: random; fresh installs only)
  --agent-port N     port nodes connect to (default: 50051)
  --local            web UI on 127.0.0.1 only, plain HTTP (reach it through an SSH tunnel)
  --uninstall        remove the panel; the core keeps running
  --purge            with --uninstall: also remove the core and all data

Running it again upgrades both binaries and keeps all settings and data.
EOF
}

REPO="ParsaKSH/WireFox"
BIN_DIR="/usr/local/bin"
PANEL_DATA="/var/lib/wirefox-panel"
PANEL_UNIT="wirefox-panel"
CORE_UNIT="wirefox-server"
CORE_CONFIG_DIR="/etc/wirefox"
CORE_DATA="/var/lib/wirefox"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

info() { echo -e "${CYAN}==>${NC} $*"; }
ok() { echo -e "${GREEN}✓${NC} $*"; }
warn() { echo -e "${YELLOW}!${NC} $*"; }
die() {
    echo -e "${RED}✗ $*${NC}" >&2
    exit 1
}

VERSION=""
MIRROR="${WIREFOX_MIRROR:-}"
PORT=""
AGENT_PORT=50051
LOCAL=0
UNINSTALL=0
PURGE=0

while [ $# -gt 0 ]; do
    case "$1" in
        --version) VERSION="${2:?--version needs a tag}"; shift 2 ;;
        --mirror) MIRROR="${2:?--mirror needs a URL}"; shift 2 ;;
        --port) PORT="${2:?--port needs a number}"; shift 2 ;;
        --agent-port) AGENT_PORT="${2:?--agent-port needs a number}"; shift 2 ;;
        --local) LOCAL=1; shift ;;
        --uninstall) UNINSTALL=1; shift ;;
        --purge) PURGE=1; shift ;;
        -h | --help) usage; exit 0 ;;
        *) die "unknown option: $1 (see --help)" ;;
    esac
done

echo -e "${CYAN}${BOLD}"
echo "╔══════════════════════════════════════════╗"
echo "║        WireFox Panel Installer           ║"
echo "╚══════════════════════════════════════════╝"
echo -e "${NC}"

[ "$(id -u)" -eq 0 ] || die "Please run as root (sudo)."
command -v systemctl >/dev/null && [ -d /run/systemd/system ] || die "This installer needs systemd."

if [ "$UNINSTALL" -eq 1 ]; then
    info "Removing the panel"
    systemctl disable --now "$PANEL_UNIT" 2>/dev/null || true
    rm -f "/etc/systemd/system/$PANEL_UNIT.service" "$BIN_DIR/wirefox-panel"
    if [ "$PURGE" -eq 1 ]; then
        info "Removing the core and all data"
        systemctl disable --now "$CORE_UNIT" 2>/dev/null || true
        rm -f "/etc/systemd/system/$CORE_UNIT.service" "$BIN_DIR/wirefox-server"
        rm -rf "$PANEL_DATA" "$CORE_CONFIG_DIR" "$CORE_DATA"
    fi
    systemctl daemon-reload
    if [ "$PURGE" -eq 1 ]; then
        ok "Panel, core and data removed."
    else
        ok "Panel removed. The core ($CORE_UNIT) keeps running; use --uninstall --purge to remove it too."
    fi
    exit 0
fi

for tool in tar sha256sum; do
    command -v "$tool" >/dev/null || die "$tool is required."
done
command -v curl >/dev/null || die "curl is required (apt install curl)."

case "$(uname -m)" in
    x86_64 | amd64) ARCH="amd64" ;;
    aarch64 | arm64) ARCH="arm64" ;;
    *) die "Unsupported architecture: $(uname -m). The panel is built for amd64 and arm64." ;;
esac
ok "Detected linux/$ARCH"

if [ -z "$VERSION" ]; then
    [ -z "$MIRROR" ] || die "--mirror needs --version."
    info "Looking up the latest release"
    # The redirect of /releases/latest names the tag and isn't rate-limited like the API.
    VERSION=$(curl -fsSI "https://github.com/$REPO/releases/latest" |
        tr -d '\r' | sed -n 's#^[Ll]ocation: .*/tag/\(.*\)$#\1#p')
    [ -n "$VERSION" ] || die "Could not find the latest release (is GitHub reachable? try --mirror)."
fi
ok "Version $VERSION"

BASE="${MIRROR:-https://github.com/$REPO/releases/download}"
BASE="${BASE%/}/$VERSION"
PANEL_PKG="wirefox-$VERSION-panel-linux-$ARCH"
CORE_PKG="wirefox-$VERSION-linux-$ARCH"

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

download() {
    curl -fL --retry 3 --progress-bar -o "$TMP/$1" "$BASE/$1" || die "Download failed: $BASE/$1"
}

info "Downloading $PANEL_PKG and $CORE_PKG"
download SHA256SUMS
download "$PANEL_PKG.tar.gz"
download "$CORE_PKG.tar.gz"
(cd "$TMP" && grep -E " ($PANEL_PKG|$CORE_PKG)\.tar\.gz$" SHA256SUMS | sha256sum -c --quiet -) ||
    die "Checksum mismatch; the download is corrupt or was tampered with."
ok "Checksums verified"

tar -xzf "$TMP/$PANEL_PKG.tar.gz" -C "$TMP"
tar -xzf "$TMP/$CORE_PKG.tar.gz" -C "$TMP"
install -m 755 "$TMP/$PANEL_PKG/wirefox-panel" "$BIN_DIR/wirefox-panel"
install -m 755 "$TMP/$CORE_PKG/wirefox-server" "$BIN_DIR/wirefox-server"
ok "Installed wirefox-panel and wirefox-server to $BIN_DIR"

if [ -f "/etc/systemd/system/$PANEL_UNIT.service" ] && [ -d "$PANEL_DATA" ]; then
    info "Existing install found; restarting with the new version"
    systemctl restart "$PANEL_UNIT"
    if systemctl is-enabled --quiet "$CORE_UNIT" 2>/dev/null; then
        warn "Restarting the core: open SOCKS connections drop for a moment."
        systemctl restart "$CORE_UNIT"
    fi
    ok "Upgraded to $VERSION. Settings, users and nodes are unchanged."
    exit 0
fi

PORT="${PORT:-$(shuf -i 20000-60000 -n 1)}"
ADMIN_USER="admin"
ADMIN_PASS="$(head -c 128 /dev/urandom | tr -dc 'A-Za-z0-9' | head -c 20)"
if [ "$LOCAL" -eq 1 ]; then
    WEB_ADDR="127.0.0.1:$PORT"
    WEB_FLAGS=""
    SCHEME="http"
else
    # Reachable from outside, so encrypt it: the panel's own self-signed certificate.
    WEB_ADDR="0.0.0.0:$PORT"
    WEB_FLAGS=" -tls"
    SCHEME="https"
fi

# Created before the first start, so the password never has to sit in the unit file.
info "Creating the admin account"
install -d -m 700 "$PANEL_DATA"
WIREFOX_PANEL_PASSWORD="$ADMIN_PASS" "$BIN_DIR/wirefox-panel" -data "$PANEL_DATA" -reset-password >/dev/null 2>&1 ||
    die "Could not create the admin account."

cat >"/etc/systemd/system/$PANEL_UNIT.service" <<EOF
[Unit]
Description=WireFox panel
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=$BIN_DIR/wirefox-panel -addr $WEB_ADDR -grpc-addr :$AGENT_PORT$WEB_FLAGS
Restart=always
RestartSec=2
StateDirectory=wirefox-panel
StateDirectoryMode=0700

[Install]
WantedBy=multi-user.target
EOF

info "Starting the panel (it sets up and starts the core)"
systemctl daemon-reload
systemctl enable --now "$PANEL_UNIT" >/dev/null 2>&1

API="$SCHEME://127.0.0.1:$PORT/api"
for _ in $(seq 1 30); do
    [ "$(curl -sk -o /dev/null -w '%{http_code}' "$API/me")" = "401" ] && break
    sleep 1
done
[ "$(curl -sk -o /dev/null -w '%{http_code}' "$API/me")" = "401" ] ||
    die "The panel did not come up; see: journalctl -u $PANEL_UNIT -n 50"
for _ in $(seq 1 20); do
    systemctl is-active --quiet "$CORE_UNIT" && break
    sleep 1
done
systemctl is-active --quiet "$CORE_UNIT" || warn "The core is not running yet; see: journalctl -u $CORE_UNIT -n 50"
ok "Panel and core are running"

PUBLIC_IP=$(curl -fs4 --max-time 5 https://api.ipify.org 2>/dev/null ||
    curl -fs4 --max-time 5 https://ifconfig.me 2>/dev/null ||
    hostname -I | awk '{print $1}')

# Tell the panel where nodes and SOCKS clients reach this server, so node keys work right away.
JAR="$TMP/cookies"
LOGIN=$(printf '{"username":"%s","password":"%s"}' "$ADMIN_USER" "$ADMIN_PASS")
if curl -sk -c "$JAR" -H 'Content-Type: application/json' -d "$LOGIN" "$API/login" -o /dev/null &&
    curl -sk -b "$JAR" -X PUT -H 'Content-Type: application/json' -o /dev/null -f \
        -d "{\"agent_public_addr\":\"$PUBLIC_IP:$AGENT_PORT\",\"socks_public_host\":\"$PUBLIC_IP\"}" "$API/settings"; then
    ok "Node address set to $PUBLIC_IP:$AGENT_PORT"
else
    warn "Could not set the node address; set it under Settings in the panel."
fi

PORTS=("$AGENT_PORT" 7443 1080 1443)
[ "$LOCAL" -eq 1 ] || PORTS=("$PORT" "${PORTS[@]}")
if command -v ufw >/dev/null && ufw status 2>/dev/null | grep -q "Status: active"; then
    for p in "${PORTS[@]}"; do ufw allow "$p/tcp" >/dev/null; done
    ok "Opened ports ${PORTS[*]} in ufw"
elif command -v firewall-cmd >/dev/null && firewall-cmd --state >/dev/null 2>&1; then
    for p in "${PORTS[@]}"; do firewall-cmd --permanent --add-port="$p/tcp" >/dev/null; done
    firewall-cmd --reload >/dev/null
    ok "Opened ports ${PORTS[*]} in firewalld"
fi

if [ "$LOCAL" -eq 1 ]; then
    URL="http://127.0.0.1:$PORT"
    REACH="ssh -L $PORT:127.0.0.1:$PORT root@$PUBLIC_IP"
else
    URL="https://$PUBLIC_IP:$PORT"
fi

echo
echo -e "${GREEN}${BOLD}"
echo "╔══════════════════════════════════════════════════════════╗"
echo "║               Installation complete ✓                    ║"
echo "╠══════════════════════════════════════════════════════════╣"
printf "║  URL:       %-45s║\n" "$URL"
printf "║  Username:  %-45s║\n" "$ADMIN_USER"
printf "║  Password:  %-45s║\n" "$ADMIN_PASS"
echo "╠══════════════════════════════════════════════════════════╣"
printf "║  Nodes connect to:  %-37s║\n" "$PUBLIC_IP:$AGENT_PORT"
printf "║  SOCKS5:            %-37s║\n" "$PUBLIC_IP:1080 (TLS: 1443)"
echo "╠══════════════════════════════════════════════════════════╣"
echo "║  Status:  systemctl status wirefox-panel wirefox-server  ║"
echo "║  Logs:    journalctl -u wirefox-panel -f                 ║"
echo "╚══════════════════════════════════════════════════════════╝"
echo -e "${NC}"
[ "$LOCAL" -eq 1 ] && echo -e "Open the panel through an SSH tunnel: ${BOLD}$REACH${NC}"
[ "$LOCAL" -eq 1 ] || warn "The panel uses a self-signed certificate; your browser will ask you to accept it."
echo -e "${YELLOW}Save these credentials now; they are not shown again.${NC}"
echo "Forgot the password? systemctl stop $PANEL_UNIT; wirefox-panel -reset-password; systemctl start $PANEL_UNIT"
