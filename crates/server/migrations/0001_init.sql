-- People, logins, device tokens, invites and device links.

CREATE TABLE users (
    id            uuid PRIMARY KEY,
    -- Lowercase a-z, 0-9 and _, 3 to 20 characters. The only required field.
    username      text NOT NULL UNIQUE,
    display_name  text NOT NULL DEFAULT '',
    role          text NOT NULL DEFAULT 'user' CHECK (role IN ('admin', 'user')),
    -- NULL: no passphrase; the person signs in with device tokens only.
    password_hash text,
    tz            text NOT NULL DEFAULT 'Europe/Amsterdam',
    created_at    timestamptz NOT NULL DEFAULT now(),
    disabled_at   timestamptz
);

-- Browser sessions: only the SHA-256 of the cookie value is stored.
CREATE TABLE web_sessions (
    token_sha256  bytea PRIMARY KEY,
    user_id       uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at    timestamptz NOT NULL DEFAULT now(),
    expires_at    timestamptz NOT NULL
);
CREATE INDEX web_sessions_user ON web_sessions (user_id);

-- Bearer tokens for /api/v1, one per device. Hash only.
CREATE TABLE api_tokens (
    id           uuid PRIMARY KEY,
    user_id      uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    label        text NOT NULL,
    token_sha256 bytea NOT NULL UNIQUE,
    created_at   timestamptz NOT NULL DEFAULT now(),
    last_used_at timestamptz,
    revoked_at   timestamptz
);
CREATE INDEX api_tokens_user ON api_tokens (user_id);

-- One-time invite links (and their QR codes). Any member can make one.
CREATE TABLE invites (
    id           uuid PRIMARY KEY,
    token_sha256 bytea NOT NULL UNIQUE,
    role         text NOT NULL DEFAULT 'user' CHECK (role IN ('admin', 'user')),
    note         text NOT NULL DEFAULT '',
    created_by   uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at   timestamptz NOT NULL DEFAULT now(),
    expires_at   timestamptz NOT NULL,
    used_at      timestamptz,
    used_by      uuid REFERENCES users (id) ON DELETE SET NULL
);

-- One-time links that sign another device in to an existing account: the
-- signed-in device (or the website) shows a QR code, the new one scans it.
CREATE TABLE device_links (
    id           uuid PRIMARY KEY,
    code_sha256  bytea NOT NULL UNIQUE,
    user_id      uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at   timestamptz NOT NULL DEFAULT now(),
    expires_at   timestamptz NOT NULL,
    used_at      timestamptz
);

-- UnifiedPush endpoints, one per device token.
CREATE TABLE push_endpoints (
    token_id   uuid PRIMARY KEY REFERENCES api_tokens (id) ON DELETE CASCADE,
    url        text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

-- Games. `config`, `seed` and `game_actions` are the truth (AGENTS.md
-- decision 6); `state` is the core's Game, cached so a move needs no replay.
CREATE TABLE games (
    id               uuid PRIMARY KEY,
    created_by       uuid REFERENCES users (id) ON DELETE SET NULL,
    config           jsonb NOT NULL,
    seed             bigint NOT NULL,
    state            jsonb NOT NULL,
    finished         boolean NOT NULL DEFAULT false,
    -- Bumped by every action; clients compare it to skip refetches.
    rev              integer NOT NULL DEFAULT 0,
    turn_seat        integer NOT NULL DEFAULT 0,
    turn_started_at  timestamptz NOT NULL DEFAULT now(),
    -- NULL: no turn timer.
    turn_timer_hours integer,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now(),
    finished_at      timestamptz
);
CREATE INDEX games_timer ON games (turn_started_at) WHERE NOT finished AND turn_timer_hours IS NOT NULL;

-- Seats. A deleted account leaves its seat with user_id NULL ("deleted player").
CREATE TABLE game_players (
    game_id uuid NOT NULL REFERENCES games (id) ON DELETE CASCADE,
    seat    integer NOT NULL,
    user_id uuid REFERENCES users (id) ON DELETE SET NULL,
    PRIMARY KEY (game_id, seat)
);
CREATE INDEX game_players_user ON game_players (user_id);
-- One seat per person per game (deleted accounts' NULLs never clash).
CREATE UNIQUE INDEX game_players_once ON game_players (game_id, user_id);

-- The action log, in order.
CREATE TABLE game_actions (
    game_id uuid NOT NULL REFERENCES games (id) ON DELETE CASCADE,
    idx     integer NOT NULL,
    seat    integer NOT NULL,
    action  jsonb NOT NULL,
    at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (game_id, idx)
);

-- One row per seat of a finished game: what the scoreboard counts.
CREATE TABLE game_results (
    game_id    uuid NOT NULL REFERENCES games (id) ON DELETE CASCADE,
    seat       integer NOT NULL,
    user_id    uuid REFERENCES users (id) ON DELETE SET NULL,
    -- 0 or 1 in team games, else NULL: partners are not each other's rivals.
    team       integer,
    score      integer NOT NULL,
    won        boolean NOT NULL,
    resigned   boolean NOT NULL,
    moves      integer NOT NULL,
    bingos     integer NOT NULL,
    best_move  integer NOT NULL,
    best_word  text NOT NULL DEFAULT '',
    language   text NOT NULL,
    finished_at timestamptz NOT NULL,
    PRIMARY KEY (game_id, seat)
);
CREATE INDEX game_results_user ON game_results (user_id);

-- Nudges, for the rate limit.
CREATE TABLE nudges (
    game_id   uuid NOT NULL REFERENCES games (id) ON DELETE CASCADE,
    from_user uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    to_user   uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    at        timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX nudges_recent ON nudges (game_id, from_user, at);
