package upstream

import (
	"context"
	"errors"
	"fmt"
	"log"
	"net/http"
	"os"
	"sync"
	"time"

	"github.com/hubosdev/cliproxy/internal/accounts"
)

// ErrNoAccount is returned when no enabled upstream account can serve a
// request.
var ErrNoAccount = errors.New("no enabled upstream account — import one, or log in with `claude /login` on the host and restart")

// AccountProvider is a CredentialProvider backed by one row of the accounts
// table. Tokens are refreshed on demand and written back (encrypted) to the
// database. When the account has SyncFile set, its source credentials file
// stays authoritative exactly like ClaudeCLIProvider: it is re-read on first
// use and before every refresh, and rewritten after a refresh, so the host's
// `claude` CLI and cliproxy can keep rotating one refresh token between them.
type AccountProvider struct {
	store      *accounts.Store
	id         int64
	label      string
	syncPath   string
	tokenURL   string
	clientID   string
	httpClient *http.Client

	mu              sync.Mutex
	cache           claudeCreds
	loaded          bool
	warnedNoRefresh bool
}

func NewAccountProvider(st *accounts.Store, a accounts.Account, tokenURL, clientID string) *AccountProvider {
	p := &AccountProvider{
		store:      st,
		id:         a.ID,
		label:      a.Label,
		tokenURL:   tokenURL,
		clientID:   clientID,
		httpClient: &http.Client{Timeout: 30 * time.Second},
	}
	if a.SyncFile {
		p.syncPath = a.SourcePath
	}
	return p
}

func (p *AccountProvider) Source() string {
	if p.syncPath != "" {
		return fmt.Sprintf("account #%d %q (synced with %s)", p.id, p.label, p.syncPath)
	}
	return fmt.Sprintf("account #%d %q", p.id, p.label)
}

// load reads the token set from the database, then lets the synced file
// override it when the file holds a different token.
func (p *AccountProvider) load(ctx context.Context) error {
	c, err := p.store.LoadCredentials(ctx, p.id)
	if err != nil {
		return err
	}
	p.cache = c
	p.loaded = true
	return p.adoptFileLocked(ctx)
}

func (p *AccountProvider) adoptFileLocked(ctx context.Context) error {
	if p.syncPath == "" {
		return nil
	}
	b, err := os.ReadFile(p.syncPath)
	if err != nil {
		// The DB copy still works; the file may be mid-rewrite or gone.
		log.Printf("oauth: account #%d: cannot read synced credentials %s: %v", p.id, p.syncPath, err)
		return nil
	}
	fc, err := parseCreds(b)
	if err != nil {
		log.Printf("oauth: account #%d: synced credentials %s unreadable: %v", p.id, p.syncPath, err)
		return nil
	}
	if fc.AccessToken == p.cache.AccessToken && fc.RefreshToken == p.cache.RefreshToken {
		return nil
	}
	if err := p.store.SaveCredentials(ctx, p.id, fc); err != nil {
		return err
	}
	p.cache = fc
	return nil
}

func (p *AccountProvider) Token(ctx context.Context) (string, error) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if !p.loaded {
		if err := p.load(ctx); err != nil {
			return "", err
		}
	}
	if !p.nearExpiry() {
		return p.cache.AccessToken, nil
	}
	if err := p.adoptFileLocked(ctx); err != nil {
		return "", err
	}
	if !p.nearExpiry() {
		return p.cache.AccessToken, nil
	}
	if p.cache.RefreshToken == "" {
		// Same reasoning as ClaudeCLIProvider: let the upstream 401 speak.
		if !p.warnedNoRefresh {
			log.Printf("oauth: account #%d has no refresh_token and expiresAt is in the past — using the existing access token", p.id)
			p.warnedNoRefresh = true
		}
		return p.cache.AccessToken, nil
	}
	if err := p.refreshLocked(ctx); err != nil {
		return "", fmt.Errorf("oauth refresh (account #%d): %w", p.id, err)
	}
	return p.cache.AccessToken, nil
}

func (p *AccountProvider) nearExpiry() bool {
	return p.cache.ExpiresAt > 0 && time.Now().UnixMilli() > p.cache.ExpiresAt-60_000
}

func (p *AccountProvider) refreshLocked(ctx context.Context) error {
	err := p.attemptRefreshLocked(ctx)
	if err == nil || !isInvalidGrant(err) {
		return err
	}
	// Someone else (the host's `claude`, or another cliproxy) may have
	// rotated the refresh token. Pick up whatever is stored now and retry
	// once if it changed.
	before := p.cache.RefreshToken
	if reloadErr := p.load(ctx); reloadErr == nil && p.cache.RefreshToken != before {
		if err = p.attemptRefreshLocked(ctx); err == nil {
			return nil
		}
	}
	reason := "refresh token rejected: " + truncate(err.Error(), 200)
	if serr := p.store.SetStatus(ctx, p.id, accounts.StatusNeedsReauth, reason); serr != nil {
		log.Printf("oauth: account #%d: mark needs_reauth: %v", p.id, serr)
	}
	return fmt.Errorf("%w — the account's refresh token is no longer accepted by Anthropic; log in again for this account", err)
}

