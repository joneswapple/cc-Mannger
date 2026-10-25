package auth

import (
	"context"
	"crypto/rand"
	"crypto/subtle"
	"encoding/hex"
	"errors"
	"strings"

	"golang.org/x/crypto/bcrypt"

	"github.com/hubosdev/cliproxy/internal/store"
)

const adminTokenSettingKey = "admin_token_hash"

var ErrAdminUnauthorized = errors.New("unauthorized")

type Admin struct {
	s *store.Store
}

func NewAdmin(s *store.Store) *Admin { return &Admin{s: s} }

// Bootstrap ensures an admin token exists. If preset is non-empty it is
// installed; otherwise a token is generated and returned. Returns the empty
// string when an admin token was already present and no preset was given.
func (a *Admin) Bootstrap(ctx context.Context, preset string) (string, error) {
	existing, err := a.s.GetSetting(ctx, adminTokenSettingKey)
	if err != nil {
		return "", err
	}
	if existing != "" && preset == "" {
		return "", nil
	}
	token := preset
	if token == "" {
		b := make([]byte, 24)
		if _, err := rand.Read(b); err != nil {
			return "", err
		}
		token = hex.EncodeToString(b)
	}
	hash, err := bcrypt.GenerateFromPassword([]byte(token), bcrypt.DefaultCost)
	if err != nil {
		return "", err
	}
	if err := a.s.SetSetting(ctx, adminTokenSettingKey, string(hash)); err != nil {
		return "", err
	}
	return token, nil
}

// Reset installs a new admin token, overwriting any existing one. If
// preset is empty a random token is generated. Returns the plaintext
// token (always non-empty on success).
func (a *Admin) Reset(ctx context.Context, preset string) (string, error) {
	token := preset
	if token == "" {
		b := make([]byte, 24)
		if _, err := rand.Read(b); err != nil {
			return "", err
		}
		token = hex.EncodeToString(b)
	}
	hash, err := bcrypt.GenerateFromPassword([]byte(token), bcrypt.DefaultCost)
	if err != nil {
		return "", err
	}
	if err := a.s.SetSetting(ctx, adminTokenSettingKey, string(hash)); err != nil {
		return "", err
	}
	return token, nil
}

func (a *Admin) Verify(ctx context.Context, token string) error {
	if strings.TrimSpace(token) == "" {
		return ErrAdminUnauthorized
	}
	hash, err := a.s.GetSetting(ctx, adminTokenSettingKey)
	if err != nil {
		return err
	}
	if hash == "" {
		return ErrAdminUnauthorized
	}
	if bcrypt.CompareHashAndPassword([]byte(hash), []byte(token)) != nil {
		return ErrAdminUnauthorized
	}
	return nil
}

// ConstantTimeEq is exported for callers that want to compare arbitrary
// secret strings without leaking length-independent timing.
func ConstantTimeEq(a, b string) bool {
	return subtle.ConstantTimeCompare([]byte(a), []byte(b)) == 1
}
