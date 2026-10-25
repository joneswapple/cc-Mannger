package server

import (
	"bufio"
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"net/http"
	"sort"
	"strings"
	"time"

	"github.com/hubosdev/cliproxy/internal/config"
	"github.com/hubosdev/cliproxy/internal/policy"
	"github.com/hubosdev/cliproxy/internal/stats"
	"github.com/hubosdev/cliproxy/internal/upstream"
)

// passthroughUserAgent returns the inbound UA only when it looks like one
// of the official Anthropic clients (claude-cli/…, claude-code/…,
// anthropic-…). For unknown / generic UAs it returns the empty string,
// causing Client.do to fall back to the configured default — that
// default mimics Claude Code which is what Anthropic's anti-abuse layer
// expects to see, so a plain `curl/8.x` UA isn't trusted further than
// our own.
func passthroughUserAgent(ua string) string {
	if ua == "" {
		return ""
	}
	low := strings.ToLower(strings.TrimSpace(ua))
	prefixes := []string{"claude-cli/", "claude-code/", "anthropic-", "claude/"}
	for _, p := range prefixes {
		if strings.HasPrefix(low, p) {
			return ua
		}
	}
	return ""
}

type messageRequest struct {
	Model  string `json:"model"`
	Stream bool   `json:"stream"`
}

func (s *Server) handleMessages(w http.ResponseWriter, r *http.Request) {
	id, ok := identityFrom(r.Context())
	if !ok {
		writeAnthropicError(w, http.StatusUnauthorized, "authentication_error", "no identity")
		return
	}
	bodyBytes, err := io.ReadAll(io.LimitReader(r.Body, 16<<20)) // 16 MiB
	if err != nil {
		writeAnthropicError(w, http.StatusBadRequest, "invalid_request_error", "read body: "+err.Error())
		return
	}
	var parsed messageRequest
	if err := json.Unmarshal(bodyBytes, &parsed); err != nil {
		writeAnthropicError(w, http.StatusBadRequest, "invalid_request_error", "parse body: "+err.Error())
		return
	}
	if parsed.Model == "" {
		writeAnthropicError(w, http.StatusBadRequest, "invalid_request_error", "model is required")
		return
	}

	// OpenAI-style effort tier → Anthropic max_tokens + thinking. Run
	// before the Claude Code system injection so subsequent code sees a
	// canonical body with `effort` already stripped.
	if rewritten, applied, err := applyEffort(bodyBytes, s.cfg.Upstream.EffortLevels); err != nil {
		writeAnthropicError(w, http.StatusBadRequest, "invalid_request_error", err.Error())
		return
	} else if applied {
		bodyBytes = rewritten
	}

	if err := s.policy.Check(r.Context(), id.UserID, parsed.Model); err != nil {
		status := http.StatusForbidden
		errType := "permission_error"
		switch {
		case errors.Is(err, policy.ErrRateLimited):
			status = http.StatusTooManyRequests
			errType = "rate_limit_error"
		case errors.Is(err, policy.ErrQuotaExceeded):
			status = http.StatusTooManyRequests
			errType = "rate_limit_error"
		}
		writeAnthropicError(w, status, errType, err.Error())
		return
	}

	// Anthropic's OAuth-bearer (Pro/Max) endpoint rejects requests whose
	// first system block isn't the literal Claude Code identifier — it
	// returns the same terse `rate_limit_error: "Error"` shape it uses for
	// real throttling, which trips up callers that aren't `claude` itself.
	// Inject the identifier when missing so plain curl, OpenAI-compatible
	// SDKs, etc. work through the proxy.
	if s.cfg.Upstream.InjectClaudeCodeSystem {
		if rewritten, changed, err := injectClaudeCodeSystem(bodyBytes); err != nil {
			log.Printf("proxy: system-prompt injection skipped: %v", err)
		} else if changed {
			bodyBytes = rewritten
		}
	}

	start := time.Now()
	resp, accountID, err := s.upstream.Forward(r.Context(), "/v1/messages", bytes.NewReader(bodyBytes), upstream.ForwardExtra{
		ExtraBeta:   r.Header.Get("anthropic-beta"),
		UserAgent:   passthroughUserAgent(r.Header.Get("User-Agent")),
		ContentType: r.Header.Get("Content-Type"),
	})
	if err != nil {
		writeAnthropicError(w, http.StatusBadGateway, "api_error", "upstream: "+err.Error())
		s.recorder.Record(stats.Event{
			Timestamp: time.Now(), UserID: id.UserID, KeyID: id.KeyID, AccountID: accountID,
			Model: parsed.Model, Latency: time.Since(start),
			Status: http.StatusBadGateway, Error: err.Error(),
		})
		return
	}
	defer resp.Body.Close()

	for k, vs := range resp.Header {
		// Strip hop-by-hop headers; copy the rest.
		switch strings.ToLower(k) {
		case "connection", "keep-alive", "transfer-encoding", "upgrade":
			continue
		}
		for _, v := range vs {
			w.Header().Add(k, v)
		}
	}
	w.WriteHeader(resp.StatusCode)

	cap := streamAndCapture(w, resp.Body, isSSE(resp))

	// If upstream returned a non-2xx, surface enough context (status, the
	// Anthropic request_id, error.type and message) into both the server
	// log and the persisted usage row so the operator can chase it down.
	// Anthropic's OAuth-bearer rate-limit responses commonly come back as
	// {"type":"error","error":{"type":"rate_limit_error","message":"Error"},
	// "request_id":"req_..."} — the message is intentionally terse, so the
	// request_id is the only useful breadcrumb upstream support can act on.
	errSummary := ""
	if resp.StatusCode >= 400 || cap.ErrorType != "" {
		retryAfter := resp.Header.Get("retry-after")
		log.Printf("upstream error: model=%s user=%d account=%d status=%d type=%s request_id=%s retry_after=%s message=%q",
			parsed.Model, id.UserID, accountID, resp.StatusCode, cap.ErrorType, cap.RequestID, retryAfter, cap.ErrorMessage)
		errSummary = fmt.Sprintf("%s: %s", nzs(cap.ErrorType, "upstream_error"), nzs(cap.ErrorMessage, http.StatusText(resp.StatusCode)))
		if cap.RequestID != "" {
			errSummary += " (req_id=" + cap.RequestID + ")"
		}
	}

	s.recorder.Record(stats.Event{
		Timestamp:           time.Now(),
		UserID:              id.UserID,
		KeyID:               id.KeyID,
		AccountID:           accountID,
		Model:               parsed.Model,
		InputTokens:         cap.Usage.InputTokens,
		OutputTokens:        cap.Usage.OutputTokens,
		CacheReadTokens:     cap.Usage.CacheReadInputTokens,
		CacheCreationTokens: cap.Usage.CacheCreationInputTokens,
		Latency:             time.Since(start),
		Status:              resp.StatusCode,
		Error:               errSummary,
	})
}

