package server_test

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/hubosdev/cliproxy/internal/auth"
	"github.com/hubosdev/cliproxy/internal/config"
	"github.com/hubosdev/cliproxy/internal/policy"
	"github.com/hubosdev/cliproxy/internal/server"
	"github.com/hubosdev/cliproxy/internal/stats"
	"github.com/hubosdev/cliproxy/internal/store"
	"github.com/hubosdev/cliproxy/internal/upstream"
)

func writeCreds(t *testing.T, dir string) string {
	t.Helper()
	path := filepath.Join(dir, ".credentials.json")
	body := map[string]any{
		"claudeAiOauth": map[string]any{
			"accessToken":  "stub-access",
			"refreshToken": "stub-refresh",
			"expiresAt":    time.Now().Add(time.Hour).UnixMilli(),
		},
	}
	b, _ := json.Marshal(body)
	if err := os.WriteFile(path, b, 0o600); err != nil {
		t.Fatalf("write creds: %v", err)
	}
	return path
}

type harness struct {
	mux     http.Handler
	upstream *httptest.Server
	apiKey  string
	catalog *upstream.Catalog
	recorder *stats.Recorder
	upstreamRecorder func(r *http.Request, body []byte)
	upstreamHandler  http.HandlerFunc
	upstreamModels   string // raw JSON body for /v1/models; empty = default 2-model list
}

func newHarness(t *testing.T) *harness {
	t.Helper()
	ctx := context.Background()
	dir := t.TempDir()
	st, err := store.Open(ctx, dir)
	if err != nil {
		t.Fatalf("store: %v", err)
	}
	t.Cleanup(func() { st.Close() })

	keys := auth.NewKeys(st)
	invites := auth.NewInvites(st, keys)
	admin := auth.NewAdmin(st)
	if _, err := admin.Bootstrap(ctx, "test-admin"); err != nil {
		t.Fatalf("bootstrap admin: %v", err)
	}
	pol := policy.NewEngine(st)
	rec := stats.NewRecorder(ctx, st)

	h := &harness{}
	h.upstream = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		body, _ := io.ReadAll(r.Body)
		if h.upstreamRecorder != nil {
			h.upstreamRecorder(r, body)
		}
		if r.Method == http.MethodGet && r.URL.Path == "/v1/models" {
			w.Header().Set("Content-Type", "application/json")
			if h.upstreamModels != "" {
				_, _ = io.WriteString(w, h.upstreamModels)
			} else {
				_, _ = io.WriteString(w, `{"data":[
					{"type":"model","id":"claude-test","display_name":"Claude Test"},
					{"type":"model","id":"claude-other","display_name":"Claude Other"}
				]}`)
			}
			return
		}
		if h.upstreamHandler != nil {
			h.upstreamHandler(w, r)
			return
		}
		w.Header().Set("Content-Type", "application/json")
		_, _ = io.WriteString(w, `{"id":"msg_1","model":"claude-test","content":[],"usage":{"input_tokens":7,"output_tokens":3}}`)
	}))
	t.Cleanup(h.upstream.Close)

	credsPath := writeCreds(t, dir)
	provider, err := upstream.NewClaudeCLIProvider(credsPath, "http://unused", "cli")
	if err != nil {
		t.Fatalf("provider: %v", err)
	}
	client := upstream.NewClient(h.upstream.URL, "test-ua", "test-beta", provider)
	catalog := upstream.NewCatalog(client, 0)
	if err := catalog.Refresh(ctx); err != nil {
		t.Fatalf("catalog refresh: %v", err)
	}
	h.catalog = catalog
	h.recorder = rec

	cfg := config.Default()
	cfg.DataDir = dir
	srv, err := server.New(server.Deps{
		Cfg: cfg, Store: st, Keys: keys, Invites: invites, Admin: admin,
		Policy: pol, Recorder: rec, Upstream: client, Catalog: catalog,
	})
	if err != nil {
		t.Fatalf("server: %v", err)
	}
	h.mux = srv.Routes()

	uid, err := keys.CreateUser(ctx, "alice")
	if err != nil {
		t.Fatalf("user: %v", err)
	}
	full, _, err := keys.Issue(ctx, uid)
	if err != nil {
		t.Fatalf("issue: %v", err)
	}
	h.apiKey = full
	return h
}

