package upstream_test

import (
	"bytes"
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/hubosdev/cliproxy/internal/accounts"
	"github.com/hubosdev/cliproxy/internal/secret"
	"github.com/hubosdev/cliproxy/internal/store"
	"github.com/hubosdev/cliproxy/internal/upstream"
)

func newAccountStore(t *testing.T) (*accounts.Store, *store.Store) {
	t.Helper()
	st, err := store.Open(context.Background(), t.TempDir())
	if err != nil {
		t.Fatalf("store: %v", err)
	}
	t.Cleanup(func() { st.Close() })
	box, err := secret.New(bytes.Repeat([]byte{9}, 32))
	if err != nil {
		t.Fatal(err)
	}
	return accounts.New(st, box), st
}

func TestImportCLICredentialsSeedsOnce(t *testing.T) {
	ctx := context.Background()
	as, st := newAccountStore(t)
	path := writeCreds(t, t.TempDir(), "file-access", "file-refresh", time.Now().Add(time.Hour).UnixMilli())

	id, err := upstream.ImportCLICredentials(ctx, as, path, true)
	if err != nil || id == 0 {
		t.Fatalf("import = %d, %v", id, err)
	}
	if again, err := upstream.ImportCLICredentials(ctx, as, path, true); err != nil || again != 0 {
		t.Fatalf("second import = %d, %v; want 0, nil", again, err)
	}
	c, err := as.LoadCredentials(ctx, id)
	if err != nil || c.AccessToken != "file-access" || c.RefreshToken != "file-refresh" {
		t.Fatalf("stored creds = %+v, %v", c, err)
	}
	// Tokens must not be stored in plaintext.
	var raw []byte
	if err := st.DB.QueryRow(`SELECT access_token_enc FROM account_secrets WHERE account_id=?`, id).Scan(&raw); err != nil {
		t.Fatal(err)
	}
	if bytes.Contains(raw, []byte("file-access")) {
		t.Fatal("access token stored in plaintext")
	}
}

func TestPoolClientForwardsWithAccountToken(t *testing.T) {
	ctx := context.Background()
	as, _ := newAccountStore(t)
	low, _ := as.Create(ctx, accounts.Account{Label: "low", Enabled: true},
		accounts.Credentials{AccessToken: "low-tok", ExpiresAt: time.Now().Add(time.Hour).UnixMilli()})
	high, _ := as.Create(ctx, accounts.Account{Label: "high", Enabled: true, Priority: 10},
		accounts.Credentials{AccessToken: "high-tok", ExpiresAt: time.Now().Add(time.Hour).UnixMilli()})

	var seen string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		seen = r.Header.Get("Authorization")
	}))
	defer srv.Close()
	client := upstream.NewPoolClient(srv.URL, "ua", "beta", upstream.NewAccountPool(as, "http://unused", "cli"))

	resp, acct, err := client.Forward(ctx, "/v1/messages", strings.NewReader("{}"), upstream.ForwardExtra{})
	if err != nil {
		t.Fatal(err)
	}
	resp.Body.Close()
	if acct != high || seen != "Bearer high-tok" {
		t.Fatalf("account=%d auth=%q; want %d, Bearer high-tok", acct, seen, high)
	}

	// Disabled accounts are skipped.
	if err := as.SetEnabled(ctx, high, false); err != nil {
		t.Fatal(err)
	}
	resp, acct, err = client.Forward(ctx, "/v1/messages", strings.NewReader("{}"), upstream.ForwardExtra{})
	if err != nil {
		t.Fatal(err)
	}
	resp.Body.Close()
	if acct != low || seen != "Bearer low-tok" {
		t.Fatalf("account=%d auth=%q; want %d, Bearer low-tok", acct, seen, low)
	}

	if err := as.SetEnabled(ctx, low, false); err != nil {
		t.Fatal(err)
	}
	if _, _, err := client.Forward(ctx, "/v1/messages", strings.NewReader("{}"), upstream.ForwardExtra{}); err == nil {
		t.Fatal("forward with no enabled account succeeded")
	}
}

func TestAccountProviderRefreshesAndSyncsFile(t *testing.T) {
	ctx := context.Background()
	as, _ := newAccountStore(t)
	tokenSrv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var req map[string]string
		_ = json.NewDecoder(r.Body).Decode(&req)
		if req["refresh_token"] != "file-refresh" {
			http.Error(w, `{"error":"invalid_grant"}`, http.StatusBadRequest)
			return
		}
		_, _ = w.Write([]byte(`{"access_token":"new-access","refresh_token":"new-refresh","expires_in":3600}`))
	}))
	defer tokenSrv.Close()

	path := writeCreds(t, t.TempDir(), "old-access", "file-refresh", time.Now().Add(-time.Minute).UnixMilli())
	id, err := upstream.ImportCLICredentials(ctx, as, path, true)
	if err != nil {
		t.Fatal(err)
	}
	a, _ := as.Get(ctx, id)
	p := upstream.NewAccountProvider(as, a, tokenSrv.URL, "cli")
	tok, err := p.Token(ctx)
	if err != nil || tok != "new-access" {
		t.Fatalf("token = %q, %v", tok, err)
	}
	if c, _ := as.LoadCredentials(ctx, id); c.RefreshToken != "new-refresh" {
		t.Fatalf("db refresh = %q", c.RefreshToken)
	}
	b, _ := os.ReadFile(path)
	if !strings.Contains(string(b), "new-refresh") {
		t.Fatalf("synced file not rewritten: %s", b)
	}
}

func TestAccountProviderMarksNeedsReauth(t *testing.T) {
	ctx := context.Background()
	as, _ := newAccountStore(t)
	tokenSrv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		http.Error(w, `{"error":"invalid_grant"}`, http.StatusBadRequest)
	}))
	defer tokenSrv.Close()
	id, _ := as.Create(ctx, accounts.Account{Label: "x", Enabled: true},
		accounts.Credentials{AccessToken: "a", RefreshToken: "r", ExpiresAt: time.Now().Add(-time.Minute).UnixMilli()})
	a, _ := as.Get(ctx, id)
	if _, err := upstream.NewAccountProvider(as, a, tokenSrv.URL, "cli").Token(ctx); err == nil {
		t.Fatal("expected refresh failure")
	}
	if a, _ := as.Get(ctx, id); a.Status != accounts.StatusNeedsReauth {
		t.Fatalf("status = %q; want needs_reauth", a.Status)
	}
}