// ApplyEffort is the exported counterpart of applyEffort. It rewrites a
// /v1/messages body in the same way the proxy does at runtime, suitable
// for `cliproxy effort expand` dry-run previews. See applyEffort for
// the precedence rules.
func ApplyEffort(body []byte, levels map[string]config.EffortLevel) ([]byte, bool, error) {
	return applyEffort(body, levels)
}

// applyEffort consumes a top-level `effort` field in /v1/messages bodies
// (an OpenAI-compatible alias for choosing a token budget tier) and
// expands it into Anthropic's native parameters. The mapping comes from
// upstream.effort_levels in config — see config.Default for the
// distribution of tiers (auto/low/medium/high/xhigh/max).
//
// Rules:
//   - No `effort` field            → no-op, returns (body, false, nil).
//   - Unknown effort string        → error with the list of known tiers.
//   - max_tokens already in body   → kept; effort never overwrites it.
//   - thinking already in body     → kept; effort never overwrites it.
//   - The `effort` field itself is always stripped before forwarding —
//     Anthropic doesn't recognise it and would 400 the request.
func applyEffort(body []byte, levels map[string]config.EffortLevel) ([]byte, bool, error) {
	var raw map[string]json.RawMessage
	if err := json.Unmarshal(body, &raw); err != nil {
		return body, false, nil // let caller's existing parser surface the JSON error
	}
	effortRaw, ok := raw["effort"]
	if !ok || len(effortRaw) == 0 || bytes.Equal(effortRaw, []byte("null")) {
		return body, false, nil
	}
	var effortStr string
	if err := json.Unmarshal(effortRaw, &effortStr); err != nil {
		return body, false, fmt.Errorf("effort must be a string (one of %s)", knownEffortLevels(levels))
	}
	level, ok := levels[strings.ToLower(strings.TrimSpace(effortStr))]
	if !ok {
		return body, false, fmt.Errorf("unknown effort %q; valid values: %s", effortStr, knownEffortLevels(levels))
	}
	if _, has := raw["max_tokens"]; !has && level.MaxTokens > 0 {
		b, _ := json.Marshal(level.MaxTokens)
		raw["max_tokens"] = b
	}
	if _, has := raw["thinking"]; !has && level.ThinkingBudget > 0 {
		thinkingBlock, _ := json.Marshal(map[string]any{
			"type":          "enabled",
			"budget_tokens": level.ThinkingBudget,
		})
		raw["thinking"] = thinkingBlock
	}
	delete(raw, "effort")
	out, err := json.Marshal(raw)
	if err != nil {
		return body, false, err
	}
	return out, true, nil
}

