package core

import (
	"bufio"
	"bytes"
	"encoding/json"
	"regexp"
	"strconv"
)

// Defaults to "info" for entries without a PRIORITY field.
const defaultPriority = 6

var ansiEscape = regexp.MustCompile(`\x1b\[[0-9;?]*[ -/]*[@-~]`)

// journalEntry holds the fields we use from "journalctl -o json". journald
// exports every value as a string, except MESSAGE when it isn't valid UTF-8
// or contains control characters: then it is an array of byte values.
type journalEntry struct {
	RealtimeTimestamp string          `json:"__REALTIME_TIMESTAMP"`
	Priority          string          `json:"PRIORITY"`
	Message           json.RawMessage `json:"MESSAGE"`
}

// ParseJournal parses "journalctl -o json" output, one entry per line.
// Lines that aren't journal entries are skipped.
func ParseJournal(out []byte) []LogEntry {
	entries := []LogEntry{}
	sc := bufio.NewScanner(bytes.NewReader(out))
	sc.Buffer(make([]byte, 0, 64*1024), 16<<20)
	for sc.Scan() {
		var raw journalEntry
		if json.Unmarshal(sc.Bytes(), &raw) != nil {
			continue
		}
		micros, err := strconv.ParseInt(raw.RealtimeTimestamp, 10, 64)
		if err != nil {
			continue
		}
		priority, err := strconv.Atoi(raw.Priority)
		if err != nil {
			priority = defaultPriority
		}
		entries = append(entries, LogEntry{
			UnixMs:   micros / 1000,
			Priority: priority,
			Message:  ansiEscape.ReplaceAllString(decodeMessage(raw.Message), ""),
		})
	}
	return entries
}

func decodeMessage(raw json.RawMessage) string {
	var s string
	if json.Unmarshal(raw, &s) == nil {
		return s
	}
	var values []int
	if json.Unmarshal(raw, &values) != nil {
		return ""
	}
	b := make([]byte, len(values))
	for i, v := range values {
		b[i] = byte(v)
	}
	return string(bytes.ToValidUTF8(b, []byte("�")))
}
