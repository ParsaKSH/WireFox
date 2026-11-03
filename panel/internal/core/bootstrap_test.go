package core

import (
	"context"
	"os"
	"path/filepath"
	"slices"
	"testing"

	"github.com/ParsaKSH/WireFox/panel/internal/keys"
	"github.com/ParsaKSH/WireFox/panel/internal/store"
)

func openStore(t *testing.T) *store.Store {
	t.Helper()
	st, err := store.Open(filepath.Join(t.TempDir(), "panel.db"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { st.Close() })
	return st
}

func TestPrepareCreatesConfig(t *testing.T) {
	st, ctx := openStore(t), context.Background()
	path := filepath.Join(t.TempDir(), "wirefox", "server.toml")

	cfg, apiKey, keyAdded, err := Prepare(ctx, st, path, "/var/lib/wirefox")
	if err != nil {
		t.Fatal(err)
	}
	if keyAdded {
		t.Error("a new config doesn't need a restart")
	}
	if stored, _ := st.Setting(ctx, store.SettingCoreAPIKey); stored != apiKey || len(apiKey) != 64 {
		t.Fatalf("API key %q not stored (%q)", apiKey, stored)
	}
	onDisk, err := LoadConfig(path)
	if err != nil {
		t.Fatal(err)
	}
	if !slices.Equal(onDisk.API.APIKeyHashes, []string{keys.Hash(apiKey)}) || onDisk.API.AdminUsername != "" {
		t.Fatalf("api section: %+v", onDisk.API)
	}
	if onDisk.Tunnel.Listen != "0.0.0.0:7443" || !slices.Equal(onDisk.Socks.Listen, []string{"0.0.0.0:1080"}) ||
		!slices.Equal(onDisk.Socks.TLSListen, []string{"0.0.0.0:1443"}) || onDisk.API.Listen != "127.0.0.1:9090" ||
		onDisk.DataDir != "/var/lib/wirefox" {
		t.Fatalf("unexpected defaults: %+v", onDisk)
	}
	if cfg.API.Listen != onDisk.API.Listen {
		t.Fatal("returned config differs from the file")
	}
	if err := onDisk.Validate(); err != nil {
		t.Fatal(err)
	}

	// Next start: nothing to change.
	before, _ := os.ReadFile(path)
	_, again, keyAdded, err := Prepare(ctx, st, path, "/elsewhere")
	if err != nil || again != apiKey || keyAdded {
		t.Fatalf("second prepare: %q, %v, %v", again, keyAdded, err)
	}
	if after, _ := os.ReadFile(path); string(after) != string(before) {
		t.Fatal("config rewritten without a change")
	}
}

func TestPrepareAdoptsExistingConfig(t *testing.T) {
	st, ctx := openStore(t), context.Background()
	path := filepath.Join(t.TempDir(), "server.toml")
	existing := fullConfig()
	if err := existing.Save(path); err != nil {
		t.Fatal(err)
	}

	_, apiKey, keyAdded, err := Prepare(ctx, st, path, "/var/lib/wirefox")
	if err != nil {
		t.Fatal(err)
	}
	if !keyAdded {
		t.Error("an adopted core must restart to accept the key")
	}
	got, err := LoadConfig(path)
	if err != nil {
		t.Fatal(err)
	}
	want := fullConfig()
	want.API.APIKeyHashes = append(want.API.APIKeyHashes, keys.Hash(apiKey))
	if !slices.Equal(got.API.APIKeyHashes, want.API.APIKeyHashes) || got.API.AdminUsername != "admin" ||
		got.Tunnel.PublicAddr != want.Tunnel.PublicAddr {
		t.Fatalf("adopted config changed more than the key:\n got %+v\nwant %+v", got, want)
	}
}
