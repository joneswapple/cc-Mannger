package stats_test

import (
	"context"
	"testing"
	"time"

	"github.com/hubosdev/cliproxy/internal/stats"
	"github.com/hubosdev/cliproxy/internal/store"
)

func newStore(t *testing.T) *store.Store {
	t.Helper()
	s, err := store.Open(context.Background(), t.TempDir())
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	t.Cleanup(func() { s.Close() })
	return s
}

func ensureUser(t *testing.T, s *store.Store, label string) int64 {
	t.Helper()
	res, err := s.DB.Exec(`INSERT INTO users(label, created_at, enabled) VALUES(?,?,1)`, label, time.Now().Unix())
	if err != nil {
		t.Fatalf("insert user: %v", err)
	}
	id, _ := res.LastInsertId()
	return id
}

// seedEvents writes a deterministic sample so each filter case can be
// asserted independently of the others.
func seedEvents(t *testing.T, rec *stats.Recorder, alice, bob int64) {
	t.Helper()
	now := time.Now()
	events := []stats.Event{
		{Timestamp: now.Add(-10 * time.Minute), UserID: alice, KeyID: 1, Model: "claude-opus-4-7", InputTokens: 100, OutputTokens: 200, Latency: 1500 * time.Millisecond, Status: 200},
		{Timestamp: now.Add(-9 * time.Minute), UserID: alice, KeyID: 1, Model: "claude-haiku-4-5", InputTokens: 10, OutputTokens: 20, Latency: 200 * time.Millisecond, Status: 200},
		{Timestamp: now.Add(-8 * time.Minute), UserID: bob, KeyID: 2, Model: "claude-opus-4-7", InputTokens: 50, OutputTokens: 75, Latency: 800 * time.Millisecond, Status: 429, Error: "rate_limit_error: Error (req_id=req_x)"},
		{Timestamp: now.Add(-7 * time.Minute), UserID: bob, KeyID: 2, Model: "claude-sonnet-4-6", InputTokens: 5_000, OutputTokens: 8_000, Latency: 30_000 * time.Millisecond, Status: 200},
		{Timestamp: now.Add(-6 * time.Minute), UserID: alice, KeyID: 1, Model: "claude-opus-4-7", InputTokens: 0, OutputTokens: 0, Latency: 50 * time.Millisecond, Status: 401, Error: "authentication_error"},
	}
	for _, e := range events {
		rec.Record(e)
	}
	// give the async writer a moment
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		rows, _, _ := rec.Search(context.Background(), stats.SearchFilter{})
		if len(rows) >= len(events) {
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatalf("seed events did not flush in time")
}

func TestSearchFilters(t *testing.T) {
	ctx := context.Background()
	s := newStore(t)
	rec := stats.NewRecorder(ctx, s)
	alice := ensureUser(t, s, "alice")
	bob := ensureUser(t, s, "bob")
	seedEvents(t, rec, alice, bob)

	cases := []struct {
		name      string
		filter    stats.SearchFilter
		wantTotal int
	}{
		{"no filter", stats.SearchFilter{}, 5},
		{"by user alice", stats.SearchFilter{UserID: alice}, 3},
		{"by user bob", stats.SearchFilter{UserID: bob}, 2},
		{"model substring opus", stats.SearchFilter{Model: "opus"}, 3},
		{"status ok bucket", stats.SearchFilter{Status: "ok"}, 3},
		{"status error bucket", stats.SearchFilter{Status: "error"}, 2},
		{"status 4xx bucket", stats.SearchFilter{Status: "4xx"}, 2},
		{"status 5xx bucket", stats.SearchFilter{Status: "5xx"}, 0},
		{"status literal 429", stats.SearchFilter{Status: "429"}, 1},
		{"only error", stats.SearchFilter{OnlyError: true}, 2},
		{"min tokens 1000", stats.SearchFilter{MinTokens: 1000}, 1},
		{"free-text alice (label)", stats.SearchFilter{Text: "alice"}, 3},
		{"free-text rate_limit (error msg)", stats.SearchFilter{Text: "rate_limit"}, 1},
		{"free-text haiku (model)", stats.SearchFilter{Text: "haiku"}, 1},
		{"since recent", stats.SearchFilter{Since: time.Now().Add(-7*time.Minute - 30*time.Second)}, 2},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			rows, total, err := rec.Search(ctx, c.filter)
			if err != nil {
				t.Fatalf("search: %v", err)
			}
			if total != c.wantTotal {
				t.Fatalf("total = %d, want %d (rows: %d)", total, c.wantTotal, len(rows))
			}
		})
	}
}

