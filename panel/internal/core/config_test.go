package core

import (
	"maps"
	"os"
	"path/filepath"
	"reflect"
	"regexp"
	"slices"
	"strings"
	"testing"

	"github.com/pelletier/go-toml/v2"
)

func fullConfig() *ServerConfig {
	cfg := DefaultConfig("/var/lib/wirefox")
	cfg.TLS.Cert = "/etc/wirefox/server.crt"
	cfg.TLS.Key = "/etc/wirefox/server.key"
	cfg.Tunnel.PublicAddr = "relay.example.com:7443"
	cfg.API.AdminUsername = "admin"
	cfg.API.AdminPasswordHash = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA"
	cfg.API.APIKeyHashes = []string{strings.Repeat("ab", 32)}
	cfg.API.CORSOrigins = []string{"https://panel.example.com"}
	return cfg
}

func TestConfigRoundTrip(t *testing.T) {
	for _, cfg := range []*ServerConfig{DefaultConfig("/var/lib/wirefox"), fullConfig()} {
		path := filepath.Join(t.TempDir(), "etc", "server.toml")
		if err := cfg.Save(path); err != nil {
			t.Fatal(err)
		}
		info, err := os.Stat(path)
		if err != nil {
			t.Fatal(err)
		}
		if info.Mode().Perm() != 0o600 {
			t.Fatalf("mode %o", info.Mode().Perm())
		}
		got, err := LoadConfig(path)
		if err != nil {
			t.Fatal(err)
		}
		if !reflect.DeepEqual(got, cfg) {
			t.Fatalf("round trip changed the config:\n got %+v\nwant %+v", got, cfg)
		}
		if err := got.Validate(); err != nil {
			t.Fatal(err)
		}
	}
}

func TestDefaultConfigHasNoAdminLogin(t *testing.T) {
	data, err := DefaultConfig("/var/lib/wirefox").Marshal()
	if err != nil {
		t.Fatal(err)
	}
	for _, key := range []string{"admin_username", "admin_password_hash", "public_addr", "cert", "key"} {
		if regexp.MustCompile(`(?m)^` + key + ` =`).Match(data) {
			t.Errorf("default config sets %s:\n%s", key, data)
		}
	}
}

// The core uses deny_unknown_fields, so the panel must write exactly the
// names in crates/server/src/config.rs. Compare against the Rust source.
func TestFieldNamesMatchRustConfig(t *testing.T) {
	src, err := os.ReadFile("../../../crates/server/src/config.rs")
	if err != nil {
		t.Skipf("Rust sources not available: %v", err)
	}
	rust := rustStructFields(t, string(src))

	var written map[string]any
	data, err := fullConfig().Marshal()
	if err != nil {
		t.Fatal(err)
	}
	if err := toml.Unmarshal(data, &written); err != nil {
		t.Fatal(err)
	}
	check := func(structName string, table map[string]any) {
		t.Helper()
		got := slices.Sorted(maps.Keys(table))
		want := rust[structName]
		if !slices.Equal(got, want) {
			t.Errorf("%s: panel writes %v, core expects %v", structName, got, want)
		}
	}
	check("ServerConfig", written)
	check("TlsSection", written["tls"].(map[string]any))
	check("TunnelSection", written["tunnel"].(map[string]any))
	check("SocksSection", written["socks"].(map[string]any))
	check("ApiSection", written["api"].(map[string]any))
}

func rustStructFields(t *testing.T, src string) map[string][]string {
	t.Helper()
	structs := map[string][]string{}
	structRe := regexp.MustCompile(`(?s)pub struct (\w+) \{(.*?)\n\}`)
	fieldRe := regexp.MustCompile(`(?m)^\s*pub (\w+):`)
	for _, m := range structRe.FindAllStringSubmatch(src, -1) {
		for _, f := range fieldRe.FindAllStringSubmatch(m[2], -1) {
			structs[m[1]] = append(structs[m[1]], f[1])
		}
		slices.Sort(structs[m[1]])
	}
	if len(structs) != 5 {
		t.Fatalf("found structs %v in config.rs", structs)
	}
	return structs
}

func TestParseFillsCoreDefaults(t *testing.T) {
	cfg, err := ParseConfig([]byte(`
[tunnel]
listen = "0.0.0.0:7443"
[socks]
[api]
listen = "127.0.0.1:9090"
`))
	if err != nil {
		t.Fatal(err)
	}
	if cfg.DataDir != "data" || cfg.Socks.Balance != "least-load" || cfg.Socks.HandshakeTimeoutSecs != 10 ||
		cfg.Socks.ConnectTimeoutSecs != 15 || cfg.API.SessionTTLSecs != 43200 {
		t.Fatalf("defaults not applied: %+v", cfg)
	}
}

func TestParseRejectsUnknownFields(t *testing.T) {
	_, err := ParseConfig([]byte(`
[tunnel]
listen = "0.0.0.0:7443"
bogus = 1
[socks]
[api]
listen = "127.0.0.1:9090"
`))
	if err == nil {
		t.Fatal("accepted an unknown field")
	}
}

func TestValidate(t *testing.T) {
	cases := map[string]func(*ServerConfig){
		"host name listener":   func(c *ServerConfig) { c.Tunnel.Listen = "localhost:7443" },
		"public addr no port":  func(c *ServerConfig) { c.Tunnel.PublicAddr = "relay.example.com" },
		"bad socks listener":   func(c *ServerConfig) { c.Socks.TLSListen = []string{"0.0.0.0"} },
		"zero timeout":         func(c *ServerConfig) { c.Socks.ConnectTimeoutSecs = 0 },
		"huge timeout":         func(c *ServerConfig) { c.Socks.HandshakeTimeoutSecs = 601 },
		"unknown balance":      func(c *ServerConfig) { c.Socks.Balance = "fastest" },
		"empty SAN":            func(c *ServerConfig) { c.TLS.SANs = []string{""} },
		"half admin login":     func(c *ServerConfig) { c.API.AdminPasswordHash = "" },
		"short session ttl":    func(c *ServerConfig) { c.API.SessionTTLSecs = 59 },
		"api listen host name": func(c *ServerConfig) { c.API.Listen = "localhost:9090" },
	}
	for name, mutate := range cases {
		cfg := fullConfig()
		mutate(cfg)
		if err := cfg.Validate(); err == nil {
			t.Errorf("%s: accepted", name)
		}
	}
	ipv6 := fullConfig()
	ipv6.Socks.Listen = []string{"[::]:1080"}
	if err := ipv6.Validate(); err != nil {
		t.Fatal(err)
	}
}
