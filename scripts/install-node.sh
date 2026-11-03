#!/bin/sh
#
# WireFox exit-node installer for Linux (systemd) and OpenWrt (procd).
# Installs wirefox-node in managed mode: it connects to your wirefox-panel
# with the node key the panel generated, and gets everything else from there.
#
#   sh -c "$(curl -fsSL https://raw.githubusercontent.com/ParsaKSH/WireFox/main/scripts/install-node.sh)"
#
# POSIX sh on purpose: OpenWrt has no bash by default.

set -eu

REPO="ParsaKSH/WireFox"
SERVICE="wirefox-node"
KEY_DIR="/etc/wirefox"

if [ -t 1 ]; then
    RED='\033[0;31m' GREEN='\033[0;32m' YELLOW='\033[1;33m' CYAN='\033[0;36m' BOLD='\033[1m' NC='\033[0m'
else
    RED='' GREEN='' YELLOW='' CYAN='' BOLD='' NC=''
fi
info() { printf "${CYAN}==>${NC} %s\n" "$*"; }
ok() { printf "${GREEN}✓${NC} %s\n" "$*"; }
warn() { printf "${YELLOW}!${NC} %s\n" "$*"; }
die() {
    printf "${RED}✗ %s${NC}\n" "$*" >&2
    exit 1
}

usage() {
    cat <<'EOF'
Usage: install-node.sh [options]

  --key KEY        node key from the panel (default: $WIREFOX_NODE_KEY, or asked for)
  --version TAG    install this release (default: the latest)
  --mirror URL     download release files from URL/<tag>/<file> instead of GitHub
  --uninstall      remove the node and its key and saved settings

Running it again upgrades the binary and keeps the key, unless a new one is given.
EOF
}

KEY="${WIREFOX_NODE_KEY:-}"
VERSION=""
MIRROR="${WIREFOX_MIRROR:-}"
UNINSTALL=0

while [ $# -gt 0 ]; do
    case "$1" in
        --key) KEY="${2:?--key needs a value}"; shift 2 ;;
        --version) VERSION="${2:?--version needs a tag}"; shift 2 ;;
        --mirror) MIRROR="${2:?--mirror needs a URL}"; shift 2 ;;
        --uninstall) UNINSTALL=1; shift ;;
        -h | --help) usage; exit 0 ;;
        *) die "unknown option: $1 (see --help)" ;;
    esac
done

printf "${CYAN}${BOLD}"
echo "╔══════════════════════════════════════════╗"
echo "║       WireFox Exit-Node Installer        ║"
echo "╚══════════════════════════════════════════╝"
printf "${NC}\n"

[ "$(id -u)" -eq 0 ] || die "Please run as root."

if [ -f /etc/openwrt_release ]; then
    PLATFORM="openwrt"
    BIN="/usr/bin/wirefox-node"
    # /var is in RAM on OpenWrt; keep the saved settings on flash.
    STATE_DIR="/etc/wirefox-node"
elif command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
    PLATFORM="systemd"
    BIN="/usr/local/bin/wirefox-node"
    STATE_DIR="/var/lib/wirefox-node"
else
    die "This installer supports systemd-based Linux and OpenWrt."
fi

stop_service() {
    if [ "$PLATFORM" = "openwrt" ]; then
        [ -x "/etc/init.d/$SERVICE" ] && "/etc/init.d/$SERVICE" stop 2>/dev/null || true
    else
        systemctl stop "$SERVICE" 2>/dev/null || true
    fi
}

if [ "$UNINSTALL" -eq 1 ]; then
    info "Removing wirefox-node"
    stop_service
    if [ "$PLATFORM" = "openwrt" ]; then
        [ -x "/etc/init.d/$SERVICE" ] && "/etc/init.d/$SERVICE" disable 2>/dev/null || true
        rm -f "/etc/init.d/$SERVICE"
    else
        systemctl disable "$SERVICE" 2>/dev/null || true
        rm -f "/etc/systemd/system/$SERVICE.service"
        systemctl daemon-reload
    fi
    rm -rf "$BIN" "$STATE_DIR" "$KEY_DIR/node.env" "$KEY_DIR/node.key"
    rmdir "$KEY_DIR" 2>/dev/null || true
    ok "wirefox-node removed. Delete the node in the panel too."
    exit 0
fi

