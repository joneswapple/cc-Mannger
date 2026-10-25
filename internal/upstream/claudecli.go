package upstream

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"time"

	"github.com/hubosdev/cliproxy/internal/accounts"
)

// ClaudeCLIProvider reads OAuth credentials from the user's logged-in Claude
// Code CLI session and refreshes them on demand.
//
// Credentials file shapes accepted:
//
//	// 1. The shape used by the current Claude Code CLI:
//	{
//	  "claudeAiOauth": {
//	    "accessToken":  "...",
//	    "refreshToken": "...",
//	    "expiresAt":    1730000000000,  // unix millis
//	    "scopes":       ["...", "..."]
//	  }
//	}
//
//	// 2. Top-level camelCase (some older builds):
//	{ "accessToken": "...", "refreshToken": "...", "expiresAt": <ms> }
//
//	// 3. Top-level snake_case (OAuth2 standard / community proxies):
//	{ "access_token": "...", "refresh_token": "...", "expires_at": <ms or s> }
//
// The host's `claude` CLI may rotate the file at any time. ClaudeCLIProvider
// re-reads from disk before every refresh attempt and, on `invalid_grant`,
// reloads once more before giving up — so a benign race with `claude`
// rotating the refresh token doesn't lock cliproxy out.
type ClaudeCLIProvider struct {
	path            string
	tokenURL        string
	clientID        string
	httpClient      *http.Client
	mu              sync.Mutex
	cache           claudeCreds
	loaded          bool
	warnedNoRefresh bool // log the "no refresh_token, using stale" message at most once per process
}

type claudeCredsFile struct {
	ClaudeAiOauth claudeCreds `json:"claudeAiOauth"`
}

type claudeCreds = accounts.Credentials

func NewClaudeCLIProvider(path, tokenURL, clientID string) (*ClaudeCLIProvider, error) {
	resolved, err := ResolveCredentialsPath(path)
	if err != nil {
		return nil, err
	}
	return &ClaudeCLIProvider{
		path:       resolved,
		tokenURL:   tokenURL,
		clientID:   clientID,
		httpClient: &http.Client{Timeout: 30 * time.Second},
	}, nil
}

// ResolveCredentialsPath finds the Claude Code CLI credentials file: the
// explicit path if given, else $CLAUDE_CREDENTIALS_PATH,
// $CLAUDE_CONFIG_DIR/.credentials.json, then ~/.claude/.credentials.json.
func ResolveCredentialsPath(p string) (string, error) {
	if p != "" {
		if _, err := os.Stat(p); err != nil {
			return "", fmt.Errorf("credentials file %q: %w", p, err)
		}
		return p, nil
	}
	if env := os.Getenv("CLAUDE_CREDENTIALS_PATH"); env != "" {
		if _, err := os.Stat(env); err == nil {
			return env, nil
		}
	}
	if dir := os.Getenv("CLAUDE_CONFIG_DIR"); dir != "" {
		candidate := filepath.Join(dir, ".credentials.json")
		if _, err := os.Stat(candidate); err == nil {
			return candidate, nil
		}
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}
	candidate := filepath.Join(home, ".claude", ".credentials.json")
	if _, err := os.Stat(candidate); err == nil {
		return candidate, nil
	}
	if runtime.GOOS == "darwin" {
		return "", fmt.Errorf("credentials not found at %s; on macOS Claude Code stores them in the keychain — export with: security find-generic-password -s 'Claude Code-credentials' -w > %s", candidate, candidate)
	}
	return "", fmt.Errorf("credentials not found at %s — log in with `claude /login` first", candidate)
}

func (p *ClaudeCLIProvider) Source() string { return p.path }

func (p *ClaudeCLIProvider) load() error {
	b, err := os.ReadFile(p.path)
	if err != nil {
		return fmt.Errorf("read credentials: %w", err)
	}
	creds, err := parseCreds(b)
	if err != nil {
		return err
	}
	p.cache = creds
	p.loaded = true
	return nil
}

// ParseCredentials accepts any of the credentials file shapes documented on
// ClaudeCLIProvider and returns the normalized token set.
func ParseCredentials(b []byte) (accounts.Credentials, error) { return parseCreds(b) }

