// Package agent is the panel's side of the control channel to exit-nodes: a
// gRPC server that each node keeps one stream open to (see agent.proto).
package agent

import (
	"context"
	"crypto/tls"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net"
	"net/netip"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/credentials"
	"google.golang.org/grpc/keepalive"
	"google.golang.org/grpc/metadata"
	"google.golang.org/grpc/peer"
	"google.golang.org/grpc/status"

	"github.com/ParsaKSH/WireFox/panel/internal/agentpb"
	"github.com/ParsaKSH/WireFox/panel/internal/core"
	"github.com/ParsaKSH/WireFox/panel/internal/keys"
	"github.com/ParsaKSH/WireFox/panel/internal/store"
)

const (
	DefaultStatusInterval = 10 * time.Second
	DefaultCommandTimeout = 10 * time.Second
	// At most one "invalid key" report per this period; the rest are counted.
	authEventPeriod = time.Minute
	// A node is dropped after this many status intervals without a message.
	silentIntervals = 3

	keepaliveTime    = 30 * time.Second
	keepaliveTimeout = 10 * time.Second
	// The shortest ping interval nodes may use.
	minPingInterval = 10 * time.Second
	// A node uses one stream; a few more cover a reconnect on the same
	// connection.
	maxStreamsPerConn = 8
	// Connections without a stream are closed after this long; nodes open
	// a new one when they reconnect.
	maxConnIdle = 2 * time.Minute
)

var (
	ErrNotConnected = errors.New("the node's agent is not connected")
	ErrTimeout      = errors.New("the node did not answer in time")

	errRevoked = status.Error(codes.Unauthenticated, "the node's key was revoked")
)

// CommandError is a command the node received but reports as failed.
type CommandError struct {
	Message string
}

func (e *CommandError) Error() string {
	return "node: " + e.Message
}

// Endpoint is where nodes find the WireFox server's tunnel listener.
type Endpoint struct {
	// host:port
	Server string
	// Hex SHA-256 of the server's certificate; empty while unknown.
	Fingerprint string
}

// Options configure a Server. Store and Endpoint are required.
type Options struct {
	Store    *store.Store
	Endpoint func(context.Context) (Endpoint, error)
	Logger   *slog.Logger
	// Zero means the defaults.
	StatusInterval time.Duration
	CommandTimeout time.Duration
}

// Server implements the Agent service and keeps track of the connected nodes.
type Server struct {
	agentpb.UnimplementedAgentServer

	store          *store.Store
	endpoint       func(context.Context) (Endpoint, error)
	log            *slog.Logger
	statusInterval time.Duration
	commandTimeout time.Duration
	commandIDs     atomic.Uint64

	mu    sync.Mutex
	conns map[string]*conn

	authMu         sync.Mutex
	lastAuthEvent  time.Time
	suppressedAuth int

	endpointMu sync.Mutex
}

func NewServer(opts Options) *Server {
	s := &Server{
		store:          opts.Store,
		endpoint:       opts.Endpoint,
		log:            opts.Logger,
		statusInterval: opts.StatusInterval,
		commandTimeout: opts.CommandTimeout,
		conns:          map[string]*conn{},
	}
	if s.log == nil {
		s.log = slog.Default()
	}
	if s.statusInterval <= 0 {
		s.statusInterval = DefaultStatusInterval
	}
	if s.commandTimeout <= 0 {
		s.commandTimeout = DefaultCommandTimeout
	}
	return s
}

// GRPCServer returns a gRPC server that serves s over TLS 1.3 with cert.
func (s *Server) GRPCServer(cert *keys.Cert) *grpc.Server {
	creds := credentials.NewTLS(&tls.Config{
		Certificates: []tls.Certificate{cert.TLS},
		MinVersion:   tls.VersionTLS13,
	})
	srv := grpc.NewServer(
		grpc.Creds(creds),
		grpc.MaxConcurrentStreams(maxStreamsPerConn),
		grpc.KeepaliveParams(keepalive.ServerParameters{
			Time:              keepaliveTime,
			Timeout:           keepaliveTimeout,
			MaxConnectionIdle: maxConnIdle,
		}),
		// Nodes ping on their own to notice dead links early, also between
		// streams while they reconnect.
		grpc.KeepaliveEnforcementPolicy(keepalive.EnforcementPolicy{
			MinTime:             minPingInterval,
			PermitWithoutStream: true,
		}),
		// Stop then waits for the stream handlers, which still record the
		// nodes' last state.
		grpc.WaitForHandlers(true),
	)
	agentpb.RegisterAgentServer(srv, s)
	return srv
}