func knownEffortLevels(levels map[string]config.EffortLevel) string {
	names := make([]string, 0, len(levels))
	for k := range levels {
		names = append(names, k)
	}
	sort.Strings(names)
	return strings.Join(names, ", ")
}

// handleCountTokens proxies POST /v1/messages/count_tokens through the
// same auth + header + Claude-Code-system pipeline as /v1/messages, but
// without policy gating or stats recording (count_tokens is metadata,
// not a billable inference). The body is forwarded byte-for-byte after
// the same system-prompt rewrite, since the count_tokens endpoint
// trips the same anti-abuse signal when the identifier is missing.
func (s *Server) handleCountTokens(w http.ResponseWriter, r *http.Request) {
	bodyBytes, err := io.ReadAll(io.LimitReader(r.Body, 16<<20))
	if err != nil {
		writeAnthropicError(w, http.StatusBadRequest, "invalid_request_error", "read body: "+err.Error())
		return
	}
	if s.cfg.Upstream.InjectClaudeCodeSystem {
		if rewritten, changed, ierr := injectClaudeCodeSystem(bodyBytes); ierr != nil {
			log.Printf("count_tokens: system-prompt injection skipped: %v", ierr)
		} else if changed {
			bodyBytes = rewritten
		}
	}
	resp, _, err := s.upstream.Forward(r.Context(), "/v1/messages/count_tokens", bytes.NewReader(bodyBytes), upstream.ForwardExtra{
		ExtraBeta:   r.Header.Get("anthropic-beta"),
		UserAgent:   passthroughUserAgent(r.Header.Get("User-Agent")),
		ContentType: r.Header.Get("Content-Type"),
	})
	if err != nil {
		writeAnthropicError(w, http.StatusBadGateway, "api_error", "upstream: "+err.Error())
		return
	}
	defer resp.Body.Close()
	for k, vs := range resp.Header {
		switch strings.ToLower(k) {
		case "connection", "keep-alive", "transfer-encoding", "upgrade":
			continue
		}
		for _, v := range vs {
			w.Header().Add(k, v)
		}
	}
	w.WriteHeader(resp.StatusCode)
	if _, copyErr := io.Copy(w, resp.Body); copyErr != nil {
		log.Printf("count_tokens: copy body: %v", copyErr)
	}
}

// handleModelsList answers GET /v1/models from the in-memory catalog the
// proxy already maintains, returning the Anthropic-compatible page shape
// `{"data":[{type,id,display_name,created_at}],"has_more":false}`. We
// serve it ourselves rather than passing through so `claude` clients can
// list models even when their network can't reach api.anthropic.com
// directly — the catalog is refreshed from upstream every 30 minutes.
func (s *Server) handleModelsList(w http.ResponseWriter, r *http.Request) {
	if s.catalog == nil {
		writeAnthropicError(w, http.StatusServiceUnavailable, "api_error", "model catalog disabled")
		return
	}
	models := s.catalog.Models()
	data := make([]map[string]any, 0, len(models))
	for _, m := range models {
		row := map[string]any{
			"type":         "model",
			"id":           m.ID,
			"display_name": m.DisplayName,
		}
		if !m.CreatedAt.IsZero() {
			row["created_at"] = m.CreatedAt.Format(time.RFC3339)
		}
		data = append(data, row)
	}
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(map[string]any{
		"data":     data,
		"has_more": false,
	})
}

