package secret_test

import (
	"bytes"
	"encoding/base64"
	"os"
	"path/filepath"
	"testing"

	"github.com/hubosdev/cliproxy/internal/secret"
)

func TestSealOpenRoundTrip(t *testing.T) {
	box, err := secret.New(bytes.Repeat([]byte{7}, 32))
	if err != nil {
		t.Fatal(err)
	}
	sealed, err := box.Seal([]byte("tok"), []byte("account:1:access"))
	if err != nil {
		t.Fatal(err)
	}
	if bytes.Contains(sealed, []byte("tok")) {
		t.Fatal("plaintext visible in ciphertext")
	}
	got, err := box.Open(sealed, []byte("account:1:access"))
	if err != nil || string(got) != "tok" {
		t.Fatalf("open = %q, %v", got, err)
	}
	if _, err := box.Open(sealed, []byte("account:2:access")); err == nil {
		t.Fatal("open with wrong aad succeeded")
	}
}

func TestLoadCreatesAndReusesKeyFile(t *testing.T) {
	dir := t.TempDir()
	b1, err := secret.Load("", dir)
	if err != nil {
		t.Fatal(err)
	}
	fi, err := os.Stat(filepath.Join(dir, secret.KeyFileName))
	if err != nil {
		t.Fatal(err)
	}
	if fi.Mode().Perm() != 0o600 {
		t.Fatalf("key file mode = %v", fi.Mode().Perm())
	}
	sealed, _ := b1.Seal([]byte("x"), nil)
	b2, err := secret.Load("", dir)
	if err != nil {
		t.Fatal(err)
	}
	if got, err := b2.Open(sealed, nil); err != nil || string(got) != "x" {
		t.Fatalf("reloaded key cannot open: %q %v", got, err)
	}
}

func TestLoadConfiguredKey(t *testing.T) {
	key := base64.StdEncoding.EncodeToString(bytes.Repeat([]byte{1}, 32))
	if _, err := secret.Load(key, t.TempDir()); err != nil {
		t.Fatal(err)
	}
	if _, err := secret.Load("c2hvcnQ=", t.TempDir()); err == nil {
		t.Fatal("short key accepted")
	}
}