// conn is one live agent stream.
type conn struct {
	nodeID string
	// The key hash the stream authenticated with.
	keyHash string
	remote  string
	// Signals the stream to send the node's current config.
	configDirty chan struct{}
	commands    chan *agentpb.Command
	// Closed when the stream must end; reason says why.
	done      chan struct{}
	closeOnce sync.Once
	reason    error

	mu       sync.Mutex
	info     store.AgentInfo
	lastSeen time.Time
	pending  map[string]chan *agentpb.CommandResult
}

func newConn(node *store.Node, remote string) *conn {
	return &conn{
		nodeID:      node.ID,
		keyHash:     node.AgentKeyHash,
		remote:      remote,
		configDirty: make(chan struct{}, 1),
		commands:    make(chan *agentpb.Command, 16),
		done:        make(chan struct{}),
		pending:     map[string]chan *agentpb.CommandResult{},
	}
}

func (c *conn) close(reason error) {
	c.closeOnce.Do(func() {
		c.reason = reason
		close(c.done)
	})
}

func (c *conn) markConfigDirty() {
	select {
	case c.configDirty <- struct{}{}:
	default:
	}
}

func (s *Server) authenticate(ctx context.Context) (*store.Node, error) {
	md, _ := metadata.FromIncomingContext(ctx)
	var secret string
	if values := md.Get("authorization"); len(values) > 0 {
		secret, _ = strings.CutPrefix(values[0], "Bearer ")
	}
	node, err := s.store.NodeByKeyHash(ctx, keys.Hash(strings.TrimSpace(secret)))
	if errors.Is(err, store.ErrNotFound) || (err == nil && secret == "") {
		s.authFailed(ctx, remoteAddr(ctx))
		return nil, status.Error(codes.Unauthenticated, "invalid node key")
	}
	if err != nil {
		s.log.Error("looking up agent key", "err", err)
		return nil, status.Error(codes.Internal, "internal error")
	}
	return node, nil
}

// authFailed reports a rejected key in the log and the event log, at most
// once per authEventPeriod, so a client trying keys in a loop can't flood
// either.
func (s *Server) authFailed(ctx context.Context, remote string) {
	s.authMu.Lock()
	now := time.Now()
	if now.Sub(s.lastAuthEvent) < authEventPeriod {
		s.suppressedAuth++
		s.authMu.Unlock()
		return
	}
	suppressed := s.suppressedAuth
	s.lastAuthEvent, s.suppressedAuth = now, 0
	s.authMu.Unlock()
	s.log.Warn("agent connection with an invalid key", "remote", remote, "suppressed", suppressed)
	msg := "rejected an agent connection with an invalid key from " + remote
	if suppressed > 0 {
		msg += fmt.Sprintf(" (and %d more since the last report)", suppressed)
	}
	s.event(ctx, store.Warn, msg)
}

func (s *Server) event(ctx context.Context, level store.Level, msg string) {
	if err := s.store.AddEvent(context.WithoutCancel(ctx), level, msg); err != nil {
		s.log.Error("recording event", "err", err)
	}
}

func remoteAddr(ctx context.Context) string {
	if p, ok := peer.FromContext(ctx); ok && p.Addr != nil {
		return p.Addr.String()
	}
	return ""
}