// claudeCodeSystemIdentifier is the literal first system block Anthropic's
// OAuth Pro/Max endpoint expects. Verified empirically: omitting it makes
// the upstream return `rate_limit_error: "Error"` regardless of actual
// quota. Real Claude Code always sends this as the very first text block.
const claudeCodeSystemIdentifier = "You are Claude Code, Anthropic's official CLI for Claude."

// injectClaudeCodeSystem rewrites a /v1/messages request body so its
// `system` field starts with the Claude Code identifier. Behaviour:
//
//   - No `system` field          → adds `[{type:text,text:identifier}]`.
//   - String                     → converts to a 2-element array: identifier
//                                  first, original text second.
//   - Array of content blocks    → if the first block already begins with
//                                  the identifier, returns the body
//                                  untouched; otherwise prepends the
//                                  identifier as a new block.
//
// Returns (newBody, changed, err). When err != nil the caller should keep
// using the original body; we log and forward as-is rather than rejecting
// the user's request.
func injectClaudeCodeSystem(body []byte) ([]byte, bool, error) {
	var raw map[string]json.RawMessage
	if err := json.Unmarshal(body, &raw); err != nil {
		return body, false, err
	}

	identifierBlock := map[string]string{"type": "text", "text": claudeCodeSystemIdentifier}
	identifierBlockJSON, _ := json.Marshal(identifierBlock)

	existing, hasSystem := raw["system"]
	var newSystemArr []json.RawMessage

	switch {
	case !hasSystem || len(existing) == 0 || bytes.Equal(existing, []byte("null")):
		newSystemArr = []json.RawMessage{identifierBlockJSON}

	case existing[0] == '[':
		var arr []json.RawMessage
		if err := json.Unmarshal(existing, &arr); err != nil {
			return body, false, fmt.Errorf("system is array but unparseable: %w", err)
		}
		// If the first text block already starts with our identifier, the
		// caller (almost certainly `claude` itself) has done this for us.
		if len(arr) > 0 {
			var first struct {
				Type string `json:"type"`
				Text string `json:"text"`
			}
			if json.Unmarshal(arr[0], &first) == nil && first.Type == "text" && strings.HasPrefix(first.Text, claudeCodeSystemIdentifier) {
				return body, false, nil
			}
		}
		newSystemArr = append([]json.RawMessage{identifierBlockJSON}, arr...)

	case existing[0] == '"':
		var s string
		if err := json.Unmarshal(existing, &s); err != nil {
			return body, false, fmt.Errorf("system is string but unparseable: %w", err)
		}
		if strings.HasPrefix(s, claudeCodeSystemIdentifier) {
			return body, false, nil
		}
		userBlock := map[string]string{"type": "text", "text": s}
		userBlockJSON, _ := json.Marshal(userBlock)
		newSystemArr = []json.RawMessage{identifierBlockJSON, userBlockJSON}

	default:
		return body, false, fmt.Errorf("system field is neither array nor string nor null")
	}

	encoded, err := json.Marshal(newSystemArr)
	if err != nil {
		return body, false, err
	}
	raw["system"] = encoded

	out, err := json.Marshal(raw)
	if err != nil {
		return body, false, err
	}
	return out, true, nil
}

func nzs(v, fallback string) string {
	if strings.TrimSpace(v) == "" {
		return fallback
	}
	return v
}

type usageBlock struct {
	InputTokens              int64 `json:"input_tokens"`
	OutputTokens             int64 `json:"output_tokens"`
	CacheCreationInputTokens int64 `json:"cache_creation_input_tokens"`
	CacheReadInputTokens     int64 `json:"cache_read_input_tokens"`
}

// captureResult is what streamAndCapture returns to the caller — token
// usage plus, when present, the Anthropic-shaped error envelope so we can
// log/record it without re-parsing the body.
type captureResult struct {
	Usage        usageBlock
	ErrorType    string
	ErrorMessage string
	RequestID    string
}

func isSSE(resp *http.Response) bool {
	return strings.HasPrefix(resp.Header.Get("Content-Type"), "text/event-stream")
}