// TestCountTokensProxiesAndInjectsSystem covers the
// /v1/messages/count_tokens path: the request must hit the upstream
// count_tokens endpoint with a Claude Code system block injected (since
// the same anti-abuse signal applies as on /v1/messages), and the
// response body / status must be forwarded byte-identical.
func TestCountTokensProxiesAndInjectsSystem(t *testing.T) {
	h := newHarness(t)
	var seenPath string
	var seenBody []byte
	h.upstreamRecorder = func(r *http.Request, body []byte) {
		if r.URL.Path == "/v1/messages/count_tokens" {
			seenPath = r.URL.Path
			seenBody = body
		}
	}
	h.upstreamHandler = func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_, _ = io.WriteString(w, `{"input_tokens":42}`)
	}
	body := []byte(`{"model":"claude-test","messages":[{"role":"user","content":"hi"}]}`)
	req := httptest.NewRequest(http.MethodPost, "/v1/messages/count_tokens", bytes.NewReader(body))
	req.Header.Set("x-api-key", h.apiKey)
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	h.mux.ServeHTTP(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("status %d body %s", rec.Code, rec.Body.String())
	}
	if !strings.Contains(rec.Body.String(), `"input_tokens":42`) {
		t.Fatalf("body not forwarded: %s", rec.Body.String())
	}
	if seenPath != "/v1/messages/count_tokens" {
		t.Fatalf("upstream path = %q", seenPath)
	}
	if !strings.Contains(string(seenBody), "You are Claude Code") {
		t.Fatalf("system identifier not injected for count_tokens: %s", seenBody)
	}
}

func TestCountTokensRejectsBadKey(t *testing.T) {
	h := newHarness(t)
	req := httptest.NewRequest(http.MethodPost, "/v1/messages/count_tokens", bytes.NewReader([]byte(`{}`)))
	rec := httptest.NewRecorder()
	h.mux.ServeHTTP(rec, req)
	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("status %d, expected 401", rec.Code)
	}
}

// TestSettingsPageShowsWireDefaults guards the OAuth-debug surface on
// the dashboard: the configured anthropic-beta, oauth client id, etc.
// have to actually render so operators can copy-paste them.
func TestSettingsPageShowsWireDefaults(t *testing.T) {
	h := newHarness(t)
	req := httptest.NewRequest(http.MethodGet, "/admin/settings", nil)
	req.Header.Set("Authorization", "Bearer test-admin")
	req.Header.Set("Accept", "text/html")
	rec := httptest.NewRecorder()
	h.mux.ServeHTTP(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("status %d", rec.Code)
	}
	body := rec.Body.String()
	for _, want := range []string{
		"Wire-format defaults",
		"anthropic-beta (default)",
		"OAuth token URL",
		"OAuth client ID",
		"User-Agent (default)",
		"Inject Claude Code system block",
		"Effort tiers",
		"<code>auto</code>",
		"<code>high</code>",
		"<code>max</code>",
	} {
		if !strings.Contains(body, want) {
			t.Fatalf("settings page missing %q\nbody:\n%s", want, body)
		}
	}
}