// Connect serves one node's stream until it ends, the node goes silent, or a
// newer stream from the same node replaces it.
func (s *Server) Connect(stream agentpb.Agent_ConnectServer) error {
	ctx := stream.Context()
	node, err := s.authenticate(ctx)
	if err != nil {
		return err
	}

	msgs := make(chan *agentpb.NodeMessage)
	recvErr := make(chan error, 1)
	go func() {
		for {
			msg, err := stream.Recv()
			if err != nil {
				recvErr <- err
				return
			}
			select {
			case msgs <- msg:
			case <-ctx.Done():
				return
			}
		}
	}()

	silence := silentIntervals * s.statusInterval
	timer := time.NewTimer(silence)
	defer timer.Stop()

	var hello *agentpb.Hello
	select {
	case msg := <-msgs:
		if hello = msg.GetHello(); hello == nil {
			return status.Error(codes.InvalidArgument, "the first message must be Hello")
		}
	case err := <-recvErr:
		return err
	case <-timer.C:
		return status.Error(codes.DeadlineExceeded, "no Hello received")
	}
	timer.Reset(silence)

	c := newConn(node, remoteAddr(ctx))
	c.info = store.AgentInfo{TunnelState: "stopped"}
	if node.Agent != nil {
		c.info = *node.Agent
	}
	c.info.Remote = c.remote
	c.info.Version = hello.GetVersion()
	c.info.OS = hello.GetOs()
	c.info.Arch = hello.GetArch()
	c.info.Hostname = hello.GetHostname()
	c.info.Interfaces = Interfaces(hello.GetInterfaces())
	c.info.ConfigVersion = hello.GetConfigVersion()
	c.lastSeen = time.Now()

	s.register(c)
	// The key may have been replaced while we waited for Hello; Disconnect
	// only finds streams that are registered.
	if !s.keyValid(ctx, c) {
		s.unregister(c)
		return errRevoked
	}
	s.saveInfo(ctx, c)
	s.log.Info("agent connected", "node", node.ID, "remote", c.remote, "version", c.info.Version)
	s.event(ctx, store.Info, fmt.Sprintf("node %s connected to the panel from %s", node.ID, c.remote))
	defer func() {
		s.log.Info("agent disconnected", "node", node.ID, "reason", c.reason)
		// A replaced stream leaves the record to its successor.
		if s.unregister(c) {
			s.saveInfo(ctx, c)
			s.event(ctx, store.Info, fmt.Sprintf("node %s disconnected from the panel", node.ID))
		}
	}()

	welcome := &agentpb.Welcome{NodeId: node.ID, StatusIntervalSecs: uint32(s.statusInterval / time.Second)}
	if welcome.StatusIntervalSecs == 0 {
		welcome.StatusIntervalSecs = 1
	}
	if err := stream.Send(&agentpb.PanelMessage{Msg: &agentpb.PanelMessage_Welcome{Welcome: welcome}}); err != nil {
		c.close(err)
		return err
	}
	c.markConfigDirty()

	for {
		select {
		case <-c.configDirty:
			if err := s.sendConfig(ctx, stream, c); err != nil {
				c.close(err)
				return err
			}
		case cmd := <-c.commands:
			if err := stream.Send(&agentpb.PanelMessage{Msg: &agentpb.PanelMessage_Command{Command: cmd}}); err != nil {
				c.close(err)
				return err
			}
		case msg := <-msgs:
			timer.Reset(silence)
			s.handle(ctx, c, msg)
		case err := <-recvErr:
			if errors.Is(err, io.EOF) {
				// The node closed its side cleanly.
				err = nil
			}
			c.close(err)
			return err
		case <-timer.C:
			err := status.Error(codes.DeadlineExceeded, "no status from the node")
			c.close(err)
			return err
		case <-c.done:
			return c.reason
		}
	}
}

func (s *Server) sendConfig(ctx context.Context, stream agentpb.Agent_ConnectServer, c *conn) error {
	node, err := s.store.Node(ctx, c.nodeID)
	if err != nil {
		return status.Error(codes.Unavailable, "node is gone")
	}
	// The config holds the node's tunnel token; a stream whose key was
	// replaced must not see the new one.
	if node.AgentKeyHash != c.keyHash {
		return errRevoked
	}
	cfg, err := s.TunnelConfig(ctx, node)
	if err != nil {
		// The node keeps its last config; it gets the new one with the next
		// change or connection.
		s.log.Warn("not sending tunnel config", "node", c.nodeID, "err", err)
		return nil
	}
	return stream.Send(&agentpb.PanelMessage{Msg: &agentpb.PanelMessage_Config{Config: cfg}})
}

// keyValid reports whether c's key is still the node's key.
func (s *Server) keyValid(ctx context.Context, c *conn) bool {
	node, err := s.store.Node(ctx, c.nodeID)
	if err != nil {
		if !errors.Is(err, store.ErrNotFound) {
			s.log.Error("looking up agent key", "node", c.nodeID, "err", err)
		}
		return false
	}
	return node.AgentKeyHash == c.keyHash
}

