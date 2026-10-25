// Package accounts persists the upstream Claude OAuth accounts cliproxy can
// forward through. Account metadata lives in plaintext; tokens are sealed
// with internal/secret before they reach SQLite.
package accounts

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/hubosdev/cliproxy/internal/secret"
	"github.com/hubosdev/cliproxy/internal/store"
)

// Account status values. Phase 0 only produces active and needs_reauth;
// the scheduler adds cooling later.
const (
	StatusActive      = "active"
	StatusNeedsReauth = "needs_reauth"
)

var ErrNotFound = errors.New("account not found")

type Account struct {
	ID           int64
	Label        string
	Email        string
	OrgUUID      string
	Plan         string
	Priority     int
	Weight       int
	Enabled      bool
	Status       string
	StatusReason string
	ProxyURL     string
	SourcePath   string
	SyncFile     bool
	CreatedAt    time.Time
	UpdatedAt    time.Time
}

// Credentials is one OAuth token set. The JSON tags match the Claude Code
// CLI credentials file so the same struct reads and writes that file.
type Credentials struct {
	AccessToken  string   `json:"accessToken"`
	RefreshToken string   `json:"refreshToken"`
	ExpiresAt    int64    `json:"expiresAt"` // unix millis
	Scopes       []string `json:"scopes,omitempty"`
}

type Store struct {
	s   *store.Store
	box *secret.Box
}

func New(s *store.Store, box *secret.Box) *Store {
	return &Store{s: s, box: box}
}

const accountCols = `id, label, email, org_uuid, plan, priority, weight, enabled,
	status, status_reason, proxy_url, source_path, sync_file, created_at, updated_at`

func scanAccount(row interface{ Scan(...any) error }) (Account, error) {
	var a Account
	var enabled, syncFile int
	var created, updated int64
	err := row.Scan(&a.ID, &a.Label, &a.Email, &a.OrgUUID, &a.Plan, &a.Priority, &a.Weight, &enabled,
		&a.Status, &a.StatusReason, &a.ProxyURL, &a.SourcePath, &syncFile, &created, &updated)
	if err != nil {
		return a, err
	}
	a.Enabled = enabled == 1
	a.SyncFile = syncFile == 1
	a.CreatedAt = time.Unix(created, 0)
	a.UpdatedAt = time.Unix(updated, 0)
	return a, nil
}

// Create inserts an account and its sealed credentials in one transaction.
func (st *Store) Create(ctx context.Context, a Account, c Credentials) (int64, error) {
	if a.Label == "" {
		return 0, errors.New("account label is required")
	}
	if c.AccessToken == "" {
		return 0, errors.New("account access token is required")
	}
	if a.Weight <= 0 {
		a.Weight = 1
	}
	if a.Status == "" {
		a.Status = StatusActive
	}
	now := time.Now().Unix()
	tx, err := st.s.DB.BeginTx(ctx, nil)
	if err != nil {
		return 0, err
	}
	defer tx.Rollback()
	res, err := tx.ExecContext(ctx, `
		INSERT INTO accounts(label, email, org_uuid, plan, priority, weight, enabled,
			status, status_reason, proxy_url, source_path, sync_file, created_at, updated_at)
		VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)`,
		a.Label, a.Email, a.OrgUUID, a.Plan, a.Priority, a.Weight, boolInt(a.Enabled),
		a.Status, a.StatusReason, a.ProxyURL, a.SourcePath, boolInt(a.SyncFile), now, now)
	if err != nil {
		return 0, err
	}
	id, err := res.LastInsertId()
	if err != nil {
		return 0, err
	}
	if err := st.saveCredentials(ctx, tx, id, c); err != nil {
		return 0, err
	}
	return id, tx.Commit()
}

func (st *Store) Get(ctx context.Context, id int64) (Account, error) {
	a, err := scanAccount(st.s.DB.QueryRowContext(ctx, `SELECT `+accountCols+` FROM accounts WHERE id=?`, id))
	if errors.Is(err, sql.ErrNoRows) {
		return a, ErrNotFound
	}
	return a, err
}

