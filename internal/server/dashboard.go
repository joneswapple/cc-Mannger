package server

import (
	"fmt"
	"net/http"
	"net/url"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/hubosdev/cliproxy/internal/stats"
)

type pageData struct {
	Title   string
	Active  string
	Flash   string
	Next    string
	Content any
}

func (s *Server) render(w http.ResponseWriter, name string, data pageData) {
	t, ok := s.templates[name]
	if !ok {
		http.Error(w, "no template "+name, http.StatusInternalServerError)
		return
	}
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	if err := t.ExecuteTemplate(w, "base", data); err != nil {
		http.Error(w, "render: "+err.Error(), http.StatusInternalServerError)
	}
}

func safeNext(raw string) string {
	if !strings.HasPrefix(raw, "/admin") || strings.HasPrefix(raw, "/admin/login") {
		return "/admin/users"
	}
	return raw
}

func (s *Server) handleAdminLoginPage(w http.ResponseWriter, r *http.Request) {
	next := safeNext(r.URL.Query().Get("next"))
	s.render(w, "login.html", pageData{Title: "Sign in", Active: "login", Next: next})
}

func (s *Server) handleAdminLoginSubmit(w http.ResponseWriter, r *http.Request) {
	if err := r.ParseForm(); err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	token := strings.TrimSpace(r.FormValue("token"))
	// Preserve `next` from either the query string or the form (the form
	// carries it as a hidden field so it survives the POST submit).
	next := safeNext(firstNonEmpty(r.URL.Query().Get("next"), r.FormValue("next")))
	if err := s.admin.Verify(r.Context(), token); err != nil {
		s.render(w, "login.html", pageData{Title: "Sign in", Active: "login", Flash: "Invalid token", Next: next})
		return
	}
	http.SetCookie(w, &http.Cookie{
		Name:     "cliproxy_admin",
		Value:    token,
		Path:     "/",
		HttpOnly: true,
		SameSite: http.SameSiteLaxMode,
		Expires:  time.Now().Add(24 * time.Hour),
	})
	http.Redirect(w, r, next, http.StatusSeeOther)
}

func firstNonEmpty(vals ...string) string {
	for _, v := range vals {
		if strings.TrimSpace(v) != "" {
			return v
		}
	}
	return ""
}

func (s *Server) handleAdminLogout(w http.ResponseWriter, r *http.Request) {
	http.SetCookie(w, &http.Cookie{
		Name:    "cliproxy_admin",
		Value:   "",
		Path:    "/",
		Expires: time.Unix(0, 0),
		MaxAge:  -1,
	})
	http.Redirect(w, r, "/admin/login", http.StatusSeeOther)
}

type usersView struct {
	Users []userRow
}

type userRow struct {
	ID        int64
	Label     string
	Enabled   bool
	CreatedAt time.Time
	Keys      []keyRow
	Policy    policyView
}

type keyRow struct {
	ID         int64
	Prefix     string
	CreatedAt  time.Time
	LastUsedAt *time.Time
	RevokedAt  *time.Time
}

type policyView struct {
	AllowedModels   string
	DailyTokenLimit string
	RPMLimit        string
}

func (s *Server) handleUsersPage(w http.ResponseWriter, r *http.Request) {
	users, err := s.keys.ListUsers(r.Context())
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	view := usersView{}
	for _, u := range users {
		row := userRow{
			ID: u.UserID, Label: u.Label, Enabled: u.Enabled, CreatedAt: u.CreatedAt,
		}
		for _, k := range u.Keys {
			row.Keys = append(row.Keys, keyRow{
				ID: k.ID, Prefix: k.Prefix, CreatedAt: k.CreatedAt,
				LastUsedAt: k.LastUsedAt, RevokedAt: k.RevokedAt,
			})
		}
		p, _ := s.policy.GetPolicy(r.Context(), u.UserID)
		pv := policyView{AllowedModels: "all", DailyTokenLimit: "unlimited", RPMLimit: "unlimited"}
		if p.AllowedModels != nil {
			pv.AllowedModels = strings.Join(p.AllowedModels, ", ")
		}
		if p.DailyTokenLimit != nil {
			pv.DailyTokenLimit = formatInt(*p.DailyTokenLimit)
		}
		if p.RPMLimit != nil {
			pv.RPMLimit = formatInt(int64(*p.RPMLimit))
		}
		row.Policy = pv
		view.Users = append(view.Users, row)
	}
	s.render(w, "users.html", pageData{Title: "Users", Active: "users", Content: view})
}

