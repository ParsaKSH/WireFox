package core

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
)

// fakeRunner records commands and answers with canned output.
type fakeRunner struct {
	calls   []string
	outputs map[string][]byte
	fail    map[string]error
}

func (f *fakeRunner) Run(_ context.Context, name string, args ...string) ([]byte, error) {
	call := strings.Join(append([]string{name}, args...), " ")
	f.calls = append(f.calls, call)
	return f.outputs[call], f.fail[call]
}

func TestUnitFile(t *testing.T) {
	unit := string(UnitFile("/usr/local/bin/wirefox-server", "/etc/wirefox/server.toml", "/var/lib/wirefox"))
	for _, line := range []string{
		"ExecStart=/usr/local/bin/wirefox-server run -c /etc/wirefox/server.toml",
		"Restart=always",
		"RestartSec=2",
		"LimitNOFILE=1048576",
		"After=network-online.target",
		"Wants=network-online.target",
		"ReadWritePaths=/var/lib/wirefox",
		"CapabilityBoundingSet=CAP_NET_BIND_SERVICE",
		"WantedBy=multi-user.target",
	} {
		if !slices.Contains(strings.Split(unit, "\n"), line) {
			t.Errorf("unit is missing %q:\n%s", line, unit)
		}
	}
}

func TestUnitFileQuotesPaths(t *testing.T) {
	unit := string(UnitFile("/opt/wire fox/server", "/etc/wirefox/100%.toml", "/var/lib/wirefox"))
	if !strings.Contains(unit, `ExecStart="/opt/wire fox/server" run -c /etc/wirefox/100%%.toml`) {
		t.Fatalf("paths not escaped:\n%s", unit)
	}
}

func newSystemd(t *testing.T, runner Runner) *Systemd {
	dir := t.TempDir()
	return &Systemd{
		Unit:       "wirefox-server",
		UnitDir:    filepath.Join(dir, "units"),
		Binary:     "/usr/local/bin/wirefox-server",
		ConfigPath: "/etc/wirefox/server.toml",
		DataDir:    filepath.Join(dir, "data"),
		Runner:     runner,
	}
}

func TestSystemdSetup(t *testing.T) {
	runner := &fakeRunner{}
	s := newSystemd(t, runner)
	ctx := context.Background()

	if err := s.Setup(ctx, false); err != nil {
		t.Fatal(err)
	}
	want := []string{"systemctl daemon-reload", "systemctl enable wirefox-server", "systemctl start wirefox-server"}
	if !slices.Equal(runner.calls, want) {
		t.Fatalf("first setup ran %q, want %q", runner.calls, want)
	}
	if _, err := os.Stat(s.DataDir); err != nil {
		t.Fatalf("data dir not created: %v", err)
	}

	// An unchanged unit is left alone.
	runner.calls = nil
	if err := s.Setup(ctx, false); err != nil {
		t.Fatal(err)
	}
	if want := []string{"systemctl enable wirefox-server", "systemctl start wirefox-server"}; !slices.Equal(runner.calls, want) {
		t.Fatalf("second setup ran %q, want %q", runner.calls, want)
	}

	runner.calls = nil
	if err := s.Setup(ctx, true); err != nil {
		t.Fatal(err)
	}
	if want := []string{"systemctl enable wirefox-server", "systemctl restart wirefox-server"}; !slices.Equal(runner.calls, want) {
		t.Fatalf("setup with restart ran %q, want %q", runner.calls, want)
	}

	// A changed unit is rewritten and the service restarted to use it.
	s.Binary = "/opt/wirefox-server"
	runner.calls = nil
	if err := s.Setup(ctx, false); err != nil {
		t.Fatal(err)
	}
	want = []string{"systemctl daemon-reload", "systemctl enable wirefox-server", "systemctl restart wirefox-server"}
	if !slices.Equal(runner.calls, want) {
		t.Fatalf("setup after a change ran %q, want %q", runner.calls, want)
	}
	unit, err := os.ReadFile(s.UnitPath())
	if err != nil || !strings.Contains(string(unit), "ExecStart=/opt/wirefox-server run") {
		t.Fatalf("unit not rewritten: %s, %v", unit, err)
	}
}

