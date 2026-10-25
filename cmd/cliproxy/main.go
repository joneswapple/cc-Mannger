package main

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/signal"
	"sort"
	"strings"
	"syscall"
	"text/tabwriter"
	"time"

	"github.com/spf13/cobra"

	"github.com/hubosdev/cliproxy/internal/auth"
	"github.com/hubosdev/cliproxy/internal/config"
	"github.com/hubosdev/cliproxy/internal/policy"
	"github.com/hubosdev/cliproxy/internal/server"
	"github.com/hubosdev/cliproxy/internal/stats"
	"github.com/hubosdev/cliproxy/internal/store"
	"github.com/hubosdev/cliproxy/internal/upstream"
)

var configPath string

func main() {
	root := &cobra.Command{
		Use:   "cliproxy",
		Short: "Proxy Claude Code traffic from LAN clients through one logged-in CLI session",
	}
	root.PersistentFlags().StringVar(&configPath, "config", defaultConfigPath(), "path to config.yaml")

	root.AddCommand(serveCmd())
	root.AddCommand(migrateCmd())
	root.AddCommand(adminCmd())
	root.AddCommand(effortCmd())

	if err := root.Execute(); err != nil {
		os.Exit(1)
	}
}

func defaultConfigPath() string {
	if v := os.Getenv("CLIPROXY_CONFIG"); v != "" {
		return v
	}
	if dir := os.Getenv("XDG_CONFIG_HOME"); dir != "" {
		return dir + "/cliproxy/config.yaml"
	}
	home, _ := os.UserHomeDir()
	return home + "/.config/cliproxy/config.yaml"
}

func loadCfg() (config.Config, error) {
	return config.Load(configPath)
}

func openStore(ctx context.Context, cfg config.Config) (*store.Store, error) {
	return store.Open(ctx, cfg.DataDir)
}

func serveCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "serve",
		Short: "Run the proxy HTTP server",
		RunE: func(cmd *cobra.Command, args []string) error {
			ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
			defer cancel()

			cfg, err := loadCfg()
			if err != nil {
				return err
			}
			st, err := openStore(ctx, cfg)
			if err != nil {
				return err
			}
			defer st.Close()

			adminAuth := auth.NewAdmin(st)
			tok, err := adminAuth.Bootstrap(ctx, cfg.Admin.InitialToken)
			if err != nil {
				return err
			}
			if tok != "" {
				fmt.Printf("\n=== ADMIN TOKEN (save now, only shown once) ===\n%s\n===============================================\n\n", tok)
			}

			provider, err := upstream.NewClaudeCLIProvider(cfg.Upstream.CredentialsPath, cfg.Upstream.OAuthTokenURL, cfg.Upstream.OAuthClientID)
			if err != nil {
				return fmt.Errorf("upstream credentials: %w", err)
			}
			fmt.Printf("upstream credentials: %s\n", provider.Source())

			client := upstream.NewClient(cfg.Upstream.BaseURL, cfg.Upstream.UserAgent, cfg.Upstream.AnthropicBeta, provider)
			catalog := upstream.NewCatalog(client, 30*time.Minute)
			catalog.Start(ctx)

			keys := auth.NewKeys(st)
			invites := auth.NewInvites(st, keys)
			pol := policy.NewEngine(st)
			rec := stats.NewRecorder(ctx, st)

			srv, err := server.New(server.Deps{
				Cfg: cfg, Store: st, Keys: keys, Invites: invites, Admin: adminAuth,
				Policy: pol, Recorder: rec, Upstream: client, Catalog: catalog,
			})
			if err != nil {
				return err
			}
			return srv.Run(ctx)
		},
	}
}

func migrateCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "migrate",
		Short: "Apply schema migrations",
		RunE: func(cmd *cobra.Command, args []string) error {
			cfg, err := loadCfg()
			if err != nil {
				return err
			}
			st, err := openStore(cmd.Context(), cfg)
			if err != nil {
				return err
			}
			defer st.Close()
			fmt.Println("schema up-to-date")
			return nil
		},
	}
}