ARCH="$(uname -m)"
case "$ARCH" in
    x86_64 | amd64) PKG_ARCH="linux-amd64" ;;
    aarch64 | arm64) PKG_ARCH="linux-arm64" ;;
    armv7*)
        PKG_ARCH="linux-arm_cortex-a7_neon-vfpv4"
        grep -qw neon /proc/cpuinfo 2>/dev/null ||
            die "This ARMv7 CPU has no NEON; the armv7 build needs a Cortex-A7 class CPU (NEON + VFPv4)."
        ;;
    *) die "Unsupported architecture: $ARCH" ;;
esac
ok "Detected $PLATFORM on $ARCH"

if command -v curl >/dev/null 2>&1; then
    fetch() { curl -fsSL --retry 3 -o "$2" "$1"; }
    location() { curl -fsSI "$1" | tr -d '\r' | sed -n 's/^[Ll]ocation: //p' | tail -n 1; }
elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -q -O "$2" "$1"; }
    location() { wget -q -S --spider --max-redirect=0 "$1" 2>&1 | sed -n 's/^ *[Ll]ocation: //p' | tail -n 1; }
else
    die "curl or wget is required."
fi
for tool in tar sha256sum; do
    command -v "$tool" >/dev/null 2>&1 || die "$tool is required."
done

# The key: given now, or kept from an earlier install.
OLD_KEY=""
if [ "$PLATFORM" = "systemd" ] && [ -f "$KEY_DIR/node.env" ]; then
    OLD_KEY=$(sed -n 's/^WIREFOX_NODE_KEY=//p' "$KEY_DIR/node.env")
elif [ -f "$KEY_DIR/node.key" ]; then
    OLD_KEY=$(cat "$KEY_DIR/node.key")
fi
if [ -z "$KEY" ] && [ -n "$OLD_KEY" ]; then
    KEY="$OLD_KEY"
    ok "Keeping the existing node key"
fi
if [ -z "$KEY" ]; then
    # /dev/tty can exist yet fail to open (no controlling terminal), so try it.
    (: </dev/tty) 2>/dev/null || die "No node key: pass --key or set WIREFOX_NODE_KEY."
    printf "Paste the node key from the panel (Nodes > Add node): "
    stty -echo </dev/tty 2>/dev/null || true
    read -r KEY </dev/tty
    stty echo </dev/tty 2>/dev/null || true
    echo
fi
case "$KEY" in
    wfx1.*) ;;
    *) die "That doesn't look like a node key (they start with wfx1.)." ;;
esac

if [ -z "$VERSION" ]; then
    [ -z "$MIRROR" ] || die "--mirror needs --version."
    info "Looking up the latest release"
    VERSION=$(location "https://github.com/$REPO/releases/latest" | sed -n 's#.*/tag/##p')
    [ -n "$VERSION" ] || die "Could not find the latest release (is GitHub reachable? try --mirror)."
fi
ok "Version $VERSION"

BASE="${MIRROR:-https://github.com/$REPO/releases/download}"
BASE="${BASE%/}/$VERSION"
PKG="wirefox-$VERSION-$PKG_ARCH"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

info "Downloading $PKG"
fetch "$BASE/SHA256SUMS" "$TMP/SHA256SUMS" || die "Download failed: $BASE/SHA256SUMS"
fetch "$BASE/$PKG.tar.gz" "$TMP/$PKG.tar.gz" || die "Download failed: $BASE/$PKG.tar.gz"
# Compared by hand: BusyBox and GNU sha256sum disagree on the flags for -c.
EXPECTED=$(grep " $PKG.tar.gz\$" "$TMP/SHA256SUMS" | cut -d' ' -f1)
ACTUAL=$(sha256sum "$TMP/$PKG.tar.gz" | cut -d' ' -f1)
[ -n "$EXPECTED" ] && [ "$EXPECTED" = "$ACTUAL" ] ||
    die "Checksum mismatch; the download is corrupt or was tampered with."
ok "Checksum verified"
tar -xzf "$TMP/$PKG.tar.gz" -C "$TMP"

stop_service
mkdir -p "$(dirname "$BIN")"
cp "$TMP/$PKG/wirefox-node" "$BIN"
chmod 755 "$BIN"
ok "Installed $BIN"

umask 077
mkdir -p "$KEY_DIR"
chmod 700 "$KEY_DIR"

if [ "$PLATFORM" = "openwrt" ]; then
    printf '%s\n' "$KEY" >"$KEY_DIR/node.key"
    cat >"/etc/init.d/$SERVICE" <<'EOF'
#!/bin/sh /etc/rc.common
# WireFox exit-node, managed by wirefox-panel. Settings it receives are saved
# on flash in /etc/wirefox-node, so it comes back up while the panel is away.
START=99
USE_PROCD=1

