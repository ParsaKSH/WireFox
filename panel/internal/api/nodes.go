package api

import (
	"context"
	"errors"
	"fmt"
	"net"
	"net/http"
	"net/netip"
	"slices"
	"strings"
	"time"

	"github.com/ParsaKSH/WireFox/panel/internal/agent"
	"github.com/ParsaKSH/WireFox/panel/internal/agentpb"
	"github.com/ParsaKSH/WireFox/panel/internal/core"
	"github.com/ParsaKSH/WireFox/panel/internal/keys"
	"github.com/ParsaKSH/WireFox/panel/internal/store"
)

type nodeView struct {
	ID           string        `json:"id"`
	Name         string        `json:"name"`
	Enabled      bool          `json:"enabled"`
	CreatedAt    int64         `json:"created_at"`
	Core         coreNodeView  `json:"core"`
	Agent        *agentView    `json:"agent"`
	Config       tunnelSection `json:"config"`
	KeyCreatedAt int64         `json:"key_created_at"`
}

type coreNodeView struct {
	Online      bool         `json:"online"`
	RTTMs       float64      `json:"rtt_ms"`
	IdlePool    int          `json:"idle_pool"`
	Remote      string       `json:"remote"`
	ConnectedAt int64        `json:"connected_at"`
	Traffic     core.Traffic `json:"traffic"`
}

type agentView struct {
	Connected bool  `json:"connected"`
	LastSeen  int64 `json:"last_seen"`
	store.AgentInfo
}

type tunnelSection struct {
	Version            uint64       `json:"version"`
	PoolSize           uint32       `json:"pool_size"`
	MaxExtra           uint32       `json:"max_extra"`
	ConnectTimeoutSecs uint32       `json:"connect_timeout_secs"`
	AllowLocal         bool         `json:"allow_local"`
	Egress             store.Egress `json:"egress"`
	Uplink             store.Egress `json:"uplink"`
}

// view merges what the core and the panel know about a node; either may be nil.
func (a *API) view(id string, cn *core.Node, local *store.Node) nodeView {
	if local == nil {
		local = store.NewNode(id)
		local.ConfigVersion = 0
	}
	v := nodeView{
		ID:        id,
		Name:      local.Name,
		Enabled:   local.Enabled,
		CreatedAt: local.CreatedAt,
		Config: tunnelSection{
			Version:            local.ConfigVersion,
			PoolSize:           local.PoolSize,
			MaxExtra:           local.MaxExtra,
			ConnectTimeoutSecs: local.ConnectTimeoutSecs,
			AllowLocal:         local.AllowLocal,
			Egress:             local.Egress,
			Uplink:             local.Uplink,
		},
		KeyCreatedAt: local.KeyCreatedAt,
	}
	if cn != nil {
		v.Name, v.Enabled, v.CreatedAt = cn.Name, cn.Enabled, cn.CreatedAt
		v.Core.Traffic = cn.Traffic
		if cn.Online && cn.Session != nil {
			v.Core.Online = true
			v.Core.RTTMs = cn.Session.RTTMs
			v.Core.IdlePool = cn.Session.IdlePool
			v.Core.Remote = cn.Session.Remote
			v.Core.ConnectedAt = cn.Session.ConnectedAt
		}
	}
	if live := a.Agents.Live(id); live != nil {
		v.Agent = &agentView{Connected: true, LastSeen: live.LastSeen, AgentInfo: live.Info}
	} else if local.Agent != nil {
		v.Agent = &agentView{LastSeen: local.LastSeen, AgentInfo: *local.Agent}
	}
	if v.Agent != nil && v.Agent.Interfaces == nil {
		v.Agent.Interfaces = []store.Interface{}
	}
	return v
}

