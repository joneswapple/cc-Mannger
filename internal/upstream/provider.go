package upstream

import "context"

// CredentialProvider supplies a bearer access token to use against the
// Anthropic API. Implementations must be safe for concurrent use and handle
// refreshing themselves so callers can call Token() on every request.
type CredentialProvider interface {
	Token(ctx context.Context) (string, error)
	// Source describes where the credential came from (e.g. file path) for
	// logging.
	Source() string
}