// parseCreds tries each accepted credentials file shape until one yields a
// non-empty access token, returning a normalized claudeCreds value.
func parseCreds(b []byte) (claudeCreds, error) {
	// Shape 1: {"claudeAiOauth":{...}} — current Claude Code CLI.
	var f1 claudeCredsFile
	if err := json.Unmarshal(b, &f1); err == nil && f1.ClaudeAiOauth.AccessToken != "" {
		return f1.ClaudeAiOauth, nil
	}
	// Shape 2: top-level camelCase.
	var f2 claudeCreds
	if err := json.Unmarshal(b, &f2); err == nil && f2.AccessToken != "" {
		return f2, nil
	}
	// Shape 3: top-level snake_case (OAuth2 / community proxies).
	var f3 struct {
		AccessToken  string   `json:"access_token"`
		RefreshToken string   `json:"refresh_token"`
		ExpiresAt    int64    `json:"expires_at"`
		ExpiresIn    int64    `json:"expires_in"`
		Scopes       []string `json:"scopes"`
	}
	if err := json.Unmarshal(b, &f3); err == nil && f3.AccessToken != "" {
		creds := claudeCreds{
			AccessToken:  f3.AccessToken,
			RefreshToken: f3.RefreshToken,
			ExpiresAt:    f3.ExpiresAt,
			Scopes:       f3.Scopes,
		}
		// expires_at may be unix seconds in some snake_case variants.
		// Heuristic: any value < 10^12 is seconds, convert to millis.
		if creds.ExpiresAt > 0 && creds.ExpiresAt < 1e12 {
			creds.ExpiresAt = creds.ExpiresAt * 1000
		}
		// If only expires_in (relative seconds) is present, derive an
		// absolute timestamp.
		if creds.ExpiresAt == 0 && f3.ExpiresIn > 0 {
			creds.ExpiresAt = time.Now().Add(time.Duration(f3.ExpiresIn) * time.Second).UnixMilli()
		}
		return creds, nil
	}
	return claudeCreds{}, errors.New("credentials file did not contain a recognised access token (expected `claudeAiOauth.accessToken`, `accessToken`, or `access_token`)")
}

func (p *ClaudeCLIProvider) Token(ctx context.Context) (string, error) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if !p.loaded {
		if err := p.load(); err != nil {
			return "", err
		}
	}
	// Refresh if within 60s of expiry. expiresAt is unix millis.
	if p.cache.ExpiresAt > 0 && time.Now().UnixMilli() > p.cache.ExpiresAt-60_000 {
		// Re-read from disk first: the host's `claude` CLI may have
		// already rotated the tokens out from under us, in which case
		// our cached refresh_token is stale and would fail with
		// invalid_grant.
		if err := p.load(); err != nil {
			return "", err
		}
		if p.cache.ExpiresAt > 0 && time.Now().UnixMilli() > p.cache.ExpiresAt-60_000 {
			if p.cache.RefreshToken == "" {
				// No refresh token to swap with. Pro/Max access tokens
				// often outlive the `expiresAt` Anthropic stamps on
				// them, and even when they don't, surfacing the real
				// 401 from the upstream call is far more actionable
				// than failing every cliproxy request before we even
				// try. Warn once per refresh attempt and let the call
				// proceed with whatever access token we have.
				if !p.warnedNoRefresh {
					log.Printf("oauth: %s has no refresh_token and expiresAt is in the past — using the existing access token; if upstream returns 401, re-run `claude /login` on the host", p.path)
					p.warnedNoRefresh = true
				}
			} else if err := p.refreshLocked(ctx); err != nil {
				return "", fmt.Errorf("oauth refresh: %w", err)
			}
		}
	}
	return p.cache.AccessToken, nil
}

type refreshRequest struct {
	GrantType    string `json:"grant_type"`
	RefreshToken string `json:"refresh_token"`
	ClientID     string `json:"client_id"`
}

type refreshResponse struct {
	AccessToken  string `json:"access_token"`
	RefreshToken string `json:"refresh_token"`
	ExpiresIn    int64  `json:"expires_in"`
}