// effortCmd exposes the OpenAI-style effort-tier expansion as a local
// dry-run, useful for poking at the YAML config without standing the
// server up. `effort list` prints the configured tier table; `effort
// expand` runs the same rewrite the proxy applies to /v1/messages and
// dumps the resulting body to stdout.
func effortCmd() *cobra.Command {
	c := &cobra.Command{
		Use:   "effort",
		Short: "Inspect / dry-run effort-tier expansion",
	}

	list := &cobra.Command{
		Use:   "list",
		Short: "Print the configured effort tiers",
		RunE: func(cmd *cobra.Command, args []string) error {
			cfg, err := loadCfg()
			if err != nil {
				return err
			}
			names := make([]string, 0, len(cfg.Upstream.EffortLevels))
			for k := range cfg.Upstream.EffortLevels {
				names = append(names, k)
			}
			sort.Strings(names)
			tw := tabwriter.NewWriter(os.Stdout, 0, 0, 2, ' ', 0)
			fmt.Fprintln(tw, "TIER\tMAX_TOKENS\tTHINKING_BUDGET")
			for _, name := range names {
				lvl := cfg.Upstream.EffortLevels[name]
				budget := "—"
				if lvl.ThinkingBudget > 0 {
					budget = fmt.Sprintf("%d", lvl.ThinkingBudget)
				}
				fmt.Fprintf(tw, "%s\t%d\t%s\n", name, lvl.MaxTokens, budget)
			}
			return tw.Flush()
		},
	}

	expand := &cobra.Command{
		Use:   "expand",
		Short: "Show the rewritten request body for a given effort tier or input JSON",
		Long: `Run the proxy's effort-rewrite locally and print the resulting body.

Examples:

  cliproxy effort expand --tier high
  cliproxy effort expand --body '{"model":"m","effort":"high","messages":[]}'
  echo '{"model":"m","effort":"max"}' | cliproxy effort expand`,
		RunE: func(cmd *cobra.Command, args []string) error {
			cfg, err := loadCfg()
			if err != nil {
				return err
			}
			tier, _ := cmd.Flags().GetString("tier")
			bodyArg, _ := cmd.Flags().GetString("body")
			pretty, _ := cmd.Flags().GetBool("pretty")

			var input []byte
			switch {
			case bodyArg != "":
				input = []byte(bodyArg)
			case tier != "":
				input = []byte(fmt.Sprintf(`{"effort":%q}`, tier))
			default:
				stdin, rerr := io.ReadAll(io.LimitReader(os.Stdin, 16<<20))
				if rerr != nil {
					return fmt.Errorf("read stdin: %w", rerr)
				}
				if len(bytes.TrimSpace(stdin)) == 0 {
					return fmt.Errorf("no input — pass --tier, --body, or pipe a JSON body on stdin")
				}
				input = stdin
			}

			out, applied, err := server.ApplyEffort(input, cfg.Upstream.EffortLevels)
			if err != nil {
				return err
			}
			if !applied {
				fmt.Fprintln(os.Stderr, "(no effort field — body forwarded unchanged)")
			}
			if pretty {
				var v any
				if jerr := json.Unmarshal(out, &v); jerr == nil {
					b, _ := json.MarshalIndent(v, "", "  ")
					out = b
				}
			}
			fmt.Println(string(out))
			return nil
		},
	}
	expand.Flags().String("tier", "", "effort tier name (auto/low/medium/high/xhigh/max)")
	expand.Flags().String("body", "", "raw JSON body to expand (overrides --tier)")
	expand.Flags().Bool("pretty", true, "pretty-print the resulting JSON")
	c.AddCommand(list, expand)
	return c
}

func adminCmd() *cobra.Command {
	c := &cobra.Command{
		Use:   "admin",
		Short: "Admin operations (calls the local server's admin API, except for `reset-token` which writes to the local store)",
	}
	c.AddCommand(adminInviteCmd(), adminUserCmd(), adminKeyCmd(), adminResetTokenCmd())
	return c
}

