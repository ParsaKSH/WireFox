// Package store keeps the panel's state in SQLite: settings, the admin
// account, sessions, nodes and the event log.
package store

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"net/url"
	"os"

	_ "modernc.org/sqlite"
)

var (
	ErrNotFound = errors.New("not found")
	ErrExists   = errors.New("already exists")
)

// Each entry upgrades the schema by one version (PRAGMA user_version).
var migrations = []string{
	`CREATE TABLE settings (
		k TEXT PRIMARY KEY,
		v TEXT NOT NULL
	);
	CREATE TABLE admin (
		username      TEXT PRIMARY KEY,
		password_hash TEXT NOT NULL
	);
	CREATE TABLE sessions (
		token_hash TEXT PRIMARY KEY,
		username   TEXT NOT NULL,
		expires_at INTEGER NOT NULL
	);
	CREATE TABLE events (
		id      INTEGER PRIMARY KEY AUTOINCREMENT,
		ts      INTEGER NOT NULL,
		level   TEXT NOT NULL,
		message TEXT NOT NULL
	);
	CREATE TABLE nodes (
		id                   TEXT PRIMARY KEY,
		name                 TEXT NOT NULL,
		enabled              INTEGER NOT NULL,
		created_at           INTEGER NOT NULL,
		token                TEXT NOT NULL,
		agent_key_hash       TEXT NOT NULL,
		key_created_at       INTEGER NOT NULL,
		pool_size            INTEGER NOT NULL,
		max_extra            INTEGER NOT NULL,
		connect_timeout_secs INTEGER NOT NULL,
		allow_local          INTEGER NOT NULL,
		egress_interface     TEXT NOT NULL,
		egress_source_ip     TEXT NOT NULL,
		uplink_interface     TEXT NOT NULL,
		uplink_source_ip     TEXT NOT NULL,
		config_version       INTEGER NOT NULL,
		agent_info           TEXT,
		last_seen            INTEGER NOT NULL DEFAULT 0
	);
	CREATE UNIQUE INDEX nodes_agent_key_hash ON nodes (agent_key_hash) WHERE agent_key_hash != '';`,
}

type Store struct {
	db *sql.DB
}

// Open opens (creating if needed) the database at path and brings its schema
// up to date.
func Open(path string) (*Store, error) {
	// The database holds the core's API key. SQLite creates its journal
	// files with the database's mode.
	f, err := os.OpenFile(path, os.O_RDWR|os.O_CREATE, 0o600)
	if err != nil {
		return nil, err
	}
	f.Close()
	dsn := "file:" + (&url.URL{Path: path}).EscapedPath() +
		"?_pragma=busy_timeout(5000)&_pragma=journal_mode(WAL)&_pragma=synchronous(NORMAL)"
	db, err := sql.Open("sqlite", dsn)
	if err != nil {
		return nil, err
	}
	// SQLite allows one writer anyway; a single connection also keeps
	// transactions from tripping over each other with SQLITE_BUSY.
	db.SetMaxOpenConns(1)
	s := &Store{db: db}
	if err := s.migrate(context.Background()); err != nil {
		db.Close()
		return nil, fmt.Errorf("migrating %s: %w", path, err)
	}
	return s, nil
}

func (s *Store) Close() error {
	return s.db.Close()
}

func (s *Store) migrate(ctx context.Context) error {
	var version int
	if err := s.db.QueryRowContext(ctx, "PRAGMA user_version").Scan(&version); err != nil {
		return err
	}
	if version > len(migrations) {
		return fmt.Errorf("database schema version %d is newer than this panel (%d)", version, len(migrations))
	}
	for i := version; i < len(migrations); i++ {
		err := s.tx(ctx, func(tx *sql.Tx) error {
			if _, err := tx.ExecContext(ctx, migrations[i]); err != nil {
				return err
			}
			_, err := tx.ExecContext(ctx, fmt.Sprintf("PRAGMA user_version = %d", i+1))
			return err
		})
		if err != nil {
			return err
		}
	}
	return nil
}

func (s *Store) tx(ctx context.Context, fn func(*sql.Tx) error) error {
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	if err := fn(tx); err != nil {
		tx.Rollback()
		return err
	}
	return tx.Commit()
}

// Keys of the settings table.
const (
	// The plaintext API key the panel uses for the core.
	SettingCoreAPIKey = "core_api_key"
	// Last known fingerprint of the core's tunnel certificate.
	SettingCoreFingerprint = "core_fingerprint"
	// The server address and fingerprint last put into tunnel configs.
	SettingTunnelEndpoint = "tunnel_endpoint"
	// Where nodes reach the panel's gRPC listener (host:port).
	SettingAgentPublicAddr = "agent_public_addr"
	// Host shown in users' socks5:// addresses.
	SettingSocksPublicHost = "socks_public_host"
	// The tunnel listener of the running core, see core.RunningTunnel.
	SettingCoreTunnel = "core_tunnel"

	// The highest config version handed to a new node or held by a deleted one.
	settingLastConfigVersion = "last_config_version"
)

// Setting returns the value stored under key, or "" when there is none.
func (s *Store) Setting(ctx context.Context, key string) (string, error) {
	var v string
	err := s.db.QueryRowContext(ctx, "SELECT v FROM settings WHERE k = ?", key).Scan(&v)
	if errors.Is(err, sql.ErrNoRows) {
		return "", nil
	}
	return v, err
}

func (s *Store) SetSetting(ctx context.Context, key, value string) error {
	_, err := s.db.ExecContext(ctx,
		"INSERT INTO settings (k, v) VALUES (?, ?) ON CONFLICT (k) DO UPDATE SET v = excluded.v", key, value)
	return err
}