type invitesView struct {
	Invites []inviteRow
}

type inviteRow struct {
	Hash       string
	Label      string
	CreatedAt  time.Time
	ExpiresAt  time.Time
	RedeemedAt *time.Time
}

func (s *Server) handleInvitesPage(w http.ResponseWriter, r *http.Request) {
	list, err := s.invites.List(r.Context())
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	v := invitesView{}
	for _, i := range list {
		v.Invites = append(v.Invites, inviteRow{
			Hash: i.TokenHash, Label: i.Label, CreatedAt: i.CreatedAt,
			ExpiresAt: i.ExpiresAt, RedeemedAt: i.RedeemedAt,
		})
	}
	s.render(w, "invites.html", pageData{Title: "Invites", Active: "invites", Content: v})
}

type statsView struct {
	// Filter form state — pre-populates the form on reload.
	Filter statsFilterForm

	// Aggregates above the list (last N days, where N follows the filter
	// "Since" if set, otherwise 30).
	SummaryDays int
	ByDay       []stats.DailySummary

	// Filter dropdown contents.
	UserOptions  []userOption
	ModelOptions []string

	// The actual paginated event list.
	Rows         []eventRowView
	Total        int
	Page         int
	PageSize     int
	TotalPages   int
	HasPrev      bool
	HasNext      bool
	PrevQuery    string
	NextQuery    string
	CurrentQuery string
}

type statsFilterForm struct {
	Q         string
	UserID    int64
	Model     string
	Status    string
	OnlyError bool
	MinTokens int64
	Since     string
	Until     string
	Sort      string
	Page      int
	PageSize  int
}

type userOption struct {
	ID    int64
	Label string
}

type eventRowView struct {
	ID            int64
	When          time.Time
	WhenRelative  string
	UserID        int64
	UserLabel     string
	UserInitial   string
	UserSwatch    string // background colour token derived from the label
	Model         string
	ModelInitial  string
	ModelSwatch   string
	InputTokens   int64
	OutputTokens  int64
	TotalTokens   int64
	CacheRead     int64
	CacheCreate   int64
	LatencyMS     int64
	LatencyHuman  string
	Status        int
	StatusBucket  string // ok | rate_limit | error
	StatusLabel   string
	Error         string
}

const (
	statsDefaultPageSize = 50
	statsMaxPageSize     = 500
)

func (s *Server) handleStatsPage(w http.ResponseWriter, r *http.Request) {
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
		http.Error(w, "search: "+err.Error(), http.StatusInternalServerError)
		return
	}
	view := statsView{
		Filter:   form,
		Rows:     toEventRowViews(rows),
		Total:    total,
		Page:     form.Page,
		PageSize: form.PageSize,
	}
	view.TotalPages = (total + form.PageSize - 1) / form.PageSize
	if view.TotalPages < 1 {
		view.TotalPages = 1
	}
	view.HasPrev = form.Page > 1
	view.HasNext = form.Page < view.TotalPages
	view.CurrentQuery = r.URL.RawQuery

	if view.HasPrev {
		view.PrevQuery = withPage(r.URL.Query(), form.Page-1)
	}
	if view.HasNext {
		view.NextQuery = withPage(r.URL.Query(), form.Page+1)
	}

	view.SummaryDays = 30
	view.ByDay, _ = s.recorder.ByDay(r.Context(), view.SummaryDays)

	users, _ := s.keys.ListUsers(r.Context())
	for _, u := range users {
		view.UserOptions = append(view.UserOptions, userOption{ID: u.UserID, Label: u.Label})
	}
	if s.catalog != nil {
		view.ModelOptions = s.catalog.IDs()
	}

	s.render(w, "stats.html", pageData{Title: "Stats", Active: "stats", Content: view})
}

