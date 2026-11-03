import { useId, useState, type KeyboardEvent } from "react";
import { AlertTriangle, KeyRound } from "lucide-react";
import { cls } from "../format";
import { arrowTarget, Banner, CopyButton, Modal } from "../ui";

type Platform = "linux" | "windows" | "openwrt";

// Every snippet hands the key to wirefox-node through WIREFOX_NODE_KEY: a
// command line is readable by every local user, the environment is not.

export function linuxSteps(key: string) {
  return {
    shell: `# A system user for the service; the key stays readable by root only
id wirefox >/dev/null 2>&1 || useradd --system --no-create-home --shell /usr/sbin/nologin wirefox
install -d -m 700 /etc/wirefox
install -m 600 /dev/null /etc/wirefox/node.env
printf 'WIREFOX_NODE_KEY=%s\\n' '${key}' > /etc/wirefox/node.env

# Optional: try it in the foreground first (Ctrl+C to stop)
(set -a; . /etc/wirefox/node.env; /usr/local/bin/wirefox-node run)`,
    unit: `# /etc/systemd/system/wirefox-node.service
[Unit]
Description=WireFox exit-node (managed by wirefox-panel)
After=network-online.target
Wants=network-online.target

[Service]
EnvironmentFile=/etc/wirefox/node.env
ExecStart=/usr/local/bin/wirefox-node run
StateDirectory=wirefox-node
StateDirectoryMode=0700
User=wirefox
Group=wirefox
Restart=always
RestartSec=2
LimitNOFILE=1048576

# Binding to an interface by name works unprivileged on Linux 5.7 and newer.
# On older kernels it needs CAP_NET_RAW: replace the empty
# CapabilityBoundingSet= below with the two commented lines.
CapabilityBoundingSet=
#CapabilityBoundingSet=CAP_NET_RAW
#AmbientCapabilities=CAP_NET_RAW
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes

[Install]
WantedBy=multi-user.target`,
    enable: "systemctl daemon-reload\nsystemctl enable --now wirefox-node",
  };
}

export function windowsScript(key: string): string {
  return `# Run in an elevated PowerShell; wirefox-node.exe is in C:\\WireFox.
# The key goes in a file only SYSTEM and Administrators can read (a machine
# environment variable would be readable by every local user).
icacls C:\\WireFox /inheritance:r /grant:r '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F'
Set-Content -Path 'C:\\WireFox\\node.key' -Value '${key}' -NoNewline

# Optional: try it in the foreground first (Ctrl+C to stop)
& 'C:\\WireFox\\wirefox-node.exe' run --key-file 'C:\\WireFox\\node.key'

# Start it at boot as SYSTEM, restarting it if it ever exits; its saved
# settings go to %ProgramData%\\wirefox-node
$action = New-ScheduledTaskAction -Execute 'C:\\WireFox\\wirefox-node.exe' -Argument 'run --key-file C:\\WireFox\\node.key' -WorkingDirectory 'C:\\WireFox'
$trigger = New-ScheduledTaskTrigger -AtStartup
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1)
$principal = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
Register-ScheduledTask -TaskName 'WireFox node' -Action $action -Trigger $trigger -Settings $settings -Principal $principal
Start-ScheduledTask -TaskName 'WireFox node'`;
}

export function openwrtSteps(key: string) {
  return {
    shell: `mkdir -p -m 700 /etc/wirefox
echo '${key}' > /etc/wirefox/node.key
chmod 600 /etc/wirefox/node.key`,
    init: `#!/bin/sh /etc/rc.common
# /etc/init.d/wirefox-node
# /var is in RAM on OpenWrt, so the saved settings go to flash in
# /etc/wirefox-node and survive a reboot while the panel is away.
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
}`,
    enable: "chmod +x /etc/init.d/wirefox-node\n/etc/init.d/wirefox-node enable\n/etc/init.d/wirefox-node start",
  };
}

function Snippet({ title, code }: { title: string; code: string }) {
  return (
    <div>
      <div className="mb-1.5 flex items-center justify-between">
        <span className="text-xs font-medium text-slate-400">{title}</span>
        <CopyButton text={code} />
      </div>
      <pre className="max-h-64 overflow-auto rounded-lg border border-ink-600/70 bg-ink-950/70 p-3 font-mono text-[11.5px] leading-relaxed text-slate-300">
        {code}
      </pre>
    </div>
  );
}

const SCRIPTS = "https://raw.githubusercontent.com/ParsaKSH/WireFox/main/scripts";

// The installers download the release, check its checksum, set up the service
// and ask for the key, so it never lands in the shell history.
export const QUICK_INSTALL: Record<Platform, string> = {
  linux: `sh -c "$(curl -fsSL ${SCRIPTS}/install-node.sh)"`,
  windows: `irm ${SCRIPTS}/install-node.bat -OutFile install-node.bat; .\\install-node.bat`,
  openwrt: `sh -c "$(wget -qO- ${SCRIPTS}/install-node.sh)"`,
};