// syncMirror copies the core's name and enabled flag into the panel's record.
// The enabled flag is part of the node's TunnelConfig, so a change there
// (made through the core API directly) is pushed to the node.
func (a *API) syncMirror(ctx context.Context, cn *core.Node, local *store.Node) *store.Node {
	if local == nil || (cn.Name == local.Name && cn.Enabled == local.Enabled && cn.CreatedAt == local.CreatedAt) {
		return local
	}
	enabledChanged := cn.Enabled != local.Enabled
	updated, err := a.Store.UpdateNode(ctx, local.ID, func(n *store.Node) error {
		n.Name, n.Enabled, n.CreatedAt = cn.Name, cn.Enabled, cn.CreatedAt
		if enabledChanged {
			n.ConfigVersion++
		}
		return nil
	})
	if err != nil {
		a.log.Warn("updating node mirror", "node", local.ID, "err", err)
		return local
	}
	if enabledChanged {
		a.Agents.PushConfig(local.ID)
	}
	return updated
}

func (a *API) listNodes(w http.ResponseWriter, r *http.Request, _ string) {
	ctx := r.Context()
	locals, err := a.Store.Nodes(ctx)
	if err != nil {
		a.internalError(w, r, err)
		return
	}
	byID := map[string]*store.Node{}
	for _, n := range locals {
		byID[n.ID] = n
	}
	// With the core down, the panel still shows what it knows itself.
	coreCtx, cancel := quick(ctx)
	defer cancel()
	coreNodes, err := a.Core.Nodes(coreCtx)
	if err != nil {
		a.log.Warn("listing core nodes", "err", err)
	}
	views := []nodeView{}
	seen := map[string]bool{}
	for i := range coreNodes {
		cn := &coreNodes[i]
		seen[cn.ID] = true
		views = append(views, a.view(cn.ID, cn, a.syncMirror(ctx, cn, byID[cn.ID])))
	}
	for _, n := range locals {
		if !seen[n.ID] {
			views = append(views, a.view(n.ID, nil, n))
		}
	}
	slices.SortFunc(views, func(a, b nodeView) int { return strings.Compare(a.ID, b.ID) })
	writeJSON(w, http.StatusOK, views)
}

// lookup finds a node in the core and the panel. It answers 404 itself when
// neither knows it; cn is nil when only the panel does.
func (a *API) lookup(w http.ResponseWriter, r *http.Request, id string) (cn *core.Node, local *store.Node, ok bool) {
	ctx := r.Context()
	local, err := a.Store.Node(ctx, id)
	if err != nil && !errors.Is(err, store.ErrNotFound) {
		a.internalError(w, r, err)
		return nil, nil, false
	}
	cn, err = a.Core.Node(ctx, id)
	switch {
	case core.IsNotFound(err) && local != nil:
		return nil, local, true
	case err != nil && local != nil:
		a.log.Warn("getting core node", "node", id, "err", err)
		return nil, local, true
	case err != nil:
		a.coreError(w, err)
		return nil, nil, false
	}
	return cn, a.syncMirror(ctx, cn, local), true
}

func (a *API) getNode(w http.ResponseWriter, r *http.Request, _ string) {
	id := r.PathValue("id")
	cn, local, ok := a.lookup(w, r, id)
	if !ok {
		return
	}
	writeJSON(w, http.StatusOK, a.view(id, cn, local))
}

// nodeKey builds the key for a fresh agent secret. Until agent_public_addr is
// set, nodes are sent to the host the admin reached the panel at; that
// address is then saved as the setting, so keys and tunnel configs agree.
func (a *API) nodeKey(r *http.Request, secret string) (string, error) {
	ctx := r.Context()
	addr, err := a.Store.Setting(ctx, store.SettingAgentPublicAddr)
	if err != nil {
		return "", err
	}
	if addr == "" {
		host, _, err := net.SplitHostPort(r.Host)
		if err != nil {
			host = r.Host
		}
		addr = net.JoinHostPort(host, a.GRPCPort)
		if err := a.Store.SetSetting(ctx, store.SettingAgentPublicAddr, addr); err != nil {
			return "", err
		}
		if err := a.Agents.SyncEndpoint(ctx); err != nil {
			a.log.Warn("updating node configs", "err", err)
		}
	}
	return keys.NodeKey{Panel: addr, Fingerprint: a.Cert.Fingerprint, Secret: secret}.Encode(), nil
}

