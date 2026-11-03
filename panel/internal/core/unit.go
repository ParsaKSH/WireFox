package core

import (
	"fmt"
	"strings"
)

// UnitFile renders the systemd service for the core. It runs as root so it
// can read its 0600 config, but with only the capability to bind low ports
// and with every path read-only except its data directory.
func UnitFile(binary, configPath, dataDir string) []byte {
	return fmt.Appendf(nil, `# Written by wirefox-panel; changes are overwritten.
[Unit]
Description=WireFox server
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=%s run -c %s
Restart=always
RestartSec=2
LimitNOFILE=1048576
ReadWritePaths=%s
CapabilityBoundingSet=CAP_NET_BIND_SERVICE
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectKernelLogs=yes
ProtectControlGroups=yes
ProtectClock=yes
ProtectHostname=yes
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX
RestrictNamespaces=yes
RestrictRealtime=yes
RestrictSUIDSGID=yes
LockPersonality=yes
SystemCallArchitectures=native

[Install]
WantedBy=multi-user.target
`, quoteArg(binary), quoteArg(configPath), quoteArg(dataDir))
}

// quoteArg escapes a path for a unit file: "%" starts a specifier, and
// whitespace or quotes need double quotes around the path.
func quoteArg(s string) string {
	s = strings.ReplaceAll(s, "%", "%%")
	if s != "" && !strings.ContainsAny(s, " \t\"'\\") {
		return s
	}
	r := strings.NewReplacer(`\`, `\\`, `"`, `\"`)
	return `"` + r.Replace(s) + `"`
}