func (p *AccountProvider) attemptRefreshLocked(ctx context.Context) error {
	next, err := refreshOAuth(ctx, p.httpClient, p.tokenURL, p.clientID, p.cache)
	if err != nil {
		return err
	}
	if err := p.store.SaveCredentials(ctx, p.id, next); err != nil {
		return err
	}
	if p.syncPath != "" {
		if err := writeCredsFile(p.syncPath, next); err != nil {
			log.Printf("oauth: account #%d: write synced credentials %s: %v", p.id, p.syncPath, err)
		}
	}
	p.cache = next
	if err := p.store.SetStatus(ctx, p.id, accounts.StatusActive, ""); err != nil && !errors.Is(err, accounts.ErrNotFound) {
		log.Printf("oauth: account #%d: mark active: %v", p.id, err)
	}
	return nil
}

// Lease is the account picked for one upstream call. AccountID is 0 when
// the client wraps a bare CredentialProvider.
type Lease struct {
	AccountID int64
	Provider  CredentialProvider
}

// AccountPicker chooses the credentials for each upstream call.
type AccountPicker interface {
	Pick(ctx context.Context) (Lease, error)
	// Source describes the picker for logs and the settings page.
	Source() string
}

type staticPicker struct{ p CredentialProvider }

func (s staticPicker) Pick(context.Context) (Lease, error) { return Lease{Provider: s.p}, nil }
func (s staticPicker) Source() string                      { return s.p.Source() }

// AccountPool picks from the accounts table. Phase 0 strategy: the first
// enabled account in priority order whose status is active, falling back to
// the first enabled account at all so a needs_reauth account still surfaces
// its real upstream error instead of a generic "no account".
type AccountPool struct {
	store    *accounts.Store
	tokenURL string
	clientID string

	mu        sync.Mutex
	providers map[int64]*AccountProvider
}

func NewAccountPool(st *accounts.Store, tokenURL, clientID string) *AccountPool {
	return &AccountPool{store: st, tokenURL: tokenURL, clientID: clientID, providers: map[int64]*AccountProvider{}}
}

func (p *AccountPool) Pick(ctx context.Context) (Lease, error) {
	a, err := p.choose(ctx)
	if err != nil {
		return Lease{}, err
	}
	return Lease{AccountID: a.ID, Provider: p.provider(a)}, nil
}

func (p *AccountPool) choose(ctx context.Context) (accounts.Account, error) {
	list, err := p.store.List(ctx)
	if err != nil {
		return accounts.Account{}, err
	}
	var fallback *accounts.Account
	for i := range list {
		a := list[i]
		if !a.Enabled {
			continue
		}
		if a.Status == accounts.StatusActive {
			return a, nil
		}
		if fallback == nil {
			fallback = &list[i]
		}
	}
	if fallback != nil {
		return *fallback, nil
	}
	return accounts.Account{}, ErrNoAccount
}

// provider returns the cached provider for an account so its in-memory
// token survives across requests, rebuilding it if the sync settings moved.
func (p *AccountPool) provider(a accounts.Account) *AccountProvider {
	p.mu.Lock()
	defer p.mu.Unlock()
	want := ""
	if a.SyncFile {
		want = a.SourcePath
	}
	if pr, ok := p.providers[a.ID]; ok && pr.syncPath == want {
		return pr
	}
	pr := NewAccountProvider(p.store, a, p.tokenURL, p.clientID)
	p.providers[a.ID] = pr
	return pr
}

func (p *AccountPool) Source() string {
	a, err := p.choose(context.Background())
	if err != nil {
		return "account pool: " + err.Error()
	}
	return p.provider(a).Source()
}

// ImportCLICredentials seeds an empty accounts table from the host's Claude
// Code CLI credentials file so pre-accounts deployments keep working with no
// config change. It returns the new account ID, or 0 when accounts already
// exist. syncFile keeps the file authoritative (see AccountProvider).
func ImportCLICredentials(ctx context.Context, st *accounts.Store, path string, syncFile bool) (int64, error) {
	n, err := st.Count(ctx)
	if err != nil {
		return 0, err
	}
	if n > 0 {
		return 0, nil
	}
	resolved, err := ResolveCredentialsPath(path)
	if err != nil {
		return 0, err
	}
	b, err := os.ReadFile(resolved)
	if err != nil {
		return 0, fmt.Errorf("read credentials: %w", err)
	}
	c, err := parseCreds(b)
	if err != nil {
		return 0, err
	}
	return st.Create(ctx, accounts.Account{
		Label:      "claude-cli",
		Enabled:    true,
		SourcePath: resolved,
		SyncFile:   syncFile,
	}, c)
}
