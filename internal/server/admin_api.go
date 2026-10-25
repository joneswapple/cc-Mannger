package server

import (
	"encoding/json"
	"net/http"
	"strconv"
	"strings"
	"time"

	"github.com/go-chi/chi/v5"

	"github.com/hubosdev/cliproxy/internal/policy"
	"github.com/hubosdev/cliproxy/internal/stats"
)

type registerRequest struct {
	Token string `json:"token"`
	Label string `json:"label"`
}

type registerResponse struct {
	APIKey string `json:"api_key"`
	UserID int64  `json:"user_id"`
}

func (s *Server) handleRegister(w http.ResponseWriter, r *http.Request) {
	var req registerRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSONError(w, http.StatusBadRequest, "invalid json: "+err.Error())
		return
	}
	out, err := s.invites.Redeem(r.Context(), strings.TrimSpace(req.Token), strings.TrimSpace(req.Label))
	if err != nil {
		writeJSONError(w, http.StatusBadRequest, err.Error())
		return
	}
	writeJSON(w, http.StatusOK, registerResponse{APIKey: out.APIKey, UserID: out.UserID})
}

type createInviteRequest struct {
	Label    string `json:"label"`
	TTLHours int    `json:"ttl_hours"`
}

type createInviteResponse struct {
	Token     string    `json:"token"`
	ExpiresAt time.Time `json:"expires_at"`
}

func (s *Server) apiCreateInvite(w http.ResponseWriter, r *http.Request) {
	var req createInviteRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSONError(w, http.StatusBadRequest, err.Error())
		return
	}
	ttl := time.Duration(req.TTLHours) * time.Hour
	if ttl <= 0 {
		ttl = 24 * time.Hour
	}
	tok, err := s.invites.Create(r.Context(), req.Label, ttl)
	if err != nil {
		writeJSONError(w, http.StatusInternalServerError, err.Error())
		return
	}
	writeJSON(w, http.StatusOK, createInviteResponse{Token: tok, ExpiresAt: time.Now().Add(ttl)})
}

