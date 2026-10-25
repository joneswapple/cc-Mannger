package server

import (
	"encoding/json"
	"strings"
	"testing"

	"github.com/hubosdev/cliproxy/internal/config"
)

func defaultEffortLevels() map[string]config.EffortLevel {
	return map[string]config.EffortLevel{
		"auto":   {MaxTokens: 8192},
		"low":    {MaxTokens: 4096},
		"medium": {MaxTokens: 16384, ThinkingBudget: 5000},
		"high":   {MaxTokens: 32768, ThinkingBudget: 15000},
		"xhigh":  {MaxTokens: 48000, ThinkingBudget: 30000},
		"max":    {MaxTokens: 64000, ThinkingBudget: 60000},
	}
}

func TestApplyEffortNoOpWhenAbsent(t *testing.T) {
	in := `{"model":"m","messages":[{"role":"user","content":"hi"}]}`
	out, applied, err := applyEffort([]byte(in), defaultEffortLevels())
	if err != nil {
		t.Fatalf("err: %v", err)
	}
	if applied {
		t.Fatalf("expected no rewrite, got changed body: %s", out)
	}
	if string(out) != in {
		t.Fatalf("body mutated: %s", out)
	}
}

func TestApplyEffortExpandsEachTier(t *testing.T) {
	levels := defaultEffortLevels()
	for _, tier := range []string{"auto", "low", "medium", "high", "xhigh", "max"} {
		t.Run(tier, func(t *testing.T) {
			in := []byte(`{"model":"m","effort":"` + tier + `","messages":[]}`)
			out, applied, err := applyEffort(in, levels)
			if err != nil {
				t.Fatalf("err: %v", err)
			}
			if !applied {
				t.Fatalf("expected applied=true for tier %s", tier)
			}
			var parsed map[string]any
			if err := json.Unmarshal(out, &parsed); err != nil {
				t.Fatalf("invalid json: %v\n%s", err, out)
			}
			if _, has := parsed["effort"]; has {
				t.Fatalf("effort field must be stripped: %s", out)
			}
			gotMax, _ := parsed["max_tokens"].(float64)
			wantMax := float64(levels[tier].MaxTokens)
			if gotMax != wantMax {
				t.Fatalf("max_tokens = %v, want %v", gotMax, wantMax)
			}
			thinking, hasThinking := parsed["thinking"]
			if levels[tier].ThinkingBudget > 0 {
				if !hasThinking {
					t.Fatalf("expected thinking block for %s; body: %s", tier, out)
				}
				m, _ := thinking.(map[string]any)
				if m["type"] != "enabled" {
					t.Fatalf("thinking.type = %v", m["type"])
				}
				if m["budget_tokens"].(float64) != float64(levels[tier].ThinkingBudget) {
					t.Fatalf("budget = %v, want %d", m["budget_tokens"], levels[tier].ThinkingBudget)
				}
			} else {
				if hasThinking {
					t.Fatalf("did not expect thinking for %s; body: %s", tier, out)
				}
			}
		})
	}
}

func TestApplyEffortIsCaseInsensitive(t *testing.T) {
	in := []byte(`{"model":"m","effort":"HIGH","messages":[]}`)
	out, applied, err := applyEffort(in, defaultEffortLevels())
	if err != nil || !applied {
		t.Fatalf("err=%v applied=%v out=%s", err, applied, out)
	}
}

func TestApplyEffortDoesNotOverrideExplicitMaxTokensOrThinking(t *testing.T) {
	in := []byte(`{"model":"m","effort":"high","max_tokens":1234,"thinking":{"type":"enabled","budget_tokens":42},"messages":[]}`)
	out, applied, err := applyEffort(in, defaultEffortLevels())
	if err != nil {
		t.Fatalf("err: %v", err)
	}
	if !applied {
		t.Fatalf("expected applied=true (effort was stripped)")
	}
	var parsed map[string]any
	if err := json.Unmarshal(out, &parsed); err != nil {
		t.Fatalf("invalid json: %v\n%s", err, out)
	}
	if _, has := parsed["effort"]; has {
		t.Fatalf("effort must be stripped")
	}
	if parsed["max_tokens"].(float64) != 1234 {
		t.Fatalf("max_tokens overwritten: got %v", parsed["max_tokens"])
	}
	thinking := parsed["thinking"].(map[string]any)
	if thinking["budget_tokens"].(float64) != 42 {
		t.Fatalf("thinking budget overwritten: got %v", thinking["budget_tokens"])
	}
}

func TestApplyEffortRejectsUnknownTier(t *testing.T) {
	in := []byte(`{"model":"m","effort":"ultra","messages":[]}`)
	_, _, err := applyEffort(in, defaultEffortLevels())
	if err == nil {
		t.Fatalf("expected error for unknown effort")
	}
	for _, tier := range []string{"auto", "low", "medium", "high", "xhigh", "max"} {
		if !strings.Contains(err.Error(), tier) {
			t.Fatalf("error %q should list known tier %q", err.Error(), tier)
		}
	}
}

func TestApplyEffortRejectsNonString(t *testing.T) {
	in := []byte(`{"model":"m","effort":7,"messages":[]}`)
	_, _, err := applyEffort(in, defaultEffortLevels())
	if err == nil {
		t.Fatalf("expected error for non-string effort")
	}
}

func TestApplyEffortIgnoresExplicitNull(t *testing.T) {
	in := []byte(`{"model":"m","effort":null,"messages":[]}`)
	out, applied, err := applyEffort(in, defaultEffortLevels())
	if err != nil {
		t.Fatalf("err: %v", err)
	}
	if applied {
		t.Fatalf("explicit null effort should be a no-op")
	}
	if string(out) != string(in) {
		t.Fatalf("body should be unchanged: %s", out)
	}
}
