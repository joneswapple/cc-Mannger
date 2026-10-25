package config

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"gopkg.in/yaml.v3"
)

type Config struct {
	Listen   string `yaml:"listen"`
	DataDir  string `yaml:"data_dir"`
	LogLevel string `yaml:"log_level"`
	// SecretKey is a base64-encoded 32-byte AES key sealing upstream
	// account tokens in the database. Empty means use (or create)
	// data_dir/secret.key.
	SecretKey string         `yaml:"secret_key"`
	Upstream  UpstreamConfig `yaml:"upstream"`
	Admin     AdminConfig    `yaml:"admin"`
}

type UpstreamConfig struct {
	BaseURL         string `yaml:"base_url"`
	CredentialsPath string `yaml:"credentials_path"`
	OAuthTokenURL   string `yaml:"oauth_token_url"`
	OAuthClientID   string `yaml:"oauth_client_id"`
	AnthropicBeta   string `yaml:"anthropic_beta"`
	UserAgent       string `yaml:"user_agent"`
	// InjectClaudeCodeSystem rewrites incoming /v1/messages requests so
	// the first system block is the literal Claude Code identifier — a
	// hard requirement for OAuth Pro/Max bearer auth. Default true.
	// Disable only if your client (e.g. `claude` itself) already prepends
	// the identifier and you want byte-for-byte passthrough.
	InjectClaudeCodeSystem bool `yaml:"inject_claude_code_system"`
	// SyncCredentialsFile applies when the accounts table is seeded from
	// the host's Claude Code credentials file on first start: the file
	// stays authoritative (re-read before refreshes, rewritten after) so
	// the host's own `claude` CLI and cliproxy don't invalidate each
	// other's refresh token. Default true.
	SyncCredentialsFile bool `yaml:"sync_credentials_file"`
	// EffortLevels maps OpenAI-style effort tiers (low/medium/high/…)
	// to Anthropic's `max_tokens` + extended-thinking `budget_tokens`.
	// Clients that send `"effort":"<tier>"` get expanded to the
	// corresponding fields here before the request is forwarded. The
	// effort field itself is stripped — Anthropic doesn't recognise it.
	EffortLevels map[string]EffortLevel `yaml:"effort_levels"`
}

// EffortLevel maps a single effort tier to upstream parameters.
// MaxTokens controls Anthropic's `max_tokens` (output cap).
// ThinkingBudget, if > 0, also enables extended thinking with that
// `budget_tokens`. Both fields are only applied when the request doesn't
// already specify them — explicit user values always win.
type EffortLevel struct {
	MaxTokens      int `yaml:"max_tokens"`
	ThinkingBudget int `yaml:"thinking_budget"`
}

type AdminConfig struct {
	InitialToken string `yaml:"initial_token"`
}

func Default() Config {
	home, _ := os.UserHomeDir()
	dataDir := filepath.Join(home, ".local", "share", "cliproxy")
	if x := os.Getenv("XDG_DATA_HOME"); x != "" {
		dataDir = filepath.Join(x, "cliproxy")
	}
	return Config{
		Listen:   "0.0.0.0:8787",
		DataDir:  dataDir,
		LogLevel: "info",
		Upstream: UpstreamConfig{
			BaseURL:       "https://api.anthropic.com",
			OAuthTokenURL: "https://console.anthropic.com/v1/oauth/token",
			OAuthClientID: "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
			// Both betas are required for OAuth-bearer requests against
			// /v1/messages — `claude-code-20250219` enables the Pro/Max
			// surface and `oauth-2025-04-20` is the OAuth-token grant
			// flag. Comma-separated; Anthropic accepts multiple values.
			AnthropicBeta:          "claude-code-20250219,oauth-2025-04-20",
			UserAgent:              "claude-cli/1.0 (cliproxy)",
			InjectClaudeCodeSystem: true,
			SyncCredentialsFile:    true,
			EffortLevels: map[string]EffortLevel{
				"auto":   {MaxTokens: 8192},
				"low":    {MaxTokens: 4096},
				"medium": {MaxTokens: 16384, ThinkingBudget: 5000},
				"high":   {MaxTokens: 32768, ThinkingBudget: 15000},
				"xhigh":  {MaxTokens: 48000, ThinkingBudget: 30000},
				"max":    {MaxTokens: 64000, ThinkingBudget: 60000},
			},
		},
	}
}

func Load(path string) (Config, error) {
	cfg := Default()
	if path != "" {
		b, err := os.ReadFile(path)
		if err != nil {
			if !os.IsNotExist(err) {
				return cfg, fmt.Errorf("read config: %w", err)
			}
		} else {
			if err := yaml.Unmarshal(b, &cfg); err != nil {
				return cfg, fmt.Errorf("parse config: %w", err)
			}
		}
	}
	applyEnv(&cfg)
	cfg.DataDir = expandHome(cfg.DataDir)
	cfg.Upstream.CredentialsPath = expandHome(cfg.Upstream.CredentialsPath)
	return cfg, nil
}

func applyEnv(c *Config) {
	if v := os.Getenv("CLIPROXY_LISTEN"); v != "" {
		c.Listen = v
	}
	if v := os.Getenv("CLIPROXY_DATA_DIR"); v != "" {
		c.DataDir = v
	}
	if v := os.Getenv("CLIPROXY_LOG_LEVEL"); v != "" {
		c.LogLevel = v
	}
	if v := os.Getenv("CLIPROXY_UPSTREAM_BASE_URL"); v != "" {
		c.Upstream.BaseURL = v
	}
	if v := os.Getenv("CLIPROXY_CREDENTIALS_PATH"); v != "" {
		c.Upstream.CredentialsPath = v
	}
	if v := os.Getenv("CLIPROXY_SECRET_KEY"); v != "" {
		c.SecretKey = v
	}
	if v := os.Getenv("CLIPROXY_SYNC_CREDENTIALS_FILE"); v != "" {
		c.Upstream.SyncCredentialsFile = v == "1" || strings.EqualFold(v, "true")
	}
	if v := os.Getenv("CLIPROXY_ADMIN_TOKEN"); v != "" {
		c.Admin.InitialToken = v
	}
}

func expandHome(p string) string {
	if p == "" {
		return p
	}
	if strings.HasPrefix(p, "~/") {
		home, err := os.UserHomeDir()
		if err == nil {
			return filepath.Join(home, p[2:])
		}
	}
	return p
}
