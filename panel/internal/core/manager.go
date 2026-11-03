package core

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/ParsaKSH/WireFox/panel/internal/atomicfile"
)

// ErrNotSupported is returned by managers that can't control the core process.
var ErrNotSupported = errors.New("the panel does not manage the core process (external mode)")

// UnitStatus is the state of the core's systemd service.
type UnitStatus struct {
	Name        string `json:"name"`
	ActiveState string `json:"active_state"`
	SubState    string `json:"sub_state"`
	MainPID     int    `json:"main_pid"`
	// Unix seconds the service last became active; 0 if never.
	Since int64 `json:"since"`
}

// LogEntry is one line of the core's log.
type LogEntry struct {
	UnixMs   int64  `json:"unix_ms"`
	Priority int    `json:"priority"`
	Message  string `json:"message"`
}

// Manager runs the core process.
type Manager interface {
	// Mode is "systemd" or "external".
	Mode() string
	// UnitPath is the service file, empty when there is none.
	UnitPath() string
	// Setup installs the service (if this manager has one) and makes sure the
	// core runs; restart forces a restart of a running core.
	Setup(ctx context.Context, restart bool) error
	// Status returns nil, nil when there is no unit to report on.
	Status(ctx context.Context) (*UnitStatus, error)
	Start(ctx context.Context) error
	Stop(ctx context.Context) error
	Restart(ctx context.Context) error
	Logs(ctx context.Context, lines int) ([]LogEntry, error)
}

// Runner runs external commands; tests replace it.
type Runner interface {
	// Run returns the command's stdout. A failure includes its stderr.
	Run(ctx context.Context, name string, args ...string) ([]byte, error)
}

// ExecRunner runs commands for real, in the C locale and UTC so their output
// has a predictable format.
type ExecRunner struct{}

func (ExecRunner) Run(ctx context.Context, name string, args ...string) ([]byte, error) {
	cmd := exec.CommandContext(ctx, name, args...)
	cmd.Env = append(os.Environ(), "LC_ALL=C", "TZ=UTC", "SYSTEMD_PAGER=", "SYSTEMD_COLORS=0")
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	out, err := cmd.Output()
	if err != nil {
		if msg := strings.TrimSpace(stderr.String()); msg != "" {
			return out, fmt.Errorf("%s %s: %w: %s", name, strings.Join(args, " "), err, msg)
		}
		return out, fmt.Errorf("%s %s: %w", name, strings.Join(args, " "), err)
	}
	return out, nil
}

// External is used when something other than the panel runs the core.
type External struct{}

func (External) Mode() string                                  { return "external" }
func (External) UnitPath() string                              { return "" }
func (External) Setup(context.Context, bool) error             { return nil }
func (External) Status(context.Context) (*UnitStatus, error)   { return nil, nil }
func (External) Start(context.Context) error                   { return ErrNotSupported }
func (External) Stop(context.Context) error                    { return ErrNotSupported }
func (External) Restart(context.Context) error                 { return ErrNotSupported }
func (External) Logs(context.Context, int) ([]LogEntry, error) { return nil, ErrNotSupported }

// Systemd runs the core as a systemd service.
type Systemd struct {
	Unit       string
	UnitDir    string
	Binary     string
	ConfigPath string
	DataDir    string
	Runner     Runner
}

func (s *Systemd) Mode() string { return "systemd" }

func (s *Systemd) UnitPath() string {
	return filepath.Join(s.UnitDir, s.Unit+".service")
}

func (s *Systemd) systemctl(ctx context.Context, args ...string) ([]byte, error) {
	return s.Runner.Run(ctx, "systemctl", args...)
}

func (s *Systemd) Setup(ctx context.Context, restart bool) error {
	// The unit makes this path writable, which fails if it doesn't exist.
	if err := os.MkdirAll(s.DataDir, 0o700); err != nil {
		return err
	}
	unit := UnitFile(s.Binary, s.ConfigPath, s.DataDir)
	current, err := os.ReadFile(s.UnitPath())
	if err != nil && !errors.Is(err, os.ErrNotExist) {
		return err
	}
	if !bytes.Equal(current, unit) {
		if err := atomicfile.Write(s.UnitPath(), unit, 0o644); err != nil {
			return fmt.Errorf("writing %s: %w", s.UnitPath(), err)
		}
		if _, err := s.systemctl(ctx, "daemon-reload"); err != nil {
			return err
		}
		// A changed unit only takes effect when the service restarts.
		restart = restart || current != nil
	}
	if _, err := s.systemctl(ctx, "enable", s.Unit); err != nil {
		return err
	}
	if restart {
		return s.Restart(ctx)
	}
	return s.Start(ctx)
}

func (s *Systemd) Status(ctx context.Context) (*UnitStatus, error) {
	out, err := s.systemctl(ctx, "show", s.Unit, "--property=ActiveState,SubState,MainPID,ActiveEnterTimestamp")
	if err != nil {
		return nil, err
	}
	return parseShow(s.Unit, out), nil
}

func parseShow(unit string, out []byte) *UnitStatus {
	st := &UnitStatus{Name: unit}
	for _, line := range strings.Split(string(out), "\n") {
		key, value, _ := strings.Cut(strings.TrimSpace(line), "=")
		switch key {
		case "ActiveState":
			st.ActiveState = value
		case "SubState":
			st.SubState = value
		case "MainPID":
			st.MainPID, _ = strconv.Atoi(value)
		case "ActiveEnterTimestamp":
			st.Since = parseTimestamp(value)
		}
	}
	return st
}

// parseTimestamp reads systemctl's timestamps: "Tue 2026-11-03 05:55:12 UTC"
// (ExecRunner forces UTC).
func parseTimestamp(s string) int64 {
	t, err := time.Parse("Mon 2006-01-02 15:04:05 MST", s)
	if err != nil {
		return 0
	}
	return t.Unix()
}

func (s *Systemd) Start(ctx context.Context) error {
	_, err := s.systemctl(ctx, "start", s.Unit)
	return err
}

func (s *Systemd) Stop(ctx context.Context) error {
	_, err := s.systemctl(ctx, "stop", s.Unit)
	return err
}

func (s *Systemd) Restart(ctx context.Context) error {
	_, err := s.systemctl(ctx, "restart", s.Unit)
	return err
}

func (s *Systemd) Logs(ctx context.Context, lines int) ([]LogEntry, error) {
	out, err := s.Runner.Run(ctx, "journalctl", "-u", s.Unit, "-o", "json", "-n", strconv.Itoa(lines), "--no-pager")
	if err != nil {
		return nil, err
	}
	return ParseJournal(out), nil
}
