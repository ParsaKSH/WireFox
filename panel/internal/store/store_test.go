package store

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"testing"
	"time"
)

func open(t *testing.T) *Store {
	t.Helper()
	s, err := Open(filepath.Join(t.TempDir(), "panel db.sqlite"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { s.Close() })
	return s
}

func TestReopenKeepsData(t *testing.T) {
	path := filepath.Join(t.TempDir(), "panel.db")
	s, err := Open(path)
	if err != nil {
		t.Fatal(err)
	}
	if info, err := os.Stat(path); err != nil || info.Mode().Perm() != 0o600 {
		t.Fatalf("database mode: %v, %v", info.Mode(), err)
	}
	ctx := context.Background()
	if err := s.SetSetting(ctx, "a", "1"); err != nil {
		t.Fatal(err)
	}
	s.Close()

	s, err = Open(path)
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	if v, err := s.Setting(ctx, "a"); err != nil || v != "1" {
		t.Fatalf("setting %q, %v", v, err)
	}
}

func TestSettings(t *testing.T) {
	s, ctx := open(t), context.Background()
	if v, err := s.Setting(ctx, "missing"); err != nil || v != "" {
		t.Fatalf("missing setting: %q, %v", v, err)
	}
	for _, v := range []string{"x", "y"} {
		if err := s.SetSetting(ctx, "k", v); err != nil {
			t.Fatal(err)
		}
		if got, _ := s.Setting(ctx, "k"); got != v {
			t.Fatalf("got %q, want %q", got, v)
		}
	}
}

func TestSessions(t *testing.T) {
	s, ctx := open(t), context.Background()
	now := time.Now()
	if err := s.SetAdmin(ctx, "admin", "hash"); err != nil {
		t.Fatal(err)
	}
	if err := s.CreateSession(ctx, "live", "admin", now.Add(time.Hour)); err != nil {
		t.Fatal(err)
	}
	if err := s.CreateSession(ctx, "other", "admin", now.Add(time.Hour)); err != nil {
		t.Fatal(err)
	}
	if err := s.CreateSession(ctx, "old", "admin", now.Add(-time.Second)); err != nil {
		t.Fatal(err)
	}
	if u, err := s.SessionUser(ctx, "live", now); err != nil || u != "admin" {
		t.Fatalf("live session: %q, %v", u, err)
	}
	if _, err := s.SessionUser(ctx, "old", now); !errors.Is(err, ErrNotFound) {
		t.Fatalf("expired session: %v", err)
	}
	if err := s.DeleteUserSessions(ctx, "admin", "live"); err != nil {
		t.Fatal(err)
	}
	if _, err := s.SessionUser(ctx, "other", now); !errors.Is(err, ErrNotFound) {
		t.Fatalf("other session survived: %v", err)
	}
	if _, err := s.SessionUser(ctx, "live", now); err != nil {
		t.Fatalf("kept session is gone: %v", err)
	}
}

func TestEventsAreCapped(t *testing.T) {
	s, ctx := open(t), context.Background()
	for i := 0; i < MaxEvents+3; i++ {
		if err := s.AddEvent(ctx, Info, "event"); err != nil {
			t.Fatal(err)
		}
	}
	if err := s.AddEvent(ctx, Warn, "last"); err != nil {
		t.Fatal(err)
	}
	all, err := s.Events(ctx, 2*MaxEvents)
	if err != nil {
		t.Fatal(err)
	}
	if len(all) != MaxEvents {
		t.Fatalf("kept %d events", len(all))
	}
	if all[0].Message != "last" || all[0].Level != Warn || all[0].ID != MaxEvents+4 {
		t.Fatalf("newest event %+v", all[0])
	}
}

func TestNodes(t *testing.T) {
	s, ctx := open(t), context.Background()
	n := NewNode("b")
	n.Token, n.AgentKeyHash = "token", "hash-b"
	if err := s.CreateNode(ctx, n); err != nil {
		t.Fatal(err)
	}
	if err := s.CreateNode(ctx, NewNode("b")); !errors.Is(err, ErrExists) {
		t.Fatalf("duplicate id: %v", err)
	}
	// Nodes without a key may coexist; equal key hashes may not.
	if err := s.CreateNode(ctx, NewNode("a")); err != nil {
		t.Fatal(err)
	}
	if err := s.CreateNode(ctx, NewNode("c")); err != nil {
		t.Fatal(err)
	}
	dup := NewNode("d")
	dup.AgentKeyHash = "hash-b"
	if err := s.CreateNode(ctx, dup); !errors.Is(err, ErrExists) {
		t.Fatalf("duplicate key hash: %v", err)
	}

	got, err := s.NodeByKeyHash(ctx, "hash-b")
	if err != nil || got.ID != "b" || got.PoolSize != DefaultPoolSize || got.Agent != nil {
		t.Fatalf("by key hash: %+v, %v", got, err)
	}
	if _, err := s.NodeByKeyHash(ctx, ""); !errors.Is(err, ErrNotFound) {
		t.Fatalf("empty hash matched: %v", err)
	}

	updated, err := s.UpdateNode(ctx, "b", func(n *Node) error {
		n.Egress = Egress{Interface: "eth1", SourceIP: "192.0.2.1"}
		n.AllowLocal = true
		n.ConfigVersion++
		return nil
	})
	if err != nil || updated.ConfigVersion != 2 {
		t.Fatalf("update: %+v, %v", updated, err)
	}
	failed := errors.New("nope")
	if _, err := s.UpdateNode(ctx, "b", func(n *Node) error { n.PoolSize = 1; return failed }); err != failed {
		t.Fatalf("update error: %v", err)
	}
	if _, err := s.UpdateNode(ctx, "zz", func(*Node) error { return nil }); !errors.Is(err, ErrNotFound) {
		t.Fatalf("update missing: %v", err)
	}

	info := &AgentInfo{Hostname: "router", Interfaces: []Interface{{Name: "eth1", Up: true, Addrs: []string{"192.0.2.1"}}}}
	if err := s.SetAgentInfo(ctx, "b", info, 42); err != nil {
		t.Fatal(err)
	}
	if err := s.BumpAllVersions(ctx); err != nil {
		t.Fatal(err)
	}
	got, err = s.Node(ctx, "b")
	if err != nil {
		t.Fatal(err)
	}
	if got.PoolSize != DefaultPoolSize || !got.AllowLocal || got.Egress.SourceIP != "192.0.2.1" ||
		got.ConfigVersion != 3 || got.LastSeen != 42 || got.Agent.Interfaces[0].Name != "eth1" {
		t.Fatalf("stored node %+v", got)
	}

	all, err := s.Nodes(ctx)
	if err != nil || len(all) != 3 || all[0].ID != "a" || all[2].ID != "c" {
		t.Fatalf("nodes %v, %v", all, err)
	}
	if err := s.DeleteNode(ctx, "b"); err != nil {
		t.Fatal(err)
	}
	if err := s.DeleteNode(ctx, "b"); !errors.Is(err, ErrNotFound) {
		t.Fatalf("second delete: %v", err)
	}
}