const TABS: { id: Platform; label: string }[] = [
  { id: "linux", label: "Linux" },
  { id: "windows", label: "Windows" },
  { id: "openwrt", label: "OpenWrt" },
];

// Shows a freshly issued node key. The panel keeps only a hash of the secret,
// so this is the one chance to copy it.
export function NodeKeyModal({ nodeId, nodeKey, onClose }: { nodeId: string; nodeKey: string; onClose: () => void }) {
  const [platform, setPlatform] = useState<Platform>("linux");
  const tabsId = useId();
  const linux = linuxSteps(nodeKey);
  const openwrt = openwrtSteps(nodeKey);

  const onTabKey = (e: KeyboardEvent<HTMLDivElement>) => {
    const next = arrowTarget(
      e.key,
      TABS.findIndex((t) => t.id === platform),
      TABS.length,
      "horizontal",
    );
    if (next === null) return;
    e.preventDefault();
    setPlatform(TABS[next].id);
    e.currentTarget.querySelectorAll<HTMLElement>("[role=tab]")[next]?.focus();
  };

  return (
    <Modal
      title={
        <span className="flex items-center gap-2">
          <KeyRound className="h-4 w-4 text-fox-400" /> Key for {nodeId}
        </span>
      }
      size="xl"
      onClose={onClose}
      dismissible={false}
      footer={
        <button className="btn-primary" onClick={onClose}>
          I have saved the key
        </button>
      }
    >
      <div className="space-y-5">
        <Banner tone="amber" icon={<AlertTriangle className="h-4 w-4" />}>
          This key is shown only once. Anyone holding it can connect as this node; if it leaks, issue a new one.
        </Banner>

        <div>
          <div className="mb-1.5 flex items-center justify-between">
            <span className="text-xs font-medium text-slate-400">Node key</span>
            <CopyButton text={nodeKey} label="Copy key" />
          </div>
          <code className="block break-all rounded-lg border border-fox-500/30 bg-ink-950/70 p-3 font-mono text-xs text-fox-300">
            {nodeKey}
          </code>
          <p className="mt-1.5 text-xs text-slate-500">
            Run the installer on the exit-node and paste the key when it asks, or set it up by hand as below. The node
            connects to this panel, receives its tunnel settings and keeps working from its saved settings when the
            panel is away.
          </p>
        </div>

        <div>
          <div
            className="mb-3 flex gap-1 rounded-xl border border-ink-600/60 bg-ink-900/60 p-1"
            role="tablist"
            aria-label="Platform"
            onKeyDown={onTabKey}
          >
            {TABS.map((t) => (
              <button
                key={t.id}
                id={`${tabsId}-${t.id}`}
                role="tab"
                aria-selected={platform === t.id}
                aria-controls={`${tabsId}-panel`}
                tabIndex={platform === t.id ? 0 : -1}
                onClick={() => setPlatform(t.id)}
                className={cls(
                  "flex-1 rounded-lg px-3 py-1.5 text-sm font-medium transition",
                  platform === t.id ? "bg-fox-500/15 text-fox-300" : "text-slate-400 hover:text-slate-200",
                )}
              >
                {t.label}
              </button>
            ))}
          </div>
          <div className="space-y-4" id={`${tabsId}-panel`} role="tabpanel" aria-labelledby={`${tabsId}-${platform}`}>
            <Snippet
              title={
                platform === "windows"
                  ? "Quick install: PowerShell as Administrator, then paste the key when asked"
                  : "Quick install: as root, then paste the key when asked"
              }
              code={QUICK_INSTALL[platform]}
            />
            <p className="text-xs font-medium uppercase tracking-wide text-slate-500">Or by hand</p>
            {platform === "linux" && (
              <>
                <Snippet title="1. Create the service user and store the key (as root)" code={linux.shell} />
                <Snippet title="2. systemd unit (binary in /usr/local/bin)" code={linux.unit} />
                <Snippet title="3. Enable and start" code={linux.enable} />
              </>
            )}
            {platform === "windows" && <Snippet title="PowerShell (as Administrator)" code={windowsScript(nodeKey)} />}
            {platform === "openwrt" && (
              <>
                <Snippet title="1. Store the key" code={openwrt.shell} />
                <Snippet
                  title="2. procd init script (binary in /usr/bin, saved settings in /etc/wirefox-node)"
                  code={openwrt.init}
                />
                <Snippet title="3. Enable and start" code={openwrt.enable} />
              </>
            )}
          </div>
        </div>
      </div>
    </Modal>
  );
}
