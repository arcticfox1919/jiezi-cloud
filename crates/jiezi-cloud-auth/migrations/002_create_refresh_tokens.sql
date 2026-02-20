-- Migration: create refresh_tokens table
--
-- Stores hashed refresh tokens for server-side revocation and rotation
-- attack detection.  The raw token is NEVER stored — only its SHA-256 hex
-- digest is persisted.
--
-- Token rotation strategy:
--   1. Each token belongs to a `family` (random UUID assigned at first login).
--   2. When a refresh succeeds the old token is marked `revoked = 1` and a
--      new token in the same family is inserted.
--   3. If a *revoked* token of a non-expired family is presented, the entire
--      family is wiped (reuse-attack detected → force re-login).

CREATE TABLE IF NOT EXISTS refresh_tokens (
    token_hash  TEXT    NOT NULL PRIMARY KEY,   -- SHA-256 hex of the raw JWT
    user_id     TEXT    NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    family      TEXT    NOT NULL,               -- rotation family UUID
    expires_at  TEXT    NOT NULL,               -- RFC-3339
    revoked     INTEGER NOT NULL DEFAULT 0,     -- 0 = valid, 1 = revoked
    created_at  TEXT    NOT NULL                -- RFC-3339
);

CREATE INDEX IF NOT EXISTS idx_refresh_tokens_user_id ON refresh_tokens (user_id);
CREATE INDEX IF NOT EXISTS idx_refresh_tokens_family  ON refresh_tokens (family);
