package api

import (
	"bytes"
	"context"
	"crypto/tls"
	"crypto/x509"
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"net"
	"net/http"
	"net/http/cookiejar"
	"net/http/httptest"
	"net/netip"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/credentials"
	"google.golang.org/grpc/metadata"
	"google.golang.org/grpc/status"

	"github.com/ParsaKSH/WireFox/panel/internal/agent"
	"github.com/ParsaKSH/WireFox/panel/internal/agentpb"
	"github.com/ParsaKSH/WireFox/panel/internal/core"
	"github.com/ParsaKSH/WireFox/panel/internal/keys"
	"github.com/ParsaKSH/WireFox/panel/internal/store"
)

const (
	testUser     = "admin"
	testPassword = "correct horse"
	testAPIKey   = "panel-api-key"
)

// fakeCore implements the parts of the WireFox server's API the panel uses.
type fakeCore struct {
	mu       sync.Mutex
	nodes    map[string]*core.Node
	settings core.Settings
	// Method, path and body of every request, for passthrough checks.
	requests []string
}

func newFakeCore(t *testing.T) (*fakeCore, *httptest.Server) {
	f := &fakeCore{
		nodes:    map[string]*core.Node{},
		settings: core.Settings{Balance: "least-load", HandshakeTimeoutSecs: 10, ConnectTimeoutSecs: 15},
	}
	srv := httptest.NewServer(http.HandlerFunc(f.serve))
	t.Cleanup(srv.Close)
	return f, srv
}

func (f *fakeCore) lastRequest() string {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.requests[len(f.requests)-1]
}

func (f *fakeCore) requestCount() int {
	f.mu.Lock()
	defer f.mu.Unlock()
	return len(f.requests)
}