func (a *API) createNode(w http.ResponseWriter, r *http.Request, _ string) {
	var req struct {
		ID   string `json:"id"`
		Name string `json:"name"`
	}
	if !decode(w, r, &req) {
		return
	}
	ctx := r.Context()
	// Built first: it may settle agent_public_addr, which the new node's
	// first config then already uses.
	secret := keys.NewSecret()
	key, err := a.nodeKey(r, secret)
	if err != nil {
		a.internalError(w, r, err)
		return
	}
	cn, token, err := a.Core.CreateNode(ctx, req.ID, req.Name)
	if err != nil {
		a.coreError(w, err)
		return
	}
	node := store.NewNode(cn.ID)
	node.Name, node.Enabled, node.CreatedAt = cn.Name, cn.Enabled, cn.CreatedAt
	node.Token = token
	node.AgentKeyHash = keys.Hash(secret)
	node.KeyCreatedAt = time.Now().Unix()
	// A leftover record from a node deleted in the core behind our back.
	if err := a.Store.DeleteNode(ctx, cn.ID); err != nil && !errors.Is(err, store.ErrNotFound) {
		a.internalError(w, r, err)
		return
	}
	if err := a.Store.CreateNode(ctx, node); err != nil {
		if delErr := a.Core.DeleteNode(context.WithoutCancel(ctx), cn.ID); delErr != nil {
			a.log.Error("removing core node after a failed create", "node", cn.ID, "err", delErr)
		}
		a.internalError(w, r, err)
		return
	}
	a.event(ctx, store.Info, fmt.Sprintf("node %s created", cn.ID))
	writeJSON(w, http.StatusOK, map[string]any{"node": a.view(cn.ID, cn, node), "key": key})
}

type nodePatch struct {
	Name               *string       `json:"name"`
	Enabled            *bool         `json:"enabled"`
	PoolSize           *uint32       `json:"pool_size"`
	MaxExtra           *uint32       `json:"max_extra"`
	ConnectTimeoutSecs *uint32       `json:"connect_timeout_secs"`
	AllowLocal         *bool         `json:"allow_local"`
	Egress             *store.Egress `json:"egress"`
	Uplink             *store.Egress `json:"uplink"`
}

// The limits wirefox-node enforces for its own config.
func (p *nodePatch) validate() error {
	switch {
	case p.Name != nil && *p.Name == "":
		return errors.New("name must not be empty")
	case p.PoolSize != nil && (*p.PoolSize < 1 || *p.PoolSize > 4096):
		return errors.New("pool_size must be between 1 and 4096")
	case p.MaxExtra != nil && *p.MaxExtra > 65536:
		return errors.New("max_extra must be at most 65536")
	case p.ConnectTimeoutSecs != nil && (*p.ConnectTimeoutSecs < 1 || *p.ConnectTimeoutSecs > 600):
		return errors.New("connect_timeout_secs must be between 1 and 600")
	}
	for name, e := range map[string]*store.Egress{"egress": p.Egress, "uplink": p.Uplink} {
		if e == nil || e.SourceIP == "" {
			continue
		}
		if _, err := netip.ParseAddr(e.SourceIP); err != nil {
			return fmt.Errorf("%s.source_ip must be an IP address", name)
		}
	}
	return nil
}