func (p *ClaudeCLIProvider) refreshLocked(ctx context.Context) error {
	err := p.attemptRefreshLocked(ctx)
	if err == nil {
		return nil
	}
	// If Anthropic says the refresh token is unknown, the host's `claude`
	// CLI most likely rotated it under us. Re-read the file and try once
	// more with whatever's there now.
	if isInvalidGrant(err) {
		if reloadErr := p.load(); reloadErr == nil {
			if retryErr := p.attemptRefreshLocked(ctx); retryErr == nil {
				return nil
			} else {
				err = retryErr
			}
		}
		return fmt.Errorf("%w — the refresh token in %s is no longer accepted by Anthropic. "+
			"This usually means another `claude` process refreshed it, or you logged out elsewhere. "+
			"Re-run `claude /login` on the host to get a fresh credential", err, p.path)
	}
	return err
}

func (p *ClaudeCLIProvider) attemptRefreshLocked(ctx context.Context) error {
	newCache, err := refreshOAuth(ctx, p.httpClient, p.tokenURL, p.clientID, p.cache)
	if err != nil {
		return err
	}
	if err := writeCredsFile(p.path, newCache); err != nil {
		return err
	}
	p.cache = newCache
	return nil
}

// refreshOAuth exchanges cur.RefreshToken at the OAuth token endpoint and
// returns the new token set. Scopes, and the refresh token when the
// endpoint doesn't rotate it, carry over from cur.
func refreshOAuth(ctx context.Context, hc *http.Client, tokenURL, clientID string, cur claudeCreds) (claudeCreds, error) {
	if cur.RefreshToken == "" {
		return claudeCreds{}, errors.New("no refresh token available")
	}
	body, _ := json.Marshal(refreshRequest{
		GrantType:    "refresh_token",
		RefreshToken: cur.RefreshToken,
		ClientID:     clientID,
	})
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, tokenURL, bytes.NewReader(body))
	if err != nil {
		return claudeCreds{}, err
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Accept", "application/json")
	resp, err := hc.Do(req)
	if err != nil {
		return claudeCreds{}, err
	}
	defer resp.Body.Close()
	respBody, _ := io.ReadAll(resp.Body)
	if resp.StatusCode/100 != 2 {
		return claudeCreds{}, fmt.Errorf("token endpoint returned %d: %s", resp.StatusCode, truncate(string(respBody), 200))
	}
	var r refreshResponse
	if err := json.Unmarshal(respBody, &r); err != nil {
		return claudeCreds{}, fmt.Errorf("decode refresh response: %w", err)
	}
	if r.AccessToken == "" {
		return claudeCreds{}, errors.New("refresh response missing access_token")
	}
	return claudeCreds{
		AccessToken:  r.AccessToken,
		RefreshToken: pickRefresh(r.RefreshToken, cur.RefreshToken),
		ExpiresAt:    time.Now().Add(time.Duration(r.ExpiresIn) * time.Second).UnixMilli(),
		Scopes:       cur.Scopes,
	}, nil
}

func isInvalidGrant(err error) bool {
	if err == nil {
		return false
	}
	s := err.Error()
	return strings.Contains(s, "invalid_grant") ||
		strings.Contains(s, "Refresh token not found") ||
		strings.Contains(s, "refresh_token_invalid")
}

func pickRefresh(new, old string) string {
	if new != "" {
		return new
	}
	return old
}

// writeCredsFile atomically rewrites a Claude Code CLI credentials file
// (0600) in the current `claudeAiOauth` shape.
func writeCredsFile(path string, c claudeCreds) error {
	envelope := claudeCredsFile{ClaudeAiOauth: c}
	b, err := json.MarshalIndent(envelope, "", "  ")
	if err != nil {
		return err
	}
	dir := filepath.Dir(path)
	tmp, err := os.CreateTemp(dir, ".credentials-*.tmp")
	if err != nil {
		return err
	}
	if _, err := tmp.Write(b); err != nil {
		tmp.Close()
		os.Remove(tmp.Name())
		return err
	}
	if err := tmp.Close(); err != nil {
		os.Remove(tmp.Name())
		return err
	}
	if err := os.Chmod(tmp.Name(), 0o600); err != nil {
		os.Remove(tmp.Name())
		return err
	}
	return os.Rename(tmp.Name(), path)
}

func truncate(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n] + "..."
}
