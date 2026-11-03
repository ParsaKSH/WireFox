// Package core manages the WireFox server ("the core"): its config file, its
// systemd unit and its REST API.
package core

import (
	"bytes"
	"errors"
	"fmt"
	"net"
	"net/netip"
	"os"
	"path/filepath"
	"slices"
	"strconv"

	"github.com/pelletier/go-toml/v2"

	"github.com/ParsaKSH/WireFox/panel/internal/atomicfile"
)

// Balance strategies the core understands.
var Strategies = []string{"round-robin", "least-ping", "least-load", "random"}

// DefaultTunnelSAN is the name a fresh core config puts in the tunnel
// certificate and the name nodes verify it against. It matches
// wirefox_common::proto::DEFAULT_SERVER_NAME.
const DefaultTunnelSAN = "wirefox"

// ServerConfig mirrors ServerConfig in crates/server/src/config.rs. The core
// rejects unknown fields, so every name here must match it exactly.
type ServerConfig struct {
	DataDir string        `toml:"data_dir"`
	TLS     TLSSection    `toml:"tls"`
	Tunnel  TunnelSection `toml:"tunnel"`
	Socks   SocksSection  `toml:"socks"`
	API     APISection    `toml:"api"`
}

type TLSSection struct {
	Cert string   `toml:"cert,omitempty"`
	Key  string   `toml:"key,omitempty"`
	SANs []string `toml:"sans"`
}

type TunnelSection struct {
	Listen     string `toml:"listen"`
	PublicAddr string `toml:"public_addr,omitempty"`
}

type SocksSection struct {
	Listen               []string `toml:"listen"`
	TLSListen            []string `toml:"tls_listen"`
	HandshakeTimeoutSecs uint64   `toml:"handshake_timeout_secs"`
	ConnectTimeoutSecs   uint64   `toml:"connect_timeout_secs"`
	Balance              string   `toml:"balance"`
}

type APISection struct {
	Listen string `toml:"listen"`
	TLS    bool   `toml:"tls"`
	// Both or neither. The panel leaves them out: it uses an API key.
	AdminUsername     string   `toml:"admin_username,omitempty"`
	AdminPasswordHash string   `toml:"admin_password_hash,omitempty"`
	APIKeyHashes      []string `toml:"api_key_hashes"`
	SessionTTLSecs    uint64   `toml:"session_ttl_secs"`
	CORSOrigins       []string `toml:"cors_origins"`
}

// DefaultConfig is what the panel writes for a fresh install.
func DefaultConfig(dataDir string) *ServerConfig {
	cfg := withDefaults()
	cfg.DataDir = dataDir
	cfg.TLS.SANs = []string{DefaultTunnelSAN}
	cfg.Tunnel.Listen = "0.0.0.0:7443"
	cfg.Socks.Listen = []string{"0.0.0.0:1080"}
	cfg.Socks.TLSListen = []string{"0.0.0.0:1443"}
	cfg.API.Listen = "127.0.0.1:9090"
	return cfg
}

// withDefaults has the values the core assumes for fields a file leaves out.
func withDefaults() *ServerConfig {
	return &ServerConfig{
		DataDir: "data",
		TLS:     TLSSection{SANs: []string{}},
		Socks: SocksSection{
			Listen:               []string{},
			TLSListen:            []string{},
			HandshakeTimeoutSecs: 10,
			ConnectTimeoutSecs:   15,
			Balance:              "least-load",
		},
		API: APISection{APIKeyHashes: []string{}, SessionTTLSecs: 12 * 3600, CORSOrigins: []string{}},
	}
}

// LoadConfig reads a core config file.
func LoadConfig(path string) (*ServerConfig, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	cfg, err := ParseConfig(data)
	if err != nil {
		return nil, fmt.Errorf("parsing %s: %w", path, err)
	}
	return cfg, nil
}

// ParseConfig parses a core config, filling in the core's defaults.
func ParseConfig(data []byte) (*ServerConfig, error) {
	cfg := withDefaults()
	dec := toml.NewDecoder(bytes.NewReader(data))
	dec.DisallowUnknownFields()
	if err := dec.Decode(cfg); err != nil {
		return nil, err
	}
	return cfg, nil
}

