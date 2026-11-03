package core

import (
	"context"
	"path/filepath"
	"testing"
	"time"
)

func TestRunningTunnelWaitsForRestart(t *testing.T) {
	st, ctx := openStore(t), context.Background()
	path := filepath.Join(t.TempDir(), "server.toml")
	cfg := DefaultConfig("/var/lib/wirefox")
	if err := cfg.Save(path); err != nil {
		t.Fatal(err)
	}
	r, err := NewRunningTunnel(ctx, st, path)
	if err != nil {
		t.Fatal(err)
	}
	t0 := time.Unix(1793600000, 0)
	if err := r.Observe(ctx, &Status{UptimeSecs: 100}, t0); err != nil {
		t.Fatal(err)
	}

	cfg.Tunnel.Listen = "0.0.0.0:8443"
	if err := cfg.Save(path); err != nil {
		t.Fatal(err)
	}
	// The same core, a few polls later.
	if err := r.Observe(ctx, &Status{UptimeSecs: 111}, t0.Add(10*time.Second)); err != nil {
		t.Fatal(err)
	}
	if got := r.Section().Listen; got != "0.0.0.0:7443" {
		t.Fatalf("listen before the restart: %s", got)
	}
	// A panel restart doesn't apply the change either.
	if r, err = NewRunningTunnel(ctx, st, path); err != nil {
		t.Fatal(err)
	}
	if got := r.Section().Listen; got != "0.0.0.0:7443" {
		t.Fatalf("listen after a panel restart: %s", got)
	}

	// The core restarted on its own.
	if err := r.Observe(ctx, &Status{UptimeSecs: 2}, t0.Add(20*time.Second)); err != nil {
		t.Fatal(err)
	}
	if got := r.Section().Listen; got != "0.0.0.0:8443" {
		t.Fatalf("listen after the core restarted: %s", got)
	}

	// The panel restarted it.
	cfg.Tunnel.PublicAddr = "relay.example.com:443"
	if err := cfg.Save(path); err != nil {
		t.Fatal(err)
	}
	if err := r.Restarted(ctx); err != nil {
		t.Fatal(err)
	}
	if got := r.Section(); got.PublicAddr != "relay.example.com:443" {
		t.Fatalf("after a restart through the panel: %+v", got)
	}
}