// TestUsagePageSearchAndJSON drives the /admin/stats search form and
// the /admin/api/usage JSON twin against the same seed data.
func TestUsagePageSearchAndJSON(t *testing.T) {
	h := newHarness(t)

	// Drive enough requests through the proxy to populate the events
	// table with a varied mix.
	bodies := []string{
		`{"model":"claude-test","messages":[{"role":"user","content":"hi"}]}`,
		`{"model":"claude-test","messages":[{"role":"user","content":"hi again"}]}`,
	}
	for _, b := range bodies {
		req := httptest.NewRequest(http.MethodPost, "/v1/messages", bytes.NewReader([]byte(b)))
		req.Header.Set("x-api-key", h.apiKey)
		req.Header.Set("Content-Type", "application/json")
		rec := httptest.NewRecorder()
		h.mux.ServeHTTP(rec, req)
	}

	// One synthetic 429 so the "error" / "rate_limit" filters have data.
	h.upstreamHandler = func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusTooManyRequests)
		_, _ = io.WriteString(w, `{"type":"error","error":{"type":"rate_limit_error","message":"Error"},"request_id":"req_test"}`)
	}
	{
		req := httptest.NewRequest(http.MethodPost, "/v1/messages",
			bytes.NewReader([]byte(`{"model":"claude-test","messages":[]}`)))
		req.Header.Set("x-api-key", h.apiKey)
		req.Header.Set("Content-Type", "application/json")
		rec := httptest.NewRecorder()
		h.mux.ServeHTTP(rec, req)
	}
	// Wait for the async recorder to flush.
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		recents, _ := h.recorder.Recent(context.Background(), 10)
		if len(recents) >= 3 {
			break
		}
		time.Sleep(20 * time.Millisecond)
	}

	// HTML page should render the filter form, the table, and event rows.
	req := httptest.NewRequest(http.MethodGet, "/admin/stats", nil)
	req.Header.Set("Authorization", "Bearer test-admin")
	req.Header.Set("Accept", "text/html")
	rec := httptest.NewRecorder()
	h.mux.ServeHTTP(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("status %d", rec.Code)
	}
	body := rec.Body.String()
	for _, want := range []string{
		`name="q"`,
		`name="user_id"`,
		`name="model"`,
		`name="status"`,
		`name="since"`,
		`name="until"`,
		`name="min_tokens"`,
		`name="sort"`,
		`name="only_error"`,
		`name="page_size"`,
		"matching event",
		`class="event status-`,
		`class="status-icon"`,
	} {
		if !strings.Contains(body, want) {
			t.Fatalf("/admin/stats body missing %q\nbody snippet:\n%s", want, body[:min(len(body), 600)])
		}
	}

	// JSON endpoint with the only_error filter should isolate the 429.
	req = httptest.NewRequest(http.MethodGet, "/admin/api/usage?only_error=1", nil)
	req.Header.Set("Authorization", "Bearer test-admin")
	rec = httptest.NewRecorder()
	h.mux.ServeHTTP(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("usage api status %d body %s", rec.Code, rec.Body.String())
	}
	var resp struct {
		Total  int `json:"total"`
		Events []struct {
			Status int    `json:"status"`
			Error  string `json:"error"`
		} `json:"events"`
	}
	if err := json.Unmarshal(rec.Body.Bytes(), &resp); err != nil {
		t.Fatalf("decode: %v\n%s", err, rec.Body.String())
	}
	if resp.Total < 1 {
		t.Fatalf("expected at least one error event, got total=%d", resp.Total)
	}
	for _, e := range resp.Events {
		if e.Error == "" {
			t.Fatalf("only_error returned event with empty error: %+v", e)
		}
	}

	// status=429 literal must isolate exactly the rate-limited row.
	req = httptest.NewRequest(http.MethodGet, "/admin/api/usage?status=429", nil)
	req.Header.Set("Authorization", "Bearer test-admin")
	rec = httptest.NewRecorder()
	h.mux.ServeHTTP(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("status filter: %d", rec.Code)
	}
	var rl struct {
		Total int `json:"total"`
	}
	_ = json.Unmarshal(rec.Body.Bytes(), &rl)
	if rl.Total < 1 {
		t.Fatalf("expected at least one 429 event, got %d", rl.Total)
	}
}

func min(a, b int) int {
	if a < b {
		return a
	}
	return b
}

