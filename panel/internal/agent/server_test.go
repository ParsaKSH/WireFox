package agent

import (
	"bytes"
	"context"
	"crypto/tls"
	"crypto/x509"
	"errors"
	"io"
	"log/slog"
	"net"
	"path/filepath"
	"sync"
	"testing"
	"time"

	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/credentials"
	"google.golang.org/grpc/metadata"
	"google.golang.org/grpc/status"

	"github.com/ParsaKSH/WireFox/panel/internal/agentpb"
	"github.com/ParsaKSH/WireFox/panel/internal/keys"
	"github.com/ParsaKSH/WireFox/panel/internal/store"
)

const testInterval = 200 * time.Millisecond

type testEnv struct {
	st   *store.Store
	srv  *Server
	addr string
	cert *keys.Cert

	mu       sync.Mutex
	endpoint Endpoint
}

func newTestEnv(t *testing.T) *testEnv {
	t.Helper()
	dir := t.TempDir()
	st, err := store.Open(filepath.Join(dir, "panel.db"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { st.Close() })
	cert, err := keys.LoadOrCreateCert(filepath.Join(dir, "panel.crt"), filepath.Join(dir, "panel.key"))
	if err != nil {
		t.Fatal(err)
	}
	e := &testEnv{st: st, cert: cert, endpoint: Endpoint{Server: "relay.example.com:7443", Fingerprint: "aa"}}
	e.srv = NewServer(Options{
		Store: st,
		Endpoint: func(context.Context) (Endpoint, error) {
			e.mu.Lock()
			defer e.mu.Unlock()
			return e.endpoint, nil
		},
		Logger:         slog.New(slog.NewTextHandler(io.Discard, nil)),
		StatusInterval: testInterval,
		CommandTimeout: 500 * time.Millisecond,
	})
	lis, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	grpcServer := e.srv.GRPCServer(cert)
	go func() { _ = grpcServer.Serve(lis) }()
	t.Cleanup(grpcServer.Stop)
	e.addr = lis.Addr().String()
	return e
}

func (e *testEnv) setEndpoint(ep Endpoint) {
	e.mu.Lock()
	e.endpoint = ep
	e.mu.Unlock()
}

// addNode creates a node and returns its agent secret.
func (e *testEnv) addNode(t *testing.T, id string) string {
	t.Helper()
	secret := keys.NewSecret()
	n := store.NewNode(id)
	n.Token = "token-" + id
	n.AgentKeyHash = keys.Hash(secret)
	n.Egress = store.Egress{Interface: "eth1"}
	if err := e.st.CreateNode(context.Background(), n); err != nil {
		t.Fatal(err)
	}
	return secret
}

// dial opens a Connect stream the way wirefox-node does: TLS 1.3, pinned
// fingerprint, SNI "wirefox-panel", secret in the metadata.
func (e *testEnv) dial(t *testing.T, secret string) agentpb.Agent_ConnectClient {
	t.Helper()
	tlsConfig := &tls.Config{
		ServerName:         keys.CertName,
		MinVersion:         tls.VersionTLS13,
		InsecureSkipVerify: true,
		VerifyPeerCertificate: func(raw [][]byte, _ [][]*x509.Certificate) error {
			if len(raw) == 0 || keys.Fingerprint(raw[0]) != e.cert.Fingerprint {
				return errors.New("fingerprint mismatch")
			}
			return nil
		},
	}
	cc, err := grpc.NewClient(e.addr, grpc.WithTransportCredentials(credentials.NewTLS(tlsConfig)))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { cc.Close() })
	ctx, cancel := context.WithCancel(context.Background())
	t.Cleanup(cancel)
	ctx = metadata.AppendToOutgoingContext(ctx, "authorization", "Bearer "+secret)
	stream, err := agentpb.NewAgentClient(cc).Connect(ctx)
	if err != nil {
		t.Fatal(err)
	}
	return stream
}

func recv(t *testing.T, stream agentpb.Agent_ConnectClient) (*agentpb.PanelMessage, error) {
	t.Helper()
	type result struct {
		msg *agentpb.PanelMessage
		err error
	}
	ch := make(chan result, 1)
	go func() {
		msg, err := stream.Recv()
		ch <- result{msg, err}
	}()
	select {
	case r := <-ch:
		return r.msg, r.err
	case <-time.After(5 * time.Second):
		t.Fatal("timed out waiting for the panel")
		return nil, nil
	}
}

func mustRecv(t *testing.T, stream agentpb.Agent_ConnectClient) *agentpb.PanelMessage {
	t.Helper()
	msg, err := recv(t, stream)
	if err != nil {
		t.Fatal(err)
	}
	return msg
}