func TestSearchSorting(t *testing.T) {
	ctx := context.Background()
	s := newStore(t)
	rec := stats.NewRecorder(ctx, s)
	alice := ensureUser(t, s, "alice")
	bob := ensureUser(t, s, "bob")
	seedEvents(t, rec, alice, bob)

	rows, _, err := rec.Search(ctx, stats.SearchFilter{Sort: "tokens_desc"})
	if err != nil {
		t.Fatalf("search: %v", err)
	}
	for i := 1; i < len(rows); i++ {
		a := rows[i-1].InputTokens + rows[i-1].OutputTokens
		b := rows[i].InputTokens + rows[i].OutputTokens
		if a < b {
			t.Fatalf("not sorted by tokens desc at %d: %d then %d", i, a, b)
		}
	}

	rows, _, err = rec.Search(ctx, stats.SearchFilter{Sort: "latency_desc"})
	if err != nil {
		t.Fatalf("search: %v", err)
	}
	for i := 1; i < len(rows); i++ {
		if rows[i-1].Latency < rows[i].Latency {
			t.Fatalf("not sorted by latency desc at %d", i)
		}
	}
}

func TestSearchPagination(t *testing.T) {
	ctx := context.Background()
	s := newStore(t)
	rec := stats.NewRecorder(ctx, s)
	alice := ensureUser(t, s, "alice")
	bob := ensureUser(t, s, "bob")
	seedEvents(t, rec, alice, bob)

	page1, total, err := rec.Search(ctx, stats.SearchFilter{Limit: 2, Offset: 0, Sort: "time_asc"})
	if err != nil {
		t.Fatalf("search p1: %v", err)
	}
	if total != 5 {
		t.Fatalf("total = %d, want 5", total)
	}
	if len(page1) != 2 {
		t.Fatalf("page1 len = %d, want 2", len(page1))
	}

	page2, _, err := rec.Search(ctx, stats.SearchFilter{Limit: 2, Offset: 2, Sort: "time_asc"})
	if err != nil {
		t.Fatalf("search p2: %v", err)
	}
	if len(page2) != 2 {
		t.Fatalf("page2 len = %d, want 2", len(page2))
	}
	for _, a := range page1 {
		for _, b := range page2 {
			if a.ID == b.ID {
				t.Fatalf("page1 and page2 overlap on id %d", a.ID)
			}
		}
	}
}

func TestSearchUserLabelJoinHandlesDeletedUsers(t *testing.T) {
	ctx := context.Background()
	s := newStore(t)
	rec := stats.NewRecorder(ctx, s)

	// Insert a usage row with a user_id that doesn't exist in users.
	_, err := s.DB.Exec(`INSERT INTO usage_events(ts,user_id,key_id,model,input_tokens,output_tokens,cache_read_tokens,cache_creation_tokens,latency_ms,status,error) VALUES(?,?,?,?,?,?,0,0,?,?,?)`,
		time.Now().Unix(), 9999, 1, "x", 1, 1, 0, 200, "")
	if err != nil {
		t.Fatalf("insert: %v", err)
	}
	rows, total, err := rec.Search(ctx, stats.SearchFilter{})
	if err != nil {
		t.Fatalf("search: %v", err)
	}
	if total != 1 || len(rows) != 1 {
		t.Fatalf("expected single row, got total=%d rows=%d", total, len(rows))
	}
	if rows[0].UserLabel != "(deleted)" {
		t.Fatalf("expected '(deleted)' label, got %q", rows[0].UserLabel)
	}
}
