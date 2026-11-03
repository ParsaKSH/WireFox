// Package keys handles the node keys operators paste into exit-nodes, random
// secrets, and the panel's own TLS certificate.
package keys

import (
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"strings"
)

const keyPrefix = "wfx1."

// NodeKey is everything a node needs to reach and authenticate to the panel.
type NodeKey struct {
	// host:port of the panel's gRPC listener.
	Panel string `json:"panel"`
	// Hex SHA-256 of the panel's certificate.
	Fingerprint string `json:"fp"`
	// Hex, 32 random bytes. The panel keeps only HashSecret(Secret).
	Secret string `json:"secret"`
}

// Encode returns the "wfx1." form of the key.
func (k NodeKey) Encode() string {
	data, err := json.Marshal(k)
	if err != nil {
		// A struct of three strings always marshals.
		panic(err)
	}
	return keyPrefix + base64.RawURLEncoding.EncodeToString(data)
}

// Decode parses and validates a key produced by Encode.
func Decode(s string) (NodeKey, error) {
	var k NodeKey
	payload, ok := strings.CutPrefix(strings.TrimSpace(s), keyPrefix)
	if !ok {
		return k, fmt.Errorf("node key must start with %q", keyPrefix)
	}
	data, err := base64.RawURLEncoding.DecodeString(payload)
	if err != nil {
		return k, fmt.Errorf("node key is not valid base64url: %w", err)
	}
	if err := json.Unmarshal(data, &k); err != nil {
		return k, fmt.Errorf("node key payload is not valid JSON: %w", err)
	}
	if _, port, err := net.SplitHostPort(k.Panel); err != nil || port == "" {
		return k, fmt.Errorf("node key has an invalid panel address %q", k.Panel)
	}
	if !isHex(k.Fingerprint, sha256.Size) {
		return k, errors.New("node key has an invalid fingerprint")
	}
	if !isHex(k.Secret, 32) {
		return k, errors.New("node key has an invalid secret")
	}
	return k, nil
}

func isHex(s string, bytes int) bool {
	if len(s) != 2*bytes || strings.ToLower(s) != s {
		return false
	}
	_, err := hex.DecodeString(s)
	return err == nil
}

// RandomHex returns n random bytes, hex encoded.
func RandomHex(n int) string {
	buf := make([]byte, n)
	// Never fails since Go 1.24; it crashes the program instead.
	_, _ = rand.Read(buf)
	return hex.EncodeToString(buf)
}

// NewSecret returns a fresh node secret.
func NewSecret() string {
	return RandomHex(32)
}

// Hash returns the hex SHA-256 of s. Node secrets, session tokens and the
// core's API keys are all stored this way.
func Hash(s string) string {
	sum := sha256.Sum256([]byte(s))
	return hex.EncodeToString(sum[:])
}
