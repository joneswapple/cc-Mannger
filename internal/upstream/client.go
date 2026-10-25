package upstream

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"
)

type Client struct {
	BaseURL       string
	Picker        AccountPicker
	HTTP          *http.Client
	UserAgent     string
	AnthropicBeta string
}

// NewClient forwards every call with one fixed credential provider.
func NewClient(baseURL, userAgent, beta string, p CredentialProvider) *Client {
	return NewPoolClient(baseURL, userAgent, beta, staticPicker{p})
}

// NewPoolClient asks picker for the account to use on every call.
func NewPoolClient(baseURL, userAgent, beta string, picker AccountPicker) *Client {
	return &Client{
		BaseURL: strings.TrimRight(baseURL, "/"),
		Picker:  picker,
		HTTP: &http.Client{
			Timeout: 0, // streaming-friendly; rely on context for cancellation
			Transport: &http.Transport{
				MaxIdleConns:        50,
				MaxIdleConnsPerHost: 20,
				IdleConnTimeout:     90 * time.Second,
				DisableCompression:  false,
				ForceAttemptHTTP2:   true,
			},
		},
		UserAgent:     userAgent,
		AnthropicBeta: beta,
	}
}

// ForwardExtra carries per-request hints from the inbound caller so the
// proxy can layer them on top of the configured defaults. All fields are
// optional — leave them empty to keep the configured behaviour.
type ForwardExtra struct {
	// ExtraBeta is merged into anthropic-beta alongside Client.AnthropicBeta.
	// Comma-separated values are split, deduplicated and re-joined; the
	// configured defaults always win on order so the OAuth-required flags
	// stay first.
	ExtraBeta string
	// UserAgent overrides Client.UserAgent if non-empty. Callers should
	// gate this themselves (only forward UAs they trust).
	UserAgent string
	// ContentType overrides the application/json default for the request
	// body. Empty means "use json".
	ContentType string
}

// Source describes where upstream credentials come from.
func (c *Client) Source() string { return c.Picker.Source() }

// Forward sends body to the upstream Anthropic API and returns the response
// plus the ID of the account that served it (0 for a bare provider).
// Streaming is the caller's responsibility (just stream resp.Body).
func (c *Client) Forward(ctx context.Context, path string, body io.Reader, extra ForwardExtra) (*http.Response, int64, error) {
	return c.do(ctx, http.MethodPost, path, body, extra)
}

// Get issues a GET against the upstream API with the same auth/header
// injection used for /v1/messages. Useful for /v1/models and other
// metadata endpoints.
func (c *Client) Get(ctx context.Context, path string) (*http.Response, error) {
	resp, _, err := c.do(ctx, http.MethodGet, path, nil, ForwardExtra{})
	return resp, err
}

func (c *Client) do(ctx context.Context, method, path string, body io.Reader, extra ForwardExtra) (*http.Response, int64, error) {
	lease, err := c.Picker.Pick(ctx)
	if err != nil {
		return nil, 0, fmt.Errorf("pick upstream account: %w", err)
	}
	tok, err := lease.Provider.Token(ctx)
	if err != nil {
		return nil, lease.AccountID, fmt.Errorf("acquire upstream token: %w", err)
	}
	req, err := http.NewRequestWithContext(ctx, method, c.BaseURL+path, body)
	if err != nil {
		return nil, lease.AccountID, err
	}
	req.Header.Set("Authorization", "Bearer "+tok)
	req.Header.Set("anthropic-version", "2023-06-01")
	if beta := mergeBeta(c.AnthropicBeta, extra.ExtraBeta); beta != "" {
		req.Header.Set("anthropic-beta", beta)
	}
	if body != nil {
		ct := extra.ContentType
		if ct == "" {
			ct = "application/json"
		}
		req.Header.Set("Content-Type", ct)
	}
	req.Header.Set("Accept", "application/json")
	ua := extra.UserAgent
	if ua == "" {
		ua = c.UserAgent
	}
	if ua != "" {
		req.Header.Set("User-Agent", ua)
	}
	resp, err := c.HTTP.Do(req)
	return resp, lease.AccountID, err
}

// mergeBeta combines two comma-separated `anthropic-beta` values,
// preserving order from `primary` first and appending any new flags from
// `extra`. Whitespace is trimmed; duplicates are dropped.
func mergeBeta(primary, extra string) string {
	seen := map[string]struct{}{}
	var out []string
	for _, src := range []string{primary, extra} {
		for _, p := range strings.Split(src, ",") {
			p = strings.TrimSpace(p)
			if p == "" {
				continue
			}
			if _, ok := seen[p]; ok {
				continue
			}
			seen[p] = struct{}{}
			out = append(out, p)
		}
	}
	return strings.Join(out, ",")
}
