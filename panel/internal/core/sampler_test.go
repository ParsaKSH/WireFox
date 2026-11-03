package core

import (
	"context"
	"errors"
	"testing"
	"time"
)

func TestSamplerRates(t *testing.T) {
	s := NewSampler(nil, SampleInterval, nil)
	t0 := time.Unix(1793600000, 0)
	s.add(&Status{UptimeSecs: 100, BytesUp: 1000, BytesDown: 5000, ActiveConnections: 1}, t0)
	if h := s.History(); len(h) != 0 {
		t.Fatalf("a single status gave samples: %+v", h)
	}
	s.add(&Status{UptimeSecs: 105, BytesUp: 6000, BytesDown: 55000, ActiveConnections: 3}, t0.Add(5*time.Second))
	// Traffic reset: counters drop while the core keeps running.
	s.add(&Status{UptimeSecs: 110, BytesUp: 100, BytesDown: 100, ActiveConnections: 3}, t0.Add(10*time.Second))
	// The core restarted and counted 2000 bytes since.
	s.add(&Status{UptimeSecs: 3, BytesUp: 2000, BytesDown: 0, ActiveConnections: 0}, t0.Add(15*time.Second))

	want := []Sample{
		{T: 1793600005, UpBps: 1000, DownBps: 10000, Connections: 3},
		{T: 1793600010, UpBps: 0, DownBps: 0, Connections: 3},
		{T: 1793600015, UpBps: 400, DownBps: 0, Connections: 0},
	}
	got := s.History()
	if len(got) != len(want) {
		t.Fatalf("got %+v, want %+v", got, want)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Errorf("sample %d: got %+v, want %+v", i, got[i], want[i])
		}
	}
}

func TestSamplerKeepsAnHour(t *testing.T) {
	s := NewSampler(nil, SampleInterval, nil)
	t0 := time.Unix(1793600000, 0)
	for i := range maxSamples + 50 {
		s.add(&Status{UptimeSecs: uint64(i)}, t0.Add(time.Duration(i)*SampleInterval))
	}
	h := s.History()
	if len(h) != maxSamples || h[len(h)-1].T != t0.Add((maxSamples+49)*SampleInterval).Unix() {
		t.Fatalf("kept %d samples ending at %d", len(h), h[len(h)-1].T)
	}
}

func TestSamplerPoll(t *testing.T) {
	fail := false
	var seen []string
	s := NewSampler(func(context.Context) (*Status, error) {
		if fail {
			return nil, errors.New("down")
		}
		return &Status{TunnelFingerprint: "fp"}, nil
	}, time.Second, func(_ context.Context, st *Status) { seen = append(seen, st.TunnelFingerprint) })
	ctx := context.Background()
	s.poll(ctx)
	fail = true
	s.poll(ctx)
	if s.prev != nil {
		t.Fatal("an unreachable core should restart the rate calculation")
	}
	if len(seen) != 1 || seen[0] != "fp" {
		t.Fatalf("callback saw %v", seen)
	}
}

func TestTunnelAddr(t *testing.T) {
	cases := []struct {
		listen, public, agentAddr, want string
	}{
		{"0.0.0.0:7443", "relay.example.com:443", "panel.example.com:50051", "relay.example.com:443"},
		{"0.0.0.0:7443", "", "panel.example.com:50051", "panel.example.com:7443"},
		{"[::]:7443", "", "[2001:db8::1]:50051", "[2001:db8::1]:7443"},
		{"0.0.0.0:7443", "", "", "0.0.0.0:7443"},
		{"203.0.113.5:7443", "", "panel.example.com:50051", "203.0.113.5:7443"},
	}
	for _, c := range cases {
		cfg := DefaultConfig("/var/lib/wirefox")
		cfg.Tunnel.Listen, cfg.Tunnel.PublicAddr = c.listen, c.public
		if got := cfg.TunnelAddr(c.agentAddr); got != c.want {
			t.Errorf("listen %s public %q agent %q: got %s, want %s", c.listen, c.public, c.agentAddr, got, c.want)
		}
	}
}
