package server

import (
	"context"
	"embed"
	"fmt"
	"html/template"
	"io/fs"
	"log"
	"net/http"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/go-chi/chi/v5/middleware"

	"github.com/hubosdev/cliproxy/internal/auth"
	"github.com/hubosdev/cliproxy/internal/config"
	"github.com/hubosdev/cliproxy/internal/policy"
	"github.com/hubosdev/cliproxy/internal/stats"
	"github.com/hubosdev/cliproxy/internal/store"
	"github.com/hubosdev/cliproxy/internal/upstream"
)

//go:embed templates/*.html static/*
var assetsFS embed.FS

type Server struct {
	cfg       config.Config
	store     *store.Store
	keys      *auth.Keys
	invites   *auth.Invites
	admin     *auth.Admin
	policy    *policy.Engine
	recorder  *stats.Recorder
	upstream  *upstream.Client
	catalog   *upstream.Catalog
	templates map[string]*template.Template
}

type Deps struct {
	Cfg      config.Config
	Store    *store.Store
	Keys     *auth.Keys
	Invites  *auth.Invites
	Admin    *auth.Admin
	Policy   *policy.Engine
	Recorder *stats.Recorder
	Upstream *upstream.Client
	Catalog  *upstream.Catalog
}

func New(d Deps) (*Server, error) {
	tmpls, err := loadTemplates()
	if err != nil {
		return nil, err
	}
	return &Server{
		cfg:       d.Cfg,
		store:     d.Store,
		keys:      d.Keys,
		invites:   d.Invites,
		admin:     d.Admin,
		policy:    d.Policy,
		recorder:  d.Recorder,
		upstream:  d.Upstream,
		catalog:   d.Catalog,
		templates: tmpls,
	}, nil
}

func loadTemplates() (map[string]*template.Template, error) {
	funcs := template.FuncMap{
		"fmtTime": func(t time.Time) string { return t.Format("2006-01-02 15:04:05") },
		"fmtTimePtr": func(t *time.Time) string {
			if t == nil {
				return "—"
			}
			return t.Format("2006-01-02 15:04:05")
		},
		"join": func(s []string) string {
			out := ""
			for i, v := range s {
				if i > 0 {
					out += ", "
				}
				out += v
			}
			return out
		},
		"list": func(values ...int) []int { return values },
	}
	sub, err := fs.Sub(assetsFS, "templates")
	if err != nil {
		return nil, err
	}
	// Each page lives in its own template tree alongside base.html so that
	// the page's `content` block doesn't collide with sibling pages — all
	// our pages declare a block named "content" and Go's html/template
	// treats names as a single global namespace per *Template.
	pages := []string{"login.html", "users.html", "invites.html", "stats.html", "settings.html"}
	out := make(map[string]*template.Template, len(pages))
	for _, p := range pages {
		t, err := template.New("").Funcs(funcs).ParseFS(sub, "base.html", p)
		if err != nil {
			return nil, fmt.Errorf("parse %s: %w", p, err)
		}
		out[p] = t
	}
	return out, nil
}

func (s *Server) Routes() http.Handler {
	r := chi.NewRouter()
	r.Use(middleware.Recoverer)
	r.Use(requestLogger)
	r.Use(middleware.RealIP)

	// Public proxy.
	r.Group(func(r chi.Router) {
		r.Use(s.requireAPIKey)
		r.Post("/v1/messages", s.handleMessages)
		r.Post("/v1/messages/count_tokens", s.handleCountTokens)
		r.Get("/v1/models", s.handleModelsList)
	})

	// Public registration via invite.
	r.Post("/register", s.handleRegister)

	// Health.
	r.Get("/healthz", func(w http.ResponseWriter, r *http.Request) {
		fmt.Fprint(w, "ok")
	})

	// Static assets.
	staticSub, _ := fs.Sub(assetsFS, "static")
	r.Handle("/static/*", http.StripPrefix("/static/", http.FileServer(http.FS(staticSub))))

	// Admin.
	r.Get("/admin/login", s.handleAdminLoginPage)
	r.Post("/admin/login", s.handleAdminLoginSubmit)
	r.Post("/admin/logout", s.handleAdminLogout)

	r.Group(func(r chi.Router) {
		r.Use(s.requireAdmin)
		r.Get("/admin", func(w http.ResponseWriter, r *http.Request) {
			http.Redirect(w, r, "/admin/users", http.StatusSeeOther)
		})
		r.Get("/admin/users", s.handleUsersPage)
		r.Get("/admin/invites", s.handleInvitesPage)
		r.Get("/admin/stats", s.handleStatsPage)
		r.Get("/admin/settings", s.handleSettingsPage)

		r.Post("/admin/api/invites", s.apiCreateInvite)
		r.Post("/admin/api/invites/{hash}/delete", s.apiDeleteInvite)
		r.Post("/admin/api/users/{id}/enable", s.apiEnableUser)
		r.Post("/admin/api/users/{id}/disable", s.apiDisableUser)
		r.Post("/admin/api/users/{id}/policy", s.apiSetPolicy)
		r.Post("/admin/api/keys/{id}/revoke", s.apiRevokeKey)
		r.Post("/admin/api/keys/issue", s.apiIssueKey)
		r.Post("/admin/api/settings/disabled-models", s.apiSetDisabledModels)
		r.Post("/admin/api/settings/refresh-models", s.apiRefreshModels)
		r.Get("/admin/api/stats", s.apiStats)
		r.Get("/admin/api/usage", s.apiUsageSearch)
	})

	return r
}

func requestLogger(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		start := time.Now()
		ww := middleware.NewWrapResponseWriter(w, r.ProtoMajor)
		next.ServeHTTP(ww, r)
		log.Printf("%s %s %d %dms", r.Method, r.URL.Path, ww.Status(), time.Since(start).Milliseconds())
	})
}

// Run starts the HTTP server until ctx is cancelled.
func (s *Server) Run(ctx context.Context) error {
	srv := &http.Server{
		Addr:              s.cfg.Listen,
		Handler:           s.Routes(),
		ReadHeaderTimeout: 30 * time.Second,
	}
	errCh := make(chan error, 1)
	go func() { errCh <- srv.ListenAndServe() }()
	log.Printf("cliproxy listening on %s", s.cfg.Listen)
	select {
	case <-ctx.Done():
		shutdownCtx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()
		return srv.Shutdown(shutdownCtx)
	case err := <-errCh:
		if err == http.ErrServerClosed {
			return nil
		}
		return err
	}
}