func TestSystemdErrors(t *testing.T) {
	runner := &fakeRunner{fail: map[string]error{"systemctl start wirefox-server": errors.New("boom")}}
	s := newSystemd(t, runner)
	if err := s.Start(context.Background()); err == nil {
		t.Fatal("start error swallowed")
	}
}

func TestSystemdStatus(t *testing.T) {
	runner := &fakeRunner{outputs: map[string][]byte{
		"systemctl show wirefox-server --property=ActiveState,SubState,MainPID,ActiveEnterTimestamp": []byte(
			"ActiveState=active\nSubState=running\nMainPID=1234\nActiveEnterTimestamp=Tue 2026-11-03 05:55:12 UTC\n"),
	}}
	st, err := newSystemd(t, runner).Status(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	want := UnitStatus{Name: "wirefox-server", ActiveState: "active", SubState: "running", MainPID: 1234, Since: 1793685312}
	if *st != want {
		t.Fatalf("got %+v, want %+v", *st, want)
	}
}

func TestParseShowNeverStarted(t *testing.T) {
	st := parseShow("x", []byte("ActiveState=inactive\nSubState=dead\nMainPID=0\nActiveEnterTimestamp=\n"))
	if st.ActiveState != "inactive" || st.MainPID != 0 || st.Since != 0 {
		t.Fatalf("got %+v", st)
	}
}

func TestSystemdLogs(t *testing.T) {
	journal := `{"__REALTIME_TIMESTAMP":"1793600000123456","PRIORITY":"6","MESSAGE":"\u001b[32m INFO\u001b[0m tunnel listening"}
{"__REALTIME_TIMESTAMP":"1793600001000000","PRIORITY":"3","MESSAGE":[27,91,51,49,109,98,97,100,32,255,27,91,48,109]}
{"__REALTIME_TIMESTAMP":"1793600002000000","MESSAGE":"no priority"}
{"__REALTIME_TIMESTAMP":"1793600003000000","PRIORITY":"6","MESSAGE":"2026-11-03T05:55:12.123456Z  WARN tunnel.public_addr is not set"}
{"__REALTIME_TIMESTAMP":"1793600004000000","PRIORITY":"6","MESSAGE":"ERROR node session failed"}
{"__REALTIME_TIMESTAMP":"1793600005000000","PRIORITY":"6","MESSAGE":"user WARN is not a level"}
not json
{"PRIORITY":"6","MESSAGE":"no timestamp"}
`
	runner := &fakeRunner{outputs: map[string][]byte{
		"journalctl -u wirefox-server -o json -n 50 --no-pager": []byte(journal),
	}}
	entries, err := newSystemd(t, runner).Logs(context.Background(), 50)
	if err != nil {
		t.Fatal(err)
	}
	want := []LogEntry{
		{UnixMs: 1793600000123, Priority: 6, Message: " INFO tunnel listening"},
		{UnixMs: 1793600001000, Priority: 3, Message: "bad \uFFFD"},
		{UnixMs: 1793600002000, Priority: 6, Message: "no priority"},
		{UnixMs: 1793600003000, Priority: 4, Message: "2026-11-03T05:55:12.123456Z  WARN tunnel.public_addr is not set"},
		{UnixMs: 1793600004000, Priority: 3, Message: "ERROR node session failed"},
		{UnixMs: 1793600005000, Priority: 6, Message: "user WARN is not a level"},
	}
	if !slices.Equal(entries, want) {
		t.Fatalf("got %q\nwant %q", entries, want)
	}
}

func TestParseJournalEmpty(t *testing.T) {
	if entries := ParseJournal(nil); entries == nil || len(entries) != 0 {
		t.Fatalf("got %#v", entries)
	}
}

func TestExternal(t *testing.T) {
	ctx := context.Background()
	var m Manager = External{}
	for name, err := range map[string]error{
		"start":   m.Start(ctx),
		"stop":    m.Stop(ctx),
		"restart": m.Restart(ctx),
	} {
		if !errors.Is(err, ErrNotSupported) {
			t.Errorf("%s: %v", name, err)
		}
	}
	if _, err := m.Logs(ctx, 10); !errors.Is(err, ErrNotSupported) {
		t.Errorf("logs: %v", err)
	}
	if st, err := m.Status(ctx); st != nil || err != nil {
		t.Errorf("status: %v, %v", st, err)
	}
	if err := m.Setup(ctx, true); err != nil {
		t.Errorf("setup: %v", err)
	}
}
