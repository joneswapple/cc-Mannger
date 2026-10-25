package policy

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"strings"
	"sync"
	"time"

	"github.com/hubosdev/cliproxy/internal/store"
)

const (
	settingDisabledModels = "disabled_models"
)

var (
	ErrModelDisabled = errors.New("model disabled by global policy")
	ErrModelNotAllowed = errors.New("model not allowed for this user")
	ErrRateLimited     = errors.New("rate limit exceeded")
	ErrQuotaExceeded   = errors.New("daily token quota exceeded")
)

type Policy struct {
	AllowedModels   []string // nil = all
	DailyTokenLimit *int64
	RPMLimit        *int
}

type Engine struct {
	s        *store.Store
	mu       sync.Mutex
	buckets  map[int64]*bucket
	clock    func() time.Time
}

type bucket struct {
	count    int
	windowStart time.Time
}

func NewEngine(s *store.Store) *Engine {
	return &Engine{s: s, buckets: map[int64]*bucket{}, clock: time.Now}
}

func (e *Engine) GetPolicy(ctx context.Context, userID int64) (Policy, error) {
	var p Policy
	var allowed sql.NullString
	var daily sql.NullInt64
	var rpm sql.NullInt64
	err := e.s.DB.QueryRowContext(ctx,
		`SELECT allowed_models, daily_token_limit, rpm_limit FROM policies WHERE user_id=?`, userID).
		Scan(&allowed, &daily, &rpm)
	if err != nil && err != sql.ErrNoRows {
		return p, err
	}
	if allowed.Valid && allowed.String != "" {
		var arr []string
		if err := json.Unmarshal([]byte(allowed.String), &arr); err != nil {
			return p, fmt.Errorf("decode allowed_models: %w", err)
		}
		p.AllowedModels = arr
	}
	if daily.Valid {
		v := daily.Int64
		p.DailyTokenLimit = &v
	}
	if rpm.Valid {
		v := int(rpm.Int64)
		p.RPMLimit = &v
	}
	return p, nil
}

func (e *Engine) SetPolicy(ctx context.Context, userID int64, p Policy) error {
	var allowed sql.NullString
	if p.AllowedModels != nil {
		b, err := json.Marshal(p.AllowedModels)
		if err != nil {
			return err
		}
		allowed = sql.NullString{String: string(b), Valid: true}
	}
	var daily sql.NullInt64
	if p.DailyTokenLimit != nil {
		daily = sql.NullInt64{Int64: *p.DailyTokenLimit, Valid: true}
	}
	var rpm sql.NullInt64
	if p.RPMLimit != nil {
		rpm = sql.NullInt64{Int64: int64(*p.RPMLimit), Valid: true}
	}
	_, err := e.s.DB.ExecContext(ctx, `
		INSERT INTO policies(user_id, allowed_models, daily_token_limit, rpm_limit)
		VALUES(?,?,?,?)
		ON CONFLICT(user_id) DO UPDATE SET
			allowed_models=excluded.allowed_models,
			daily_token_limit=excluded.daily_token_limit,
			rpm_limit=excluded.rpm_limit`,
		userID, allowed, daily, rpm)
	return err
}

func (e *Engine) DisabledModels(ctx context.Context) ([]string, error) {
	v, err := e.s.GetSetting(ctx, settingDisabledModels)
	if err != nil || v == "" {
		return nil, err
	}
	var arr []string
	if err := json.Unmarshal([]byte(v), &arr); err != nil {
		return nil, err
	}
	return arr, nil
}

func (e *Engine) SetDisabledModels(ctx context.Context, models []string) error {
	if models == nil {
		models = []string{}
	}
	b, err := json.Marshal(models)
	if err != nil {
		return err
	}
	return e.s.SetSetting(ctx, settingDisabledModels, string(b))
}

// Check evaluates static (non-quota) constraints: model allowed and RPM bucket.
// Daily token quota is enforced separately after we know the token cost.
func (e *Engine) Check(ctx context.Context, userID int64, model string) error {
	disabled, err := e.DisabledModels(ctx)
	if err != nil {
		return err
	}
	for _, m := range disabled {
		if strings.EqualFold(m, model) {
			return ErrModelDisabled
		}
	}
	p, err := e.GetPolicy(ctx, userID)
	if err != nil {
		return err
	}
	if p.AllowedModels != nil {
		ok := false
		for _, m := range p.AllowedModels {
			if strings.EqualFold(m, model) {
				ok = true
				break
			}
		}
		if !ok {
			return ErrModelNotAllowed
		}
	}
	if p.RPMLimit != nil {
		if !e.allowRPM(userID, *p.RPMLimit) {
			return ErrRateLimited
		}
	}
	if p.DailyTokenLimit != nil {
		used, err := e.dailyTokensUsed(ctx, userID)
		if err != nil {
			return err
		}
		if used >= *p.DailyTokenLimit {
			return ErrQuotaExceeded
		}
	}
	return nil
}

func (e *Engine) allowRPM(userID int64, limit int) bool {
	e.mu.Lock()
	defer e.mu.Unlock()
	now := e.clock()
	b := e.buckets[userID]
	if b == nil || now.Sub(b.windowStart) >= time.Minute {
		e.buckets[userID] = &bucket{count: 1, windowStart: now}
		return true
	}
	if b.count >= limit {
		return false
	}
	b.count++
	return true
}

func (e *Engine) dailyTokensUsed(ctx context.Context, userID int64) (int64, error) {
	now := e.clock()
	startOfDay := time.Date(now.Year(), now.Month(), now.Day(), 0, 0, 0, 0, now.Location())
	var total sql.NullInt64
	err := e.s.DB.QueryRowContext(ctx, `
		SELECT COALESCE(SUM(input_tokens + output_tokens), 0)
		FROM usage_events WHERE user_id=? AND ts>=?`,
		userID, startOfDay.Unix()).Scan(&total)
	if err != nil {
		return 0, err
	}
	if total.Valid {
		return total.Int64, nil
	}
	return 0, nil
}
