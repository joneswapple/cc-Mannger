// Package secret seals small values (OAuth tokens) at rest with AES-256-GCM.
package secret

import (
	"crypto/aes"
	"crypto/cipher"
	"crypto/rand"
	"encoding/base64"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// KeyFileName is the key file created under data_dir when no key is
// configured. Losing it makes every stored account token unreadable.
const KeyFileName = "secret.key"

// Box encrypts and decrypts with a single 32-byte key. Safe for concurrent use.
type Box struct {
	aead cipher.AEAD
}

func New(key []byte) (*Box, error) {
	if len(key) != 32 {
		return nil, fmt.Errorf("secret key must be 32 bytes, got %d", len(key))
	}
	block, err := aes.NewCipher(key)
	if err != nil {
		return nil, err
	}
	aead, err := cipher.NewGCM(block)
	if err != nil {
		return nil, err
	}
	return &Box{aead: aead}, nil
}

// Seal returns nonce||ciphertext. aad binds the ciphertext to its context
// (e.g. which account and field it belongs to) so rows can't be swapped.
func (b *Box) Seal(plain, aad []byte) ([]byte, error) {
	nonce := make([]byte, b.aead.NonceSize())
	if _, err := rand.Read(nonce); err != nil {
		return nil, err
	}
	return b.aead.Seal(nonce, nonce, plain, aad), nil
}

func (b *Box) Open(sealed, aad []byte) ([]byte, error) {
	n := b.aead.NonceSize()
	if len(sealed) < n {
		return nil, errors.New("secret: ciphertext too short")
	}
	plain, err := b.aead.Open(nil, sealed[:n], sealed[n:], aad)
	if err != nil {
		return nil, fmt.Errorf("secret: decrypt failed (wrong key?): %w", err)
	}
	return plain, nil
}

// Load resolves the key: a configured base64 value wins; otherwise
// dataDir/secret.key is read, or created with a fresh random key (0600).
func Load(configured, dataDir string) (*Box, error) {
	if configured != "" {
		key, err := decodeKey(configured)
		if err != nil {
			return nil, fmt.Errorf("secret_key: %w", err)
		}
		return New(key)
	}
	path := filepath.Join(dataDir, KeyFileName)
	b, err := os.ReadFile(path)
	if err == nil {
		key, err := decodeKey(string(b))
		if err != nil {
			return nil, fmt.Errorf("%s: %w", path, err)
		}
		return New(key)
	}
	if !os.IsNotExist(err) {
		return nil, fmt.Errorf("read %s: %w", path, err)
	}
	key := make([]byte, 32)
	if _, err := rand.Read(key); err != nil {
		return nil, err
	}
	if err := os.MkdirAll(dataDir, 0o700); err != nil {
		return nil, err
	}
	f, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o600)
	if err != nil {
		return nil, fmt.Errorf("create %s: %w", path, err)
	}
	if _, err := f.WriteString(base64.StdEncoding.EncodeToString(key) + "\n"); err != nil {
		f.Close()
		return nil, err
	}
	if err := f.Close(); err != nil {
		return nil, err
	}
	return New(key)
}

func decodeKey(s string) ([]byte, error) {
	key, err := base64.StdEncoding.DecodeString(strings.TrimSpace(s))
	if err != nil {
		return nil, fmt.Errorf("expected base64-encoded 32-byte key: %w", err)
	}
	if len(key) != 32 {
		return nil, fmt.Errorf("expected 32-byte key, got %d bytes", len(key))
	}
	return key, nil
}