// List returns all accounts in scheduling order: highest priority first,
// then oldest first.
func (st *Store) List(ctx context.Context) ([]Account, error) {
	rows, err := st.s.DB.QueryContext(ctx, `SELECT `+accountCols+` FROM accounts ORDER BY priority DESC, id ASC`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Account
	for rows.Next() {
		a, err := scanAccount(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, a)
	}
	return out, rows.Err()
}

func (st *Store) Count(ctx context.Context) (int, error) {
	var n int
	err := st.s.DB.QueryRowContext(ctx, `SELECT COUNT(*) FROM accounts`).Scan(&n)
	return n, err
}

func (st *Store) SetStatus(ctx context.Context, id int64, status, reason string) error {
	return st.exec(ctx, `UPDATE accounts SET status=?, status_reason=?, updated_at=? WHERE id=?`,
		status, reason, time.Now().Unix(), id)
}

func (st *Store) SetEnabled(ctx context.Context, id int64, enabled bool) error {
	return st.exec(ctx, `UPDATE accounts SET enabled=?, updated_at=? WHERE id=?`,
		boolInt(enabled), time.Now().Unix(), id)
}

func (st *Store) Delete(ctx context.Context, id int64) error {
	return st.exec(ctx, `DELETE FROM accounts WHERE id=?`, id)
}

func (st *Store) exec(ctx context.Context, q string, args ...any) error {
	res, err := st.s.DB.ExecContext(ctx, q, args...)
	if err != nil {
		return err
	}
	if n, _ := res.RowsAffected(); n == 0 {
		return ErrNotFound
	}
	return nil
}

// LoadCredentials decrypts the stored token set for an account.
func (st *Store) LoadCredentials(ctx context.Context, id int64) (Credentials, error) {
	var accessEnc, refreshEnc []byte
	var c Credentials
	var scopes string
	err := st.s.DB.QueryRowContext(ctx,
		`SELECT access_token_enc, refresh_token_enc, expires_at, scopes FROM account_secrets WHERE account_id=?`, id).
		Scan(&accessEnc, &refreshEnc, &c.ExpiresAt, &scopes)
	if errors.Is(err, sql.ErrNoRows) {
		return c, ErrNotFound
	}
	if err != nil {
		return c, err
	}
	access, err := st.box.Open(accessEnc, aad(id, "access"))
	if err != nil {
		return c, fmt.Errorf("account %d access token: %w", id, err)
	}
	c.AccessToken = string(access)
	if len(refreshEnc) > 0 {
		refresh, err := st.box.Open(refreshEnc, aad(id, "refresh"))
		if err != nil {
			return c, fmt.Errorf("account %d refresh token: %w", id, err)
		}
		c.RefreshToken = string(refresh)
	}
	if scopes != "" {
		c.Scopes = strings.Split(scopes, " ")
	}
	return c, nil
}

// SaveCredentials replaces the stored token set for an account.
func (st *Store) SaveCredentials(ctx context.Context, id int64, c Credentials) error {
	return st.saveCredentials(ctx, st.s.DB, id, c)
}

type execer interface {
	ExecContext(ctx context.Context, q string, args ...any) (sql.Result, error)
}

func (st *Store) saveCredentials(ctx context.Context, db execer, id int64, c Credentials) error {
	accessEnc, err := st.box.Seal([]byte(c.AccessToken), aad(id, "access"))
	if err != nil {
		return err
	}
	var refreshEnc []byte
	if c.RefreshToken != "" {
		if refreshEnc, err = st.box.Seal([]byte(c.RefreshToken), aad(id, "refresh")); err != nil {
			return err
		}
	}
	_, err = db.ExecContext(ctx, `
		INSERT INTO account_secrets(account_id, access_token_enc, refresh_token_enc, expires_at, scopes, updated_at)
		VALUES(?,?,?,?,?,?)
		ON CONFLICT(account_id) DO UPDATE SET
			access_token_enc=excluded.access_token_enc,
			refresh_token_enc=excluded.refresh_token_enc,
			expires_at=excluded.expires_at,
			scopes=excluded.scopes,
			updated_at=excluded.updated_at`,
		id, accessEnc, refreshEnc, c.ExpiresAt, strings.Join(c.Scopes, " "), time.Now().Unix())
	return err
}

func aad(id int64, field string) []byte {
	return []byte(fmt.Sprintf("account:%d:%s", id, field))
}

func boolInt(b bool) int {
	if b {
		return 1
	}
	return 0
}
