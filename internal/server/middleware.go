package server

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/url"
	"strings"

	"github.com/hubosdev/cliproxy/internal/auth"
)

type ctxKey int

const ctxKeyIdentity ctxKey = 1

func identityFrom(ctx context.Context) (auth.Identity, bool) {
	v, ok := ctx.Value(ctxKeyIdentity).(auth.Identity)
	return v, ok
}

func (s *Server) requireAPIKey(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		key := r.Header.Get("x-api-key")
		if key == "" {
			if a := r.Header.Get("Authorization"); strings.HasPrefix(a, "Bearer ") {
				key = strings.TrimPrefix(a, "Bearer ")
			}
		}
		if key == "" {
			writeAnthropicError(w, http.StatusUnauthorized, "authentication_error",
				"missing api key — set the x-api-key header to an sk-cp-… key issued by this proxy")
			return
		}
		id, err := s.keys.Lookup(r.Context(), key)
		if err != nil {
			status := http.StatusUnauthorized
			msg := apiKeyHint(key)
			switch {
			case errors.Is(err, auth.ErrKeyRevoked):
				msg = "api key revoked"
			case errors.Is(err, auth.ErrUserDisabled):
				status = http.StatusForbidden
				msg = "user disabled"
			}
			writeAnthropicError(w, status, "authentication_error", msg)
			return
		}
		ctx := context.WithValue(r.Context(), ctxKeyIdentity, id)
		next.ServeHTTP(w, r.WithContext(ctx))
	})
}

// apiKeyHint produces a helpful message tailored to whatever the caller
// shoved into x-api-key: an invite token, a real Anthropic key, etc.
func apiKeyHint(key string) string {
	switch {
	case strings.HasPrefix(key, auth.KeyPrefix):
		return "invalid api key (no matching key found — was it revoked, or does it belong to a different cliproxy instance?)"
	case strings.HasPrefix(key, "sk-ant-"):
		return "this looks like a real Anthropic api key; cliproxy expects its own sk-cp-… keys (issue one via POST /register with an invite token)"
	case isHexLike(key):
		return "this looks like an invite token, not an api key — first redeem it: POST /register with body {\"token\":\"<this value>\",\"label\":\"...\"}, then use the returned api_key"
	default:
		return "invalid api key — cliproxy keys start with sk-cp-…; redeem an invite via POST /register to get one"
	}
}

func isHexLike(s string) bool {
	if len(s) < 16 {
		return false
	}
	for _, r := range s {
		if !(r >= '0' && r <= '9') && !(r >= 'a' && r <= 'f') && !(r >= 'A' && r <= 'F') {
			return false
		}
	}
	return true
}

func (s *Server) requireAdmin(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		token := ""
		if c, err := r.Cookie("cliproxy_admin"); err == nil {
			token = c.Value
		}
		if token == "" {
			if a := r.Header.Get("Authorization"); strings.HasPrefix(a, "Bearer ") {
				token = strings.TrimPrefix(a, "Bearer ")
			}
		}
		if token == "" {
			token = r.Header.Get("x-admin-token")
		}
		if err := s.admin.Verify(r.Context(), token); err != nil {
			if isHTML(r) {
				http.Redirect(w, r, "/admin/login?next="+url.QueryEscape(r.URL.Path), http.StatusSeeOther)
				return
			}
			writeJSONError(w, http.StatusUnauthorized, "unauthorized")
			return
		}
		next.ServeHTTP(w, r)
	})
}

func isHTML(r *http.Request) bool {
	return strings.Contains(r.Header.Get("Accept"), "text/html") ||
		!strings.HasPrefix(r.URL.Path, "/admin/api/")
}

func writeAnthropicError(w http.ResponseWriter, status int, errType, message string) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(map[string]any{
		"type":  "error",
		"error": map[string]string{"type": errType, "message": message},
	})
}

func writeJSONError(w http.ResponseWriter, status int, message string) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(map[string]string{"error": message})
}
