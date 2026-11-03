package store

import (
	"context"
	"time"
)

// MaxEvents is how many events the log keeps.
const MaxEvents = 5000

type Level string

const (
	Info  Level = "info"
	Warn  Level = "warn"
	Error Level = "error"
)

type Event struct {
	ID      int64  `json:"id"`
	TS      int64  `json:"ts"`
	Level   Level  `json:"level"`
	Message string `json:"message"`
}

// AddEvent appends to the event log and drops the oldest entries beyond MaxEvents.
func (s *Store) AddEvent(ctx context.Context, level Level, message string) error {
	res, err := s.db.ExecContext(ctx, "INSERT INTO events (ts, level, message) VALUES (?, ?, ?)",
		time.Now().Unix(), level, message)
	if err != nil {
		return err
	}
	id, err := res.LastInsertId()
	if err != nil {
		return err
	}
	_, err = s.db.ExecContext(ctx, "DELETE FROM events WHERE id <= ?", id-MaxEvents)
	return err
}

// Events returns up to limit events, newest first.
func (s *Store) Events(ctx context.Context, limit int) ([]Event, error) {
	rows, err := s.db.QueryContext(ctx, "SELECT id, ts, level, message FROM events ORDER BY id DESC LIMIT ?", limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	events := []Event{}
	for rows.Next() {
		var e Event
		if err := rows.Scan(&e.ID, &e.TS, &e.Level, &e.Message); err != nil {
			return nil, err
		}
		events = append(events, e)
	}
	return events, rows.Err()
}