func (s *Server) apiDeleteInvite(w http.ResponseWriter, r *http.Request) {
	hash := chi.URLParam(r, "hash")
	if err := s.invites.DeleteByHash(r.Context(), hash); err != nil {
		writeJSONError(w, http.StatusInternalServerError, err.Error())
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func (s *Server) apiEnableUser(w http.ResponseWriter, r *http.Request) {
	id, err := strconv.ParseInt(chi.URLParam(r, "id"), 10, 64)
	if err != nil {
		writeJSONError(w, http.StatusBadRequest, "bad id")
		return
	}
	if err := s.keys.SetUserEnabled(r.Context(), id, true); err != nil {
		writeJSONError(w, http.StatusInternalServerError, err.Error())
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func (s *Server) apiDisableUser(w http.ResponseWriter, r *http.Request) {
	id, err := strconv.ParseInt(chi.URLParam(r, "id"), 10, 64)
	if err != nil {
		writeJSONError(w, http.StatusBadRequest, "bad id")
		return
	}
	if err := s.keys.SetUserEnabled(r.Context(), id, false); err != nil {
		writeJSONError(w, http.StatusInternalServerError, err.Error())
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

type setPolicyRequest struct {
	AllowedModels   *[]string `json:"allowed_models"`
	DailyTokenLimit *int64    `json:"daily_token_limit"`
	RPMLimit        *int      `json:"rpm_limit"`
}

func (s *Server) apiSetPolicy(w http.ResponseWriter, r *http.Request) {
	id, err := strconv.ParseInt(chi.URLParam(r, "id"), 10, 64)
	if err != nil {
		writeJSONError(w, http.StatusBadRequest, "bad id")
		return
	}
	var req setPolicyRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSONError(w, http.StatusBadRequest, err.Error())
		return
	}
	p := policy.Policy{
		DailyTokenLimit: req.DailyTokenLimit,
		RPMLimit:        req.RPMLimit,
	}
	if req.AllowedModels != nil {
		p.AllowedModels = *req.AllowedModels
	}
	if err := s.policy.SetPolicy(r.Context(), id, p); err != nil {
		writeJSONError(w, http.StatusInternalServerError, err.Error())
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func (s *Server) apiRevokeKey(w http.ResponseWriter, r *http.Request) {
	id, err := strconv.ParseInt(chi.URLParam(r, "id"), 10, 64)
	if err != nil {
		writeJSONError(w, http.StatusBadRequest, "bad id")
		return
	}
	if err := s.keys.Revoke(r.Context(), id); err != nil {
		writeJSONError(w, http.StatusInternalServerError, err.Error())
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

type issueKeyRequest struct {
	UserID int64 `json:"user_id"`
}

type issueKeyResponse struct {
	APIKey string `json:"api_key"`
	KeyID  int64  `json:"key_id"`
}

func (s *Server) apiIssueKey(w http.ResponseWriter, r *http.Request) {
	var req issueKeyRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSONError(w, http.StatusBadRequest, err.Error())
		return
	}
	if req.UserID <= 0 {
		writeJSONError(w, http.StatusBadRequest, "user_id required")
		return
	}
	full, id, err := s.keys.Issue(r.Context(), req.UserID)
	if err != nil {
		writeJSONError(w, http.StatusInternalServerError, err.Error())
		return
	}
	writeJSON(w, http.StatusOK, issueKeyResponse{APIKey: full, KeyID: id})
}

type setDisabledModelsRequest struct {
	Models []string `json:"models"`
}

func (s *Server) apiSetDisabledModels(w http.ResponseWriter, r *http.Request) {
	var req setDisabledModelsRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSONError(w, http.StatusBadRequest, err.Error())
		return
	}
	if err := s.policy.SetDisabledModels(r.Context(), req.Models); err != nil {
		writeJSONError(w, http.StatusInternalServerError, err.Error())
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func (s *Server) apiRefreshModels(w http.ResponseWriter, r *http.Request) {
	if s.catalog == nil {
		writeJSONError(w, http.StatusServiceUnavailable, "model catalog disabled")
		return
	}
	if err := s.catalog.Refresh(r.Context()); err != nil {
		writeJSONError(w, http.StatusBadGateway, err.Error())
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"models": s.catalog.IDs(),
	})
}

// apiUsageSearch is the JSON twin of the dashboard usage page. It
// accepts the same query-string filters and returns a paginated list of
// usage events. Useful for ad-hoc tooling and `cliproxy admin …` style
// scripts.
func (s *Server) apiUsageSearch(w http.ResponseWriter, r *http.Request) {
	form := parseStatsFilter(r.URL.Query())
	filter := stats.SearchFilter{
		Text:      form.Q,
		UserID:    form.UserID,
		Model:     form.Model,
		Status:    form.Status,
		OnlyError: form.OnlyError,
		MinTokens: form.MinTokens,
		Sort:      form.Sort,
		Limit:     form.PageSize,
		Offset:    (form.Page - 1) * form.PageSize,
	}
	if form.Since != "" {
		if t, err := time.Parse("2006-01-02T15:04", form.Since); err == nil {
			filter.Since = t
		}
	}
	if form.Until != "" {
		if t, err := time.Parse("2006-01-02T15:04", form.Until); err == nil {
			filter.Until = t
		}
	}
	rows, total, err := s.recorder.Search(r.Context(), filter)
	if err != nil {
		writeJSONError(w, http.StatusInternalServerError, err.Error())
		return
	}
	out := make([]map[string]any, 0, len(rows))
	for _, row := range rows {
		out = append(out, map[string]any{
			"id":                    row.ID,
			"ts":                    row.Timestamp.Format(time.RFC3339),
			"user_id":               row.UserID,
			"user_label":            row.UserLabel,
			"model":                 row.Model,
			"input_tokens":          row.InputTokens,
			"output_tokens":         row.OutputTokens,
			"cache_read_tokens":     row.CacheReadTokens,
			"cache_creation_tokens": row.CacheCreationTokens,
			"latency_ms":            row.Latency.Milliseconds(),
			"status":                row.Status,
			"error":                 row.Error,
		})
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"events":    out,
		"total":     total,
		"page":      form.Page,
		"page_size": form.PageSize,
	})
}

func (s *Server) apiStats(w http.ResponseWriter, r *http.Request) {
	days := 30
	if v := r.URL.Query().Get("days"); v != "" {
		if d, err := strconv.Atoi(v); err == nil && d > 0 {
			days = d
		}
	}
	byDay, _ := s.recorder.ByDay(r.Context(), days)
	byUser, _ := s.recorder.ByUser(r.Context(), days)
	byModel, _ := s.recorder.ByModel(r.Context(), days)
	writeJSON(w, http.StatusOK, map[string]any{
		"days":     days,
		"by_day":   byDay,
		"by_user":  byUser,
		"by_model": byModel,
	})
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}