// apply changes n and reports whether its TunnelConfig changed.
func (p *nodePatch) apply(n *store.Node) bool {
	before := *n
	if p.Enabled != nil {
		n.Enabled = *p.Enabled
	}
	if p.PoolSize != nil {
		n.PoolSize = *p.PoolSize
	}
	if p.MaxExtra != nil {
		n.MaxExtra = *p.MaxExtra
	}
	if p.ConnectTimeoutSecs != nil {
		n.ConnectTimeoutSecs = *p.ConnectTimeoutSecs
	}
	if p.AllowLocal != nil {
		n.AllowLocal = *p.AllowLocal
	}
	if p.Egress != nil {
		n.Egress = *p.Egress
	}
	if p.Uplink != nil {
		n.Uplink = *p.Uplink
	}
	if p.Name != nil {
		n.Name = *p.Name
	}
	changed := n.Enabled != before.Enabled || n.PoolSize != before.PoolSize || n.MaxExtra != before.MaxExtra ||
		n.ConnectTimeoutSecs != before.ConnectTimeoutSecs || n.AllowLocal != before.AllowLocal ||
		n.Egress != before.Egress || n.Uplink != before.Uplink
	if changed {
		n.ConfigVersion++
	}
	return changed
}

func (a *API) updateNode(w http.ResponseWriter, r *http.Request, _ string) {
	id := r.PathValue("id")
	var patch nodePatch
	if !decode(w, r, &patch) {
		return
	}
	if err := patch.validate(); err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	cn, local, ok := a.lookup(w, r, id)
	if !ok {
		return
	}
	ctx := r.Context()
	if patch.Name != nil || patch.Enabled != nil {
		if cn == nil {
			writeError(w, http.StatusConflict, "the node no longer exists in the WireFox server")
			return
		}
		var err error
		if cn, err = a.Core.UpdateNode(ctx, id, core.NodePatch{Name: patch.Name, Enabled: patch.Enabled}); err != nil {
			a.coreError(w, err)
			return
		}
	}
	if local == nil {
		// A node created in the core directly: the panel adopts it.
		local = store.NewNode(id)
		local.Name, local.Enabled, local.CreatedAt = cn.Name, cn.Enabled, cn.CreatedAt
		if err := a.Store.CreateNode(ctx, local); err != nil && !errors.Is(err, store.ErrExists) {
			a.internalError(w, r, err)
			return
		}
	}
	var changed bool
	local, err := a.Store.UpdateNode(ctx, id, func(n *store.Node) error {
		changed = patch.apply(n)
		return nil
	})
	if err != nil {
		a.internalError(w, r, err)
		return
	}
	if changed {
		a.Agents.PushConfig(id)
	}
	writeJSON(w, http.StatusOK, a.view(id, cn, local))
}

func (a *API) deleteNode(w http.ResponseWriter, r *http.Request, _ string) {
	id := r.PathValue("id")
	ctx := r.Context()
	coreErr := a.Core.DeleteNode(ctx, id)
	if coreErr != nil && !core.IsNotFound(coreErr) {
		a.coreError(w, coreErr)
		return
	}
	localErr := a.Store.DeleteNode(ctx, id)
	if localErr != nil && !errors.Is(localErr, store.ErrNotFound) {
		a.internalError(w, r, localErr)
		return
	}
	if coreErr != nil && localErr != nil {
		writeError(w, http.StatusNotFound, fmt.Sprintf("node %q not found", id))
		return
	}
	a.Agents.Disconnect(id)
	a.event(ctx, store.Info, fmt.Sprintf("node %s deleted", id))
	writeOK(w)
}

