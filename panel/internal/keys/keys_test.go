package keys

import (
	"crypto/sha256"
	"crypto/x509"
	"encoding/base64"
	"encoding/hex"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestKeyRoundTrip(t *testing.T) {
	want := NodeKey{Panel: "panel.example.com:50051", Fingerprint: RandomHex(32), Secret: NewSecret()}
	encoded := want.Encode()
	if !strings.HasPrefix(encoded, "wfx1.") {
		t.Fatalf("missing prefix: %s", encoded)
	}
	if strings.ContainsAny(encoded[len("wfx1."):], "+/=") {
		t.Fatalf("not base64url without padding: %s", encoded)
	}
	got, err := Decode(" " + encoded + "\n")
	if err != nil {
		t.Fatal(err)
	}
	if got != want {
		t.Fatalf("got %+v, want %+v", got, want)
	}
}

func TestKeyWireFormat(t *testing.T) {
	k := NodeKey{Panel: "[2001:db8::1]:50051", Fingerprint: strings.Repeat("ab", 32), Secret: strings.Repeat("01", 32)}
	payload, err := base64.RawURLEncoding.DecodeString(strings.TrimPrefix(k.Encode(), "wfx1."))
	if err != nil {
		t.Fatal(err)
	}
	want := `{"panel":"[2001:db8::1]:50051","fp":"` + k.Fingerprint + `","secret":"` + k.Secret + `"}`
	if string(payload) != want {
		t.Fatalf("payload %s, want %s", payload, want)
	}
}

func TestDecodeRejectsMalformedKeys(t *testing.T) {
	fp, secret := strings.Repeat("ab", 32), strings.Repeat("cd", 32)
	encode := func(json string) string { return "wfx1." + base64.RawURLEncoding.EncodeToString([]byte(json)) }
	cases := map[string]string{
		"empty":           "",
		"wrong prefix":    "wfx2." + strings.TrimPrefix(NodeKey{"a:1", fp, secret}.Encode(), "wfx1."),
		"bad base64":      "wfx1.!!!",
		"padded base64":   "wfx1." + base64.URLEncoding.EncodeToString([]byte(`{"panel":"a:1"}`)),
		"not json":        encode("panel"),
		"no port":         encode(`{"panel":"example.com","fp":"` + fp + `","secret":"` + secret + `"}`),
		"short fp":        encode(`{"panel":"a:1","fp":"abcd","secret":"` + secret + `"}`),
		"uppercase fp":    encode(`{"panel":"a:1","fp":"` + strings.ToUpper(fp) + `","secret":"` + secret + `"}`),
		"non-hex secret":  encode(`{"panel":"a:1","fp":"` + fp + `","secret":"` + strings.Repeat("zz", 32) + `"}`),
		"missing secret":  encode(`{"panel":"a:1","fp":"` + fp + `"}`),
		"missing payload": "wfx1.",
	}
	for name, key := range cases {
		if _, err := Decode(key); err == nil {
			t.Errorf("%s: accepted %q", name, key)
		}
	}
}

func TestHash(t *testing.T) {
	// echo -n abc | sha256sum
	if got := Hash("abc"); got != "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad" {
		t.Fatal(got)
	}
	if a, b := NewSecret(), NewSecret(); a == b || len(a) != 64 {
		t.Fatalf("secrets %q %q", a, b)
	}
}

func TestCertGeneratedOnceAndReloaded(t *testing.T) {
	dir := t.TempDir()
	certPath, keyPath := filepath.Join(dir, "panel.crt"), filepath.Join(dir, "panel.key")
	first, err := LoadOrCreateCert(certPath, keyPath)
	if err != nil {
		t.Fatal(err)
	}
	info, err := os.Stat(keyPath)
	if err != nil {
		t.Fatal(err)
	}
	if perm := info.Mode().Perm(); perm != 0o600 {
		t.Fatalf("key mode %o", perm)
	}

	leaf, err := x509.ParseCertificate(first.TLS.Certificate[0])
	if err != nil {
		t.Fatal(err)
	}
	if len(leaf.DNSNames) != 1 || leaf.DNSNames[0] != CertName {
		t.Fatalf("SANs %v", leaf.DNSNames)
	}
	if years := leaf.NotAfter.Sub(leaf.NotBefore).Hours() / 24 / 365; years < 9.9 {
		t.Fatalf("valid for %.1f years", years)
	}
	if leaf.IsCA {
		t.Fatal("certificate must not be a CA")
	}
	sum := sha256.Sum256(leaf.Raw)
	if first.Fingerprint != hex.EncodeToString(sum[:]) {
		t.Fatalf("fingerprint %q", first.Fingerprint)
	}

	second, err := LoadOrCreateCert(certPath, keyPath)
	if err != nil {
		t.Fatal(err)
	}
	if second.Fingerprint != first.Fingerprint {
		t.Fatal("certificate was regenerated")
	}
}

func TestCertNeedsBothFiles(t *testing.T) {
	dir := t.TempDir()
	certPath, keyPath := filepath.Join(dir, "panel.crt"), filepath.Join(dir, "panel.key")
	if _, err := LoadOrCreateCert(certPath, keyPath); err != nil {
		t.Fatal(err)
	}
	if err := os.Remove(keyPath); err != nil {
		t.Fatal(err)
	}
	if _, err := LoadOrCreateCert(certPath, keyPath); err == nil {
		t.Fatal("loaded a certificate without its key")
	}
}

func TestCertFinishesInterruptedGeneration(t *testing.T) {
	dir := t.TempDir()
	certPath, keyPath := filepath.Join(dir, "panel.crt"), filepath.Join(dir, "panel.key")
	// A first start that died after writing the key.
	if err := os.WriteFile(keyPath, []byte("-----BEGIN PRIVATE"), 0o600); err != nil {
		t.Fatal(err)
	}
	cert, err := LoadOrCreateCert(certPath, keyPath)
	if err != nil {
		t.Fatal(err)
	}
	again, err := LoadOrCreateCert(certPath, keyPath)
	if err != nil || again.Fingerprint != cert.Fingerprint {
		t.Fatalf("reloaded %+v, %v", again, err)
	}
}