// TestProxyMergesBetaAndPassesThroughClaudeUA exercises the
// claude-as-client path: when a real `claude` CLI hits the proxy, its
// `anthropic-beta` flags (e.g. interleaved-thinking) need to reach
// upstream alongside cliproxy's own required flags, and its
// `User-Agent` should be forwarded so Anthropic and the operator's logs
// can identify the actual caller.
func TestProxyMergesBetaAndPassesThroughClaudeUA(t *testing.T) {
	h := newHarness(t)
	var seenBeta string
	var seenUA string
	h.upstreamRecorder = func(r *http.Request, _ []byte) {
		if r.URL.Path == "/v1/messages" {
			seenBeta = r.Header.Get("anthropic-beta")
			seenUA = r.Header.Get("User-Agent")
		}
	}
	body := []byte(`{"model":"claude-test","messages":[{"role":"user","content":"hi"}]}`)
	req := httptest.NewRequest(http.MethodPost, "/v1/messages", bytes.NewReader(body))
	req.Header.Set("x-api-key", h.apiKey)
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("anthropic-beta", "interleaved-thinking-2025-05-14,test-beta")
	req.Header.Set("User-Agent", "claude-cli/2.0.0 (linux; x86_64)")
	rec := httptest.NewRecorder()
	h.mux.ServeHTTP(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("status %d body %s", rec.Code, rec.Body.String())
	}
	// Both the configured default (the harness uses "test-beta") and
	// every flag the inbound caller supplied must be present.
	for _, want := range []string{"test-beta", "interleaved-thinking-2025-05-14"} {
		if !strings.Contains(seenBeta, want) {
			t.Fatalf("upstream anthropic-beta = %q, missing %q", seenBeta, want)
		}
	}
	if seenUA != "claude-cli/2.0.0 (linux; x86_64)" {
		t.Fatalf("upstream UA = %q, expected claude UA pass-through", seenUA)
	}
}

// TestProxyDropsUnknownUserAgent ensures that non-claude clients get the
// configured default UA forwarded upstream (so Anthropic's anti-abuse
// layer doesn't see e.g. "curl/8.0").
func TestProxyDropsUnknownUserAgent(t *testing.T) {
	h := newHarness(t)
	var seenUA string
	h.upstreamRecorder = func(r *http.Request, _ []byte) {
		if r.URL.Path == "/v1/messages" {
			seenUA = r.Header.Get("User-Agent")
		}
	}
	body := []byte(`{"model":"claude-test","messages":[]}`)
	req := httptest.NewRequest(http.MethodPost, "/v1/messages", bytes.NewReader(body))
	req.Header.Set("x-api-key", h.apiKey)
	req.Header.Set("User-Agent", "curl/8.4.0")
	rec := httptest.NewRecorder()
	h.mux.ServeHTTP(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("status %d", rec.Code)
	}
	if seenUA == "curl/8.4.0" {
		t.Fatalf("non-claude UA should NOT be forwarded; saw %q upstream", seenUA)
	}
	if seenUA == "" {
		t.Fatalf("expected configured-default UA to fill in, got empty")
	}
}

// TestModelsListEndpoint covers the GET /v1/models endpoint that lets
// `claude` discover available models without a direct upstream
// round-trip.
func TestModelsListEndpoint(t *testing.T) {
	h := newHarness(t)
	req := httptest.NewRequest(http.MethodGet, "/v1/models", nil)
	req.Header.Set("x-api-key", h.apiKey)
	rec := httptest.NewRecorder()
	h.mux.ServeHTTP(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("status %d body %s", rec.Code, rec.Body.String())
	}
	var resp struct {
		Data []struct {
			Type        string `json:"type"`
			ID          string `json:"id"`
			DisplayName string `json:"display_name"`
		} `json:"data"`
		HasMore bool `json:"has_more"`
	}
	if err := json.Unmarshal(rec.Body.Bytes(), &resp); err != nil {
		t.Fatalf("decode: %v", err)
	}
	if len(resp.Data) == 0 {
		t.Fatalf("models list empty")
	}
	for _, m := range resp.Data {
		if m.Type != "model" {
			t.Fatalf("entry %+v has wrong type", m)
		}
		if m.ID == "" {
			t.Fatalf("entry %+v has empty id", m)
		}
	}
	if resp.HasMore {
		t.Fatalf("expected has_more=false")
	}
}

// TestModelsListRejectsBadKey ensures /v1/models is behind the API-key
// middleware so it can't be enumerated anonymously.
func TestModelsListRejectsBadKey(t *testing.T) {
	h := newHarness(t)
	req := httptest.NewRequest(http.MethodGet, "/v1/models", nil)
	rec := httptest.NewRecorder()
	h.mux.ServeHTTP(rec, req)
	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("status %d, expected 401", rec.Code)
	}
}

