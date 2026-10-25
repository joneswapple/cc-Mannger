package upstream_test

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"sync/atomic"
	"testing"
	"time"

	"github.com/hubosdev/cliproxy/internal/upstream"
)

func writeCreds(t *testing.T, dir string, accessToken, refresh string, expiresAt int64) string {
	t.Helper()
	path := filepath.Join(dir, ".credentials.json")
	body := map[string]any{
		"claudeAiOauth": map[string]any{
			"accessToken":  accessToken,
			"refreshToken": refresh,
			"expiresAt":    expiresAt,
			"scopes":       []string{"user:inference"},
		},
	}
	b, _ := json.Marshal(body)
	if err := os.WriteFile(path, b, 0o600); err != nil {
		t.Fatalf("write creds: %v", err)
	}
	return path
}

func TestTokenReturnsExisting(t *testing.T) {
	dir := t.TempDir()
	path := writeCreds(t, dir, "tok-1", "ref-1", time.Now().Add(time.Hour).UnixMilli())
	p, err := upstream.NewClaudeCLIProvider(path, "http://unused", "client-id")
	if err != nil {
		t.Fatalf("new: %v", err)
	}
	tok, err := p.Token(context.Background())
	if err != nil {
		t.Fatalf("token: %v", err)
	}
	if tok != "tok-1" {
		t.Fatalf("expected tok-1, got %s", tok)
	}
}

func TestTokenRefreshesWhenExpired(t *testing.T) {
	dir := t.TempDir()
	path := writeCreds(t, dir, "old", "ref", time.Now().Add(-time.Minute).UnixMilli())

	var hits int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		atomic.AddInt32(&hits, 1)
		var req map[string]string
		_ = json.NewDecoder(r.Body).Decode(&req)
		if req["grant_type"] != "refresh_token" || req["refresh_token"] != "ref" || req["client_id"] != "cli" {
			http.Error(w, "bad request", http.StatusBadRequest)
			return
		}
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]any{
			"access_token":  "new",
			"refresh_token": "ref2",
			"expires_in":    3600,
		})
	}))
	defer srv.Close()

	p, err := upstream.NewClaudeCLIProvider(path, srv.URL, "cli")
	if err != nil {
		t.Fatalf("new: %v", err)
	}
	tok, err := p.Token(context.Background())
	if err != nil {
		t.Fatalf("token: %v", err)
	}
	if tok != "new" {
		t.Fatalf("expected new, got %s", tok)
	}
	if atomic.LoadInt32(&hits) != 1 {
		t.Fatalf("expected 1 refresh call, got %d", hits)
	}
	// File should be rewritten with the new token.
	b, _ := os.ReadFile(path)
	var f struct {
		ClaudeAiOauth struct {
			AccessToken  string `json:"accessToken"`
			RefreshToken string `json:"refreshToken"`
		} `json:"claudeAiOauth"`
	}
	if err := json.Unmarshal(b, &f); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if f.ClaudeAiOauth.AccessToken != "new" || f.ClaudeAiOauth.RefreshToken != "ref2" {
		t.Fatalf("creds not persisted: %+v", f)
	}
}

func TestTokenRefreshFailureSurfaces(t *testing.T) {
	dir := t.TempDir()
	path := writeCreds(t, dir, "old", "ref", time.Now().Add(-time.Minute).UnixMilli())
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		http.Error(w, "nope", http.StatusUnauthorized)
	}))
	defer srv.Close()
	p, err := upstream.NewClaudeCLIProvider(path, srv.URL, "cli")
	if err != nil {
		t.Fatalf("new: %v", err)
	}
	if _, err := p.Token(context.Background()); err == nil {
		t.Fatalf("expected refresh error")
	}
}

// TestTokenRefreshRereadsBeforeRefresh simulates the race where the host's
// `claude` CLI rotated the credentials file out from under us: the file we
// load on first read has stale tokens, but after the file is rewritten by
// the other process we should pick up the new ones rather than burning
// through the refresh token endpoint with the stale value.
func TestTokenRefreshRereadsBeforeRefresh(t *testing.T) {
	dir := t.TempDir()
	// Initial state: access expired, refresh = "stale".
	path := writeCreds(t, dir, "old", "stale", time.Now().Add(-time.Minute).UnixMilli())

	var receivedRefresh string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var req map[string]string
		_ = json.NewDecoder(r.Body).Decode(&req)
		receivedRefresh = req["refresh_token"]
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]any{
			"access_token":  "fresh-access",
			"refresh_token": "fresh-refresh",
			"expires_in":    3600,
		})
	}))
	defer srv.Close()

	p, err := upstream.NewClaudeCLIProvider(path, srv.URL, "cli")
	if err != nil {
		t.Fatalf("new: %v", err)
	}

	// Simulate `claude` rotating the file before our refresh fires: the
	// access token in the file is still expired but refresh is now
	// "rotated-by-claude".
	_ = writeCreds(t, dir, "old", "rotated-by-claude", time.Now().Add(-time.Minute).UnixMilli())

	if _, err := p.Token(context.Background()); err != nil {
		t.Fatalf("token: %v", err)
	}
	if receivedRefresh != "rotated-by-claude" {
		t.Fatalf("expected refresh to be sent with rotated token, got %q", receivedRefresh)
	}
}