// TunnelConfig is everything node needs to run its tunnel. It fails while
// the panel doesn't know enough yet for a working config.
func (s *Server) TunnelConfig(ctx context.Context, node *store.Node) (*agentpb.TunnelConfig, error) {
	ep, err := s.endpoint(ctx)
	if err != nil {
		return nil, err
	}
	if host, _, err := net.SplitHostPort(ep.Server); err != nil || !Dialable(host) {
		return nil, fmt.Errorf("no public address for the WireFox server's tunnel listener (%s): "+
			"set the core's tunnel public address or the panel's agent address", ep.Server)
	}
	switch {
	case ep.Fingerprint == "":
		return nil, errors.New("the WireFox server's certificate fingerprint is not known yet")
	case node.Token == "":
		return nil, errors.New("the node has no tunnel token yet")
	}
	return &agentpb.TunnelConfig{
		Version:            node.ConfigVersion,
		Enabled:            node.Enabled,
		Server:             ep.Server,
		ServerName:         core.DefaultTunnelSAN,
		ServerFingerprint:  ep.Fingerprint,
		NodeId:             node.ID,
		Token:              node.Token,
		PoolSize:           node.PoolSize,
		MaxExtra:           node.MaxExtra,
		ConnectTimeoutSecs: node.ConnectTimeoutSecs,
		AllowLocal:         node.AllowLocal,
		Egress:             &agentpb.Egress{Interface: node.Egress.Interface, SourceIp: node.Egress.SourceIP},
		Uplink:             &agentpb.Egress{Interface: node.Uplink.Interface, SourceIp: node.Uplink.SourceIP},
	}, nil
}

// Dialable reports whether nodes on other machines can reach host: it must
// not be empty, localhost, or a loopback, link-local or unspecified IP.
func Dialable(host string) bool {
	host = strings.TrimSuffix(strings.ToLower(host), ".")
	if host == "" || host == "localhost" || strings.HasSuffix(host, ".localhost") {
		return false
	}
	ip, err := netip.ParseAddr(host)
	if err != nil {
		return true
	}
	ip = ip.Unmap()
	return !ip.IsLoopback() && !ip.IsLinkLocalUnicast() && !ip.IsUnspecified()
}

func (s *Server) handle(ctx context.Context, c *conn, msg *agentpb.NodeMessage) {
	c.mu.Lock()
	c.lastSeen = time.Now()
	c.mu.Unlock()
	switch m := msg.GetMsg().(type) {
	case *agentpb.NodeMessage_Status:
		st := m.Status
		c.mu.Lock()
		changed := c.info.TunnelState != tunnelState(st.GetTunnelState()) || c.info.LastError != st.GetLastError() ||
			c.info.ConfigVersion != st.GetConfigVersion()
		c.info.ConfigVersion = st.GetConfigVersion()
		c.info.TunnelState = tunnelState(st.GetTunnelState())
		c.info.LastError = st.GetLastError()
		c.info.UptimeSecs = st.GetUptimeSecs()
		c.info.ConnectedSecs = st.GetConnectedSecs()
		c.info.ActiveStreams = st.GetActiveStreams()
		c.info.TotalStreams = st.GetTotalStreams()
		c.mu.Unlock()
		if changed {
			s.saveInfo(ctx, c)
		}
	case *agentpb.NodeMessage_Result:
		if list := m.Result.GetInterfaces(); list != nil {
			c.mu.Lock()
			c.info.Interfaces = Interfaces(list.GetInterfaces())
			c.mu.Unlock()
		}
		c.mu.Lock()
		reply, ok := c.pending[m.Result.GetId()]
		delete(c.pending, m.Result.GetId())
		c.mu.Unlock()
		if ok {
			reply <- m.Result
		}
	case *agentpb.NodeMessage_Hello:
		s.log.Warn("agent sent a second Hello", "node", c.nodeID)
	}
}

func (s *Server) saveInfo(ctx context.Context, c *conn) {
	c.mu.Lock()
	info, lastSeen := c.info, c.lastSeen.Unix()
	c.mu.Unlock()
	if err := s.store.SetAgentInfo(context.WithoutCancel(ctx), c.nodeID, &info, lastSeen); err != nil {
		s.log.Error("saving agent info", "node", c.nodeID, "err", err)
	}
}

func tunnelState(st agentpb.TunnelState) string {
	switch st {
	case agentpb.TunnelState_TUNNEL_STATE_CONNECTING:
		return "connecting"
	case agentpb.TunnelState_TUNNEL_STATE_CONNECTED:
		return "connected"
	case agentpb.TunnelState_TUNNEL_STATE_ERROR:
		return "error"
	default:
		return "stopped"
	}
}

// Interfaces converts interfaces reported by a node.
func Interfaces(list []*agentpb.Interface) []store.Interface {
	out := make([]store.Interface, 0, len(list))
	for _, i := range list {
		out = append(out, interfaceFromProto(i))
	}
	return out
}

func interfaceFromProto(i *agentpb.Interface) store.Interface {
	addrs := i.GetAddrs()
	if addrs == nil {
		addrs = []string{}
	}
	return store.Interface{
		Name:        i.GetName(),
		AdapterName: i.GetAdapterName(),
		Index:       i.GetIndex(),
		Up:          i.GetUp(),
		Addrs:       addrs,
	}
}