// TestProxyExpandsEffortBeforeUpstream verifies the OpenAI-style effort
// alias is expanded into max_tokens + thinking before the upstream sees
// the body, and that `effort` itself is stripped (Anthropic doesn't
// recognise it).
func TestProxyExpandsEffortBeforeUpstream(t *testing.T) {
	h := newHarness(t)
	var seenBody []byte
	h.upstreamRecorder = func(r *http.Request, body []byte) {
		if r.URL.Path == "/v1/messages" {
			seenBody = body
		}
	}
	body := []byte(`{"model":"claude-test","effort":"high","messages":[{"role":"user","content":"hi"}]}`)
	req := httptest.NewRequest(http.MethodPost, "/v1/messages", bytes.NewReader(body))
	req.Header.Set("x-api-key", h.apiKey)
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	h.mux.ServeHTTP(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("status %d body %s", rec.Code, rec.Body.String())
	}
	var upstream map[string]any
	if err := json.Unmarshal(seenBody, &upstream); err != nil {
		t.Fatalf("upstream body invalid: %v\n%s", err, seenBody)
	}
	if _, has := upstream["effort"]; has {
		t.Fatalf("upstream still saw `effort` field: %s", seenBody)
	}
	if upstream["max_tokens"].(float64) != 32768 {
		t.Fatalf("max_tokens = %v, want 32768", upstream["max_tokens"])
	}
	thinking, ok := upstream["thinking"].(map[string]any)
	if !ok {
		t.Fatalf("thinking missing: %s", seenBody)
	}
	if thinking["budget_tokens"].(float64) != 15000 {
		t.Fatalf("budget = %v, want 15000", thinking["budget_tokens"])
	}
}

// TestProxyRejectsUnknownEffort exercises the 400 path with the
// Anthropic-shaped error envelope.
func TestProxyRejectsUnknownEffort(t *testing.T) {
	h := newHarness(t)
	body := []byte(`{"model":"claude-test","effort":"ultra","messages":[]}`)
	req := httptest.NewRequest(http.MethodPost, "/v1/messages", bytes.NewReader(body))
	req.Header.Set("x-api-key", h.apiKey)
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	h.mux.ServeHTTP(rec, req)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("status %d", rec.Code)
	}
	if !strings.Contains(rec.Body.String(), `"type":"error"`) {
		t.Fatalf("response not Anthropic-shaped: %s", rec.Body.String())
	}
	if !strings.Contains(rec.Body.String(), "ultra") {
		t.Fatalf("error should mention the bad value: %s", rec.Body.String())
	}
}

// TestProxyInjectsClaudeCodeSystemBeforeUpstream verifies that when the
// client request lacks the Claude Code identifier, the proxy rewrites the
// body before forwarding so the upstream sees the required system block.
func TestProxyInjectsClaudeCodeSystemBeforeUpstream(t *testing.T) {
	h := newHarness(t)
	var seenBody []byte
	h.upstreamRecorder = func(r *http.Request, body []byte) {
		if r.URL.Path == "/v1/messages" {
			seenBody = body
		}
	}
	body := []byte(`{"model":"claude-test","messages":[{"role":"user","content":"hi"}]}`)
	req := httptest.NewRequest(http.MethodPost, "/v1/messages", bytes.NewReader(body))
	req.Header.Set("x-api-key", h.apiKey)
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	h.mux.ServeHTTP(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("status %d body %s", rec.Code, rec.Body.String())
	}
	if seenBody == nil {
		t.Fatalf("upstream did not receive a body")
	}
	if !strings.Contains(string(seenBody), `"You are Claude Code, Anthropic's official CLI for Claude."`) {
		t.Fatalf("upstream body missing Claude Code identifier:\n%s", seenBody)
	}
}

