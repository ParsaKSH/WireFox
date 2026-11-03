package api

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"slices"

	"github.com/ParsaKSH/WireFox/panel/internal/core"
	"github.com/ParsaKSH/WireFox/panel/internal/store"
)

// coreConfig is the part of the core's config the UI edits.
type coreConfig struct {
	TunnelListen         string   `json:"tunnel_listen"`
	TunnelPublicAddr     string   `json:"tunnel_public_addr"`
	SocksListen          []string `json:"socks_listen"`
	SocksTLSListen       []string `json:"socks_tls_listen"`
	HandshakeTimeoutSecs uint64   `json:"handshake_timeout_secs"`
	ConnectTimeoutSecs   uint64   `json:"connect_timeout_secs"`
	Balance              string   `json:"balance"`
	TLSSANs              []string `json:"tls_sans"`
}

func coreConfigOf(c *core.ServerConfig) coreConfig {
	return coreConfig{
		TunnelListen:         c.Tunnel.Listen,
		TunnelPublicAddr:     c.Tunnel.PublicAddr,
		SocksListen:          nonNil(c.Socks.Listen),
		SocksTLSListen:       nonNil(c.Socks.TLSListen),
		HandshakeTimeoutSecs: c.Socks.HandshakeTimeoutSecs,
		ConnectTimeoutSecs:   c.Socks.ConnectTimeoutSecs,
		Balance:              c.Socks.Balance,
		TLSSANs:              nonNil(c.TLS.SANs),
	}
}

func nonNil(s []string) []string {
	if s == nil {
		return []string{}
	}
	return s
}

func (a *API) getCore(w http.ResponseWriter, r *http.Request, _ string) {
	ctx := r.Context()
	cfg, err := core.LoadConfig(a.CoreConfigPath)
	if err != nil {
		a.internalError(w, r, err)
		return
	}
	resp := struct {
		Mode        string            `json:"mode"`
		Unit        *core.UnitStatus  `json:"unit"`
		Reachable   bool              `json:"reachable"`
		Fingerprint string            `json:"fingerprint"`
		Config      coreConfig        `json:"config"`
		Paths       map[string]string `json:"paths"`
	}{
		Mode:   a.Manager.Mode(),
		Unit:   a.unitStatus(ctx),
		Config: coreConfigOf(cfg),
		Paths: map[string]string{
			"config": a.CoreConfigPath,
			"unit":   a.Manager.UnitPath(),
			"binary": a.CoreBinary,
			"data":   cfg.DataPath(a.CoreConfigPath),
		},
	}
	statusCtx, cancel := quick(ctx)
	defer cancel()
	if st, err := a.Core.Status(statusCtx); err == nil {
		resp.Reachable, resp.Fingerprint = true, st.TunnelFingerprint
	}
	writeJSON(w, http.StatusOK, resp)
}

// unitStatus is nil when there is no unit or systemd can't be asked.
func (a *API) unitStatus(ctx context.Context) *core.UnitStatus {
	st, err := a.Manager.Status(ctx)
	if err != nil {
		a.log.Warn("reading core unit status", "err", err)
		return nil
	}
	return st
}

func (a *API) putCoreConfig(w http.ResponseWriter, r *http.Request, _ string) {
	var req coreConfig
	if !decode(w, r, &req) {
		return
	}
	ctx := r.Context()
	cfg, err := core.LoadConfig(a.CoreConfigPath)
	if err != nil {
		a.internalError(w, r, err)
		return
	}
	before := coreConfigOf(cfg)
	cfg.Tunnel.Listen = req.TunnelListen
	cfg.Tunnel.PublicAddr = req.TunnelPublicAddr
	cfg.Socks.Listen = nonNil(req.SocksListen)
	cfg.Socks.TLSListen = nonNil(req.SocksTLSListen)
	cfg.Socks.HandshakeTimeoutSecs = req.HandshakeTimeoutSecs
	cfg.Socks.ConnectTimeoutSecs = req.ConnectTimeoutSecs
	cfg.Socks.Balance = req.Balance
	cfg.TLS.SANs = nonNil(req.TLSSANs)
	if err := cfg.Validate(); err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	if err := cfg.Save(a.CoreConfigPath); err != nil {
		a.internalError(w, r, err)
		return
	}
	after := coreConfigOf(cfg)

	restart := before.TunnelListen != after.TunnelListen || before.TunnelPublicAddr != after.TunnelPublicAddr ||
		!slices.Equal(before.SocksListen, after.SocksListen) ||
		!slices.Equal(before.SocksTLSListen, after.SocksTLSListen) ||
		!slices.Equal(before.TLSSANs, after.TLSSANs)
	liveChanged := before.Balance != after.Balance || before.HandshakeTimeoutSecs != after.HandshakeTimeoutSecs ||
		before.ConnectTimeoutSecs != after.ConnectTimeoutSecs
	_, err = a.Core.UpdateSettings(ctx, core.Settings{
		Balance:              after.Balance,
		HandshakeTimeoutSecs: after.HandshakeTimeoutSecs,
		ConnectTimeoutSecs:   after.ConnectTimeoutSecs,
	})
	if err != nil {
		// Without a running core the file is all that matters; a core that
		// refuses only takes the new values when it restarts.
		a.log.Warn("applying core settings live", "err", err)
		var apiErr *core.APIError
		restart = restart || (liveChanged && errors.As(err, &apiErr))
	}
	// A new tunnel address reaches the nodes once the core runs with it,
	// see core.RunningTunnel.
	a.event(ctx, store.Info, "WireFox server config updated")
	writeJSON(w, http.StatusOK, map[string]bool{"restart_required": restart})
}

func (a *API) coreAction(action func(core.Manager, context.Context) error, done string) func(http.ResponseWriter, *http.Request, string) {
	return func(w http.ResponseWriter, r *http.Request, _ string) {
		err := action(a.Manager, r.Context())
		switch {
		case errors.Is(err, core.ErrNotSupported):
			writeError(w, http.StatusNotImplemented, err.Error())
		case err != nil:
			a.log.Error("controlling the core", "action", done, "err", err)
			writeError(w, http.StatusInternalServerError, err.Error())
		default:
			a.event(r.Context(), store.Info, fmt.Sprintf("WireFox server %s", done))
			writeOK(w)
		}
	}
}

// restartCore restarts the core and then gives the nodes the tunnel address
// it now listens on.
func (a *API) restartCore(m core.Manager, ctx context.Context) error {
	if err := m.Restart(ctx); err != nil {
		return err
	}
	if err := a.Tunnel.Restarted(ctx); err != nil {
		a.log.Warn("reading the core's tunnel listener", "err", err)
		return nil
	}
	if err := a.Agents.SyncEndpoint(ctx); err != nil {
		a.log.Warn("updating node configs", "err", err)
	}
	return nil
}

func (a *API) coreLogs(w http.ResponseWriter, r *http.Request, _ string) {
	entries, err := a.Manager.Logs(r.Context(), queryInt(r, "lines", defaultLogLines, maxLogLines))
	switch {
	case errors.Is(err, core.ErrNotSupported):
		writeError(w, http.StatusNotImplemented, err.Error())
	case err != nil:
		a.internalError(w, r, err)
	default:
		if entries == nil {
			entries = []core.LogEntry{}
		}
		writeJSON(w, http.StatusOK, map[string]any{"entries": entries})
	}
}