func (f *fakeCore) serve(w http.ResponseWriter, r *http.Request) {
	if r.Header.Get("Authorization") != "Bearer "+testAPIKey {
		writeError(w, http.StatusUnauthorized, "missing or invalid bearer token")
		return
	}
	body, _ := io.ReadAll(r.Body)
	f.mu.Lock()
	defer f.mu.Unlock()
	f.requests = append(f.requests, strings.TrimSpace(r.Method+" "+r.URL.RequestURI()+" "+string(body)))

	path := strings.TrimPrefix(r.URL.Path, "/api/v1")
	id, sub, _ := strings.Cut(strings.TrimPrefix(path, "/nodes/"), "/")
	switch {
	case path == "/status":
		writeJSON(w, http.StatusOK, core.Status{Version: "0.1.0", TunnelFingerprint: "aa", NodesTotal: uint64(len(f.nodes))})
	case path == "/settings" && r.Method == http.MethodPatch:
		if err := json.Unmarshal(body, &f.settings); err != nil {
			writeError(w, http.StatusBadRequest, err.Error())
			return
		}
		writeJSON(w, http.StatusOK, f.settings)
	case path == "/nodes" && r.Method == http.MethodGet:
		nodes := []core.Node{}
		for _, n := range f.nodes {
			nodes = append(nodes, *n)
		}
		writeJSON(w, http.StatusOK, nodes)
	case path == "/nodes" && r.Method == http.MethodPost:
		var req struct{ ID, Name string }
		_ = json.Unmarshal(body, &req)
		if req.Name == "" {
			req.Name = req.ID
		}
		n := &core.Node{ID: req.ID, Name: req.Name, Enabled: true, CreatedAt: 1793600000}
		f.nodes[req.ID] = n
		writeJSON(w, http.StatusOK, map[string]any{"node": n, "token": "core-token", "node_config": ""})
	case strings.HasPrefix(path, "/nodes/"):
		n, ok := f.nodes[id]
		if !ok {
			writeError(w, http.StatusNotFound, "node not found")
			return
		}
		switch {
		case sub == "" && r.Method == http.MethodGet:
			writeJSON(w, http.StatusOK, n)
		case sub == "" && r.Method == http.MethodPatch:
			_ = json.Unmarshal(body, n)
			var patch map[string]json.RawMessage
			_ = json.Unmarshal(body, &patch)
			if string(patch["egress"]) == "null" {
				n.EgressOverride = nil
			}
			writeJSON(w, http.StatusOK, n)
		case sub == "" && r.Method == http.MethodDelete:
			delete(f.nodes, id)
			writeOK(w)
		case sub == "rotate-token":
			writeJSON(w, http.StatusOK, map[string]string{"token": "rotated-token"})
		case sub == "reset-traffic":
			n.Traffic = core.Traffic{}
			writeJSON(w, http.StatusOK, n)
		}
	case path == "/users" && r.Method == http.MethodPost:
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{"user":{"username":"alice"},"password":"generated"}`))
	case path == "/users/bob":
		writeError(w, http.StatusNotFound, `user "bob" not found`)
	default:
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`[]`))
	}
}

// fakeManager stands in for systemd.
type fakeManager struct {
	mu      sync.Mutex
	actions []string
}

func (m *fakeManager) Mode() string                      { return "systemd" }
func (m *fakeManager) UnitPath() string                  { return "/etc/systemd/system/wirefox-server.service" }
func (m *fakeManager) Setup(context.Context, bool) error { return nil }
func (m *fakeManager) Status(context.Context) (*core.UnitStatus, error) {
	return &core.UnitStatus{Name: "wirefox-server", ActiveState: "active", SubState: "running"}, nil
}
func (m *fakeManager) Start(context.Context) error   { return m.record("start") }
func (m *fakeManager) Stop(context.Context) error    { return m.record("stop") }
func (m *fakeManager) Restart(context.Context) error { return m.record("restart") }
func (m *fakeManager) Logs(context.Context, int) ([]core.LogEntry, error) {
	return []core.LogEntry{{UnixMs: 1, Priority: 6, Message: "hello"}}, nil
}

func (m *fakeManager) record(action string) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.actions = append(m.actions, action)
	return nil
}

type testEnv struct {
	t          *testing.T
	st         *store.Store
	core       *fakeCore
	agents     *agent.Server
	cert       *keys.Cert
	grpcAddr   string
	configPath string
	srv        *httptest.Server
	client     *http.Client
}

func newTestEnv(t *testing.T, manager core.Manager) *testEnv {
	t.Helper()
	dir := t.TempDir()
	st, err := store.Open(filepath.Join(dir, "panel.db"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { st.Close() })
	ctx := context.Background()
	hash, err := HashPassword(testPassword)
	if err != nil {
		t.Fatal(err)
	}
	if err := st.SetAdmin(ctx, testUser, hash); err != nil {
		t.Fatal(err)
	}
	if err := st.SetSetting(ctx, store.SettingCoreFingerprint, "aa"); err != nil {
		t.Fatal(err)
	}
	cert, err := keys.LoadOrCreateCert(filepath.Join(dir, "panel.crt"), filepath.Join(dir, "panel.key"))
	if err != nil {
		t.Fatal(err)
	}

	fake, coreSrv := newFakeCore(t)
	coreCfg := core.DefaultConfig(filepath.Join(dir, "core"))
	coreCfg.Tunnel.PublicAddr = "relay.example.com:7443"
	coreCfg.API.Listen = coreSrv.Listener.Addr().String()
	configPath := filepath.Join(dir, "server.toml")
	if err := coreCfg.Save(configPath); err != nil {
		t.Fatal(err)
	}
	client, err := core.NewClient(coreCfg, configPath, testAPIKey)
	if err != nil {
		t.Fatal(err)
	}
	tunnel, err := core.NewRunningTunnel(ctx, st, configPath)
	if err != nil {
		t.Fatal(err)
	}

	logger := slog.New(slog.NewTextHandler(io.Discard, nil))
	agents := agent.NewServer(agent.Options{
		Store: st,
		Endpoint: func(ctx context.Context) (agent.Endpoint, error) {
			return agent.Endpoint{Server: tunnel.Section().Addr(""), Fingerprint: "aa"}, nil
		},
		Logger:         logger,
		CommandTimeout: 300 * time.Millisecond,
	})
	lis, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	grpcServer := agents.GRPCServer(cert)
	go func() { _ = grpcServer.Serve(lis) }()
	t.Cleanup(grpcServer.Stop)
	_, grpcPort, _ := net.SplitHostPort(lis.Addr().String())
	// Nodes can't use the 127.0.0.1 the tests reach the panel at.
	if err := st.SetSetting(ctx, store.SettingAgentPublicAddr, net.JoinHostPort("panel.example.com", grpcPort)); err != nil {
		t.Fatal(err)
	}

	mux := http.NewServeMux()
	mux.Handle("/api/", New(Options{
		Store:          st,
		Core:           client,
		Manager:        manager,
		Agents:         agents,
		Sampler:        core.NewSampler(client.Status, time.Second, nil),
		Tunnel:         tunnel,
		Cert:           cert,
		CoreConfigPath: configPath,
		CoreBinary:     "/usr/local/bin/wirefox-server",
		GRPCPort:       grpcPort,
		Logger:         logger,
	}))
	srv := httptest.NewServer(SecurityHeaders(mux))
	t.Cleanup(srv.Close)
	jar, _ := cookiejar.New(nil)
	return &testEnv{
		t: t, st: st, core: fake, agents: agents, cert: cert, grpcAddr: lis.Addr().String(),
		configPath: configPath, srv: srv, client: &http.Client{Jar: jar},
	}
}

// do sends a request; body (if not nil) is sent as JSON. The answer is decoded
// into out when out isn't nil.
func (e *testEnv) do(method, path string, body, out any) int {
	e.t.Helper()
	var reader io.Reader
	if body != nil {
		data, err := json.Marshal(body)
		if err != nil {
			e.t.Fatal(err)
		}
		reader = bytes.NewReader(data)
	}
	req, err := http.NewRequest(method, e.srv.URL+path, reader)
	if err != nil {
		e.t.Fatal(err)
	}
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	resp, err := e.client.Do(req)
	if err != nil {
		e.t.Fatal(err)
	}
	defer resp.Body.Close()
	data, _ := io.ReadAll(resp.Body)
	if out != nil {
		if err := json.Unmarshal(data, out); err != nil {
			e.t.Fatalf("%s %s: decoding %s: %v", method, path, data, err)
		}
	}
	return resp.StatusCode
}

func (e *testEnv) login() {
	e.t.Helper()
	if code := e.do("POST", "/api/login", map[string]string{"username": testUser, "password": testPassword}, nil); code != 200 {
		e.t.Fatalf("login: %d", code)
	}
}

func (e *testEnv) createNode(id string) (nodeView, keys.NodeKey) {
	e.t.Helper()
	var resp struct {
		Node nodeView `json:"node"`
		Key  string   `json:"key"`
	}
	if code := e.do("POST", "/api/nodes", map[string]string{"id": id}, &resp); code != 200 {
		e.t.Fatalf("create node: %d", code)
	}
	key, err := keys.Decode(resp.Key)
	if err != nil {
		e.t.Fatalf("decoding %q: %v", resp.Key, err)
	}
	return resp.Node, key
}

// connectAgent connects a node agent with key and returns its stream after the
// handshake.
func (e *testEnv) connectAgent(key keys.NodeKey) agentpb.Agent_ConnectClient {
	e.t.Helper()
	tlsConfig := &tls.Config{
		ServerName:         keys.CertName,
		MinVersion:         tls.VersionTLS13,
		InsecureSkipVerify: true,
		VerifyPeerCertificate: func(raw [][]byte, _ [][]*x509.Certificate) error {
			if keys.Fingerprint(raw[0]) != key.Fingerprint {
				return errors.New("fingerprint mismatch")
			}
			return nil
		},
	}
	cc, err := grpc.NewClient(e.grpcAddr, grpc.WithTransportCredentials(credentials.NewTLS(tlsConfig)))
	if err != nil {
		e.t.Fatal(err)
	}
	e.t.Cleanup(func() { cc.Close() })
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	e.t.Cleanup(cancel)
	ctx = metadata.AppendToOutgoingContext(ctx, "authorization", "Bearer "+key.Secret)
	stream, err := agentpb.NewAgentClient(cc).Connect(ctx)
	if err != nil {
		e.t.Fatal(err)
	}
	if err := stream.Send(&agentpb.NodeMessage{Msg: &agentpb.NodeMessage_Hello{Hello: &agentpb.Hello{Version: "0.1.0"}}}); err != nil {
		e.t.Fatal(err)
	}
	for _, want := range []string{"welcome", "config"} {
		msg, err := stream.Recv()
		if err != nil {
			e.t.Fatalf("waiting for %s: %v", want, err)
		}
		if (want == "welcome" && msg.GetWelcome() == nil) || (want == "config" && msg.GetConfig() == nil) {
			e.t.Fatalf("expected %s, got %v", want, msg)
		}
	}
	return stream
}

func TestLoginLogout(t *testing.T) {
	e := newTestEnv(t, core.External{})
	if code := e.do("GET", "/api/me", nil, nil); code != 401 {
		t.Fatalf("me without session: %d", code)
	}
	if code := e.do("POST", "/api/login", map[string]string{"username": testUser, "password": "wrong"}, nil); code != 401 {
		t.Fatalf("wrong password: %d", code)
	}
	if code := e.do("POST", "/api/login", map[string]string{"username": "root", "password": testPassword}, nil); code != 401 {
		t.Fatalf("unknown user: %d", code)
	}

	body := strings.NewReader(`{"username":"admin","password":"correct horse"}`)
	resp, err := http.Post(e.srv.URL+"/api/login", "application/json", body)
	if err != nil {
		t.Fatal(err)
	}
	resp.Body.Close()
	cookies := resp.Cookies()
	if len(cookies) != 1 || cookies[0].Name != sessionCookie || !cookies[0].HttpOnly ||
		cookies[0].SameSite != http.SameSiteStrictMode || cookies[0].MaxAge != 7*24*3600 {
		t.Fatalf("session cookie: %+v", cookies)
	}

	e.login()
	var me struct{ Username string }
	if code := e.do("GET", "/api/me", nil, &me); code != 200 || me.Username != testUser {
		t.Fatalf("me: %d %+v", code, me)
	}
	if code := e.do("POST", "/api/logout", nil, nil); code != 200 {
		t.Fatalf("logout: %d", code)
	}
	if code := e.do("GET", "/api/me", nil, nil); code != 401 {
		t.Fatalf("me after logout: %d", code)
	}
}

func TestLoginRateLimit(t *testing.T) {
	e := newTestEnv(t, core.External{})
	for i := range 10 {
		if code := e.do("POST", "/api/login", map[string]string{"username": testUser, "password": "nope"}, nil); code != 401 {
			t.Fatalf("attempt %d: %d", i, code)
		}
	}
	if code := e.do("POST", "/api/login", map[string]string{"username": testUser, "password": testPassword}, nil); code != 429 {
		t.Fatalf("after 10 failures: %d", code)
	}
}

func TestChangePassword(t *testing.T) {
	e := newTestEnv(t, core.External{})
	e.login()
	if code := e.do("POST", "/api/password", map[string]string{"current": "wrong", "new": "new password"}, nil); code != 400 {
		t.Fatalf("wrong current password: %d", code)
	}
	if code := e.do("POST", "/api/password", map[string]string{"current": testPassword, "new": "short"}, nil); code != 400 {
		t.Fatalf("short password: %d", code)
	}
	if code := e.do("POST", "/api/password", map[string]string{"current": testPassword, "new": "new password"}, nil); code != 200 {
		t.Fatalf("change: %d", code)
	}
	if code := e.do("GET", "/api/me", nil, nil); code != 200 {
		t.Fatalf("own session ended: %d", code)
	}
	if code := e.do("POST", "/api/login", map[string]string{"username": testUser, "password": "new password"}, nil); code != 200 {
		t.Fatalf("login with the new password: %d", code)
	}
}

func TestRequestHardening(t *testing.T) {
	e := newTestEnv(t, core.External{})
	e.login()

	resp, err := e.client.Get(e.srv.URL + "/api/me")
	if err != nil {
		t.Fatal(err)
	}
	resp.Body.Close()
	for header, want := range map[string]string{
		"X-Frame-Options":        "DENY",
		"X-Content-Type-Options": "nosniff",
		"Referrer-Policy":        "no-referrer",
	} {
		if got := resp.Header.Get(header); got != want {
			t.Errorf("%s: %q", header, got)
		}
	}
	if !strings.HasPrefix(resp.Header.Get("Content-Security-Policy"), "default-src 'self'") {
		t.Errorf("CSP: %q", resp.Header.Get("Content-Security-Policy"))
	}

	post := func(contentType, origin, body string) int {
		req, _ := http.NewRequest("POST", e.srv.URL+"/api/nodes", strings.NewReader(body))
		if contentType != "" {
			req.Header.Set("Content-Type", contentType)
		}
		if origin != "" {
			req.Header.Set("Origin", origin)
		}
		resp, err := e.client.Do(req)
		if err != nil {
			t.Fatal(err)
		}
		resp.Body.Close()
		return resp.StatusCode
	}
	if code := post("application/x-www-form-urlencoded", "", "id=x"); code != http.StatusUnsupportedMediaType {
		t.Errorf("form post: %d", code)
	}
	if code := post("text/plain", "", `{"id":"x"}`); code != http.StatusUnsupportedMediaType {
		t.Errorf("text/plain post: %d", code)
	}
	if code := post("application/json", "https://evil.example", `{"id":"x"}`); code != http.StatusForbidden {
		t.Errorf("cross-origin post: %d", code)
	}
	if code := post("application/json", e.srv.URL, `{"id":"x","bogus":1}`); code != http.StatusBadRequest {
		t.Errorf("unknown field: %d", code)
	}
}

func TestCreateNode(t *testing.T) {
	e := newTestEnv(t, core.External{})
	e.login()
	node, key := e.createNode("office-1")
	if node.ID != "office-1" || node.Config.Version < uint64(time.Now().Add(-time.Hour).UnixMilli()) ||
		node.Config.PoolSize != store.DefaultPoolSize || node.Agent != nil || node.KeyCreatedAt == 0 {
		t.Fatalf("node: %+v", node)
	}
	_, grpcPort, _ := net.SplitHostPort(e.grpcAddr)
	if key.Panel != net.JoinHostPort("panel.example.com", grpcPort) || key.Fingerprint != e.cert.Fingerprint {
		t.Fatalf("key: %+v", key)
	}
	stored, err := e.st.Node(context.Background(), "office-1")
	if err != nil || stored.AgentKeyHash != keys.Hash(key.Secret) || stored.Token != "core-token" {
		t.Fatalf("stored node: %+v, %v", stored, err)
	}

	var nodes []nodeView
	if code := e.do("GET", "/api/nodes", nil, &nodes); code != 200 || len(nodes) != 1 || nodes[0].ID != "office-1" {
		t.Fatalf("list: %d %+v", code, nodes)
	}

	// The agent connects with the key and appears in the node.
	stream := e.connectAgent(key)
	var got nodeView
	e.do("GET", "/api/nodes/office-1", nil, &got)
	if got.Agent == nil || !got.Agent.Connected || got.Agent.Version != "0.1.0" || got.Agent.Interfaces == nil {
		t.Fatalf("agent: %+v", got.Agent)
	}

	// A new key revokes everything the old one gave access to.
	var newKey struct{ Key string }
	if code := e.do("POST", "/api/nodes/office-1/key", nil, &newKey); code != 200 {
		t.Fatalf("new key: %d", code)
	}
	k, err := keys.Decode(newKey.Key)
	if err != nil || k.Secret == key.Secret {
		t.Fatalf("new key %+v, %v", k, err)
	}
	if _, err := stream.Recv(); status.Code(err) != codes.Unauthenticated {
		t.Fatalf("old agent stream: %v", err)
	}
	rotated, err := e.st.Node(context.Background(), "office-1")
	if err != nil || rotated.Token != "rotated-token" || rotated.ConfigVersion != stored.ConfigVersion+1 ||
		rotated.AgentKeyHash != keys.Hash(k.Secret) {
		t.Fatalf("after the new key: %+v, %v", rotated, err)
	}
	e.connectAgent(k)

	if code := e.do("DELETE", "/api/nodes/office-1", nil, nil); code != 200 {
		t.Fatalf("delete: %d", code)
	}
	if _, err := e.st.Node(context.Background(), "office-1"); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("node still stored: %v", err)
	}
	if code := e.do("DELETE", "/api/nodes/office-1", nil, nil); code != 404 {
		t.Fatalf("second delete: %d", code)
	}
}

func TestPatchNodeBumpsVersionAndPushes(t *testing.T) {
	e := newTestEnv(t, core.External{})
	e.login()
	created, key := e.createNode("office-1")
	v := created.Config.Version
	stream := e.connectAgent(key)

	var node nodeView
	patch := map[string]any{"pool_size": 32, "egress": map[string]string{"interface": "eth1", "source_ip": ""}}
	if code := e.do("PATCH", "/api/nodes/office-1", patch, &node); code != 200 {
		t.Fatalf("patch: %d", code)
	}
	if node.Config.Version != v+1 || node.Config.PoolSize != 32 || node.Config.Egress.Interface != "eth1" {
		t.Fatalf("patched node: %+v", node.Config)
	}
	msg, err := stream.Recv()
	if err != nil {
		t.Fatal(err)
	}
	if cfg := msg.GetConfig(); cfg.GetVersion() != v+1 || cfg.GetPoolSize() != 32 || cfg.GetEgress().GetInterface() != "eth1" {
		t.Fatalf("pushed: %v", msg)
	}

	// A rename goes to the core only.
	if code := e.do("PATCH", "/api/nodes/office-1", map[string]string{"name": "Office"}, &node); code != 200 {
		t.Fatalf("rename: %d", code)
	}
	if node.Name != "Office" || node.Config.Version != v+1 || e.core.lastRequest() != `PATCH /api/v1/nodes/office-1 {"name":"Office"}` {
		t.Fatalf("renamed: %+v, core saw %s", node, e.core.lastRequest())
	}

	// Disabling goes to the core and into the tunnel config.
	if code := e.do("PATCH", "/api/nodes/office-1", map[string]bool{"enabled": false}, &node); code != 200 {
		t.Fatalf("disable: %d", code)
	}
	if node.Enabled || node.Config.Version != v+2 {
		t.Fatalf("disabled: %+v", node)
	}
	if msg, err = stream.Recv(); err != nil || msg.GetConfig().GetEnabled() || msg.GetConfig().GetVersion() != v+2 {
		t.Fatalf("pushed: %v, %v", msg, err)
	}

	if code := e.do("PATCH", "/api/nodes/office-1", map[string]int{"pool_size": 0}, nil); code != 400 {
		t.Fatalf("invalid pool size: %d", code)
	}
	if code := e.do("PATCH", "/api/nodes/ghost", map[string]int{"pool_size": 2}, nil); code != 404 {
		t.Fatalf("unknown node: %d", code)
	}
}

func TestNodeCommands(t *testing.T) {
	e := newTestEnv(t, core.External{})
	e.login()
	_, key := e.createNode("office-1")
	if code := e.do("POST", "/api/nodes/office-1/restart", nil, nil); code != http.StatusConflict {
		t.Fatalf("restart while disconnected: %d", code)
	}

	stream := e.connectAgent(key)
	// The node answers interface lists and ignores everything else.
	go func() {
		for {
			msg, err := stream.Recv()
			if err != nil {
				return
			}
			cmd := msg.GetCommand()
			if cmd.GetListInterfaces() == nil {
				continue
			}
			_ = stream.Send(&agentpb.NodeMessage{Msg: &agentpb.NodeMessage_Result{Result: &agentpb.CommandResult{
				Id: cmd.GetId(),
				Payload: &agentpb.CommandResult_Interfaces{Interfaces: &agentpb.InterfaceList{
					Interfaces: []*agentpb.Interface{{Name: "eth0", Up: true, Addrs: []string{"192.0.2.1"}}},
				}},
			}}})
		}
	}()

	var list struct{ Interfaces []store.Interface }
	if code := e.do("POST", "/api/nodes/office-1/interfaces", nil, &list); code != 200 ||
		len(list.Interfaces) != 1 || list.Interfaces[0].Name != "eth0" {
		t.Fatalf("interfaces: %d %+v", code, list)
	}
	if code := e.do("POST", "/api/nodes/office-1/restart", nil, nil); code != http.StatusGatewayTimeout {
		t.Fatalf("unanswered restart: %d", code)
	}
	if code := e.do("GET", "/api/nodes/office-1/logs?lines=5", nil, nil); code != http.StatusGatewayTimeout {
		t.Fatalf("unanswered logs: %d", code)
	}
}

func TestUsersPassThrough(t *testing.T) {
	e := newTestEnv(t, core.External{})
	e.login()
	var created map[string]any
	body := map[string]any{"username": "alice", "nodes": []string{}, "balance": nil}
	if code := e.do("POST", "/api/users", body, &created); code != 200 || created["password"] != "generated" {
		t.Fatalf("create: %d %v", code, created)
	}
	if got := e.core.lastRequest(); got != `POST /api/v1/users {"balance":null,"nodes":[],"username":"alice"}` {
		t.Fatalf("core saw %s", got)
	}
	var errResp struct{ Error string }
	if code := e.do("PATCH", "/api/users/bob", map[string]any{"note": nil}, &errResp); code != 404 ||
		errResp.Error != `user "bob" not found` {
		t.Fatalf("missing user: %d %+v", code, errResp)
	}
	if code := e.do("GET", "/api/users", nil, nil); code != 200 {
		t.Fatalf("list: %d", code)
	}
	e.do("GET", "/api/connections?user=alice&other=x", nil, nil)
	if got := e.core.lastRequest(); got != "GET /api/v1/connections?user=alice" {
		t.Fatalf("core saw %s", got)
	}
	e.do("DELETE", "/api/connections/7", nil, nil)
	if got := e.core.lastRequest(); got != "DELETE /api/v1/connections/7" {
		t.Fatalf("core saw %s", got)
	}
}

func TestCoreConfig(t *testing.T) {
	manager := &fakeManager{}
	e := newTestEnv(t, manager)
	e.login()

	var info struct {
		Mode        string
		Unit        *core.UnitStatus
		Reachable   bool
		Fingerprint string
		Config      coreConfig
		Paths       map[string]string
	}
	if code := e.do("GET", "/api/core", nil, &info); code != 200 {
		t.Fatalf("get: %d", code)
	}
	if info.Mode != "systemd" || info.Unit == nil || !info.Reachable || info.Fingerprint != "aa" ||
		info.Config.TunnelListen != "0.0.0.0:7443" || info.Paths["config"] != e.configPath {
		t.Fatalf("core: %+v", info)
	}

	put := func(cfg coreConfig) (int, bool) {
		var resp struct {
			RestartRequired bool `json:"restart_required"`
		}
		code := e.do("PUT", "/api/core/config", cfg, &resp)
		return code, resp.RestartRequired
	}
	cfg := info.Config
	cfg.Balance = "round-robin"
	cfg.ConnectTimeoutSecs = 30
	if code, restart := put(cfg); code != 200 || restart {
		t.Fatalf("live change: %d, restart %v", code, restart)
	}
	if got := e.core.lastRequest(); got != `PATCH /api/v1/settings {"balance":"round-robin","handshake_timeout_secs":10,"connect_timeout_secs":30}` {
		t.Fatalf("core saw %s", got)
	}
	cfg.SocksListen = []string{"0.0.0.0:1080", "[::]:1080"}
	if code, restart := put(cfg); code != 200 || !restart {
		t.Fatalf("listener change: %d, restart %v", code, restart)
	}
	saved, err := core.LoadConfig(e.configPath)
	if err != nil || saved.Socks.Balance != "round-robin" || len(saved.Socks.Listen) != 2 || saved.Socks.ConnectTimeoutSecs != 30 {
		t.Fatalf("saved: %+v, %v", saved, err)
	}
	cfg.Balance = "fastest"
	if code, _ := put(cfg); code != 400 {
		t.Fatalf("invalid balance: %d", code)
	}

	if code := e.do("POST", "/api/core/restart", nil, nil); code != 200 || len(manager.actions) != 1 {
		t.Fatalf("restart: %d %v", code, manager.actions)
	}
	var logs struct{ Entries []core.LogEntry }
	if code := e.do("GET", "/api/core/logs?lines=10", nil, &logs); code != 200 || len(logs.Entries) != 1 {
		t.Fatalf("logs: %d %+v", code, logs)
	}
}

func TestExternalCoreNotManaged(t *testing.T) {
	e := newTestEnv(t, core.External{})
	e.login()
	for _, action := range []string{"start", "stop", "restart"} {
		if code := e.do("POST", "/api/core/"+action, nil, nil); code != http.StatusNotImplemented {
			t.Errorf("%s: %d", action, code)
		}
	}
	if code := e.do("GET", "/api/core/logs", nil, nil); code != http.StatusNotImplemented {
		t.Errorf("logs: %d", code)
	}
	var overview struct {
		Core    *coreSummary
		Unit    *struct{}
		History []core.Sample
		Events  []store.Event
	}
	if code := e.do("GET", "/api/overview", nil, &overview); code != 200 || overview.Core == nil ||
		!overview.Core.Reachable || overview.Unit != nil || overview.History == nil || overview.Events == nil {
		t.Fatalf("overview: %d %+v", code, overview)
	}
}

func TestSettingsAndEvents(t *testing.T) {
	e := newTestEnv(t, core.External{})
	e.login()
	want := settings{AgentPublicAddr: "panel.example.com:50051", SocksPublicHost: "relay.example.com"}
	var got settings
	if code := e.do("PUT", "/api/settings", want, &got); code != 200 || got != want {
		t.Fatalf("put: %d %+v", code, got)
	}
	if code := e.do("PUT", "/api/settings", settings{AgentPublicAddr: "no-port"}, nil); code != 400 {
		t.Fatalf("invalid address: %d", code)
	}
	e.createNode("office-1")
	var events []store.Event
	if code := e.do("GET", "/api/events?limit=1", nil, &events); code != 200 || len(events) != 1 ||
		events[0].Message != "node office-1 created" {
		t.Fatalf("events: %d %+v", code, events)
	}
}

func TestNodeKeyAddress(t *testing.T) {
	e := newTestEnv(t, core.External{})
	e.login()
	if err := e.st.SetSetting(context.Background(), store.SettingAgentPublicAddr, ""); err != nil {
		t.Fatal(err)
	}
	// The tests reach the panel at 127.0.0.1, which nodes elsewhere can't.
	var errResp struct{ Error string }
	if code := e.do("POST", "/api/nodes", map[string]string{"id": "office-1"}, &errResp); code != http.StatusConflict ||
		!strings.Contains(errResp.Error, "Settings") {
		t.Fatalf("loopback host: %d %+v", code, errResp)
	}
	if n := e.core.requestCount(); n != 0 {
		t.Fatalf("core saw %d requests", n)
	}

	// A host nodes can reach becomes the agent address.
	req, _ := http.NewRequest("POST", e.srv.URL+"/api/nodes", strings.NewReader(`{"id":"office-1"}`))
	req.Host = "panel.example.com:8080"
	req.Header.Set("Content-Type", "application/json")
	// The jar would pick cookies by Host.
	for _, c := range e.client.Jar.Cookies(req.URL) {
		req.AddCookie(c)
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	var created struct{ Key string }
	if err := json.NewDecoder(resp.Body).Decode(&created); err != nil || resp.StatusCode != 200 {
		t.Fatalf("create: %d, %v", resp.StatusCode, err)
	}
	key, err := keys.Decode(created.Key)
	_, grpcPort, _ := net.SplitHostPort(e.grpcAddr)
	if err != nil || key.Panel != net.JoinHostPort("panel.example.com", grpcPort) {
		t.Fatalf("key: %+v, %v", key, err)
	}
	var settings settings
	if e.do("GET", "/api/settings", nil, &settings); settings.AgentPublicAddr != key.Panel {
		t.Fatalf("settings: %+v", settings)
	}
}

func TestAdoptedNodeLosesEgressOverride(t *testing.T) {
	e := newTestEnv(t, core.External{})
	e.login()
	e.core.mu.Lock()
	e.core.nodes["legacy"] = &core.Node{ID: "legacy", Name: "legacy", Enabled: true,
		EgressOverride: &core.Egress{Interface: "wlan0"}}
	e.core.mu.Unlock()

	var before nodeView
	if code := e.do("GET", "/api/nodes/legacy", nil, &before); code != 200 || before.Core.EgressOverride == nil ||
		before.Core.EgressOverride.Interface != "wlan0" {
		t.Fatalf("before adoption: %d %+v", code, before.Core)
	}
	var node nodeView
	if code := e.do("PATCH", "/api/nodes/legacy", map[string]int{"pool_size": 8}, &node); code != 200 {
		t.Fatalf("patch: %d", code)
	}
	if node.Core.EgressOverride != nil || node.Config.PoolSize != 8 {
		t.Fatalf("adopted: %+v", node)
	}
	if got := e.core.lastRequest(); got != `PATCH /api/v1/nodes/legacy {"egress":null}` {
		t.Fatalf("core saw %s", got)
	}
}

func TestDotsOnlyIDsRefused(t *testing.T) {
	e := newTestEnv(t, core.External{})
	e.login()
	for _, id := range []string{".", ".."} {
		if code := e.do("POST", "/api/nodes", map[string]string{"id": id}, nil); code != 400 {
			t.Errorf("node %q: %d", id, code)
		}
		if code := e.do("POST", "/api/users", map[string]string{"username": id}, nil); code != 400 {
			t.Errorf("user %q: %d", id, code)
		}
	}
	if n := e.core.requestCount(); n != 0 {
		t.Fatalf("core saw %d requests", n)
	}
}

func TestTunnelChangeWaitsForRestart(t *testing.T) {
	e := newTestEnv(t, &fakeManager{})
	e.login()
	ctx := context.Background()
	created, key := e.createNode("office-1")
	stream := e.connectAgent(key)
	if err := e.agents.SyncEndpoint(ctx); err != nil {
		t.Fatal(err)
	}

	var info struct{ Config coreConfig }
	e.do("GET", "/api/core", nil, &info)
	cfg := info.Config
	cfg.TunnelPublicAddr = "relay.example.com:8443"
	var resp struct {
		RestartRequired bool `json:"restart_required"`
	}
	if code := e.do("PUT", "/api/core/config", cfg, &resp); code != 200 || !resp.RestartRequired {
		t.Fatalf("put: %d %+v", code, resp)
	}
	// The running core still listens where it did, also at the next poll.
	if err := e.agents.SyncEndpoint(ctx); err != nil {
		t.Fatal(err)
	}
	if n, _ := e.st.Node(ctx, "office-1"); n.ConfigVersion != created.Config.Version {
		t.Fatalf("version bumped to %d before the restart", n.ConfigVersion)
	}

	if code := e.do("POST", "/api/core/restart", nil, nil); code != 200 {
		t.Fatalf("restart: %d", code)
	}
	msg, err := stream.Recv()
	if err != nil || msg.GetConfig().GetServer() != "relay.example.com:8443" {
		t.Fatalf("after the restart: %v, %v", msg, err)
	}
}

func TestLoginLimitHoldsForParallelGuesses(t *testing.T) {
	e := newTestEnv(t, core.External{})
	const guesses = 40
	var wg sync.WaitGroup
	var mu sync.Mutex
	answers := map[int]int{}
	for range guesses {
		wg.Add(1)
		go func() {
			defer wg.Done()
			body := strings.NewReader(`{"username":"admin","password":"guess"}`)
			resp, err := http.Post(e.srv.URL+"/api/login", "application/json", body)
			if err != nil {
				t.Error(err)
				return
			}
			resp.Body.Close()
			mu.Lock()
			answers[resp.StatusCode]++
			mu.Unlock()
		}()
	}
	wg.Wait()
	if answers[401] > maxLoginFailures || answers[401]+answers[429] != guesses {
		t.Fatalf("answers: %v", answers)
	}
}

func TestLimiterKey(t *testing.T) {
	key := func(s string) string { return limiterKey(netip.MustParseAddr(s)) }
	if key("2001:db8::1") != key("2001:db8::abcd:1") {
		t.Error("addresses of one /64 are counted apart")
	}
	if key("2001:db8::1") == key("2001:db8:0:1::1") {
		t.Error("different /64s are counted together")
	}
	if key("192.0.2.1") == key("192.0.2.2") {
		t.Error("IPv4 addresses are counted together")
	}
}

func TestTrustedProxies(t *testing.T) {
	if _, err := ParseProxies("10.0.0.0/8,nonsense"); err == nil {
		t.Fatal("parsed a bad CIDR")
	}
	proxies, err := ParseProxies(" 10.0.0.0/8, 192.0.2.1 ")
	if err != nil {
		t.Fatal(err)
	}
	request := func(remote string) *http.Request {
		r := httptest.NewRequest("POST", "/api/login", nil)
		r.RemoteAddr = remote
		r.Header.Add("X-Forwarded-For", "203.0.113.9, 198.51.100.7")
		r.Header.Add("X-Forwarded-For", "10.1.2.3")
		r.Header.Set("X-Forwarded-Proto", "https")
		r.Header.Set("X-Forwarded-Host", "panel.example.com")
		return r
	}

	behind := request("192.0.2.1:40000")
	if got := proxies.clientIP(behind); got != netip.MustParseAddr("198.51.100.7") {
		t.Errorf("client behind the proxies: %s", got)
	}
	if !proxies.https(behind) || proxies.host(behind) != "panel.example.com" {
		t.Error("forwarded proto or host ignored")
	}
	a := &API{Options: Options{TrustedProxies: proxies}}
	if !a.sessionCookie(behind, "x", 1).Secure {
		t.Error("session cookie not Secure behind a TLS proxy")
	}

	direct := request("198.51.100.1:40000")
	if got := proxies.clientIP(direct); got != netip.MustParseAddr("198.51.100.1") {
		t.Errorf("client without a proxy: %s", got)
	}
	if proxies.https(direct) || proxies.host(direct) != direct.Host || a.sessionCookie(direct, "x", 1).Secure {
		t.Error("forwarding headers believed from an untrusted peer")
	}
	if got := Proxies(nil).clientIP(behind); got != netip.MustParseAddr("192.0.2.1") {
		t.Errorf("no trusted proxies: %s", got)
	}
}