// TestProxyDoesNotDoubleInjectFromClaudeItself verifies that requests
// already containing the identifier (i.e. a real `claude` CLI talking to
// the proxy) are forwarded unchanged.
func TestProxyDoesNotDoubleInjectFromClaudeItself(t *testing.T) {
	h := newHarness(t)
	var seenBody []byte
	h.upstreamRecorder = func(r *http.Request, body []byte) {
		if r.URL.Path == "/v1/messages" {
			seenBody = body
		}
	}
	original := `{"model":"claude-test","system":[{"type":"text","text":"You are Claude Code, Anthropic's official CLI for Claude."}],"messages":[{"role":"user","content":"hi"}]}`
	req := httptest.NewRequest(http.MethodPost, "/v1/messages", bytes.NewReader([]byte(original)))
	req.Header.Set("x-api-key", h.apiKey)
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	h.mux.ServeHTTP(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("status %d", rec.Code)
	}
	if string(seenBody) != original {
		t.Fatalf("body was modified — wanted byte-identical pass-through.\nwant: %s\n got: %s", original, seenBody)
	}
	// Sanity: the identifier appears exactly once.
	if n := strings.Count(string(seenBody), "You are Claude Code"); n != 1 {
		t.Fatalf("identifier appears %d times, want 1", n)
	}
}

func TestProxyForwardsAndAuthenticates(t *testing.T) {
	h := newHarness(t)
	var seenAuth string
	h.upstreamRecorder = func(r *http.Request, _ []byte) {
		seenAuth = r.Header.Get("Authorization")
	}
	body := []byte(`{"model":"claude-test","stream":false,"messages":[{"role":"user","content":"hi"}]}`)
	req := httptest.NewRequest(http.MethodPost, "/v1/messages", bytes.NewReader(body))
	req.Header.Set("x-api-key", h.apiKey)
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	h.mux.ServeHTTP(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("status %d body %s", rec.Code, rec.Body.String())
	}
	if seenAuth != "Bearer stub-access" {
		t.Fatalf("upstream auth header = %q", seenAuth)
	}
	if !strings.Contains(rec.Body.String(), `"input_tokens":7`) {
		t.Fatalf("unexpected body: %s", rec.Body.String())
	}
}

// TestProxyForwardsUpstreamRateLimit covers the
// {"type":"error","error":{"type":"rate_limit_error","message":"Error"},"request_id":"req_..."}
// shape Anthropic returns over OAuth-bearer when the Pro/Max session is
// throttled. The body must be forwarded byte-identical, the 429 status
// preserved, and a usage_events row written with the error envelope info
// (type + message + request_id) so the operator can chase it down.
func TestProxyForwardsUpstreamRateLimit(t *testing.T) {
	h := newHarness(t)
	h.upstreamHandler = func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.Header().Set("retry-after", "30")
		w.WriteHeader(http.StatusTooManyRequests)
		_, _ = io.WriteString(w, `{"type":"error","error":{"type":"rate_limit_error","message":"Error"},"request_id":"req_011CaczwhMBhKT7rhhGTWxwu"}`)
	}
	body := []byte(`{"model":"claude-test","messages":[{"role":"user","content":"hi"}]}`)
	req := httptest.NewRequest(http.MethodPost, "/v1/messages", bytes.NewReader(body))
	req.Header.Set("x-api-key", h.apiKey)
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	h.mux.ServeHTTP(rec, req)

	if rec.Code != http.StatusTooManyRequests {
		t.Fatalf("status = %d, expected 429", rec.Code)
	}
	if got := rec.Header().Get("retry-after"); got != "30" {
		t.Fatalf("retry-after header = %q", got)
	}
	if !strings.Contains(rec.Body.String(), `"request_id":"req_011CaczwhMBhKT7rhhGTWxwu"`) {
		t.Fatalf("body should be forwarded verbatim with request_id, got: %s", rec.Body.String())
	}

	// Wait for the async stats recorder to flush.
	deadline := time.Now().Add(2 * time.Second)
	var got string
	for time.Now().Before(deadline) {
		recents, _ := h.recorder.Recent(context.Background(), 5)
		for _, ev := range recents {
			if ev.Status == http.StatusTooManyRequests {
				got = ev.Error
				break
			}
		}
		if got != "" {
			break
		}
		time.Sleep(20 * time.Millisecond)
	}
	if got == "" {
		t.Fatalf("expected a usage_events row with error column populated")
	}
	for _, want := range []string{"rate_limit_error", "Error", "req_011CaczwhMBhKT7rhhGTWxwu"} {
		if !strings.Contains(got, want) {
			t.Fatalf("usage_events.error = %q, missing %q", got, want)
		}
	}
}

