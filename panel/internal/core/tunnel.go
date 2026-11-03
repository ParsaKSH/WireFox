package core

import (
	"context"
	"encoding/json"
	"sync"
	"time"

	"github.com/ParsaKSH/WireFox/panel/internal/store"
)

// Statuses whose start times (now - uptime) are this close come from the
// same core process: uptime is in whole seconds and a poll takes a moment.
const sameStartSlack = 3

// RunningTunnel remembers the tunnel listener of the running core. A new
// tunnel.listen or tunnel.public_addr in the config file only takes effect
// when the core restarts, and until then nodes must keep dialing the old
// address. The record lives in the panel database, so a pending change also
// stays pending across panel restarts.
type RunningTunnel struct {
	store      *store.Store
	configPath string

	mu    sync.Mutex
	state runningTunnel
}

type runningTunnel struct {
	Listen     string `json:"listen"`
	PublicAddr string `json:"public_addr"`
	// Unix seconds the core started at; 0 until a status shows it.
	StartedAt int64 `json:"started_at"`
}

// NewRunningTunnel loads the record, or starts one from the config file.
func NewRunningTunnel(ctx context.Context, st *store.Store, configPath string) (*RunningTunnel, error) {
	r := &RunningTunnel{store: st, configPath: configPath}
	data, err := st.Setting(ctx, store.SettingCoreTunnel)
	if err != nil {
		return nil, err
	}
	if data != "" && json.Unmarshal([]byte(data), &r.state) == nil {
		return r, nil
	}
	// Without a record the file is the best guess.
	return r, r.reload(ctx, 0)
}

// Section is the tunnel listener the running core uses.
func (r *RunningTunnel) Section() TunnelSection {
	r.mu.Lock()
	defer r.mu.Unlock()
	return TunnelSection{Listen: r.state.Listen, PublicAddr: r.state.PublicAddr}
}

// Restarted records that the core was just restarted with the config file as
// it is now.
func (r *RunningTunnel) Restarted(ctx context.Context) error {
	r.mu.Lock()
	defer r.mu.Unlock()
	return r.reload(ctx, 0)
}

// Observe takes a status of the core taken at now. When the core started
// since the last one, it now runs with the config file as it is.
func (r *RunningTunnel) Observe(ctx context.Context, status *Status, now time.Time) error {
	started := now.Unix() - int64(status.UptimeSecs)
	r.mu.Lock()
	defer r.mu.Unlock()
	switch {
	case r.state.StartedAt == 0:
		r.state.StartedAt = started
		return r.save(ctx)
	case abs(started-r.state.StartedAt) <= sameStartSlack:
		return nil
	}
	return r.reload(ctx, started)
}

// reload reads the tunnel listener from the config file; call with mu held.
func (r *RunningTunnel) reload(ctx context.Context, startedAt int64) error {
	cfg, err := LoadConfig(r.configPath)
	if err != nil {
		return err
	}
	r.state = runningTunnel{Listen: cfg.Tunnel.Listen, PublicAddr: cfg.Tunnel.PublicAddr, StartedAt: startedAt}
	return r.save(ctx)
}

func (r *RunningTunnel) save(ctx context.Context) error {
	data, err := json.Marshal(r.state)
	if err != nil {
		return err
	}
	return r.store.SetSetting(ctx, store.SettingCoreTunnel, string(data))
}

func abs(n int64) int64 {
	if n < 0 {
		return -n
	}
	return n
}
