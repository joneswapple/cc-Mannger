package stats

import (
	"context"
	"database/sql"
	"fmt"
	"log"
	"strconv"
	"strings"
	"time"

	"github.com/hubosdev/cliproxy/internal/store"
)

type Event struct {
	Timestamp           time.Time
	UserID              int64
	KeyID               int64
	Model               string
	InputTokens         int64
	OutputTokens        int64
	CacheReadTokens     int64
	CacheCreationTokens int64
	Latency             time.Duration
	Status              int
	Error               string
}

type Recorder struct {
	s  *store.Store
	ch chan Event
}

func NewRecorder(ctx context.Context, s *store.Store) *Recorder {
	r := &Recorder{s: s, ch: make(chan Event, 256)}
	go r.run(ctx)
	return r
}

func (r *Recorder) Record(e Event) {
	select {
	case r.ch <- e:
	default:
		log.Printf("stats: drop event (queue full): user=%d model=%s", e.UserID, e.Model)
	}
}

func (r *Recorder) run(ctx context.Context) {
	for {
		select {
		case <-ctx.Done():
			return
		case e := <-r.ch:
			if err := r.write(ctx, e); err != nil {
				log.Printf("stats: write failed: %v", err)
			}
		}
	}
}

func (r *Recorder) write(ctx context.Context, e Event) error {
	_, err := r.s.DB.ExecContext(ctx, `
		INSERT INTO usage_events(
			ts, user_id, key_id, model,
			input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
			latency_ms, status, error)
		VALUES(?,?,?,?,?,?,?,?,?,?,?)`,
		e.Timestamp.Unix(), e.UserID, e.KeyID, e.Model,
		e.InputTokens, e.OutputTokens, e.CacheReadTokens, e.CacheCreationTokens,
		e.Latency.Milliseconds(), e.Status, nullString(e.Error))
	return err
}

func nullString(s string) any {
	if s == "" {
		return nil
	}
	return s
}

type DailySummary struct {
	Date         string
	Requests     int64
	InputTokens  int64
	OutputTokens int64
}

type UserSummary struct {
	UserID       int64
	Label        string
	Requests     int64
	InputTokens  int64
	OutputTokens int64
}

type ModelSummary struct {
	Model        string
	Requests     int64
	InputTokens  int64
	OutputTokens int64
}

func (r *Recorder) ByDay(ctx context.Context, days int) ([]DailySummary, error) {
	since := time.Now().Add(-time.Duration(days) * 24 * time.Hour).Unix()
	rows, err := r.s.DB.QueryContext(ctx, `
		SELECT date(ts, 'unixepoch') AS d,
		       COUNT(*),
		       COALESCE(SUM(input_tokens),0),
		       COALESCE(SUM(output_tokens),0)
		FROM usage_events WHERE ts>=?
		GROUP BY d ORDER BY d`, since)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []DailySummary
	for rows.Next() {
		var s DailySummary
		if err := rows.Scan(&s.Date, &s.Requests, &s.InputTokens, &s.OutputTokens); err != nil {
			return nil, err
		}
		out = append(out, s)
	}
	return out, nil
}

func (r *Recorder) ByUser(ctx context.Context, days int) ([]UserSummary, error) {
	since := time.Now().Add(-time.Duration(days) * 24 * time.Hour).Unix()
	rows, err := r.s.DB.QueryContext(ctx, `
		SELECT u.id, u.label,
		       COUNT(*),
		       COALESCE(SUM(input_tokens),0),
		       COALESCE(SUM(output_tokens),0)
		FROM usage_events e JOIN users u ON u.id = e.user_id
		WHERE e.ts>=? GROUP BY u.id, u.label ORDER BY u.id`, since)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []UserSummary
	for rows.Next() {
		var s UserSummary
		if err := rows.Scan(&s.UserID, &s.Label, &s.Requests, &s.InputTokens, &s.OutputTokens); err != nil {
			return nil, err
		}
		out = append(out, s)
	}
	return out, nil
}