func parseStatsFilter(q url.Values) statsFilterForm {
	f := statsFilterForm{
		Q:        strings.TrimSpace(q.Get("q")),
		Model:    strings.TrimSpace(q.Get("model")),
		Status:   q.Get("status"),
		Sort:     q.Get("sort"),
		Since:    q.Get("since"),
		Until:    q.Get("until"),
		PageSize: statsDefaultPageSize,
		Page:     1,
	}
	if v := q.Get("user_id"); v != "" {
		if id, err := strconv.ParseInt(v, 10, 64); err == nil {
			f.UserID = id
		}
	}
	if v := q.Get("only_error"); v == "1" || v == "true" || v == "on" {
		f.OnlyError = true
	}
	if v := q.Get("min_tokens"); v != "" {
		if n, err := strconv.ParseInt(v, 10, 64); err == nil && n > 0 {
			f.MinTokens = n
		}
	}
	if v := q.Get("page"); v != "" {
		if n, err := strconv.Atoi(v); err == nil && n > 0 {
			f.Page = n
		}
	}
	if v := q.Get("page_size"); v != "" {
		if n, err := strconv.Atoi(v); err == nil && n > 0 {
			f.PageSize = n
		}
	}
	if f.PageSize > statsMaxPageSize {
		f.PageSize = statsMaxPageSize
	}
	if f.Sort == "" {
		f.Sort = "time_desc"
	}
	if f.Status == "" {
		f.Status = "any"
	}
	return f
}

func withPage(q url.Values, page int) string {
	q.Set("page", strconv.Itoa(page))
	return q.Encode()
}

func toEventRowViews(rows []stats.EventRow) []eventRowView {
	out := make([]eventRowView, 0, len(rows))
	for _, r := range rows {
		v := eventRowView{
			ID:           r.ID,
			When:         r.Timestamp,
			WhenRelative: relativeTime(r.Timestamp),
			UserID:       r.UserID,
			UserLabel:    r.UserLabel,
			UserInitial:  initial(r.UserLabel),
			UserSwatch:   swatch(r.UserLabel),
			Model:        r.Model,
			ModelInitial: modelInitial(r.Model),
			ModelSwatch:  swatch(r.Model),
			InputTokens:  r.InputTokens,
			OutputTokens: r.OutputTokens,
			TotalTokens:  r.InputTokens + r.OutputTokens,
			CacheRead:    r.CacheReadTokens,
			CacheCreate:  r.CacheCreationTokens,
			LatencyMS:    r.Latency.Milliseconds(),
			LatencyHuman: humanLatency(r.Latency),
			Status:       r.Status,
			Error:        r.Error,
		}
		v.StatusBucket, v.StatusLabel = bucketStatus(r.Status, r.Error)
		out = append(out, v)
	}
	return out
}

func bucketStatus(code int, errStr string) (string, string) {
	switch {
	case code == http.StatusTooManyRequests || strings.Contains(errStr, "rate_limit"):
		return "rate_limit", "rate-limit"
	case code >= 500:
		return "error", "5xx"
	case code >= 400:
		return "error", "4xx"
	case code >= 200:
		return "ok", "ok"
	default:
		return "error", "?"
	}
}

func initial(s string) string {
	for _, r := range strings.TrimSpace(s) {
		return strings.ToUpper(string(r))
	}
	return "·"
}

func modelInitial(model string) string {
	low := strings.ToLower(model)
	switch {
	case strings.Contains(low, "opus"):
		return "O"
	case strings.Contains(low, "sonnet"):
		return "S"
	case strings.Contains(low, "haiku"):
		return "H"
	}
	return initial(model)
}

func swatch(s string) string {
	// Stable 8-bucket hash so the same label/model always gets the same
	// avatar colour. CSS class names are .swatch-0 … .swatch-7.
	if s == "" {
		return "swatch-0"
	}
	var h uint32 = 2166136261
	for i := 0; i < len(s); i++ {
		h ^= uint32(s[i])
		h *= 16777619
	}
	return fmt.Sprintf("swatch-%d", h%8)
}

func relativeTime(t time.Time) string {
	d := time.Since(t)
	switch {
	case d < 0:
		return "just now"
	case d < time.Minute:
		return "just now"
	case d < time.Hour:
		return fmt.Sprintf("%dm ago", int(d.Minutes()))
	case d < 24*time.Hour:
		return fmt.Sprintf("%dh ago", int(d.Hours()))
	case d < 7*24*time.Hour:
		return fmt.Sprintf("%dd ago", int(d.Hours()/24))
	}
	return t.Format("2006-01-02")
}

