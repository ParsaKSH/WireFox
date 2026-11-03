package api

import (
	"context"
	"net"
	"net/http"
	"strings"

	"github.com/ParsaKSH/WireFox/panel/internal/core"
	"github.com/ParsaKSH/WireFox/panel/internal/store"
)

const (
	// Events shown on the overview, and returned by default from /api/events.
	overviewEvents = 10
	defaultEvents  = 100
)

func (a *API) event(ctx context.Context, level store.Level, msg string) {
	if err := a.Store.AddEvent(context.WithoutCancel(ctx), level, msg); err != nil {
		a.log.Error("recording event", "err", err)
	}
}

type coreSummary struct {
	Reachable         bool   `json:"reachable"`
	Version           string `json:"version"`
	UptimeSecs        uint64 `json:"uptime_secs"`
	NodesTotal        uint64 `json:"nodes_total"`
	NodesOnline       uint64 `json:"nodes_online"`
	UsersTotal        uint64 `json:"users_total"`
	ActiveConnections uint64 `json:"active_connections"`
	BytesUp           uint64 `json:"bytes_up"`
	BytesDown         uint64 `json:"bytes_down"`
	DefaultBalance    string `json:"default_balance"`
}

func (a *API) overview(w http.ResponseWriter, r *http.Request, _ string) {
	ctx := r.Context()
	type unitView struct {
		ActiveState string `json:"active_state"`
		SubState    string `json:"sub_state"`
	}
	type agentsView struct {
		Total     int `json:"total"`
		Connected int `json:"connected"`
	}
	var resp struct {
		Core    *coreSummary  `json:"core"`
		Unit    *unitView     `json:"unit"`
		Agents  agentsView    `json:"agents"`
		History []core.Sample `json:"history"`
		Events  []store.Event `json:"events"`
	}

	statusCtx, cancel := quick(ctx)
	defer cancel()
	if st, err := a.Core.Status(statusCtx); err == nil {
		resp.Core = &coreSummary{
			Reachable:         true,
			Version:           st.Version,
			UptimeSecs:        st.UptimeSecs,
			NodesTotal:        st.NodesTotal,
			NodesOnline:       st.NodesOnline,
			UsersTotal:        st.UsersTotal,
			ActiveConnections: st.ActiveConnections,
			BytesUp:           st.BytesUp,
			BytesDown:         st.BytesDown,
			DefaultBalance:    st.DefaultBalance,
		}
	}
	if st := a.unitStatus(ctx); st != nil {
		resp.Unit = &unitView{ActiveState: st.ActiveState, SubState: st.SubState}
	}
	nodes, err := a.Store.Nodes(ctx)
	if err != nil {
		a.internalError(w, r, err)
		return
	}
	for _, n := range nodes {
		if n.AgentKeyHash != "" {
			resp.Agents.Total++
		}
	}
	resp.Agents.Connected = a.Agents.Connected()
	resp.History = a.Sampler.History()
	if resp.Events, err = a.Store.Events(ctx, overviewEvents); err != nil {
		a.internalError(w, r, err)
		return
	}
	writeJSON(w, http.StatusOK, resp)
}

type settings struct {
	AgentPublicAddr string `json:"agent_public_addr"`
	SocksPublicHost string `json:"socks_public_host"`
}

func (a *API) getSettings(w http.ResponseWriter, r *http.Request, _ string) {
	var s settings
	var err error
	if s.AgentPublicAddr, err = a.Store.Setting(r.Context(), store.SettingAgentPublicAddr); err != nil {
		a.internalError(w, r, err)
		return
	}
	if s.SocksPublicHost, err = a.Store.Setting(r.Context(), store.SettingSocksPublicHost); err != nil {
		a.internalError(w, r, err)
		return
	}
	writeJSON(w, http.StatusOK, s)
}

func (a *API) putSettings(w http.ResponseWriter, r *http.Request, _ string) {
	var s settings
	if !decode(w, r, &s) {
		return
	}
	s.AgentPublicAddr = strings.TrimSpace(s.AgentPublicAddr)
	s.SocksPublicHost = strings.TrimSpace(s.SocksPublicHost)
	if s.AgentPublicAddr != "" {
		host, port, err := net.SplitHostPort(s.AgentPublicAddr)
		if err != nil || host == "" || port == "" {
			writeError(w, http.StatusBadRequest, "agent_public_addr must be host:port")
			return
		}
	}
	if net.ParseIP(s.SocksPublicHost) == nil && strings.ContainsAny(s.SocksPublicHost, " \t/:@[]") {
		writeError(w, http.StatusBadRequest, "socks_public_host must be a host name or IP address")
		return
	}
	ctx := r.Context()
	for key, value := range map[string]string{
		store.SettingAgentPublicAddr: s.AgentPublicAddr,
		store.SettingSocksPublicHost: s.SocksPublicHost,
	} {
		if err := a.Store.SetSetting(ctx, key, value); err != nil {
			a.internalError(w, r, err)
			return
		}
	}
	// The tunnel address nodes get can depend on agent_public_addr.
	if err := a.Agents.SyncEndpoint(ctx); err != nil {
		a.log.Warn("updating node configs", "err", err)
	}
	writeJSON(w, http.StatusOK, s)
}

func (a *API) events(w http.ResponseWriter, r *http.Request, _ string) {
	events, err := a.Store.Events(r.Context(), queryInt(r, "limit", defaultEvents, store.MaxEvents))
	if err != nil {
		a.internalError(w, r, err)
		return
	}
	writeJSON(w, http.StatusOK, events)
}
