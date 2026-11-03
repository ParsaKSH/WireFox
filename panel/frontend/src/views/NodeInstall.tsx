import { useState } from "react";
import { AlertTriangle, KeyRound } from "lucide-react";
import { Banner, cls, CopyButton, Modal } from "../ui";

type Platform = "linux" | "windows" | "openwrt";

function linuxSteps(key: string) {
  return {
    shell: `# Keep the key readable by root only
install -d -m 700 /etc/wirefox
printf 'WIREFOX_NODE_KEY=%s\\n' '${key}' > /etc/wirefox/node.env
chmod 600 /etc/wirefox/node.env

# Optional: try it in the foreground first (Ctrl+C to stop)
wirefox-node run --key '${key}'`,
    unit: `# /etc/systemd/system/wirefox-node.service
[Unit]
Description=WireFox exit-node
After=network-online.target
Wants=network-online.target

[Service]
EnvironmentFile=/etc/wirefox/node.env
ExecStart=/usr/local/bin/wirefox-node run --key \${WIREFOX_NODE_KEY}
DynamicUser=yes
StateDirectory=wirefox-node
WorkingDirectory=/var/lib/wirefox-node
Restart=always
RestartSec=2
LimitNOFILE=1048576

[Install]
WantedBy=multi-user.target`,
    enable: "systemctl daemon-reload\nsystemctl enable --now wirefox-node",
  };
}

function windowsScript(key: string): string {
  return `# Run in an elevated PowerShell; wirefox-node.exe is in C:\\WireFox
$key = '${key}'

# Optional: try it in the foreground first (Ctrl+C to stop)
& 'C:\\WireFox\\wirefox-node.exe' run --key $key

# Start it at boot, restarting it if it ever exits
$action = New-ScheduledTaskAction -Execute 'C:\\WireFox\\wirefox-node.exe' -Argument "run --key $key" -WorkingDirectory 'C:\\WireFox'
$trigger = New-ScheduledTaskTrigger -AtStartup
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1)
Register-ScheduledTask -TaskName 'WireFox node' -Action $action -Trigger $trigger -Settings $settings -User 'SYSTEM' -RunLevel Highest
Start-ScheduledTask -TaskName 'WireFox node'`;
}

function openwrtSteps(key: string) {
  return {
    shell: `mkdir -p /etc/wirefox
echo '${key}' > /etc/wirefox/node.key
chmod 600 /etc/wirefox/node.key`,
    init: `#!/bin/sh /etc/rc.common
# /etc/init.d/wirefox-node
START=99
USE_PROCD=1

start_service() {
	procd_open_instance
	procd_set_param command /usr/bin/wirefox-node run --key "$(cat /etc/wirefox/node.key)"
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

const TABS: { id: Platform; label: string }[] = [
  { id: "linux", label: "Linux" },
  { id: "windows", label: "Windows" },
  { id: "openwrt", label: "OpenWrt" },
];

// Shows a freshly issued node key. The panel keeps only a hash of the secret,
// so this is the one chance to copy it.
export function NodeKeyModal({ nodeId, nodeKey, onClose }: { nodeId: string; nodeKey: string; onClose: () => void }) {
  const [platform, setPlatform] = useState<Platform>("linux");
  const linux = linuxSteps(nodeKey);
  const openwrt = openwrtSteps(nodeKey);

  return (
    <Modal
      title={
        <span className="flex items-center gap-2">
          <KeyRound className="h-4 w-4 text-fox-400" /> Key for {nodeId}
        </span>
      }
      size="xl"
      onClose={onClose}
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
            Install the <span className="font-mono">wirefox-node</span> binary on the exit-node, then start it with{" "}
            <span className="font-mono text-slate-300">wirefox-node run --key &lt;KEY&gt;</span>. It connects to this
            panel, receives its tunnel settings and keeps working from its saved settings when the panel is away.
          </p>
        </div>

        <div>
          <div className="mb-3 flex gap-1 rounded-xl border border-ink-600/60 bg-ink-900/60 p-1" role="tablist">
            {TABS.map((t) => (
              <button
                key={t.id}
                role="tab"
                aria-selected={platform === t.id}
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
          <div className="space-y-4" role="tabpanel">
            {platform === "linux" && (
              <>
                <Snippet title="1. Store the key (as root)" code={linux.shell} />
                <Snippet title="2. systemd unit (binary in /usr/local/bin)" code={linux.unit} />
                <Snippet title="3. Enable and start" code={linux.enable} />
              </>
            )}
            {platform === "windows" && <Snippet title="PowerShell (as Administrator)" code={windowsScript(nodeKey)} />}
            {platform === "openwrt" && (
              <>
                <Snippet title="1. Store the key" code={openwrt.shell} />
                <Snippet title="2. procd init script (binary in /usr/bin)" code={openwrt.init} />
                <Snippet title="3. Enable and start" code={openwrt.enable} />
              </>
            )}
          </div>
        </div>
      </div>
    </Modal>
  );
}
