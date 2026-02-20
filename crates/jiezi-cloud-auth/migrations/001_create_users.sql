-- Migration: create users table
--
-- Stores user accounts.  Passwords are stored as Argon2id PHC strings.
-- The `role` column holds the system-level role ('owner' | 'admin' | 'member' | 'guest').
-- All timestamps are stored as RFC-3339 text in UTC.

CREATE TABLE IF NOT EXISTS users (
    id              TEXT    NOT NULL PRIMARY KEY,   -- UUIDv7 serialised as hyphenated text
    username        TEXT    NOT NULL UNIQUE,
    email           TEXT    NOT NULL UNIQUE,
    password_hash   TEXT    NOT NULL,               -- Argon2id PHC string
    role            TEXT    NOT NULL DEFAULT 'member',
    is_active       INTEGER NOT NULL DEFAULT 1,     -- 0 = suspended, 1 = active
    display_name    TEXT,
    avatar_url      TEXT,
    storage_quota   INTEGER,                        -- max bytes; NULL = unlimited
    storage_used    INTEGER NOT NULL DEFAULT 0,     -- current bytes consumed
    created_at      TEXT    NOT NULL,               -- RFC-3339
    updated_at      TEXT    NOT NULL                -- RFC-3339
);

CREATE INDEX IF NOT EXISTS idx_users_email    ON users (email);
CREATE INDEX IF NOT EXISTS idx_users_username ON users (username);
