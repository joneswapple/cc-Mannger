package server

import (
	"encoding/json"
	"strings"
	"testing"
)

// claudeCodeSystemIdentifier and injectClaudeCodeSystem live in proxy.go;
// because they're package-private, this test file sits in package server
// (not server_test) so it can exercise them directly.
func TestInjectClaudeCodeSystem(t *testing.T) {
	cases := []struct {
		name        string
		in          string
		wantChanged bool
		// firstText is the .text of the first system block after rewrite
		// (or the original system value when changed=false).
		firstText string
		// extraTexts are additional text values that must appear in order
		// after the first block when present.
		extraTexts []string
	}{
		{
			name:        "no system field",
			in:          `{"model":"m","messages":[{"role":"user","content":"hi"}]}`,
			wantChanged: true,
			firstText:   claudeCodeSystemIdentifier,
		},
		{
			name:        "system as plain string",
			in:          `{"model":"m","system":"You are a helpful pirate.","messages":[]}`,
			wantChanged: true,
			firstText:   claudeCodeSystemIdentifier,
			extraTexts:  []string{"You are a helpful pirate."},
		},
		{
			name:        "system as array missing identifier",
			in:          `{"model":"m","system":[{"type":"text","text":"Be terse."}],"messages":[]}`,
			wantChanged: true,
			firstText:   claudeCodeSystemIdentifier,
			extraTexts:  []string{"Be terse."},
		},
		{
			name:        "system already starts with identifier (string)",
			in:          `{"model":"m","system":"You are Claude Code, Anthropic's official CLI for Claude.\n\nExtra.","messages":[]}`,
			wantChanged: false,
		},
		{
			name:        "system already starts with identifier (array)",
			in:          `{"model":"m","system":[{"type":"text","text":"You are Claude Code, Anthropic's official CLI for Claude."},{"type":"text","text":"Extra"}],"messages":[]}`,
			wantChanged: false,
		},
		{
			name:        "system explicitly null",
			in:          `{"model":"m","system":null,"messages":[]}`,
			wantChanged: true,
			firstText:   claudeCodeSystemIdentifier,
		},
	}

	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			out, changed, err := injectClaudeCodeSystem([]byte(c.in))
			if err != nil {
				t.Fatalf("err: %v", err)
			}
			if changed != c.wantChanged {
				t.Fatalf("changed = %v, want %v", changed, c.wantChanged)
			}
			if !changed {
				return
			}
			var parsed struct {
				System []struct {
					Type string `json:"type"`
					Text string `json:"text"`
				} `json:"system"`
			}
			if err := json.Unmarshal(out, &parsed); err != nil {
				t.Fatalf("rewritten body not valid json: %v\n%s", err, out)
			}
			if len(parsed.System) == 0 {
				t.Fatalf("rewritten system is empty: %s", out)
			}
			if parsed.System[0].Type != "text" || parsed.System[0].Text != c.firstText {
				t.Fatalf("first system block = %+v, want type=text text=%q", parsed.System[0], c.firstText)
			}
			for i, want := range c.extraTexts {
				if i+1 >= len(parsed.System) {
					t.Fatalf("missing extra block %d (%q): %s", i, want, out)
				}
				if parsed.System[i+1].Text != want {
					t.Fatalf("system[%d].text = %q, want %q", i+1, parsed.System[i+1].Text, want)
				}
			}
		})
	}
}

func TestInjectClaudeCodeSystemPreservesOtherFields(t *testing.T) {
	in := `{"model":"claude-x","stream":true,"max_tokens":256,"messages":[{"role":"user","content":"hi"}]}`
	out, changed, err := injectClaudeCodeSystem([]byte(in))
	if err != nil || !changed {
		t.Fatalf("expected change with no error; err=%v changed=%v", err, changed)
	}
	var parsed map[string]any
	if err := json.Unmarshal(out, &parsed); err != nil {
		t.Fatalf("invalid json: %v\n%s", err, out)
	}
	for _, k := range []string{"model", "stream", "max_tokens", "messages", "system"} {
		if _, ok := parsed[k]; !ok {
			t.Fatalf("rewritten body lost field %q: %s", k, out)
		}
	}
	if !strings.Contains(string(out), claudeCodeSystemIdentifier) {
		t.Fatalf("identifier not in body: %s", out)
	}
}