func (r *Recorder) ByModel(ctx context.Context, days int) ([]ModelSummary, error) {
	since := time.Now().Add(-time.Duration(days) * 24 * time.Hour).Unix()
	rows, err := r.s.DB.QueryContext(ctx, `
		SELECT model, COUNT(*),
		       COALESCE(SUM(input_tokens),0),
		       COALESCE(SUM(output_tokens),0)
		FROM usage_events WHERE ts>=? GROUP BY model ORDER BY model`, since)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []ModelSummary
	for rows.Next() {
		var s ModelSummary
		if err := rows.Scan(&s.Model, &s.Requests, &s.InputTokens, &s.OutputTokens); err != nil {
			return nil, err
		}
		out = append(out, s)
	}
	return out, nil
}

func (r *Recorder) Recent(ctx context.Context, limit int) ([]Event, error) {
	rows, err := r.s.DB.QueryContext(ctx, `
		SELECT ts, user_id, key_id, model, input_tokens, output_tokens,
		       cache_read_tokens, cache_creation_tokens, latency_ms, status, error
		FROM usage_events ORDER BY id DESC LIMIT ?`, limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Event
	for rows.Next() {
		var e Event
		var ts, latencyMs int64
		var errStr sql.NullString
		if err := rows.Scan(&ts, &e.UserID, &e.KeyID, &e.Model,
			&e.InputTokens, &e.OutputTokens, &e.CacheReadTokens, &e.CacheCreationTokens,
			&latencyMs, &e.Status, &errStr); err != nil {
			return nil, err
		}
		e.Timestamp = time.Unix(ts, 0)
		e.Latency = time.Duration(latencyMs) * time.Millisecond
		if errStr.Valid {
			e.Error = errStr.String
		}
		out = append(out, e)
	}
	return out, nil
}

// EventRow is a single usage event joined with its user label, ready for
// rendering on the searchable usage page.
type EventRow struct {
	ID                  int64
	Timestamp           time.Time
	UserID              int64
	UserLabel           string
	KeyID               int64
	Model               string
	InputTokens         int64
	OutputTokens        int64
	CacheReadTokens     int64
	CacheCreationTokens int64
	Latency             time.Duration
	Status              int
	Error               string
}

// SearchFilter narrows down a usage_events scan along the seven
// dimensions the dashboard exposes. All fields are optional; zero values
// mean "don't filter on this".
type SearchFilter struct {
	Text      string    // free-text matched against model, user label, error
	UserID    int64     // exact user
	Model     string    // substring match against model
	Status    string    // "ok" / "error" / "2xx" / "4xx" / "5xx" / literal HTTP code
	Since     time.Time // inclusive lower bound on timestamp
	Until     time.Time // inclusive upper bound on timestamp
	MinTokens int64     // minimum (input+output) tokens
	OnlyError bool      // only rows with non-empty error column
	Sort      string    // time_desc (default) | time_asc | tokens_desc | tokens_asc | latency_desc | latency_asc
	Limit     int       // page size; defaults to 50, capped at 500
	Offset    int
}

// Search returns the matching rows plus the total count under the same
// filter (for pagination). The query is built from `f` so callers can
// rely on every supported dimension being applied even when others are
// blank.
func (r *Recorder) Search(ctx context.Context, f SearchFilter) (rows []EventRow, total int, err error) {
	limit := f.Limit
	if limit <= 0 {
		limit = 50
	}
	if limit > 500 {
		limit = 500
	}

	where := []string{"1=1"}
	args := []any{}
	if f.UserID > 0 {
		where = append(where, "e.user_id = ?")
		args = append(args, f.UserID)
	}
	if strings.TrimSpace(f.Model) != "" {
		where = append(where, "e.model LIKE ?")
		args = append(args, "%"+strings.TrimSpace(f.Model)+"%")
	}
	if !f.Since.IsZero() {
		where = append(where, "e.ts >= ?")
		args = append(args, f.Since.Unix())
	}
	if !f.Until.IsZero() {
		where = append(where, "e.ts <= ?")
		args = append(args, f.Until.Unix())
	}
	if f.MinTokens > 0 {
		where = append(where, "(e.input_tokens + e.output_tokens) >= ?")
		args = append(args, f.MinTokens)
	}
	if f.OnlyError {
		where = append(where, "(e.error IS NOT NULL AND e.error != '')")
	}
	if cond, condArgs := statusWhere(f.Status); cond != "" {
		where = append(where, cond)
		args = append(args, condArgs...)
	}
	if t := strings.TrimSpace(f.Text); t != "" {
		// Free-text matches model, user label, or error message.
		where = append(where, "(e.model LIKE ? OR COALESCE(u.label,'') LIKE ? OR COALESCE(e.error,'') LIKE ?)")
		needle := "%" + t + "%"
		args = append(args, needle, needle, needle)
	}

	whereSQL := strings.Join(where, " AND ")

	// Total count under the same filter — drives pagination.
	countQuery := `SELECT COUNT(*)
	               FROM usage_events e
	               LEFT JOIN users u ON u.id = e.user_id
	               WHERE ` + whereSQL
	if err := r.s.DB.QueryRowContext(ctx, countQuery, args...).Scan(&total); err != nil {
		return nil, 0, fmt.Errorf("count: %w", err)
	}

	orderBy := sortClause(f.Sort)
	listQuery := fmt.Sprintf(`
		SELECT e.id, e.ts, e.user_id, COALESCE(u.label, '(deleted)'), e.key_id, e.model,
		       e.input_tokens, e.output_tokens, e.cache_read_tokens, e.cache_creation_tokens,
		       e.latency_ms, e.status, COALESCE(e.error, '')
		FROM usage_events e
		LEFT JOIN users u ON u.id = e.user_id
		WHERE %s
		ORDER BY %s
		LIMIT ? OFFSET ?`, whereSQL, orderBy)
	listArgs := append(append([]any{}, args...), limit, f.Offset)

	dbRows, err := r.s.DB.QueryContext(ctx, listQuery, listArgs...)
	if err != nil {
		return nil, 0, fmt.Errorf("query: %w", err)
	}
	defer dbRows.Close()
	for dbRows.Next() {
		var er EventRow
		var ts, latencyMs int64
		if err := dbRows.Scan(&er.ID, &ts, &er.UserID, &er.UserLabel, &er.KeyID, &er.Model,
			&er.InputTokens, &er.OutputTokens, &er.CacheReadTokens, &er.CacheCreationTokens,
			&latencyMs, &er.Status, &er.Error); err != nil {
			return nil, 0, err
		}
		er.Timestamp = time.Unix(ts, 0)
		er.Latency = time.Duration(latencyMs) * time.Millisecond
		rows = append(rows, er)
	}
	return rows, total, nil
}

// statusWhere translates a status filter token into a SQL fragment.
// Accepts: "ok" (2xx/3xx), "error" (>=400), "2xx"/"4xx"/"5xx" buckets,
// or a literal HTTP code.
func statusWhere(s string) (string, []any) {
	switch strings.ToLower(strings.TrimSpace(s)) {
	case "", "any":
		return "", nil
	case "ok":
		return "(e.status >= 200 AND e.status < 400)", nil
	case "error":
		return "e.status >= 400", nil
	case "2xx":
		return "(e.status >= 200 AND e.status < 300)", nil
	case "3xx":
		return "(e.status >= 300 AND e.status < 400)", nil
	case "4xx":
		return "(e.status >= 400 AND e.status < 500)", nil
	case "5xx":
		return "e.status >= 500", nil
	default:
		if code, err := strconv.Atoi(s); err == nil {
			return "e.status = ?", []any{code}
		}
	}
	return "", nil
}

func sortClause(s string) string {
	switch s {
	case "time_asc":
		return "e.id ASC"
	case "tokens_desc":
		return "(e.input_tokens + e.output_tokens) DESC, e.id DESC"
	case "tokens_asc":
		return "(e.input_tokens + e.output_tokens) ASC, e.id ASC"
	case "latency_desc":
		return "e.latency_ms DESC, e.id DESC"
	case "latency_asc":
		return "e.latency_ms ASC, e.id ASC"
	default:
		return "e.id DESC"
	}
}