// Marshal renders the config as TOML.
func (c *ServerConfig) Marshal() ([]byte, error) {
	return toml.Marshal(c)
}

// DataPath is the data directory as the core resolves it: relative paths are
// relative to the directory of the config file.
func (c *ServerConfig) DataPath(configPath string) string {
	return resolve(configPath, c.DataDir)
}

// CertPath is the core's TLS certificate (by default generated in the data
// directory).
func (c *ServerConfig) CertPath(configPath string) string {
	if c.TLS.Cert != "" {
		return resolve(configPath, c.TLS.Cert)
	}
	return filepath.Join(c.DataPath(configPath), "server.crt")
}

func resolve(configPath, p string) string {
	if filepath.IsAbs(p) {
		return p
	}
	return filepath.Join(filepath.Dir(configPath), p)
}

// Validate checks what the core would refuse to start with.
func (c *ServerConfig) Validate() error {
	if err := checkSocketAddr("tunnel.listen", c.Tunnel.Listen); err != nil {
		return err
	}
	if c.Tunnel.PublicAddr != "" {
		if _, _, err := net.SplitHostPort(c.Tunnel.PublicAddr); err != nil {
			return fmt.Errorf("tunnel.public_addr must be host:port: %w", err)
		}
	}
	for _, addr := range append(slices.Clone(c.Socks.Listen), c.Socks.TLSListen...) {
		if err := checkSocketAddr("socks listener", addr); err != nil {
			return err
		}
	}
	if err := checkTimeout("handshake timeout", c.Socks.HandshakeTimeoutSecs); err != nil {
		return err
	}
	if err := checkTimeout("connect timeout", c.Socks.ConnectTimeoutSecs); err != nil {
		return err
	}
	if !slices.Contains(Strategies, c.Socks.Balance) {
		return fmt.Errorf("balance must be one of %v", Strategies)
	}
	for _, san := range c.TLS.SANs {
		if san == "" {
			return errors.New("TLS names must not be empty")
		}
	}
	if err := checkSocketAddr("api.listen", c.API.Listen); err != nil {
		return err
	}
	if (c.API.AdminUsername == "") != (c.API.AdminPasswordHash == "") {
		return errors.New("api.admin_username and api.admin_password_hash go together")
	}
	if c.API.SessionTTLSecs < 60 {
		return errors.New("api.session_ttl_secs must be at least 60")
	}
	return nil
}

// Addr is the address nodes dial for this tunnel listener: public_addr when
// set, else listen, with an unspecified listen IP replaced by the host nodes
// already use to reach the panel (the host part of agentPublicAddr).
func (t TunnelSection) Addr(agentPublicAddr string) string {
	if t.PublicAddr != "" {
		return t.PublicAddr
	}
	listen, err := netip.ParseAddrPort(t.Listen)
	if err != nil || !listen.Addr().IsUnspecified() {
		return t.Listen
	}
	host, _, err := net.SplitHostPort(agentPublicAddr)
	if err != nil || host == "" {
		return t.Listen
	}
	return net.JoinHostPort(host, strconv.Itoa(int(listen.Port())))
}

// checkTimeout applies the core's limits for its timeouts.
func checkTimeout(name string, secs uint64) error {
	if secs < 1 || secs > 600 {
		return fmt.Errorf("%s must be between 1 and 600 seconds", name)
	}
	return nil
}

// The core parses listen addresses as Rust SocketAddrs: an IP and a port,
// no host names.
func checkSocketAddr(name, addr string) error {
	if _, err := netip.ParseAddrPort(addr); err != nil {
		return fmt.Errorf("%s %q must be an IP address and port", name, addr)
	}
	return nil
}

// Save writes the config atomically with mode 0600.
func (c *ServerConfig) Save(path string) error {
	data, err := c.Marshal()
	if err != nil {
		return err
	}
	return atomicfile.Write(path, data, 0o600)
}
