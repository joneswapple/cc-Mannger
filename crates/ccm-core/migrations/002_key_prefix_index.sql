-- Key lookup scans every row sharing a prefix and bcrypt-checks each, so the
-- prefix doesn't need to be unique. The Go cliproxy used an 8-character
-- prefix ("sk-cp-" + 2 chars) under a UNIQUE index, which made issuing a key
-- fail on prefix collisions after a few dozen keys. New keys use 12 chars.
DROP INDEX IF EXISTS api_keys_prefix;
CREATE INDEX api_keys_prefix ON api_keys(prefix);
