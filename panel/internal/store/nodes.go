package store

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"strings"
)

// Defaults for new nodes; the same as wirefox-node's own.
const (
	DefaultPoolSize           = 16
	DefaultMaxExtra           = 1024
	DefaultConnectTimeoutSecs = 10
)

// Egress picks where traffic leaves a node. Empty strings mean "not set".
type Egress struct {
	Interface string `json:"interface"`
	SourceIP  string `json:"source_ip"`
}

type Interface struct {
	Name        string   `json:"name"`
	AdapterName string   `json:"adapter_name"`
	Index       uint32   `json:"index"`
	Up          bool     `json:"up"`
	Addrs       []string `json:"addrs"`
}

// AgentInfo is what a node last reported over its agent stream.
type AgentInfo struct {
	Remote        string      `json:"remote"`
	Version       string      `json:"version"`
	OS            string      `json:"os"`
	Arch          string      `json:"arch"`
	Hostname      string      `json:"hostname"`
	TunnelState   string      `json:"tunnel_state"`
	LastError     string      `json:"last_error"`
	ConfigVersion uint64      `json:"config_version"`
	UptimeSecs    uint64      `json:"uptime_secs"`
	ActiveStreams uint64      `json:"active_streams"`
	TotalStreams  uint64      `json:"total_streams"`
	Interfaces    []Interface `json:"interfaces"`
}

// Node is the panel's record of an exit-node. Name, Enabled and CreatedAt
// mirror the WireFox server, which owns them; the rest belongs to the panel.
type Node struct {
	ID        string
	Name      string
	Enabled   bool
	CreatedAt int64
	// The node's tunnel token in the WireFox server. Empty for nodes the
	// panel found in the server but didn't create; a new one is issued
	// together with their first agent key.
	Token string
	// SHA-256 of the agent secret; empty until a key is issued.
	AgentKeyHash string
	KeyCreatedAt int64

	PoolSize           uint32
	MaxExtra           uint32
	ConnectTimeoutSecs uint32
	AllowLocal         bool
	Egress             Egress
	Uplink             Egress
	// Bumped whenever anything that goes into the node's TunnelConfig changes.
	ConfigVersion uint64

	// Nil until the node's agent first connects.
	Agent    *AgentInfo
	LastSeen int64
}

// NewNode returns a node with the default tunnel settings.
func NewNode(id string) *Node {
	return &Node{
		ID:                 id,
		Name:               id,
		Enabled:            true,
		PoolSize:           DefaultPoolSize,
		MaxExtra:           DefaultMaxExtra,
		ConnectTimeoutSecs: DefaultConnectTimeoutSecs,
		ConfigVersion:      1,
	}
}

const nodeColumns = `id, name, enabled, created_at, token, agent_key_hash, key_created_at,
	pool_size, max_extra, connect_timeout_secs, allow_local,
	egress_interface, egress_source_ip, uplink_interface, uplink_source_ip,
	config_version, agent_info, last_seen`

type scanner interface {
	Scan(dest ...any) error
}

func scanNode(row scanner) (*Node, error) {
	var n Node
	var agent sql.NullString
	err := row.Scan(&n.ID, &n.Name, &n.Enabled, &n.CreatedAt, &n.Token, &n.AgentKeyHash, &n.KeyCreatedAt,
		&n.PoolSize, &n.MaxExtra, &n.ConnectTimeoutSecs, &n.AllowLocal,
		&n.Egress.Interface, &n.Egress.SourceIP, &n.Uplink.Interface, &n.Uplink.SourceIP,
		&n.ConfigVersion, &agent, &n.LastSeen)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, ErrNotFound
	}
	if err != nil {
		return nil, err
	}
	if agent.Valid {
		n.Agent = new(AgentInfo)
		if err := json.Unmarshal([]byte(agent.String), n.Agent); err != nil {
			return nil, err
		}
	}
	return &n, nil
}