func sendHello(t *testing.T, stream agentpb.Agent_ConnectClient) {
	t.Helper()
	err := stream.Send(&agentpb.NodeMessage{Msg: &agentpb.NodeMessage_Hello{Hello: &agentpb.Hello{
		Version:  "0.1.0",
		Os:       "linux",
		Arch:     "aarch64",
		Hostname: "router",
		Interfaces: []*agentpb.Interface{
			{Name: "eth1", Index: 3, Up: true, Addrs: []string{"192.0.2.10"}},
		},
	}}})
	if err != nil {
		t.Fatal(err)
	}
}

// connect does the handshake and returns the stream and the first config.
func connect(t *testing.T, e *testEnv, secret string) (agentpb.Agent_ConnectClient, *agentpb.TunnelConfig) {
	t.Helper()
	stream := e.dial(t, secret)
	sendHello(t, stream)
	if mustRecv(t, stream).GetWelcome() == nil {
		t.Fatal("first message is not Welcome")
	}
	cfg := mustRecv(t, stream).GetConfig()
	if cfg == nil {
		t.Fatal("second message is not TunnelConfig")
	}
	return stream, cfg
}

func waitFor(t *testing.T, what string, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for !cond() {
		if time.Now().After(deadline) {
			t.Fatalf("timed out waiting for %s", what)
		}
		time.Sleep(10 * time.Millisecond)
	}
}

func TestBadKeyRejected(t *testing.T) {
	e := newTestEnv(t)
	e.addNode(t, "office-1")
	for _, secret := range []string{keys.NewSecret(), ""} {
		stream := e.dial(t, secret)
		_, err := recv(t, stream)
		if status.Code(err) != codes.Unauthenticated {
			t.Fatalf("secret %q: got %v, want Unauthenticated", secret, err)
		}
	}
	events, err := e.st.Events(context.Background(), 10)
	if err != nil {
		t.Fatal(err)
	}
	// The second failure falls in the same rate-limit period.
	if len(events) != 1 || events[0].Level != store.Warn {
		t.Fatalf("events: %+v", events)
	}
}

func TestHelloGetsWelcomeAndConfig(t *testing.T) {
	e := newTestEnv(t)
	secret := e.addNode(t, "office-1")
	stream := e.dial(t, secret)
	sendHello(t, stream)

	welcome := mustRecv(t, stream).GetWelcome()
	if welcome.GetNodeId() != "office-1" || welcome.GetStatusIntervalSecs() != 1 {
		t.Fatalf("welcome: %v", welcome)
	}
	cfg := mustRecv(t, stream).GetConfig()
	if cfg.GetVersion() != 1 || !cfg.GetEnabled() || cfg.GetServer() != "relay.example.com:7443" ||
		cfg.GetServerName() != "wirefox" || cfg.GetServerFingerprint() != "aa" || cfg.GetNodeId() != "office-1" ||
		cfg.GetToken() != "token-office-1" || cfg.GetPoolSize() != store.DefaultPoolSize ||
		cfg.GetMaxExtra() != store.DefaultMaxExtra || cfg.GetConnectTimeoutSecs() != store.DefaultConnectTimeoutSecs ||
		cfg.GetEgress().GetInterface() != "eth1" || cfg.GetUplink() == nil {
		t.Fatalf("config: %v", cfg)
	}

	live := e.srv.Live("office-1")
	if live == nil || live.Info.Hostname != "router" || live.Info.Arch != "aarch64" || len(live.Info.Interfaces) != 1 {
		t.Fatalf("live: %+v", live)
	}
	node, err := e.st.Node(context.Background(), "office-1")
	if err != nil || node.Agent == nil || node.Agent.Version != "0.1.0" || node.LastSeen == 0 {
		t.Fatalf("stored agent info: %+v, %v", node, err)
	}

	err = stream.Send(&agentpb.NodeMessage{Msg: &agentpb.NodeMessage_Status{Status: &agentpb.Status{
		ConfigVersion: 1, TunnelState: agentpb.TunnelState_TUNNEL_STATE_CONNECTED, ActiveStreams: 3,
	}}})
	if err != nil {
		t.Fatal(err)
	}
	waitFor(t, "status", func() bool {
		live := e.srv.Live("office-1")
		return live != nil && live.Info.TunnelState == "connected" && live.Info.ActiveStreams == 3
	})

	stream.CloseSend()
	waitFor(t, "disconnect", func() bool { return e.srv.Live("office-1") == nil })
	node, _ = e.st.Node(context.Background(), "office-1")
	if node.Agent.TunnelState != "connected" {
		t.Fatalf("last state not kept: %+v", node.Agent)
	}
}

