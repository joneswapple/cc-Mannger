package policy_test

import (
	"context"
	"errors"
	"testing"
	"time"

	"github.com/hubosdev/cliproxy/internal/policy"
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

func mustCreateUser(t *testing.T, s *store.Store, label string) int64 {
	t.Helper()
	res, err := s.DB.Exec(`INSERT INTO users(label, created_at, enabled) VALUES(?,?,1)`, label, time.Now().Unix())
	if err != nil {
		t.Fatalf("insert user: %v", err)
	}
	id, _ := res.LastInsertId()
	return id
}

func TestModelAllowlist(t *testing.T) {
	ctx := context.Background()
	s := newStore(t)
	e := policy.NewEngine(s)
	uid := mustCreateUser(t, s, "alice")
	if err := e.SetPolicy(ctx, uid, policy.Policy{AllowedModels: []string{"claude-sonnet-4-5"}}); err != nil {
		t.Fatalf("set: %v", err)
	}
	if err := e.Check(ctx, uid, "claude-sonnet-4-5"); err != nil {
		t.Fatalf("expected allowed: %v", err)
	}
	if err := e.Check(ctx, uid, "claude-opus-4-7"); !errors.Is(err, policy.ErrModelNotAllowed) {
		t.Fatalf("expected not allowed, got %v", err)
	}
}

func TestGloballyDisabledModel(t *testing.T) {
	ctx := context.Background()
	s := newStore(t)
	e := policy.NewEngine(s)
	uid := mustCreateUser(t, s, "alice")
	if err := e.SetDisabledModels(ctx, []string{"claude-opus-4-7"}); err != nil {
		t.Fatalf("set: %v", err)
	}
	if err := e.Check(ctx, uid, "claude-opus-4-7"); !errors.Is(err, policy.ErrModelDisabled) {
		t.Fatalf("expected disabled, got %v", err)
	}
	if err := e.Check(ctx, uid, "claude-sonnet-4-5"); err != nil {
		t.Fatalf("expected allowed: %v", err)
	}
}

func TestRPM(t *testing.T) {
	ctx := context.Background()
	s := newStore(t)
	e := policy.NewEngine(s)
	uid := mustCreateUser(t, s, "alice")
	limit := 2
	if err := e.SetPolicy(ctx, uid, policy.Policy{RPMLimit: &limit}); err != nil {
		t.Fatalf("set: %v", err)
	}
	for i := 0; i < limit; i++ {
		if err := e.Check(ctx, uid, "m"); err != nil {
			t.Fatalf("check %d: %v", i, err)
		}
	}
	if err := e.Check(ctx, uid, "m"); !errors.Is(err, policy.ErrRateLimited) {
		t.Fatalf("expected rate limited, got %v", err)
	}
}

func TestDailyQuota(t *testing.T) {
	ctx := context.Background()
	s := newStore(t)
	e := policy.NewEngine(s)
	uid := mustCreateUser(t, s, "alice")
	limit := int64(100)
	if err := e.SetPolicy(ctx, uid, policy.Policy{DailyTokenLimit: &limit}); err != nil {
		t.Fatalf("set: %v", err)
	}
	// No usage yet — should pass.
	if err := e.Check(ctx, uid, "m"); err != nil {
		t.Fatalf("expected allowed: %v", err)
	}
	// Insert usage exceeding limit, in the current day.
	_, err := s.DB.Exec(`INSERT INTO usage_events(ts,user_id,key_id,model,input_tokens,output_tokens,cache_read_tokens,cache_creation_tokens,latency_ms,status)
		VALUES(?,?,?,?,?,?,0,0,0,200)`, time.Now().Unix(), uid, 0, "m", 60, 60)
	if err != nil {
		t.Fatalf("insert usage: %v", err)
	}
	if err := e.Check(ctx, uid, "m"); !errors.Is(err, policy.ErrQuotaExceeded) {
		t.Fatalf("expected quota exceeded, got %v", err)
	}
}