start_service() {
	procd_open_instance
	procd_set_param command /usr/bin/wirefox-node run --state-dir /etc/wirefox-node
	procd_set_param env WIREFOX_NODE_KEY_FILE=/etc/wirefox/node.key
	procd_set_param respawn 3600 5 0
	procd_set_param stdout 1
	procd_set_param stderr 1
	procd_close_instance
}
EOF
    chmod 755 "/etc/init.d/$SERVICE"
    "/etc/init.d/$SERVICE" enable
    LOG_START=$(logread -e wirefox-node 2>/dev/null | wc -l)
    "/etc/init.d/$SERVICE" start
else
    printf 'WIREFOX_NODE_KEY=%s\n' "$KEY" >"$KEY_DIR/node.env"
    id wirefox >/dev/null 2>&1 ||
        useradd --system --no-create-home --shell /usr/sbin/nologin wirefox 2>/dev/null ||
        adduser --system --no-create-home --group wirefox >/dev/null
    # Binding to an interface by name needs CAP_NET_RAW before Linux 5.7.
    KERNEL=$(uname -r | cut -d. -f1-2)
    CAPS="CapabilityBoundingSet="
    if [ "$(printf '%s\n5.7\n' "$KERNEL" | sort -V | head -n 1)" != "5.7" ]; then
        CAPS="CapabilityBoundingSet=CAP_NET_RAW
AmbientCapabilities=CAP_NET_RAW"
    fi
    cat >"/etc/systemd/system/$SERVICE.service" <<EOF
[Unit]
Description=WireFox exit-node (managed by wirefox-panel)
After=network-online.target
Wants=network-online.target

[Service]
EnvironmentFile=$KEY_DIR/node.env
ExecStart=$BIN run
StateDirectory=wirefox-node
StateDirectoryMode=0700
User=wirefox
Group=wirefox
Restart=always
RestartSec=2
LimitNOFILE=1048576
$CAPS
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes

[Install]
WantedBy=multi-user.target
EOF
    systemctl daemon-reload
    systemctl enable "$SERVICE" >/dev/null 2>&1
    LOG_START=$(date '+%Y-%m-%d %H:%M:%S')
    systemctl restart "$SERVICE"
fi

# Only lines from this start count; an earlier run may have logged success.
logs() {
    if [ "$PLATFORM" = "openwrt" ]; then
        logread -e wirefox-node 2>/dev/null | tail -n "+$((LOG_START + 1))"
    else
        journalctl -u "$SERVICE" --since "$LOG_START" --no-pager -o cat 2>/dev/null
    fi
}

# Two separate links: the agent connection to the panel, and the tunnel to the
# WireFox server whose settings the panel sends.
info "Waiting for the node to reach the panel and the WireFox server"
PANEL=0
TUNNEL=0
for _ in $(seq 1 40); do
    LINES=$(logs)
    echo "$LINES" | grep -q "registered with the panel" && PANEL=1
    echo "$LINES" | grep -q "connected to server" && TUNNEL=1
    [ "$PANEL" -eq 1 ] && [ "$TUNNEL" -eq 1 ] && break
    sleep 1
done

echo
if [ "$PANEL" -eq 1 ] && [ "$TUNNEL" -eq 1 ]; then
    printf "${GREEN}${BOLD}✓ wirefox-node %s is connected to the panel and its tunnel is up.${NC}\n" "$VERSION"
    echo "  It shows up as online on the panel's Nodes page and is ready to carry traffic."
else
    if [ "$PANEL" -eq 1 ]; then
        warn "Connected to the panel, but the tunnel to the WireFox server isn't up yet."
        echo "  Check that the server's tunnel port (7443 by default) is reachable from here,"
        echo "  and the tunnel address under Core in the panel."
    elif [ "$TUNNEL" -eq 1 ]; then
        warn "The tunnel is up from saved settings, but the panel isn't reachable."
        echo "  Check that the panel's agent port (50051 by default) is reachable from here."
    else
        warn "wirefox-node is running but has reached neither the panel nor the server yet."
        echo "  Check that the panel's agent port (50051 by default) is reachable from here."
    fi
    echo "  Recent log lines:"
    logs | tail -n 8 | sed 's/^/    /'
fi
if [ "$PLATFORM" = "openwrt" ]; then
    echo "  Logs:    logread -e wirefox-node -f"
    echo "  Service: /etc/init.d/$SERVICE {start|stop|restart}"
else
    echo "  Logs:    journalctl -u $SERVICE -f"
    echo "  Service: systemctl {status|restart} $SERVICE"
fi