func TestPushConfig(t *testing.T) {
	e := newTestEnv(t)
	secret := e.addNode(t, "office-1")
	stream, _ := connect(t, e, secret)

	_, err := e.st.UpdateNode(context.Background(), "office-1", func(n *store.Node) error {
		n.PoolSize = 32
		n.ConfigVersion++
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	e.srv.PushConfig("office-1")
	cfg := mustRecv(t, stream).GetConfig()
	if cfg.GetVersion() != 2 || cfg.GetPoolSize() != 32 {
		t.Fatalf("pushed config: %v", cfg)
	}
}

func TestSyncEndpointPushesEveryNode(t *testing.T) {
	e := newTestEnv(t)
	secret := e.addNode(t, "office-1")
	stream, _ := connect(t, e, secret)
	ctx := context.Background()

	if err := e.srv.SyncEndpoint(ctx); err != nil {
		t.Fatal(err)
	}
	// The first sync only records the endpoint the node already has.
	if n, _ := e.st.Node(ctx, "office-1"); n.ConfigVersion != 1 {
		t.Fatalf("first sync bumped the version to %d", n.ConfigVersion)
	}
	e.setEndpoint(Endpoint{Server: "relay.example.com:8443", Fingerprint: "bb"})
	if err := e.srv.SyncEndpoint(ctx); err != nil {
		t.Fatal(err)
	}
	cfg := mustRecv(t, stream).GetConfig()
	if cfg.GetVersion() != 2 || cfg.GetServer() != "relay.example.com:8443" || cfg.GetServerFingerprint() != "bb" {
		t.Fatalf("config after endpoint change: %v", cfg)
	}
	if err := e.srv.SyncEndpoint(ctx); err != nil {
		t.Fatal(err)
	}
	if n, _ := e.st.Node(ctx, "office-1"); n.ConfigVersion != 2 {
		t.Fatalf("unchanged endpoint bumped the version to %d", n.ConfigVersion)
	}
}

func TestTunnelConfigNeedsEndpoint(t *testing.T) {
	e := newTestEnv(t)
	n := store.NewNode("n")
	n.Token = "t"
	for _, ep := range []Endpoint{
		{Server: "relay.example.com:7443"},
		{Server: "0.0.0.0:7443", Fingerprint: "aa"},
		{Server: "[::]:7443", Fingerprint: "aa"},
	} {
		e.setEndpoint(ep)
		if _, err := e.srv.TunnelConfig(context.Background(), n); err == nil {
			t.Errorf("built a config for %+v", ep)
		}
	}
	e.setEndpoint(Endpoint{Server: "203.0.113.1:7443", Fingerprint: "aa"})
	n.Token = ""
	if _, err := e.srv.TunnelConfig(context.Background(), n); err == nil {
		t.Error("built a config without a token")
	}
}

// answerCommands plays the node: it answers every command until the stream ends.
func answerCommands(stream agentpb.Agent_ConnectClient, handle func(*agentpb.Command) *agentpb.CommandResult) {
	go func() {
		for {
			msg, err := stream.Recv()
			if err != nil {
				return
			}
			cmd := msg.GetCommand()
			if cmd == nil {
				continue
			}
			res := handle(cmd)
			if res == nil {
				continue
			}
			res.Id = cmd.GetId()
			if stream.Send(&agentpb.NodeMessage{Msg: &agentpb.NodeMessage_Result{Result: res}}) != nil {
				return
			}
		}
	}()
}

func TestCommands(t *testing.T) {
	e := newTestEnv(t)
	ctx := context.Background()
	listInterfaces := &agentpb.Command{Kind: &agentpb.Command_ListInterfaces{ListInterfaces: &agentpb.ListInterfaces{}}}
	if _, err := e.srv.Send(ctx, "office-1", listInterfaces); !errors.Is(err, ErrNotConnected) {
		t.Fatalf("not connected: %v", err)
	}

	secret := e.addNode(t, "office-1")
	stream, _ := connect(t, e, secret)
	answerCommands(stream, func(cmd *agentpb.Command) *agentpb.CommandResult {
		switch cmd.GetKind().(type) {
		case *agentpb.Command_ListInterfaces:
			return &agentpb.CommandResult{Payload: &agentpb.CommandResult_Interfaces{Interfaces: &agentpb.InterfaceList{
				Interfaces: []*agentpb.Interface{{Name: "wlan0", Up: true}, {Name: "eth0"}},
			}}}
		case *agentpb.Command_RestartTunnel:
			return &agentpb.CommandResult{Error: "tunnel is disabled"}
		default:
			return nil // FetchLogs goes unanswered.
		}
	})

	res, err := e.srv.Send(ctx, "office-1", listInterfaces)
	if err != nil {
		t.Fatal(err)
	}
	if got := Interfaces(res.GetInterfaces().GetInterfaces()); len(got) != 2 || got[0].Name != "wlan0" || got[1].Addrs == nil {
		t.Fatalf("interfaces: %+v", got)
	}
	if live := e.srv.Live("office-1"); len(live.Info.Interfaces) != 2 {
		t.Fatalf("live interfaces not refreshed: %+v", live.Info.Interfaces)
	}

	restart := &agentpb.Command{Kind: &agentpb.Command_RestartTunnel{RestartTunnel: &agentpb.RestartTunnel{}}}
	var cmdErr *CommandError
	if _, err := e.srv.Send(ctx, "office-1", restart); !errors.As(err, &cmdErr) || cmdErr.Message != "tunnel is disabled" {
		t.Fatalf("failed command: %v", err)
	}

	logs := &agentpb.Command{Kind: &agentpb.Command_FetchLogs{FetchLogs: &agentpb.FetchLogs{Lines: 10}}}
	if _, err := e.srv.Send(ctx, "office-1", logs); !errors.Is(err, ErrTimeout) {
		t.Fatalf("unanswered command: %v", err)
	}
}

func TestSecondConnectionReplacesFirst(t *testing.T) {
	e := newTestEnv(t)
	secret := e.addNode(t, "office-1")
	first, _ := connect(t, e, secret)
	second, _ := connect(t, e, secret)

	_, err := recv(t, first)
	if status.Code(err) != codes.Aborted {
		t.Fatalf("first stream: got %v, want Aborted", err)
	}
	answerCommands(second, func(*agentpb.Command) *agentpb.CommandResult { return &agentpb.CommandResult{} })
	restart := &agentpb.Command{Kind: &agentpb.Command_RestartTunnel{RestartTunnel: &agentpb.RestartTunnel{}}}
	if _, err := e.srv.Send(context.Background(), "office-1", restart); err != nil {
		t.Fatalf("command over the second stream: %v", err)
	}
	if e.srv.Connected() != 1 {
		t.Fatalf("%d connections", e.srv.Connected())
	}
}

func TestSilentNodeDropped(t *testing.T) {
	e := newTestEnv(t)
	secret := e.addNode(t, "office-1")
	stream, _ := connect(t, e, secret)
	start := time.Now()
	_, err := recv(t, stream)
	if status.Code(err) != codes.DeadlineExceeded {
		t.Fatalf("got %v, want DeadlineExceeded", err)
	}
	if elapsed := time.Since(start); elapsed < 2*testInterval {
		t.Fatalf("dropped after %v", elapsed)
	}
	waitFor(t, "unregister", func() bool { return e.srv.Live("office-1") == nil })
}

func TestStatusKeepsNodeConnected(t *testing.T) {
	e := newTestEnv(t)
	secret := e.addNode(t, "office-1")
	stream, _ := connect(t, e, secret)
	for range 8 {
		time.Sleep(testInterval)
		err := stream.Send(&agentpb.NodeMessage{Msg: &agentpb.NodeMessage_Status{Status: &agentpb.Status{}}})
		if err != nil {
			t.Fatal(err)
		}
	}
	if e.srv.Live("office-1") == nil {
		t.Fatal("a node sending status was dropped")
	}
}

func TestDisconnectRevokes(t *testing.T) {
	e := newTestEnv(t)
	secret := e.addNode(t, "office-1")
	stream, _ := connect(t, e, secret)
	e.srv.Disconnect("office-1")
	if _, err := recv(t, stream); status.Code(err) != codes.Unauthenticated {
		t.Fatalf("got %v", err)
	}
}

func TestFirstMessageMustBeHello(t *testing.T) {
	e := newTestEnv(t)
	stream := e.dial(t, e.addNode(t, "office-1"))
	if err := stream.Send(&agentpb.NodeMessage{Msg: &agentpb.NodeMessage_Status{Status: &agentpb.Status{}}}); err != nil {
		t.Fatal(err)
	}
	if _, err := recv(t, stream); status.Code(err) != codes.InvalidArgument {
		t.Fatalf("got %v", err)
	}
}

func TestOnlyTLS13(t *testing.T) {
	e := newTestEnv(t)
	conn, err := tls.Dial("tcp", e.addr, &tls.Config{
		InsecureSkipVerify: true,
		MaxVersion:         tls.VersionTLS12,
		NextProtos:         []string{"h2"},
	})
	if err == nil {
		conn.Close()
		t.Fatal("TLS 1.2 handshake succeeded")
	}
	conn, err = tls.Dial("tcp", e.addr, &tls.Config{InsecureSkipVerify: true, NextProtos: []string{"h2"}})
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	if cert := conn.ConnectionState().PeerCertificates[0]; !bytes.Equal(cert.Raw, e.cert.TLS.Certificate[0]) {
		t.Fatal("unexpected certificate")
	}
}
