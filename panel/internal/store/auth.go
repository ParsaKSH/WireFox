package store

import (
	"context"
	"database/sql"
	"errors"
	"time"
)

// AdminHash returns the password hash of the given admin user.
func (s *Store) AdminHash(ctx context.Context, username string) (string, error) {
	var hash string
	err := s.db.QueryRowContext(ctx, "SELECT password_hash FROM admin WHERE username = ?", username).Scan(&hash)
	if errors.Is(err, sql.ErrNoRows) {
		return "", ErrNotFound
	}
	return hash, err
}

func (s *Store) HasAdmin(ctx context.Context) (bool, error) {
	var n int
	err := s.db.QueryRowContext(ctx, "SELECT COUNT(*) FROM admin").Scan(&n)
	return n > 0, err
}

// SetAdmin creates the admin user or replaces its password.
func (s *Store) SetAdmin(ctx context.Context, username, passwordHash string) error {
	_, err := s.db.ExecContext(ctx,
		`INSERT INTO admin (username, password_hash) VALUES (?, ?)
		 ON CONFLICT (username) DO UPDATE SET password_hash = excluded.password_hash`,
		username, passwordHash)
	return err
}

func (s *Store) CreateSession(ctx context.Context, tokenHash, username string, expires time.Time) error {
	_, err := s.db.ExecContext(ctx, "INSERT INTO sessions (token_hash, username, expires_at) VALUES (?, ?, ?)",
		tokenHash, username, expires.Unix())
	return err
}

// SessionUser returns the user a live session belongs to.
func (s *Store) SessionUser(ctx context.Context, tokenHash string, now time.Time) (string, error) {
	var username string
	err := s.db.QueryRowContext(ctx,
		`SELECT s.username FROM sessions s JOIN admin a ON a.username = s.username
		 WHERE s.token_hash = ? AND s.expires_at > ?`,
		tokenHash, now.Unix()).Scan(&username)
	if errors.Is(err, sql.ErrNoRows) {
		return "", ErrNotFound
	}
	return username, err
}

func (s *Store) DeleteSession(ctx context.Context, tokenHash string) error {
	_, err := s.db.ExecContext(ctx, "DELETE FROM sessions WHERE token_hash = ?", tokenHash)
	return err
}

// DeleteUserSessions ends every session of username except the one with keepHash.
func (s *Store) DeleteUserSessions(ctx context.Context, username, keepHash string) error {
	_, err := s.db.ExecContext(ctx, "DELETE FROM sessions WHERE username = ? AND token_hash != ?", username, keepHash)
	return err
}

func (s *Store) PurgeExpiredSessions(ctx context.Context, now time.Time) error {
	_, err := s.db.ExecContext(ctx, "DELETE FROM sessions WHERE expires_at <= ?", now.Unix())
	return err
}
