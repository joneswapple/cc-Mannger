package auth

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"errors"
	"strings"
	"time"

	"github.com/hubosdev/cliproxy/internal/store"
)

var (
	ErrInviteInvalid  = errors.New("invite invalid")
	ErrInviteExpired  = errors.New("invite expired")
	ErrInviteRedeemed = errors.New("invite already redeemed")
)

type Invites struct {
	s    *store.Store
	keys *Keys
}

func NewInvites(s *store.Store, keys *Keys) *Invites {
	return &Invites{s: s, keys: keys}
}

type InviteInfo struct {
	Label      string
	CreatedAt  time.Time
	ExpiresAt  time.Time
	RedeemedAt *time.Time
	UserID     *int64
	TokenHash  string
}

func hashToken(token string) string {
	h := sha256.Sum256([]byte(token))
	return hex.EncodeToString(h[:])
}

func (in *Invites) Create(ctx context.Context, label string, ttl time.Duration) (token string, err error) {
	if strings.TrimSpace(label) == "" {
		return "", errors.New("label required")
	}
	b := make([]byte, 24)
	if _, err := rand.Read(b); err != nil {
		return "", err
	}
	token = hex.EncodeToString(b)
	now := time.Now()
	_, err = in.s.DB.ExecContext(ctx,
		`INSERT INTO invites(token_hash, label, created_at, expires_at) VALUES(?,?,?,?)`,
		hashToken(token), label, now.Unix(), now.Add(ttl).Unix())
	if err != nil {
		return "", err
	}
	return token, nil
}

func (in *Invites) List(ctx context.Context) ([]InviteInfo, error) {
	rows, err := in.s.DB.QueryContext(ctx,
		`SELECT token_hash, label, created_at, expires_at, redeemed_at, redeemed_user_id FROM invites ORDER BY created_at DESC`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []InviteInfo
	for rows.Next() {
		var ii InviteInfo
		var created, expires int64
		var redeemed sql.NullInt64
		var userID sql.NullInt64
		if err := rows.Scan(&ii.TokenHash, &ii.Label, &created, &expires, &redeemed, &userID); err != nil {
			return nil, err
		}
		ii.CreatedAt = time.Unix(created, 0)
		ii.ExpiresAt = time.Unix(expires, 0)
		if redeemed.Valid {
			t := time.Unix(redeemed.Int64, 0)
			ii.RedeemedAt = &t
		}
		if userID.Valid {
			id := userID.Int64
			ii.UserID = &id
		}
		out = append(out, ii)
	}
	return out, nil
}

func (in *Invites) DeleteByHash(ctx context.Context, tokenHash string) error {
	_, err := in.s.DB.ExecContext(ctx, `DELETE FROM invites WHERE token_hash=? AND redeemed_at IS NULL`, tokenHash)
	return err
}

type Redeemed struct {
	UserID int64
	APIKey string
	KeyID  int64
}

func (in *Invites) Redeem(ctx context.Context, token, label string) (Redeemed, error) {
	if strings.TrimSpace(label) == "" {
		label = "(unnamed)"
	}
	tx, err := in.s.DB.BeginTx(ctx, nil)
	if err != nil {
		return Redeemed{}, err
	}
	defer tx.Rollback()

	var inviteLabel string
	var expires int64
	var redeemed sql.NullInt64
	err = tx.QueryRowContext(ctx,
		`SELECT label, expires_at, redeemed_at FROM invites WHERE token_hash=?`, hashToken(token)).
		Scan(&inviteLabel, &expires, &redeemed)
	if err == sql.ErrNoRows {
		return Redeemed{}, ErrInviteInvalid
	}
	if err != nil {
		return Redeemed{}, err
	}
	if redeemed.Valid {
		return Redeemed{}, ErrInviteRedeemed
	}
	if time.Now().Unix() > expires {
		return Redeemed{}, ErrInviteExpired
	}

	finalLabel := label
	if finalLabel == "(unnamed)" && inviteLabel != "" {
		finalLabel = inviteLabel
	}
	res, err := tx.ExecContext(ctx,
		`INSERT INTO users(label, created_at, enabled) VALUES(?,?,1)`, finalLabel, time.Now().Unix())
	if err != nil {
		return Redeemed{}, err
	}
	userID, _ := res.LastInsertId()

	full, err := GenerateKey()
	if err != nil {
		return Redeemed{}, err
	}
	hash, err := bcryptHash(full)
	if err != nil {
		return Redeemed{}, err
	}
	res, err = tx.ExecContext(ctx,
		`INSERT INTO api_keys(user_id, prefix, hash, created_at) VALUES(?,?,?,?)`,
		userID, keyPrefix(full), hash, time.Now().Unix())
	if err != nil {
		return Redeemed{}, err
	}
	keyID, _ := res.LastInsertId()

	_, err = tx.ExecContext(ctx,
		`UPDATE invites SET redeemed_at=?, redeemed_user_id=? WHERE token_hash=?`,
		time.Now().Unix(), userID, hashToken(token))
	if err != nil {
		return Redeemed{}, err
	}
	if err := tx.Commit(); err != nil {
		return Redeemed{}, err
	}
	return Redeemed{UserID: userID, APIKey: full, KeyID: keyID}, nil
}