// TestTokenRefreshRetriesOnInvalidGrant covers the case where Anthropic
// returns invalid_grant (because `claude` already used the refresh token
// to mint a new pair). Reloading the file should yield a fresh refresh
// token and the second attempt should succeed.
func TestTokenRefreshRetriesOnInvalidGrant(t *testing.T) {
	dir := t.TempDir()
	path := writeCreds(t, dir, "old", "stale", time.Now().Add(-time.Minute).UnixMilli())

	var calls int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var req map[string]string
		_ = json.NewDecoder(r.Body).Decode(&req)
		n := atomic.AddInt32(&calls, 1)
		if n == 1 {
			// First call: simulate `claude` having rotated the file
			// out from under us — we'll write the rotated file BEFORE
			// returning invalid_grant so the provider's reload+retry
			// path picks the new value up.
			_ = writeCreds(t, dir, "old", "rotated", time.Now().Add(-time.Minute).UnixMilli())
			w.Header().Set("Content-Type", "application/json")
			w.WriteHeader(http.StatusBadRequest)
			_, _ = w.Write([]byte(`{"error":"invalid_grant","error_description":"Refresh token not found or invalid"}`))
			return
		}
		// Second call should arrive with the freshly-loaded token.
		if req["refresh_token"] != "rotated" {
			t.Errorf("retry sent %q, expected rotated", req["refresh_token"])
		}
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]any{
			"access_token":  "after-retry",
			"refresh_token": "after-retry-ref",
			"expires_in":    3600,
		})
	}))
	defer srv.Close()

	p, err := upstream.NewClaudeCLIProvider(path, srv.URL, "cli")
	if err != nil {
		t.Fatalf("new: %v", err)
	}
	tok, err := p.Token(context.Background())
	if err != nil {
		t.Fatalf("token: %v", err)
	}
	if tok != "after-retry" {
		t.Fatalf("expected after-retry, got %s", tok)
	}
	if got := atomic.LoadInt32(&calls); got != 2 {
		t.Fatalf("expected exactly 2 token-endpoint calls, got %d", got)
	}
}

// TestTokenSucceedsWhenExpiredButNoRefreshToken covers the
// catalog-startup failure mode: a credentials file with `expiresAt` in
// the past and no `refreshToken` should NOT abort startup. The proxy
// hands the upstream the access token it already has and lets the real
// 401 (if any) surface as an upstream error, rather than blocking every
// downstream call with a "no refresh token available" before we even
// try.
func TestTokenSucceedsWhenExpiredButNoRefreshToken(t *testing.T) {
	dir := t.TempDir()
	// expiresAt 10 minutes in the past, refresh_token deliberately empty.
	path := writeCreds(t, dir, "still-usable-token", "", time.Now().Add(-10*time.Minute).UnixMilli())

	p, err := upstream.NewClaudeCLIProvider(path, "http://unused", "cli")
	if err != nil {
		t.Fatalf("new: %v", err)
	}
	tok, err := p.Token(context.Background())
	if err != nil {
		t.Fatalf("Token() returned an error when it should have warned and proceeded: %v", err)
	}
	if tok != "still-usable-token" {
		t.Fatalf("expected still-usable-token, got %q", tok)
	}
	// A second call must keep working (and only log the warning once,
	// but we don't assert on that — we just need it not to fail).
	if _, err := p.Token(context.Background()); err != nil {
		t.Fatalf("second Token(): %v", err)
	}
}

// TestParseCredsAcceptsAllShapes verifies the loader is lenient about
// historical / community variants of the credentials file.
func TestParseCredsAcceptsAllShapes(t *testing.T) {
	cases := []struct {
		name string
		body string
	}{
		{"claudeAiOauth", `{"claudeAiOauth":{"accessToken":"a","refreshToken":"r","expiresAt":99999999999999}}`},
		{"top-level camel", `{"accessToken":"a","refreshToken":"r","expiresAt":99999999999999}`},
		{"top-level snake (millis)", `{"access_token":"a","refresh_token":"r","expires_at":99999999999999}`},
		{"top-level snake (seconds)", `{"access_token":"a","refresh_token":"r","expires_at":99999999999}`},
		{"top-level snake (expires_in)", `{"access_token":"a","refresh_token":"r","expires_in":3600}`},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			dir := t.TempDir()
			path := filepath.Join(dir, ".credentials.json")
			if err := os.WriteFile(path, []byte(c.body), 0o600); err != nil {
				t.Fatalf("write: %v", err)
			}
			p, err := upstream.NewClaudeCLIProvider(path, "http://unused", "cli")
			if err != nil {
				t.Fatalf("new: %v", err)
			}
			tok, err := p.Token(context.Background())
			if err != nil {
				t.Fatalf("token: %v", err)
			}
			if tok != "a" {
				t.Fatalf("expected a, got %s", tok)
			}
		})
	}
}