// streamAndCapture copies the upstream body to the client while extracting
// token usage from either SSE message_start/message_delta events or the
// final JSON body for non-stream responses, plus any Anthropic error
// envelope that may be in either path.
func streamAndCapture(w http.ResponseWriter, body io.Reader, sse bool) captureResult {
	flusher, _ := w.(http.Flusher)
	if !sse {
		// Buffer the full body so we can both forward it and parse usage.
		buf, err := io.ReadAll(body)
		if err != nil {
			log.Printf("proxy: read upstream: %v", err)
			return captureResult{}
		}
		_, _ = w.Write(buf)
		var parsed struct {
			Usage usageBlock `json:"usage"`
			Type  string     `json:"type"`
			Error struct {
				Type    string `json:"type"`
				Message string `json:"message"`
			} `json:"error"`
			RequestID string `json:"request_id"`
		}
		_ = json.Unmarshal(buf, &parsed)
		out := captureResult{Usage: parsed.Usage, RequestID: parsed.RequestID}
		if parsed.Type == "error" || parsed.Error.Type != "" {
			out.ErrorType = parsed.Error.Type
			out.ErrorMessage = parsed.Error.Message
		}
		return out
	}

	var cap captureResult
	reader := bufio.NewReaderSize(body, 64<<10)
	currentEvent := ""
	for {
		line, err := reader.ReadBytes('\n')
		if len(line) > 0 {
			if _, werr := w.Write(line); werr != nil {
				return cap
			}
			if flusher != nil && (bytes.Equal(line, []byte("\n")) || bytes.Equal(line, []byte("\r\n"))) {
				flusher.Flush()
			}
			parseSSELine(line, &currentEvent, &cap)
		}
		if err != nil {
			if !errors.Is(err, io.EOF) {
				log.Printf("proxy: stream read: %v", err)
			}
			return cap
		}
	}
}

func parseSSELine(line []byte, currentEvent *string, cap *captureResult) {
	trim := bytes.TrimRight(line, "\r\n")
	if len(trim) == 0 {
		*currentEvent = ""
		return
	}
	if bytes.HasPrefix(trim, []byte("event:")) {
		*currentEvent = strings.TrimSpace(string(trim[len("event:"):]))
		return
	}
	if !bytes.HasPrefix(trim, []byte("data:")) {
		return
	}
	payload := bytes.TrimSpace(trim[len("data:"):])
	if len(payload) == 0 || bytes.Equal(payload, []byte("[DONE]")) {
		return
	}
	switch *currentEvent {
	case "message_start":
		var ev struct {
			Message struct {
				Usage usageBlock `json:"usage"`
				ID    string     `json:"id"`
			} `json:"message"`
		}
		if err := json.Unmarshal(payload, &ev); err == nil {
			if ev.Message.Usage.InputTokens > 0 {
				cap.Usage.InputTokens = ev.Message.Usage.InputTokens
			}
			if ev.Message.Usage.CacheCreationInputTokens > 0 {
				cap.Usage.CacheCreationInputTokens = ev.Message.Usage.CacheCreationInputTokens
			}
			if ev.Message.Usage.CacheReadInputTokens > 0 {
				cap.Usage.CacheReadInputTokens = ev.Message.Usage.CacheReadInputTokens
			}
		}
	case "message_delta":
		var ev struct {
			Usage usageBlock `json:"usage"`
		}
		if err := json.Unmarshal(payload, &ev); err == nil {
			if ev.Usage.OutputTokens > 0 {
				cap.Usage.OutputTokens = ev.Usage.OutputTokens
			}
			if ev.Usage.InputTokens > 0 {
				cap.Usage.InputTokens = ev.Usage.InputTokens
			}
		}
	case "error":
		// Anthropic emits SSE error events as
		// `event: error\ndata: {"type":"error","error":{"type":"...","message":"..."},"request_id":"..."}`.
		var ev struct {
			Error struct {
				Type    string `json:"type"`
				Message string `json:"message"`
			} `json:"error"`
			RequestID string `json:"request_id"`
		}
		if err := json.Unmarshal(payload, &ev); err == nil {
			if ev.Error.Type != "" {
				cap.ErrorType = ev.Error.Type
				cap.ErrorMessage = ev.Error.Message
			}
			if ev.RequestID != "" {
				cap.RequestID = ev.RequestID
			}
		}
	}
}
