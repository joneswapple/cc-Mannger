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

func setupClient(t *testing.T, handler http.Handler) (*upstream.Client, *httptest.Server) {
	t.Helper()
	srv := httptest.NewServer(handler)
	t.Cleanup(srv.Close)

	dir := t.TempDir()
	credsPath := filepath.Join(dir, ".credentials.json")
	body, _ := json.Marshal(map[string]any{
		"claudeAiOauth": map[string]any{
			"accessToken":  "tok",
			"refreshToken": "ref",
			"expiresAt":    time.Now().Add(time.Hour).UnixMilli(),
		},
	})
	if err := os.WriteFile(credsPath, body, 0o600); err != nil {
		t.Fatalf("write creds: %v", err)
	}
	provider, err := upstream.NewClaudeCLIProvider(credsPath, "http://unused", "cli")
	if err != nil {
		t.Fatalf("provider: %v", err)
	}
	return upstream.NewClient(srv.URL, "test-ua", "test-beta", provider), srv
}

func TestCatalogRefreshSuccess(t *testing.T) {
	var hits int32
	client, _ := setupClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		atomic.AddInt32(&hits, 1)
		if r.URL.Path != "/v1/models" {
			t.Errorf("unexpected path %s", r.URL.Path)
		}
		if got := r.Header.Get("Authorization"); got != "Bearer tok" {
			t.Errorf("auth header = %q", got)
		}
		if got := r.Header.Get("anthropic-beta"); got != "test-beta" {
			t.Errorf("beta header = %q", got)
		}
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{"data":[
			{"type":"model","id":"claude-opus-4-7","display_name":"Claude Opus 4.7"},
			{"type":"model","id":"claude-sonnet-4-6","display_name":"Claude Sonnet 4.6"}
		]}`))
	}))

	cat := upstream.NewCatalog(client, 0)
	if err := cat.Refresh(context.Background()); err != nil {
		t.Fatalf("refresh: %v", err)
	}
	ids := cat.IDs()
	if len(ids) != 2 || ids[0] != "claude-opus-4-7" || ids[1] != "claude-sonnet-4-6" {
		t.Fatalf("unexpected ids: %v", ids)
	}
	if !cat.Has("claude-opus-4-7") || cat.Has("claude-haiku-99") {
		t.Fatalf("Has() incorrect")
	}
	if st := cat.Status(); st.LastFetched.IsZero() || st.LastError != "" {
		t.Fatalf("status: %+v", st)
	}
	if atomic.LoadInt32(&hits) != 1 {
		t.Fatalf("expected 1 upstream hit, got %d", hits)
	}
}

func TestCatalogRefreshFailureKeepsCache(t *testing.T) {
	var calls int32
	client, _ := setupClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		n := atomic.AddInt32(&calls, 1)
		if n == 1 {
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`{"data":[{"type":"model","id":"m1","display_name":"M1"}]}`))
			return
		}
		http.Error(w, "boom", http.StatusInternalServerError)
	}))

	cat := upstream.NewCatalog(client, 0)
	if err := cat.Refresh(context.Background()); err != nil {
		t.Fatalf("first refresh: %v", err)
	}
	if got := cat.IDs(); len(got) != 1 {
		t.Fatalf("expected 1 model after first refresh, got %v", got)
	}
	if err := cat.Refresh(context.Background()); err == nil {
		t.Fatalf("expected error on second refresh")
	}
	if got := cat.IDs(); len(got) != 1 {
		t.Fatalf("cache mutated after failed refresh: %v", got)
	}
	st := cat.Status()
	if st.LastError == "" {
		t.Fatalf("expected last error to be recorded")
	}
}

func TestCatalogRefreshEmptyData(t *testing.T) {
	client, _ := setupClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{"data":[]}`))
	}))
	cat := upstream.NewCatalog(client, 0)
	if err := cat.Refresh(context.Background()); err == nil {
		t.Fatalf("expected error for empty data")
	}
}