func (a *API) newNodeKey(w http.ResponseWriter, r *http.Request, _ string) {
	id := r.PathValue("id")
	cn, local, ok := a.lookup(w, r, id)
	if !ok {
		return
	}
	ctx := r.Context()
	if local == nil {
		local = store.NewNode(id)
		local.Name, local.Enabled, local.CreatedAt = cn.Name, cn.Enabled, cn.CreatedAt
		if err := a.Store.CreateNode(ctx, local); err != nil && !errors.Is(err, store.ErrExists) {
			a.internalError(w, r, err)
			return
		}
	}
	// The panel only learns a node's tunnel token when it creates one.
	var token string
	if local.Token == "" {
		if cn == nil {
			writeError(w, http.StatusConflict, "the node no longer exists in the WireFox server")
			return
		}
		var err error
		if token, err = a.Core.RotateToken(ctx, id); err != nil {
			a.coreError(w, err)
			return
		}
	}
	secret := keys.NewSecret()
	_, err := a.Store.UpdateNode(ctx, id, func(n *store.Node) error {
		n.AgentKeyHash = keys.Hash(secret)
		n.KeyCreatedAt = time.Now().Unix()
		if token != "" {
			n.Token = token
			n.ConfigVersion++
		}
		return nil
	})
	if err != nil {
		a.internalError(w, r, err)
		return
	}
	a.Agents.Disconnect(id)
	key, err := a.nodeKey(r, secret)
	if err != nil {
		a.internalError(w, r, err)
		return
	}
	a.event(ctx, store.Info, fmt.Sprintf("new key issued for node %s", id))
	writeJSON(w, http.StatusOK, map[string]string{"key": key})
}

// command sends cmd to the node's agent and maps the failures to statuses.
func (a *API) command(w http.ResponseWriter, r *http.Request, cmd *agentpb.Command) (*agentpb.CommandResult, bool) {
	res, err := a.Agents.Send(r.Context(), r.PathValue("id"), cmd)
	var cmdErr *agent.CommandError
	switch {
	case err == nil:
		return res, true
	case errors.Is(err, agent.ErrNotConnected):
		writeError(w, http.StatusConflict, err.Error())
	case errors.Is(err, agent.ErrTimeout):
		writeError(w, http.StatusGatewayTimeout, err.Error())
	case errors.As(err, &cmdErr):
		writeError(w, http.StatusBadGateway, cmdErr.Error())
	default:
		writeError(w, http.StatusServiceUnavailable, err.Error())
	}
	return nil, false
}

func (a *API) restartNode(w http.ResponseWriter, r *http.Request, _ string) {
	cmd := &agentpb.Command{Kind: &agentpb.Command_RestartTunnel{RestartTunnel: &agentpb.RestartTunnel{}}}
	if _, ok := a.command(w, r, cmd); ok {
		writeOK(w)
	}
}

func (a *API) nodeInterfaces(w http.ResponseWriter, r *http.Request, _ string) {
	cmd := &agentpb.Command{Kind: &agentpb.Command_ListInterfaces{ListInterfaces: &agentpb.ListInterfaces{}}}
	res, ok := a.command(w, r, cmd)
	if !ok {
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"interfaces": agent.Interfaces(res.GetInterfaces().GetInterfaces())})
}

type logLine struct {
	UnixMs  int64  `json:"unix_ms"`
	Level   string `json:"level"`
	Message string `json:"message"`
}

func (a *API) nodeLogs(w http.ResponseWriter, r *http.Request, _ string) {
	lines := queryInt(r, "lines", 200, 5000)
	cmd := &agentpb.Command{Kind: &agentpb.Command_FetchLogs{FetchLogs: &agentpb.FetchLogs{Lines: uint32(lines)}}}
	res, ok := a.command(w, r, cmd)
	if !ok {
		return
	}
	out := []logLine{}
	for _, l := range res.GetLogs().GetLines() {
		out = append(out, logLine{UnixMs: l.GetUnixMs(), Level: l.GetLevel(), Message: l.GetMessage()})
	}
	writeJSON(w, http.StatusOK, map[string]any{"lines": out})
}

func (a *API) resetNodeTraffic(w http.ResponseWriter, r *http.Request, _ string) {
	id := r.PathValue("id")
	cn, err := a.Core.ResetNodeTraffic(r.Context(), id)
	if err != nil {
		a.coreError(w, err)
		return
	}
	local, err := a.Store.Node(r.Context(), id)
	if err != nil && !errors.Is(err, store.ErrNotFound) {
		a.internalError(w, r, err)
		return
	}
	writeJSON(w, http.StatusOK, a.view(id, cn, local))
}