// adminResetTokenCmd rewrites the admin token in the local SQLite store
// without going through the HTTP server. Use it to recover access when
// you've lost the original token printed at first-run. The proxy must NOT
// be running while this is executed (the SQLite database is opened
// directly).
func adminResetTokenCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "reset-token",
		Short: "Reset the dashboard admin token (writes to the local DB; requires the server to be stopped)",
		RunE: func(cmd *cobra.Command, args []string) error {
			cfg, err := loadCfg()
			if err != nil {
				return err
			}
			st, err := openStore(cmd.Context(), cfg)
			if err != nil {
				return err
			}
			defer st.Close()
			preset, _ := cmd.Flags().GetString("token")
			a := auth.NewAdmin(st)
			tok, err := a.Reset(cmd.Context(), preset)
			if err != nil {
				return err
			}
			fmt.Printf("\n=== NEW ADMIN TOKEN ===\n%s\n=======================\n", tok)
			return nil
		},
	}
	cmd.Flags().String("token", "", "explicit token value to install (default: generate a random one)")
	return cmd
}

func adminBaseURL() string {
	if v := os.Getenv("CLIPROXY_ADMIN_URL"); v != "" {
		return v
	}
	return "http://127.0.0.1:8787"
}

func adminToken() (string, error) {
	if v := os.Getenv("CLIPROXY_ADMIN_TOKEN"); v != "" {
		return v, nil
	}
	return "", fmt.Errorf("set CLIPROXY_ADMIN_TOKEN to use admin subcommands")
}

func adminCall(method, path string, body any) ([]byte, error) {
	tok, err := adminToken()
	if err != nil {
		return nil, err
	}
	var rdr io.Reader
	if body != nil {
		b, err := json.Marshal(body)
		if err != nil {
			return nil, err
		}
		rdr = strings.NewReader(string(b))
	}
	req, err := http.NewRequest(method, adminBaseURL()+path, rdr)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Authorization", "Bearer "+tok)
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	c := &http.Client{Timeout: 10 * time.Second}
	resp, err := c.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	b, _ := io.ReadAll(resp.Body)
	if resp.StatusCode/100 != 2 {
		return nil, fmt.Errorf("%s %s -> %d: %s", method, path, resp.StatusCode, string(b))
	}
	return b, nil
}

func adminInviteCmd() *cobra.Command {
	c := &cobra.Command{Use: "invite", Short: "Manage invites"}
	create := &cobra.Command{
		Use:   "create",
		Short: "Create an invite token",
		RunE: func(cmd *cobra.Command, args []string) error {
			label, _ := cmd.Flags().GetString("label")
			ttl, _ := cmd.Flags().GetInt("ttl-hours")
			b, err := adminCall("POST", "/admin/api/invites", map[string]any{"label": label, "ttl_hours": ttl})
			if err != nil {
				return err
			}
			fmt.Println(string(b))
			return nil
		},
	}
	create.Flags().String("label", "", "label for the invite (required)")
	create.Flags().Int("ttl-hours", 24, "invite TTL in hours")
	_ = create.MarkFlagRequired("label")
	c.AddCommand(create)
	return c
}

func adminUserCmd() *cobra.Command {
	c := &cobra.Command{Use: "user", Short: "Manage users"}
	disable := &cobra.Command{
		Use:   "disable [id]",
		Short: "Disable a user",
		Args:  cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			_, err := adminCall("POST", "/admin/api/users/"+args[0]+"/disable", nil)
			return err
		},
	}
	enable := &cobra.Command{
		Use:   "enable [id]",
		Short: "Enable a user",
		Args:  cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			_, err := adminCall("POST", "/admin/api/users/"+args[0]+"/enable", nil)
			return err
		},
	}
	c.AddCommand(disable, enable)
	return c
}

func adminKeyCmd() *cobra.Command {
	c := &cobra.Command{Use: "key", Short: "Manage api keys"}
	revoke := &cobra.Command{
		Use:   "revoke [id]",
		Short: "Revoke an api key",
		Args:  cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			_, err := adminCall("POST", "/admin/api/keys/"+args[0]+"/revoke", nil)
			return err
		},
	}
	c.AddCommand(revoke)
	return c
}