func (s *Server) register(c *conn) {
	s.mu.Lock()
	old := s.conns[c.nodeID]
	s.conns[c.nodeID] = c
	s.mu.Unlock()
	if old != nil {
		old.close(status.Error(codes.Aborted, "replaced by a newer connection from the same node"))
	}
}

// unregister forgets c unless a newer stream replaced it; it reports whether
// c was still the node's stream.
func (s *Server) unregister(c *conn) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.conns[c.nodeID] != c {
		return false
	}
	delete(s.conns, c.nodeID)
	return true
}

func (s *Server) conn(nodeID string) *conn {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.conns[nodeID]
}

// Live is the current state of a connected agent.
type Live struct {
	Info     store.AgentInfo
	LastSeen int64
}

// Live returns the state of the node's agent, or nil if it isn't connected.
func (s *Server) Live(nodeID string) *Live {
	c := s.conn(nodeID)
	if c == nil {
		return nil
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	info := c.info
	info.Interfaces = append([]store.Interface(nil), c.info.Interfaces...)
	return &Live{Info: info, LastSeen: c.lastSeen.Unix()}
}

// Connected returns how many agents are connected.
func (s *Server) Connected() int {
	s.mu.Lock()
	defer s.mu.Unlock()
	return len(s.conns)
}

// PushConfig sends the node its current TunnelConfig if it is connected. A
// node that isn't gets it when it next connects.
func (s *Server) PushConfig(nodeID string) {
	if c := s.conn(nodeID); c != nil {
		c.markConfigDirty()
	}
}

// Disconnect ends the node's stream, if any. Used when its key changes or
// the node is deleted, after the change is stored: a stream that registers
// later checks its key again.
func (s *Server) Disconnect(nodeID string) {
	if c := s.conn(nodeID); c != nil {
		c.close(errRevoked)
	}
}

// SyncEndpoint gives every node a new config when the WireFox server's
// address or fingerprint changed since configs were last built.
func (s *Server) SyncEndpoint(ctx context.Context) error {
	s.endpointMu.Lock()
	defer s.endpointMu.Unlock()
	ep, err := s.endpoint(ctx)
	if err != nil {
		return err
	}
	current := ep.Server + " " + ep.Fingerprint
	previous, err := s.store.Setting(ctx, store.SettingTunnelEndpoint)
	if err != nil || previous == current {
		return err
	}
	// Without a previous record (a fresh panel), there are no older configs
	// to replace.
	if previous == "" {
		return s.store.SetSetting(ctx, store.SettingTunnelEndpoint, current)
	}
	if err := s.store.BumpAllVersions(ctx); err != nil {
		return err
	}
	if err := s.store.SetSetting(ctx, store.SettingTunnelEndpoint, current); err != nil {
		return err
	}
	s.log.Info("tunnel endpoint changed, pushing new configs", "server", ep.Server, "fingerprint", ep.Fingerprint)
	s.mu.Lock()
	for _, c := range s.conns {
		c.markConfigDirty()
	}
	s.mu.Unlock()
	return nil
}

// Send delivers cmd to the node and waits for its result. It fails with
// ErrNotConnected, ErrTimeout, a *CommandError or the context's error.
func (s *Server) Send(ctx context.Context, nodeID string, cmd *agentpb.Command) (*agentpb.CommandResult, error) {
	c := s.conn(nodeID)
	if c == nil {
		return nil, ErrNotConnected
	}
	cmd.Id = strconv.FormatUint(s.commandIDs.Add(1), 10)
	reply := make(chan *agentpb.CommandResult, 1)
	c.mu.Lock()
	c.pending[cmd.Id] = reply
	c.mu.Unlock()
	defer func() {
		c.mu.Lock()
		delete(c.pending, cmd.Id)
		c.mu.Unlock()
	}()

	timeout := time.NewTimer(s.commandTimeout)
	defer timeout.Stop()
	select {
	case c.commands <- cmd:
	case <-c.done:
		return nil, ErrNotConnected
	case <-timeout.C:
		return nil, ErrTimeout
	case <-ctx.Done():
		return nil, ctx.Err()
	}
	select {
	case res := <-reply:
		if res.GetError() != "" {
			return res, &CommandError{Message: res.GetError()}
		}
		return res, nil
	case <-c.done:
		return nil, ErrNotConnected
	case <-timeout.C:
		return nil, ErrTimeout
	case <-ctx.Done():
		return nil, ctx.Err()
	}
}
