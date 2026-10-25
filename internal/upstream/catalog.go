package upstream

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"net/http"
	"sort"
	"sync"
	"time"
)

// Model is a single entry from the upstream Anthropic /v1/models response.
type Model struct {
	ID          string    `json:"id"`
	DisplayName string    `json:"display_name"`
	Type        string    `json:"type"`
	CreatedAt   time.Time `json:"created_at"`
}

// CatalogStatus describes the current state of the upstream model fetch.
type CatalogStatus struct {
	LastFetched time.Time
	LastError   string
}

// Catalog keeps an in-memory list of models the upstream credentials are
// allowed to use, refreshing periodically via /v1/models. The first refresh
// happens synchronously at startup so the dashboard has data on first paint;
// subsequent refreshes are best-effort and never block request handling.
type Catalog struct {
	client   *Client
	interval time.Duration

	mu          sync.RWMutex
	models      []Model
	idSet       map[string]struct{}
	lastFetched time.Time
	lastErr     error
}

// NewCatalog wires up the catalog. Call Start to kick off the background
// refresher. refreshEvery <= 0 disables periodic refresh (Refresh is still
// callable on demand).
func NewCatalog(client *Client, refreshEvery time.Duration) *Catalog {
	return &Catalog{
		client:   client,
		interval: refreshEvery,
		idSet:    map[string]struct{}{},
	}
}

// Start runs an initial Refresh synchronously, logs the outcome, and (if
// interval > 0) kicks off a goroutine that refreshes on a ticker until ctx
// is cancelled. A failed initial fetch is *not* fatal — the catalog stays
// empty and the dashboard surfaces the same error string under
// `Status().LastError`. Most issues at this point are upstream-auth
// related (the OAuth refresh token is gone, the access token is too
// stale, etc.) and the operator will see them again the moment they hit
// /v1/messages.
func (c *Catalog) Start(ctx context.Context) {
	if err := c.Refresh(ctx); err != nil {
		log.Printf("model catalog: initial fetch failed: %v — re-run `claude /login` on the host if every request also fails with an auth error; otherwise the catalog will retry every %s", err, c.interval)
	} else {
		log.Printf("model catalog: %d model(s) loaded from upstream", len(c.IDs()))
	}
	if c.interval <= 0 {
		return
	}
	go func() {
		t := time.NewTicker(c.interval)
		defer t.Stop()
		for {
			select {
			case <-ctx.Done():
				return
			case <-t.C:
				rctx, cancel := context.WithTimeout(ctx, 30*time.Second)
				if err := c.Refresh(rctx); err != nil {
					log.Printf("model catalog: refresh failed: %v", err)
				}
				cancel()
			}
		}
	}()
}

type modelsListResponse struct {
	Data []Model `json:"data"`
}

// Refresh fetches /v1/models and replaces the in-memory cache atomically.
// On error the previous cache is left untouched so the dashboard keeps
// showing the last good list.
func (c *Catalog) Refresh(ctx context.Context) error {
	resp, err := c.client.Get(ctx, "/v1/models?limit=1000")
	if err != nil {
		c.recordErr(err)
		return err
	}
	defer resp.Body.Close()
	body, _ := io.ReadAll(resp.Body)
	if resp.StatusCode/100 != 2 {
		err := fmt.Errorf("upstream /v1/models returned %d: %s", resp.StatusCode, truncate(string(body), 200))
		c.recordErr(err)
		return err
	}
	var parsed modelsListResponse
	if err := json.Unmarshal(body, &parsed); err != nil {
		err = fmt.Errorf("decode models response: %w", err)
		c.recordErr(err)
		return err
	}
	if len(parsed.Data) == 0 {
		err = errors.New("upstream /v1/models returned no models for this account/plan")
		c.recordErr(err)
		return err
	}
	sort.Slice(parsed.Data, func(i, j int) bool { return parsed.Data[i].ID < parsed.Data[j].ID })
	idSet := make(map[string]struct{}, len(parsed.Data))
	for _, m := range parsed.Data {
		idSet[m.ID] = struct{}{}
	}
	c.mu.Lock()
	c.models = parsed.Data
	c.idSet = idSet
	c.lastFetched = time.Now()
	c.lastErr = nil
	c.mu.Unlock()
	return nil
}

func (c *Catalog) recordErr(err error) {
	c.mu.Lock()
	c.lastErr = err
	c.mu.Unlock()
}

// Models returns a copy of the cached model list (sorted by ID).
func (c *Catalog) Models() []Model {
	c.mu.RLock()
	defer c.mu.RUnlock()
	out := make([]Model, len(c.models))
	copy(out, c.models)
	return out
}

// IDs returns just the model IDs.
func (c *Catalog) IDs() []string {
	c.mu.RLock()
	defer c.mu.RUnlock()
	out := make([]string, len(c.models))
	for i, m := range c.models {
		out[i] = m.ID
	}
	return out
}

// Has reports whether the named model id is currently in the catalog.
func (c *Catalog) Has(id string) bool {
	c.mu.RLock()
	defer c.mu.RUnlock()
	_, ok := c.idSet[id]
	return ok
}

// Status returns the last-fetched timestamp and last error for display.
func (c *Catalog) Status() CatalogStatus {
	c.mu.RLock()
	defer c.mu.RUnlock()
	st := CatalogStatus{LastFetched: c.lastFetched}
	if c.lastErr != nil {
		st.LastError = c.lastErr.Error()
	}
	return st
}

// Ensure we don't clash with any future *http.Client method named Get when
// the upstream client is consumed only via this package.
var _ = http.MethodGet