func (s *Store) CreateNode(ctx context.Context, n *Node) error {
	_, err := s.db.ExecContext(ctx, `INSERT INTO nodes (`+nodeColumns+`)
		VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, 0)`,
		n.ID, n.Name, n.Enabled, n.CreatedAt, n.Token, n.AgentKeyHash, n.KeyCreatedAt,
		n.PoolSize, n.MaxExtra, n.ConnectTimeoutSecs, n.AllowLocal,
		n.Egress.Interface, n.Egress.SourceIP, n.Uplink.Interface, n.Uplink.SourceIP, n.ConfigVersion)
	if err != nil && strings.Contains(err.Error(), "UNIQUE constraint failed") {
		return ErrExists
	}
	return err
}

func (s *Store) Node(ctx context.Context, id string) (*Node, error) {
	return scanNode(s.db.QueryRowContext(ctx, "SELECT "+nodeColumns+" FROM nodes WHERE id = ?", id))
}

// NodeByKeyHash finds the node whose agent secret hashes to hash.
func (s *Store) NodeByKeyHash(ctx context.Context, hash string) (*Node, error) {
	if hash == "" {
		return nil, ErrNotFound
	}
	return scanNode(s.db.QueryRowContext(ctx, "SELECT "+nodeColumns+" FROM nodes WHERE agent_key_hash = ?", hash))
}

// Nodes returns every node, sorted by id.
func (s *Store) Nodes(ctx context.Context) ([]*Node, error) {
	rows, err := s.db.QueryContext(ctx, "SELECT "+nodeColumns+" FROM nodes ORDER BY id")
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var nodes []*Node
	for rows.Next() {
		n, err := scanNode(rows)
		if err != nil {
			return nil, err
		}
		nodes = append(nodes, n)
	}
	return nodes, rows.Err()
}

// UpdateNode loads a node, lets fn change it and saves the result, all in one
// transaction. fn may change everything except the ID and the agent info.
func (s *Store) UpdateNode(ctx context.Context, id string, fn func(*Node) error) (*Node, error) {
	var node *Node
	err := s.tx(ctx, func(tx *sql.Tx) error {
		n, err := scanNode(tx.QueryRowContext(ctx, "SELECT "+nodeColumns+" FROM nodes WHERE id = ?", id))
		if err != nil {
			return err
		}
		if err := fn(n); err != nil {
			return err
		}
		_, err = tx.ExecContext(ctx, `UPDATE nodes SET name = ?, enabled = ?, created_at = ?, token = ?,
			agent_key_hash = ?, key_created_at = ?, pool_size = ?, max_extra = ?, connect_timeout_secs = ?,
			allow_local = ?, egress_interface = ?, egress_source_ip = ?, uplink_interface = ?,
			uplink_source_ip = ?, config_version = ?
			WHERE id = ?`,
			n.Name, n.Enabled, n.CreatedAt, n.Token, n.AgentKeyHash, n.KeyCreatedAt,
			n.PoolSize, n.MaxExtra, n.ConnectTimeoutSecs, n.AllowLocal,
			n.Egress.Interface, n.Egress.SourceIP, n.Uplink.Interface, n.Uplink.SourceIP, n.ConfigVersion, id)
		node = n
		return err
	})
	return node, err
}

// SetAgentInfo records what the node's agent reported and when.
func (s *Store) SetAgentInfo(ctx context.Context, id string, info *AgentInfo, lastSeen int64) error {
	data, err := json.Marshal(info)
	if err != nil {
		return err
	}
	_, err = s.db.ExecContext(ctx, "UPDATE nodes SET agent_info = ?, last_seen = ? WHERE id = ?", data, lastSeen, id)
	return err
}

// BumpAllVersions marks every node's TunnelConfig as changed.
func (s *Store) BumpAllVersions(ctx context.Context) error {
	_, err := s.db.ExecContext(ctx, "UPDATE nodes SET config_version = config_version + 1")
	return err
}

func (s *Store) DeleteNode(ctx context.Context, id string) error {
	res, err := s.db.ExecContext(ctx, "DELETE FROM nodes WHERE id = ?", id)
	if err != nil {
		return err
	}
	n, err := res.RowsAffected()
	if err == nil && n == 0 {
		return ErrNotFound
	}
	return err
}
