package upstream

import "testing"

func TestMergeBeta(t *testing.T) {
	cases := []struct {
		primary, extra, want string
	}{
		{"", "", ""},
		{"a", "", "a"},
		{"", "b", "b"},
		{"a,b", "c", "a,b,c"},
		{"a,b", "b,c", "a,b,c"}, // dedupe
		{"a, b ,  c", "d", "a,b,c,d"},
		{"claude-code-20250219,oauth-2025-04-20", "interleaved-thinking-2025-05-14",
			"claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14"},
		{"claude-code-20250219", "claude-code-20250219", "claude-code-20250219"}, // identical
	}
	for _, c := range cases {
		got := mergeBeta(c.primary, c.extra)
		if got != c.want {
			t.Errorf("mergeBeta(%q,%q) = %q, want %q", c.primary, c.extra, got, c.want)
		}
	}
}
