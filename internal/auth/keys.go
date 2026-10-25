package auth

import (
	"context"
	"crypto/rand"
	"database/sql"
	"encoding/base32"
	"errors"
	"fmt"
	"strings"
	"sync"
	"time"

	"golang.org/x/crypto/bcrypt"

	"github.com/hubosdev/cliproxy/internal/store"
)

const (
	KeyPrefix    = "sk-cp-"
	keyBodyBytes = 24
	prefixLen    = 8
)

var (
	ErrKeyInvalid = errors.New("invalid api key")
	ErrKeyRevoked = errors.New("api key revoked")
	ErrUserDisabled = errors.New("user disabled")
)

type Identity struct {
	UserID  int64
	UserLabel string
	UserEnabled bool
	KeyID   int64
	KeyPrefix string
}

type Keys struct {
	s     *store.Store
	cache sync.Map // map[string]cacheEntry, keyed by full key
	ttl   time.Duration
}

type cacheEntry struct {
	id     Identity
	expiry time.Time
}

func NewKeys(s *store.Store) *Keys {
	return &Keys{s: s, ttl: 60 * time.Second}
}

func GenerateKey() (string, error) {
	b := make([]byte, keyBodyBytes)
	if _, err := rand.Read(b); err != nil {
		return "", err
	}
	body := strings.ToLower(strings.TrimRight(base32.StdEncoding.EncodeToString(b), "="))
	return KeyPrefix + body, nil
}

func keyPrefix(full string) string {
	if len(full) < prefixLen {
		return full
	}
	return full[:prefixLen]
}

func (k *Keys) Issue(ctx context.Context, userID int64) (string, int64, error) {
	full, err := GenerateKey()
	if err != nil {
		return "", 0, err
	}
	hash, err := bcrypt.GenerateFromPassword([]byte(full), bcrypt.DefaultCost)
	if err != nil {
		return "", 0, err
	}
	res, err := k.s.DB.ExecContext(ctx,
		`INSERT INTO api_keys(user_id, prefix, hash, created_at) VALUES(?,?,?,?)`,
		userID, keyPrefix(full), string(hash), time.Now().Unix())
	if err != nil {
		return "", 0, err
	}
	id, _ := res.LastInsertId()
	return full, id, nil
}

func (k *Keys) Lookup(ctx context.Context, full string) (Identity, error) {
	if !strings.HasPrefix(full, KeyPrefix) {
		return Identity{}, ErrKeyInvalid
	}
	if v, ok := k.cache.Load(full); ok {
		ent := v.(cacheEntry)
		if time.Now().Before(ent.expiry) {
			return ent.id, nil
		}
		k.cache.Delete(full)
	}
	rows, err := k.s.DB.QueryContext(ctx, `
		SELECT api_keys.id, api_keys.hash, api_keys.revoked_at,
		       users.id, users.label, users.enabled
		FROM api_keys
		JOIN users ON users.id = api_keys.user_id
		WHERE api_keys.prefix = ?`, keyPrefix(full))
	if err != nil {
		return Identity{}, err
	}
	defer rows.Close()
	for rows.Next() {
		var keyID, userID int64
		var hash, label string
		var enabled int
		var revokedAt sql.NullInt64
		if err := rows.Scan(&keyID, &hash, &revokedAt, &userID, &label, &enabled); err != nil {
			return Identity{}, err
		}
		if bcrypt.CompareHashAndPassword([]byte(hash), []byte(full)) != nil {
			continue
		}
		if revokedAt.Valid {
			return Identity{}, ErrKeyRevoked
		}
		if enabled == 0 {
			return Identity{}, ErrUserDisabled
		}
		id := Identity{
			UserID: userID, UserLabel: label, UserEnabled: enabled == 1,
			KeyID: keyID, KeyPrefix: keyPrefix(full),
		}
		k.cache.Store(full, cacheEntry{id: id, expiry: time.Now().Add(k.ttl)})
		go k.touch(keyID)
		return id, nil
	}
	return Identity{}, ErrKeyInvalid
}

func (k *Keys) touch(keyID int64) {
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	_, _ = k.s.DB.ExecContext(ctx, `UPDATE api_keys SET last_used_at=? WHERE id=?`, time.Now().Unix(), keyID)
}

func (k *Keys) Revoke(ctx context.Context, keyID int64) error {
	_, err := k.s.DB.ExecContext(ctx, `UPDATE api_keys SET revoked_at=? WHERE id=? AND revoked_at IS NULL`, time.Now().Unix(), keyID)
	if err != nil {
		return err
	}
	k.cache.Range(func(key, _ any) bool {
		k.cache.Delete(key)
		return true
	})
	return nil
}

func (k *Keys) SetUserEnabled(ctx context.Context, userID int64, enabled bool) error {
	v := 0
	if enabled {
		v = 1
	}
	if _, err := k.s.DB.ExecContext(ctx, `UPDATE users SET enabled=? WHERE id=?`, v, userID); err != nil {
		return err
	}
	k.cache.Range(func(key, _ any) bool {
		k.cache.Delete(key)
		return true
	})
	return nil
}

type UserKeys struct {
	UserID    int64
	Label     string
	Enabled   bool
	CreatedAt time.Time
	Keys      []KeyInfo
}

type KeyInfo struct {
	ID         int64
	Prefix     string
	CreatedAt  time.Time
	LastUsedAt *time.Time
	RevokedAt  *time.Time
}

func (k *Keys) ListUsers(ctx context.Context) ([]UserKeys, error) {
	rows, err := k.s.DB.QueryContext(ctx, `SELECT id, label, enabled, created_at FROM users ORDER BY id`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []UserKeys
	for rows.Next() {
		var u UserKeys
		var enabled int
		var created int64
		if err := rows.Scan(&u.UserID, &u.Label, &enabled, &created); err != nil {
			return nil, err
		}
		u.Enabled = enabled == 1
		u.CreatedAt = time.Unix(created, 0)
		out = append(out, u)
	}
	for i, u := range out {
		krows, err := k.s.DB.QueryContext(ctx,
			`SELECT id, prefix, created_at, last_used_at, revoked_at FROM api_keys WHERE user_id=? ORDER BY id`, u.UserID)
		if err != nil {
			return nil, err
		}
		for krows.Next() {
			var ki KeyInfo
			var created int64
			var last, revoked sql.NullInt64
			if err := krows.Scan(&ki.ID, &ki.Prefix, &created, &last, &revoked); err != nil {
				krows.Close()
				return nil, err
			}
			ki.CreatedAt = time.Unix(created, 0)
			if last.Valid {
				t := time.Unix(last.Int64, 0)
				ki.LastUsedAt = &t
			}
			if revoked.Valid {
				t := time.Unix(revoked.Int64, 0)
				ki.RevokedAt = &t
			}
			out[i].Keys = append(out[i].Keys, ki)
		}
		krows.Close()
	}
	return out, nil
}

func (k *Keys) CreateUser(ctx context.Context, label string) (int64, error) {
	if strings.TrimSpace(label) == "" {
		return 0, fmt.Errorf("label required")
	}
	res, err := k.s.DB.ExecContext(ctx,
		`INSERT INTO users(label, created_at, enabled) VALUES(?,?,1)`, label, time.Now().Unix())
	if err != nil {
		return 0, err
	}
	id, _ := res.LastInsertId()
	return id, nil
}
