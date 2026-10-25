package auth_test

import (
	"context"
	"errors"
	"strings"
	"testing"
	"time"

	"github.com/hubosdev/cliproxy/internal/auth"
	"github.com/hubosdev/cliproxy/internal/store"
)

func newStore(t *testing.T) *store.Store {
	t.Helper()
	dir := t.TempDir()
	s, err := store.Open(context.Background(), dir)
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	t.Cleanup(func() { s.Close() })
	return s
}

func TestKeyIssueAndLookup(t *testing.T) {
	ctx := context.Background()
	s := newStore(t)
	keys := auth.NewKeys(s)

	uid, err := keys.CreateUser(ctx, "alice")
	if err != nil {
		t.Fatalf("create user: %v", err)
	}
	full, _, err := keys.Issue(ctx, uid)
	if err != nil {
		t.Fatalf("issue: %v", err)
	}
	if !strings.HasPrefix(full, "sk-cp-") {
		t.Fatalf("unexpected prefix: %s", full)
	}
	id, err := keys.Lookup(ctx, full)
	if err != nil {
		t.Fatalf("lookup: %v", err)
	}
	if id.UserID != uid || id.UserLabel != "alice" {
		t.Fatalf("identity mismatch: %+v", id)
	}
	if _, err := keys.Lookup(ctx, "sk-cp-bogus"); !errors.Is(err, auth.ErrKeyInvalid) {
		t.Fatalf("expected invalid for bogus key, got %v", err)
	}
}

func TestKeyRevoke(t *testing.T) {
	ctx := context.Background()
	s := newStore(t)
	keys := auth.NewKeys(s)
	uid, _ := keys.CreateUser(ctx, "alice")
	full, kid, _ := keys.Issue(ctx, uid)

	if _, err := keys.Lookup(ctx, full); err != nil {
		t.Fatalf("pre-revoke lookup: %v", err)
	}
	if err := keys.Revoke(ctx, kid); err != nil {
		t.Fatalf("revoke: %v", err)
	}
	if _, err := keys.Lookup(ctx, full); !errors.Is(err, auth.ErrKeyRevoked) {
		t.Fatalf("expected revoked, got %v", err)
	}
}

func TestUserDisabled(t *testing.T) {
	ctx := context.Background()
	s := newStore(t)
	keys := auth.NewKeys(s)
	uid, _ := keys.CreateUser(ctx, "alice")
	full, _, _ := keys.Issue(ctx, uid)
	if err := keys.SetUserEnabled(ctx, uid, false); err != nil {
		t.Fatalf("disable: %v", err)
	}
	if _, err := keys.Lookup(ctx, full); !errors.Is(err, auth.ErrUserDisabled) {
		t.Fatalf("expected disabled, got %v", err)
	}
}

func TestInviteRedeemSingleUse(t *testing.T) {
	ctx := context.Background()
	s := newStore(t)
	keys := auth.NewKeys(s)
	in := auth.NewInvites(s, keys)

	tok, err := in.Create(ctx, "alice", time.Hour)
	if err != nil {
		t.Fatalf("create: %v", err)
	}
	out, err := in.Redeem(ctx, tok, "alice-laptop")
	if err != nil {
		t.Fatalf("redeem: %v", err)
	}
	if !strings.HasPrefix(out.APIKey, "sk-cp-") {
		t.Fatalf("bad key: %s", out.APIKey)
	}
	id, err := keys.Lookup(ctx, out.APIKey)
	if err != nil {
		t.Fatalf("lookup issued: %v", err)
	}
	if id.UserID != out.UserID {
		t.Fatalf("user id mismatch")
	}
	if _, err := in.Redeem(ctx, tok, "again"); !errors.Is(err, auth.ErrInviteRedeemed) {
		t.Fatalf("expected already redeemed, got %v", err)
	}
}

func TestInviteExpired(t *testing.T) {
	ctx := context.Background()
	s := newStore(t)
	keys := auth.NewKeys(s)
	in := auth.NewInvites(s, keys)
	tok, err := in.Create(ctx, "alice", -time.Hour)
	if err != nil {
		t.Fatalf("create: %v", err)
	}
	if _, err := in.Redeem(ctx, tok, "x"); !errors.Is(err, auth.ErrInviteExpired) {
		t.Fatalf("expected expired, got %v", err)
	}
}

func TestInviteInvalid(t *testing.T) {
	ctx := context.Background()
	s := newStore(t)
	keys := auth.NewKeys(s)
	in := auth.NewInvites(s, keys)
	if _, err := in.Redeem(ctx, "deadbeef", "x"); !errors.Is(err, auth.ErrInviteInvalid) {
		t.Fatalf("expected invalid, got %v", err)
	}
}

func TestAdminBootstrap(t *testing.T) {
	ctx := context.Background()
	s := newStore(t)
	a := auth.NewAdmin(s)
	tok, err := a.Bootstrap(ctx, "")
	if err != nil {
		t.Fatalf("bootstrap: %v", err)
	}
	if tok == "" {
		t.Fatalf("expected generated token")
	}
	if err := a.Verify(ctx, tok); err != nil {
		t.Fatalf("verify: %v", err)
	}
	if err := a.Verify(ctx, "wrong"); err == nil {
		t.Fatalf("expected unauth for wrong token")
	}
	tok2, err := a.Bootstrap(ctx, "")
	if err != nil {
		t.Fatalf("second bootstrap: %v", err)
	}
	if tok2 != "" {
		t.Fatalf("expected empty on subsequent bootstrap, got %q", tok2)
	}
}
