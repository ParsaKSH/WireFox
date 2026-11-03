package core

import (
	"bytes"
	"context"
	"crypto/tls"
	"crypto/x509"
	"encoding/json"
	"encoding/pem"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/netip"
	"net/url"
	"os"
	"time"
)

// APIError is an error answer from the core API.
type APIError struct {
	Status  int
	Message string
}

func (e *APIError) Error() string {
	return fmt.Sprintf("core API: %s (HTTP %d)", e.Message, e.Status)
}

// IsNotFound reports whether err is a 404 from the core.
func IsNotFound(err error) bool {
	var apiErr *APIError
	return errors.As(err, &apiErr) && apiErr.Status == http.StatusNotFound
}

type Traffic struct {
	BytesUp           uint64 `json:"bytes_up"`
	BytesDown         uint64 `json:"bytes_down"`
	ActiveConnections uint64 `json:"active_connections"`
	TotalConnections  uint64 `json:"total_connections"`
}

type Status struct {
	Version           string `json:"version"`
	UptimeSecs        uint64 `json:"uptime_secs"`
	TunnelFingerprint string `json:"tunnel_fingerprint"`
	DefaultBalance    string `json:"default_balance"`
	NodesTotal        uint64 `json:"nodes_total"`
	NodesOnline       uint64 `json:"nodes_online"`
	UsersTotal        uint64 `json:"users_total"`
	ActiveConnections uint64 `json:"active_connections"`
	BytesUp           uint64 `json:"bytes_up"`
	BytesDown         uint64 `json:"bytes_down"`
}

type Session struct {
	Remote      string  `json:"remote"`
	ConnectedAt int64   `json:"connected_at"`
	RTTMs       float64 `json:"rtt_ms"`
	IdlePool    int     `json:"idle_pool"`
}

type Node struct {
	ID        string   `json:"id"`
	Name      string   `json:"name"`
	Enabled   bool     `json:"enabled"`
	CreatedAt int64    `json:"created_at"`
	Online    bool     `json:"online"`
	Traffic   Traffic  `json:"traffic"`
	Session   *Session `json:"session"`
}

// Settings are the core settings that apply without a restart.
type Settings struct {
	Balance              string `json:"balance"`
	HandshakeTimeoutSecs uint64 `json:"handshake_timeout_secs"`
	ConnectTimeoutSecs   uint64 `json:"connect_timeout_secs"`
}

// Client talks to the core's REST API with an API key.
type Client struct {
	base   string
	apiKey string
	http   *http.Client
}

// NewClient returns a client for the API of the core configured by the file
// at configPath. When the API uses TLS, the client trusts exactly the core's
// own certificate.
func NewClient(cfg *ServerConfig, configPath, apiKey string) (*Client, error) {
	addr, err := netip.ParseAddrPort(cfg.API.Listen)
	if err != nil {
		return nil, fmt.Errorf("api.listen: %w", err)
	}
	host := addr.Addr()
	if host.IsUnspecified() {
		host = netip.IPv6Loopback()
		if addr.Addr().Is4() {
			host = netip.AddrFrom4([4]byte{127, 0, 0, 1})
		}
	}
	transport := http.DefaultTransport.(*http.Transport).Clone()
	transport.Proxy = nil
	scheme := "http"
	if cfg.API.TLS {
		scheme = "https"
		transport.TLSClientConfig = pinnedTLS(cfg.CertPath(configPath))
	}
	return &Client{
		base:   scheme + "://" + netip.AddrPortFrom(host, addr.Port()).String() + "/api/v1",
		apiKey: apiKey,
		http:   &http.Client{Transport: transport, Timeout: 15 * time.Second},
	}, nil
}

// pinnedTLS accepts only the certificate stored at certPath. It's read on each
// handshake, as the core generates it on its first start.
func pinnedTLS(certPath string) *tls.Config {
	return &tls.Config{
		InsecureSkipVerify: true,
		VerifyPeerCertificate: func(rawCerts [][]byte, _ [][]*x509.Certificate) error {
			data, err := os.ReadFile(certPath)
			if err != nil {
				return err
			}
			block, _ := pem.Decode(data)
			if block == nil || len(rawCerts) == 0 || !bytes.Equal(block.Bytes, rawCerts[0]) {
				return errors.New("core certificate does not match " + certPath)
			}
			return nil
		},
	}
}