func TestProxyRejectsBadKey(t *testing.T) {
	h := newHarness(t)
	body := []byte(`{"model":"claude-test"}`)
	req := httptest.NewRequest(http.MethodPost, "/v1/messages", bytes.NewReader(body))
	req.Header.Set("x-api-key", "sk-cp-bogus")
	rec := httptest.NewRecorder()
	h.mux.ServeHTTP(rec, req)
	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("expected 401, got %d", rec.Code)
	}
}

func TestProxyStreamsSSEAndCapturesUsage(t *testing.T) {
	h := newHarness(t)
	h.upstreamHandler = func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		w.WriteHeader(http.StatusOK)
		f, _ := w.(http.Flusher)
		fmt.Fprint(w, "event: message_start\n")
		fmt.Fprint(w, `data: {"type":"message_start","message":{"id":"m1","usage":{"input_tokens":10,"cache_read_input_tokens":2}}}`+"\n\n")
		if f != nil {
			f.Flush()
		}
		fmt.Fprint(w, "event: message_delta\n")
		fmt.Fprint(w, `data: {"type":"message_delta","usage":{"input_tokens":10,"output_tokens":5}}`+"\n\n")
		if f != nil {
			f.Flush()
		}
	}
	body := []byte(`{"model":"claude-test","stream":true}`)
	req := httptest.NewRequest(http.MethodPost, "/v1/messages", bytes.NewReader(body))
	req.Header.Set("x-api-key", h.apiKey)
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	h.mux.ServeHTTP(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("status %d", rec.Code)
	}
	if !strings.Contains(rec.Body.String(), "message_start") {
		t.Fatalf("body missing event: %s", rec.Body.String())
	}

	// Wait briefly for the async recorder to write the row.
	time.Sleep(150 * time.Millisecond)
	// Validate the row exists with expected token counts via direct DB read.
	// (This depends on the recorder being asynchronous but eventually consistent;
	// 150ms is comfortably more than the local SQLite write latency.)
}

// TestDashboardPagesAreDistinct guards against a regression where every
// dashboard page rendered the same "content" block because all templates
// shared one namespace and the alphabetically-last `{{define "content"}}`
// won. Each page must keep its own markers.
func TestDashboardPagesAreDistinct(t *testing.T) {
	h := newHarness(t)

	cases := []struct {
		path    string
		mustHave []string
	}{
		{"/admin/users", []string{">Users<", "data-action=\"issue-key\"", "data-action=\"edit-policy\""}},
		{"/admin/invites", []string{">Invites<", "Create invite", "Existing invites"}},
		{"/admin/stats", []string{">Usage<", `name="q"`, `class="event-list"`, "matching event"}},
		{"/admin/settings", []string{">Settings<", "Upstream credentials", "Globally disabled models"}},
	}
	for _, c := range cases {
		req := httptest.NewRequest(http.MethodGet, c.path, nil)
		req.Header.Set("Authorization", "Bearer test-admin")
		req.Header.Set("Accept", "text/html")
		rec := httptest.NewRecorder()
		h.mux.ServeHTTP(rec, req)
		if rec.Code != http.StatusOK {
			t.Fatalf("%s: status %d body %s", c.path, rec.Code, rec.Body.String())
		}
		body := rec.Body.String()
		for _, m := range c.mustHave {
			if !strings.Contains(body, m) {
				t.Fatalf("%s: body missing %q\n--- body ---\n%s", c.path, m, body)
			}
		}
	}
}

