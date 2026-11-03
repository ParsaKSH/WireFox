package keys

import (
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/sha256"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/hex"
	"encoding/pem"
	"errors"
	"fmt"
	"io/fs"
	"math/big"
	"os"
	"time"

	"github.com/ParsaKSH/WireFox/panel/internal/atomicfile"
)

// CertName is the DNS name in the panel certificate and the SNI nodes send.
const CertName = "wirefox-panel"

const certValidity = 10 * 365 * 24 * time.Hour

// Cert is the panel's TLS identity.
type Cert struct {
	TLS tls.Certificate
	// Lowercase hex SHA-256 of the certificate's DER encoding.
	Fingerprint string
}

// LoadOrCreateCert loads the certificate and key at the given paths, or
// generates a self-signed pair there when there is no certificate yet. The
// certificate is written last, so a key without one is left from a first
// start that didn't finish, and nothing can have pinned it.
func LoadOrCreateCert(certPath, keyPath string) (*Cert, error) {
	_, certErr := os.Stat(certPath)
	_, keyErr := os.Stat(keyPath)
	switch {
	case errors.Is(certErr, fs.ErrNotExist) && (keyErr == nil || errors.Is(keyErr, fs.ErrNotExist)):
		if err := generate(certPath, keyPath); err != nil {
			return nil, err
		}
	case certErr != nil || keyErr != nil:
		return nil, fmt.Errorf("need both %s and %s: %w", certPath, keyPath, errors.Join(certErr, keyErr))
	}
	return LoadCert(certPath, keyPath)
}

// LoadCert loads a PEM certificate and key.
func LoadCert(certPath, keyPath string) (*Cert, error) {
	pair, err := tls.LoadX509KeyPair(certPath, keyPath)
	if err != nil {
		return nil, fmt.Errorf("loading certificate: %w", err)
	}
	return &Cert{TLS: pair, Fingerprint: Fingerprint(pair.Certificate[0])}, nil
}

// Fingerprint returns the lowercase hex SHA-256 of a DER certificate.
func Fingerprint(der []byte) string {
	sum := sha256.Sum256(der)
	return hex.EncodeToString(sum[:])
}

func generate(certPath, keyPath string) error {
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		return err
	}
	serial, err := rand.Int(rand.Reader, new(big.Int).Lsh(big.NewInt(1), 128))
	if err != nil {
		return err
	}
	now := time.Now()
	tmpl := &x509.Certificate{
		SerialNumber:          serial,
		Subject:               pkix.Name{CommonName: CertName},
		DNSNames:              []string{CertName},
		NotBefore:             now.Add(-time.Hour),
		NotAfter:              now.Add(certValidity),
		KeyUsage:              x509.KeyUsageDigitalSignature,
		ExtKeyUsage:           []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth},
		BasicConstraintsValid: true,
	}
	der, err := x509.CreateCertificate(rand.Reader, tmpl, tmpl, &key.PublicKey, key)
	if err != nil {
		return err
	}
	keyDER, err := x509.MarshalPKCS8PrivateKey(key)
	if err != nil {
		return err
	}
	if err := atomicfile.Write(keyPath, pem.EncodeToMemory(&pem.Block{Type: "PRIVATE KEY", Bytes: keyDER}), 0o600); err != nil {
		return fmt.Errorf("writing %s: %w", keyPath, err)
	}
	if err := atomicfile.Write(certPath, pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der}), 0o644); err != nil {
		return fmt.Errorf("writing %s: %w", certPath, err)
	}
	return nil
}