// Do sends a request to the core and returns the status and body of any
// answer it gets. Errors are transport errors only.
func (c *Client) Do(ctx context.Context, method, path string, body []byte) (int, []byte, error) {
	var reader io.Reader
	if body != nil {
		reader = bytes.NewReader(body)
	}
	req, err := http.NewRequestWithContext(ctx, method, c.base+path, reader)
	if err != nil {
		return 0, nil, err
	}
	req.Header.Set("Authorization", "Bearer "+c.apiKey)
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	resp, err := c.http.Do(req)
	if err != nil {
		return 0, nil, fmt.Errorf("core API unreachable: %w", unwrapURLError(err))
	}
	defer resp.Body.Close()
	data, err := io.ReadAll(io.LimitReader(resp.Body, 32<<20))
	if err != nil {
		return 0, nil, fmt.Errorf("reading core API response: %w", err)
	}
	return resp.StatusCode, data, nil
}

func unwrapURLError(err error) error {
	var urlErr *url.Error
	if errors.As(err, &urlErr) {
		var opErr *net.OpError
		if errors.As(urlErr.Err, &opErr) {
			return opErr
		}
		return urlErr.Err
	}
	return err
}

// call sends in (if not nil) as JSON and decodes the answer into out (if not nil).
func (c *Client) call(ctx context.Context, method, path string, in, out any) error {
	var body []byte
	if in != nil {
		var err error
		if body, err = json.Marshal(in); err != nil {
			return err
		}
	}
	status, data, err := c.Do(ctx, method, path, body)
	if err != nil {
		return err
	}
	if status >= 300 {
		return &APIError{Status: status, Message: ErrorMessage(data)}
	}
	if out == nil {
		return nil
	}
	if err := json.Unmarshal(data, out); err != nil {
		return fmt.Errorf("decoding core API response to %s %s: %w", method, path, err)
	}
	return nil
}

// ErrorMessage extracts the message from a core error body.
func ErrorMessage(body []byte) string {
	var e struct {
		Error string `json:"error"`
	}
	if json.Unmarshal(body, &e) == nil && e.Error != "" {
		return e.Error
	}
	if len(body) > 200 {
		body = body[:200]
	}
	return string(body)
}

func (c *Client) Status(ctx context.Context) (*Status, error) {
	var s Status
	return &s, c.call(ctx, http.MethodGet, "/status", nil, &s)
}

func (c *Client) Nodes(ctx context.Context) ([]Node, error) {
	var nodes []Node
	return nodes, c.call(ctx, http.MethodGet, "/nodes", nil, &nodes)
}

func (c *Client) Node(ctx context.Context, id string) (*Node, error) {
	var n Node
	return &n, c.call(ctx, http.MethodGet, "/nodes/"+url.PathEscape(id), nil, &n)
}

// CreateNode creates a node and returns it with its tunnel token. An empty id
// lets the core pick one.
func (c *Client) CreateNode(ctx context.Context, id, name string) (*Node, string, error) {
	type request struct {
		ID   string `json:"id,omitempty"`
		Name string `json:"name,omitempty"`
	}
	var resp struct {
		Node  Node   `json:"node"`
		Token string `json:"token"`
	}
	if err := c.call(ctx, http.MethodPost, "/nodes", request{id, name}, &resp); err != nil {
		return nil, "", err
	}
	return &resp.Node, resp.Token, nil
}

// NodePatch changes a node's name and/or enabled flag; nil fields stay.
type NodePatch struct {
	Name    *string `json:"name,omitempty"`
	Enabled *bool   `json:"enabled,omitempty"`
}

func (c *Client) UpdateNode(ctx context.Context, id string, patch NodePatch) (*Node, error) {
	var n Node
	return &n, c.call(ctx, http.MethodPatch, "/nodes/"+url.PathEscape(id), patch, &n)
}

func (c *Client) DeleteNode(ctx context.Context, id string) error {
	return c.call(ctx, http.MethodDelete, "/nodes/"+url.PathEscape(id), nil, nil)
}

// RotateToken issues a new tunnel token for the node.
func (c *Client) RotateToken(ctx context.Context, id string) (string, error) {
	var resp struct {
		Token string `json:"token"`
	}
	return resp.Token, c.call(ctx, http.MethodPost, "/nodes/"+url.PathEscape(id)+"/rotate-token", nil, &resp)
}

func (c *Client) ResetNodeTraffic(ctx context.Context, id string) (*Node, error) {
	var n Node
	return &n, c.call(ctx, http.MethodPost, "/nodes/"+url.PathEscape(id)+"/reset-traffic", nil, &n)
}

func (c *Client) Settings(ctx context.Context) (*Settings, error) {
	var s Settings
	return &s, c.call(ctx, http.MethodGet, "/settings", nil, &s)
}

func (c *Client) UpdateSettings(ctx context.Context, s Settings) (*Settings, error) {
	var out Settings
	return &out, c.call(ctx, http.MethodPatch, "/settings", s, &out)
}