func humanLatency(d time.Duration) string {
	ms := d.Milliseconds()
	switch {
	case ms < 1000:
		return fmt.Sprintf("%dms", ms)
	case ms < 60_000:
		return fmt.Sprintf("%.2fs", float64(ms)/1000)
	}
	return fmt.Sprintf("%dm%ds", ms/60_000, (ms%60_000)/1000)
}

type settingsView struct {
	UpstreamSource    string
	UpstreamBaseURL   string
	AnthropicBeta     string
	OAuthTokenURL     string
	OAuthClientID     string
	UserAgent         string
	InjectSystem      bool
	EffortTiers       []effortTierRow
	CatalogFetched    string
	CatalogError      string
	AvailableModels   []modelChoice    // models currently exposed by /v1/models
	UnavailableModels []modelChoice    // disabled in DB but no longer in upstream
}

type effortTierRow struct {
	Name            string
	MaxTokens       int
	ThinkingBudget  int
	ThinkingEnabled bool
}

type modelChoice struct {
	ID          string
	DisplayName string
	Disabled    bool // currently in the global denylist
	Available   bool // present in upstream catalog
}

func (s *Server) handleSettingsPage(w http.ResponseWriter, r *http.Request) {
	disabled, _ := s.policy.DisabledModels(r.Context())
	disabledSet := make(map[string]struct{}, len(disabled))
	for _, m := range disabled {
		disabledSet[m] = struct{}{}
	}

	view := settingsView{
		UpstreamSource:  s.upstream.Source(),
		UpstreamBaseURL: s.cfg.Upstream.BaseURL,
		AnthropicBeta:   s.cfg.Upstream.AnthropicBeta,
		OAuthTokenURL:   s.cfg.Upstream.OAuthTokenURL,
		OAuthClientID:   s.cfg.Upstream.OAuthClientID,
		UserAgent:       s.cfg.Upstream.UserAgent,
		InjectSystem:    s.cfg.Upstream.InjectClaudeCodeSystem,
	}
	tierNames := make([]string, 0, len(s.cfg.Upstream.EffortLevels))
	for k := range s.cfg.Upstream.EffortLevels {
		tierNames = append(tierNames, k)
	}
	sort.Strings(tierNames)
	for _, name := range tierNames {
		lvl := s.cfg.Upstream.EffortLevels[name]
		view.EffortTiers = append(view.EffortTiers, effortTierRow{
			Name:            name,
			MaxTokens:       lvl.MaxTokens,
			ThinkingBudget:  lvl.ThinkingBudget,
			ThinkingEnabled: lvl.ThinkingBudget > 0,
		})
	}
	if s.catalog != nil {
		st := s.catalog.Status()
		if !st.LastFetched.IsZero() {
			view.CatalogFetched = st.LastFetched.Format("2006-01-02 15:04:05")
		}
		view.CatalogError = st.LastError

		for _, m := range s.catalog.Models() {
			name := m.DisplayName
			if name == "" {
				name = m.ID
			}
			_, isDisabled := disabledSet[m.ID]
			view.AvailableModels = append(view.AvailableModels, modelChoice{
				ID:          m.ID,
				DisplayName: name,
				Disabled:    isDisabled,
				Available:   true,
			})
			delete(disabledSet, m.ID)
		}
	}
	// Anything left in disabledSet was stored previously but the upstream
	// catalog no longer lists it. Surface it as unavailable so the operator
	// notices and can clean up.
	leftovers := make([]string, 0, len(disabledSet))
	for id := range disabledSet {
		leftovers = append(leftovers, id)
	}
	sort.Strings(leftovers)
	for _, id := range leftovers {
		view.UnavailableModels = append(view.UnavailableModels, modelChoice{
			ID:        id,
			Disabled:  true,
			Available: false,
		})
	}
	s.render(w, "settings.html", pageData{Title: "Settings", Active: "settings", Content: view})
}

func formatInt(v int64) string {
	return strings.TrimSpace(itoa(v))
}

func itoa(v int64) string {
	const digits = "0123456789"
	if v == 0 {
		return "0"
	}
	neg := v < 0
	if neg {
		v = -v
	}
	buf := [20]byte{}
	i := len(buf)
	for v > 0 {
		i--
		buf[i] = digits[v%10]
		v /= 10
	}
	if neg {
		i--
		buf[i] = '-'
	}
	return string(buf[i:])
}