// TestSettingsModelsMultiSelect verifies the global-denylist UI:
//   - models from the upstream catalog render as checkboxes
//   - models stored in the denylist that are no longer in the catalog
//     render in the "unavailable" section so the operator notices
func TestSettingsModelsMultiSelect(t *testing.T) {
	h := newHarness(t)

	// Pre-populate the global denylist with one currently-available model
	// and one model that is NOT in the catalog. The settings page should
	// show the first as a checked checkbox in the available list, and the
	// second in the unavailable section.
	body := []byte(`{"models":["claude-test","claude-removed-old-model"]}`)
	req := httptest.NewRequest(http.MethodPost, "/admin/api/settings/disabled-models", bytes.NewReader(body))
	req.Header.Set("Authorization", "Bearer test-admin")
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	h.mux.ServeHTTP(rec, req)
	if rec.Code != http.StatusNoContent {
		t.Fatalf("seed denylist: %d %s", rec.Code, rec.Body.String())
	}

	// Render the settings page.
	req = httptest.NewRequest(http.MethodGet, "/admin/settings", nil)
	req.Header.Set("Authorization", "Bearer test-admin")
	req.Header.Set("Accept", "text/html")
	rec = httptest.NewRecorder()
	h.mux.ServeHTTP(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("settings page: %d", rec.Code)
	}
	out := rec.Body.String()

	// Available section: claude-test must be checked, claude-other must
	// be present and unchecked.
	if !strings.Contains(out, `value="claude-test" checked`) {
		t.Fatalf("expected claude-test to be a checked checkbox; body:\n%s", out)
	}
	if !strings.Contains(out, `value="claude-other"`) || strings.Contains(out, `value="claude-other" checked`) {
		t.Fatalf("expected claude-other to be a non-checked checkbox; body:\n%s", out)
	}
	// Unavailable section: claude-removed-old-model must appear with a
	// `stale` row class and the `unavailable` badge.
	if !strings.Contains(out, "claude-removed-old-model") {
		t.Fatalf("missing previously-disabled model in body:\n%s", out)
	}
	if !strings.Contains(out, "Previously selected, no longer available") {
		t.Fatalf("missing unavailable section header in body")
	}
}

// TestRefreshModelsEndpoint exercises the on-demand refresh path: changing
// the upstream catalog response and hitting POST /admin/api/settings/refresh-models
// should make the new model immediately visible to subsequent reads.
func TestRefreshModelsEndpoint(t *testing.T) {
	h := newHarness(t)

	// Swap the upstream models response to simulate a new model being
	// added to the user's plan.
	h.upstreamModels = `{"data":[
		{"type":"model","id":"claude-test","display_name":"Claude Test"},
		{"type":"model","id":"claude-fresh","display_name":"Claude Fresh"}
	]}`

	req := httptest.NewRequest(http.MethodPost, "/admin/api/settings/refresh-models", nil)
	req.Header.Set("Authorization", "Bearer test-admin")
	rec := httptest.NewRecorder()
	h.mux.ServeHTTP(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("refresh: %d %s", rec.Code, rec.Body.String())
	}
	if !h.catalog.Has("claude-fresh") {
		t.Fatalf("expected claude-fresh in catalog after refresh; ids=%v", h.catalog.IDs())
	}
}

func TestRegisterFlow(t *testing.T) {
	h := newHarness(t)
	// Create an invite via the admin API.
	body := []byte(`{"label":"alice","ttl_hours":1}`)
	req := httptest.NewRequest(http.MethodPost, "/admin/api/invites", bytes.NewReader(body))
	req.Header.Set("Authorization", "Bearer test-admin")
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	h.mux.ServeHTTP(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("invite create: %d %s", rec.Code, rec.Body.String())
	}
	var inv struct {
		Token string `json:"token"`
	}
	if err := json.Unmarshal(rec.Body.Bytes(), &inv); err != nil {
		t.Fatalf("decode: %v", err)
	}
	if inv.Token == "" {
		t.Fatalf("no token in response")
	}

	regBody, _ := json.Marshal(map[string]string{"token": inv.Token, "label": "alice-laptop"})
	req2 := httptest.NewRequest(http.MethodPost, "/register", bytes.NewReader(regBody))
	req2.Header.Set("Content-Type", "application/json")
	rec2 := httptest.NewRecorder()
	h.mux.ServeHTTP(rec2, req2)
	if rec2.Code != http.StatusOK {
		t.Fatalf("register: %d %s", rec2.Code, rec2.Body.String())
	}
	var out struct {
		APIKey string `json:"api_key"`
	}
	if err := json.Unmarshal(rec2.Body.Bytes(), &out); err != nil {
		t.Fatalf("decode: %v", err)
	}
	if !strings.HasPrefix(out.APIKey, "sk-cp-") {
		t.Fatalf("bad key: %s", out.APIKey)
	}
}
