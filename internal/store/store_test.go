package store_test

import (
	"context"
	"database/sql"
	"os"
	"path/filepath"
	"testing"

	"github.com/hubosdev/cliproxy/internal/store"
)

// A database created before schema_migrations existed must upgrade cleanly
// and keep its data; reopening must not re-run non-idempotent migrations.
func TestMigrateUpgradesUntrackedDatabase(t *testing.T) {
	ctx := context.Background()
	dir := t.TempDir()
	init, err := os.ReadFile("migrations/001_init.sql")
	if err != nil {
		t.Fatal(err)
	}
	db, err := sql.Open("sqlite", filepath.Join(dir, "cliproxy.db"))
	if err != nil {
		t.Fatal(err)
	}
	if _, err := db.Exec(string(init)); err != nil {
		t.Fatal(err)
	}
	if _, err := db.Exec(`INSERT INTO users(label, created_at) VALUES('old', 1)`); err != nil {
		t.Fatal(err)
	}
	db.Close()

	for i := 0; i < 2; i++ {
		st, err := store.Open(ctx, dir)
		if err != nil {
			t.Fatalf("open #%d: %v", i, err)
		}
		var n int
		if err := st.DB.QueryRow(`SELECT COUNT(*) FROM users`).Scan(&n); err != nil || n != 1 {
			t.Fatalf("users = %d, %v", n, err)
		}
		if _, err := st.DB.Exec(`SELECT account_id FROM usage_events LIMIT 1`); err != nil {
			t.Fatalf("account_id column missing: %v", err)
		}
		st.Close()
	}
}
