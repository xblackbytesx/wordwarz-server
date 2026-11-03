# WordWarZ server: agent guide and decision register

WordWarZ is a family word game: letter tiles on a board with premium squares,
two to four players, asynchronous turns, Dutch or English (or both) per game.
No ads, no tracking, no profiling. This repo holds the rules engine, the word
lists, the server (accounts, games, turns, nudges, scoreboard, website) and
the FFI crate the app runs the engine through. The Android app lives in `/workspace/wordwarz` (its own `AGENTS.md`).

This file is the contract for both repos. Record decisions here when they are
made, with the date and the reason, and keep it true to the code. Never use
em dashes.

## Build and verify

Everything runs through `make` (docker compose); no host toolchain is needed.

| Target | What |
|---|---|
| `make dev` | bacon hot reload + Postgres 17 on `${PORT:-8080}` |
| `make test` | `cargo test --workspace --release`: rules, bot games, and Postgres integration tests |
| `make lint` | `cargo fmt --check` + `clippy -D warnings` (the CI gate) |
| `make lexicons` | re-download the pinned word lists, rebuild `lexicons/*.fst` and the tile sets |
| `make android-core` | `wordwarz-ffi` for Android (cargo-ndk) + Kotlin bindings + the word lists as app assets, into `dist/` (the app's `make core`) |
| `make image` / `up` / `down` / `logs` | build and smoke-test the production image locally |
| `make admin NAME=..` / `password NAME=..` / `invite NAME=..` | first account, its passphrase (stdin), an invite link |

CI: `.github/workflows/ci.yml` runs fmt, clippy and the tests (Postgres
service on `localhost`) on every push; no `container:`, Rust from
`rust-toolchain.toml`. `release.yml` on a `vX.Y.Z` tag runs the same gate,
checks the tag equals `[workspace.package] version`, pushes the amd64 + arm64
image `ghcr.io/xblackbytesx/wordwarz-server` and creates the GitHub release.
**GitHub Actions and GHCR, not Gitea (decided 2026-11-03):** the server runs
outside the home network, so its image must be pullable from anywhere. The
GHCR package must be public (package settings, once after the first
release), or every host pulling it needs a registry login.
`deploy/` is the production example (Traefik, Postgres), with its README.

Agent sandbox fallback (no Docker): rustup toolchain `1.92.0`, a scratch
`CARGO_TARGET_DIR` outside the repo, and a throwaway Postgres 17 from the
zonky embedded-postgres binaries; point `WORDWARZ_TEST_DATABASE_URL` at its
`postgres` database. Integration tests create and drop their own database
and print `SKIP` when the variable is unset. Never relax a test to make it pass.

## Crates

| Crate | Role | Rule |
|---|---|---|
| `crates/core` (`wordwarz-core`) | letters, board layouts, tile sets, lexicons, move validation and scoring, the game state machine, the move generator | Pure: no IO, no clock, no ambient randomness (the game seed drives `rng`) |
| `crates/lexicon` (`wordwarz-lexicon`) | builds `lexicons/*.fst`, derives the tile sets into `tilesets.rs` | Tool only; never in the server image |
| `crates/server` (`wordwarz-server`) | axum + sqlx Postgres + askama/HTMX, `/api/v1`, turn timers, push | Runtime sqlx queries only (no `query!`), so builds never need a database |
| `crates/ffi` (`wordwarz-ffi`) | UniFFI 0.32 bindings for the app: `Lexicons`, `LocalGame`, `preview`, `best_moves`, presets, tile sets | Thin: JSON in and out (the core's own shapes), refusals as `Outcome` data, `flat_error` for real errors |

Pins: rust 1.92, fst 0.4, and slopstream-server's: axum 0.8, sqlx 0.8 (0.9
needs rustc 1.94), askama 0.16, reqwest 0.13 on rustls/ring (aws-lc needs
cmake in every build image), qrcode 0.14 (SVG only). HTMX 2.0.4 vendored
(`static/htmx.min.js`, sha256
`e209dda5c8235479f3166defc7750e1dbcd5a5c1808b7792fc2e6733768fb447`).

Core modules: `letters` (Letter, Tile, PlacedTile), `board` (Layout, Board,
Pos, Dir), `tiles` (TileSet::derive), `tilesets` (generated), `lexicon`
(normalize_word, Lexicon, Dictionary), `config` (GameConfig, presets),
`moves` (evaluate), `game` (Game, Action, GameView), `movegen`
(best_moves), `bot` (the practice opponent's pick), `rng` (SplitMix64).

Server modules: `users` (usernames, invites, device links, tokens), `games`
(create, act, list, nudge, timers, results, account deletion), `push`
(UnifiedPush), `stats` (scoreboard), `lexicons` (embedded `.fst`), `changes`
(long-poll), `api` (`/api/v1`), `web/*` (pages), `migrations/` (numbered SQL,
embedded, run at every start).

## Decisions

Settled 2026-11-02 (do not reopen):

1. **Stack and repos.** `wordwarz-server` (this repo: core + server, Rust +
   HTMX, GPL-3.0-or-later) and `wordwarz` (Android, Kotlin + Compose, kagami
   and slopstream conventions). Same build and distribution as slopstream and
   slopstream-server: containerised builds, CI on push, images/APKs on tags.
2. **Our own game in the genre, not a Wordfeud clone.** Own name, own
   premium layouts (`board.rs`: one octant per size, mirrored, so every
   layout is symmetric on both axes and diagonals), own tile sets, own look.
   Boards are 11, 15 or 19 squares; the centre is a plain start square.
3. **Tile sets are derived, then frozen.** `TileSet::derive` shares 100
   letter tiles by letter frequency in the lexicon's words of up to 8
   letters (largest remainder, each letter 1 to 16), values on a log scale
   from 1 (most common) to 10 (rarest), plus 2 blanks. The output is
   generated into `tilesets.rs` and never regenerated in place: a changed set
   changes stored bags, so it needs a new `RULES_VERSION`. Known quirk:
   English gets 9 S tiles (ENABLE's plurals); left as derived until play
   testing says otherwise.
4. **Lexicons.** OpenTaal (Dutch, BSD-3/CC BY 3.0) and ENABLE (English,
   public domain), pinned by commit and sha256 in `lexicons/SOURCES.md`,
   built into `fst` sets and checked in (2 MiB). The server embeds them; the
   app ships them as assets and passes the bytes to the core. Licensed lists
   (Collins, NWL) are out.
5. **Our own RNG** (`rng.rs`, SplitMix64 + Fisher-Yates) with separate
   derived streams for board, bounties and bag. The `rand` crates may change
   algorithms between versions; a stored game must replay to the same bag
   forever. `stream_is_stable` pins the output.
6. **A game is config + seed + action log.** The server stores those (the
   seed from its CSPRNG, never sent to clients) and may cache the `Game`
   JSON. `Game::replay` rebuilds without a dictionary: words were checked when
   played, so a later word list update never breaks old games. Hints are
   logged actions too (they spend tokens).
7. **Alphabet A to Z for every language.** Diacritics fold (`ideeën` is
   IDEEEN), IJ is two tiles, proper nouns, abbreviations and words with
   hyphens or apostrophes are dropped (`normalize_word`). Words are 2 to 19
   letters.
8. **Bilingual games** (`Language::Bilingual`): a word counts if it is Dutch
   or English. The dictionary walks both tries side by side; the tile set is
   derived from the averaged frequencies.
9. **Presets plus modifiers.** The new-game screen starts from a preset and
   every field stays tweakable. Classic: plain rules, fixed board. Family:
   3 hints, comeback (+25 % when 50 behind), 2v2 teams with four players.
   Chaos: random board, 4 hidden bounties, comeback (+50 % when 40 behind).
   Modifiers in rules version 1: board size, random board, per-seat rack size
   (5 to 9) and score percent (50 to 200), teams, bingo bonus, hint tokens,
   hidden bounties, comeback.
10. **Handicaps are visible.** Every player sees every seat's rack size and
    score percent; a handicap is a family agreement, not a secret.
11. **The server is authoritative.** Clients send actions and receive a
    `GameView`: no bag order, no seed, no hidden racks (team partners see
    each other's), no unclaimed bounties. Unseen tiles are tallied for tile
    tracking. The app's live score uses `GameView::preview`, the same scoring
    code minus bounties (they stay hidden until played).

## Rules (rules version 1)

- Turn order is seat order; the server shuffles seats when a game starts.
- First play covers the centre; later plays touch an existing tile. All new
  tiles in one row or column without gaps. Every word formed (main and cross,
  2+ letters) must be in the game's dictionary.
- Letter premiums multiply a new tile's value, word premiums the word; both
  count only under tiles placed this turn. Blanks are worth 0.
- Bingo: 7 or more tiles in one move adds `bingoBonus` (default 40).
- Score of a move: words + bingo + bounties, then the seat's score percent
  and the comeback bonus, both on that subtotal, rounded half up.
- Swap: any number of rack tiles, only while the bag holds `swapMinBag`
  (default 7). New tiles are drawn before the old go back.
- End: a player empties their rack with the bag empty (they gain the others'
  rack values, the others lose them); or 2 rounds of scoreless turns in a row
  (passes, swaps, timeouts; everyone loses their rack value); or resignations
  leave one player or team. In 3 and 4 player games a resigning player's
  tiles return to the bag and the rest play on; in team games one partner
  resigning ends it for the team. Ties share the win.
- Hints spend a token on your own turn and return the highest-scoring move
  (base score only, so they never reveal bounties).

## Server (decided and built 2026-11-02)

12. **Accounts are a username.** 3 to 20 of `a-z 0-9 _`, starting with a
    letter, case-insensitive. Display name (30 characters) and passphrase
    (Argon2id, 10+ characters) are optional. No email, real name, birth date
    or phone, ever. Login answers the same for unknown, disabled,
    passphrase-less and wrong, and always pays one Argon2 verification.
13. **Invite-only; only admins invite** (changed 2026-11-03 from "any
    member", so children cannot bring in classmates). A one-time link with a
    QR code: a code shown for someone in the room (`inPerson`, the app's
    default and the website's "Scan now") lives 10 minutes, so a photo of it
    is soon useless; a link to send by message lives 7 days. The app asks
    for the owner's fingerprint or screen lock before it shows any invite or
    device sign-in code. In the app, `POST /join` with the
    scanned invite and a username creates the account and a device token at
    once (no passphrase). On the website a passphrase is required, since it is
    how the website logs in.
14. **Device links instead of pairing codes** (no TVs here): a signed-in
    device or the Devices page shows a one-time QR code (10 minutes); the new
    device scans it and gets its own token. Tokens are per device, revocable;
    disabling an account stops them all.
15. **No server secret.** Only hashes are stored (passphrases, tokens,
    invites, links), so there is no key to configure, lose or rotate.
16. **Seats are shuffled** from OS randomness; per-seat handicaps travel with
    their person, and team games keep partners on alternate seats (0 and 2
    against 1 and 3) with a random team starting. The game seed is OS
    randomness too, stored, never sent.
17. **Actions run under a row lock** (`SELECT ... FOR UPDATE` on the game):
    apply with the core, append to `game_actions`, replace the cached state,
    and on the last move write `game_results`, all in one transaction.
    Refusals are `422` with `{"error", "rule"}` where `rule` is the core's
    `ActionError` as JSON: `{"error": "notYourTurn"}`, or for a move
    `{"error": "move", "reason": "notWords", "words": [...]}` (MoveError has
    its own tag, `reason`, so the two never collide; pinned by
    `rule_errors_have_a_stable_json_shape`).
18. **Turn timer** per game: off, 24, 72 or 168 hours. The timer task (every
    minute) applies `Timeout`; a seat whose previous own turn was also a
    timeout is resigned instead.
19. **Nudges**: only to the player whose turn it is, one per sender per game
    per 4 hours (`429` otherwise).
20. **Push is UnifiedPush, declared hosts only.** `WORDWARZ_PUSH_HOSTS` lists
    the push servers (the family's ntfy); endpoints elsewhere are refused at
    registration and again at send time, redirects are off, and a `404`/`410`
    drops the endpoint. The body is `{"event", "game"}` only (`turn`,
    `nudge`, `newGame`, `finished`). Open apps long-poll `GET /changes`.
    **Encrypted (2026-11-03):** devices register their WebPush keys with the
    endpoint, and every push is RFC 8291 `aes128gcm` to them (`webpush.rs`,
    RustCrypto p256/hkdf/aes-gcm, pinned byte for byte by the RFC's test
    vector). The push server relays ciphertext; the app drops anything that
    does not decrypt, so knowing an endpoint is not enough to fake a push.
    Endpoints without keys (old connectors) get the plaintext body.
21. **Scoreboard** from `game_results`: games, wins, win rate, average score,
    best move and word, bingos per person; head to head per pair (team
    partners are not rivals) decided by the win flag, then not resigning,
    then score; the 30 latest matches. Computed per request (family scale).
22. **Website is read-only for games**: lists, a board page that steps
    through moves by replaying the log (`?move=N`), the scoreboard. Players
    see their own games; any member may look at a finished game. Racks are
    never shown on the website.
23. **Deleting an account** resigns its running games first; finished games
    keep the seat as "deleted" (`user_id` NULL). The only admin cannot leave
    while others remain. Export (`/me/export`, Settings) gives everything the
    server holds as JSON.
24. **Privacy**: no third-party scripts, fonts, analytics or crash
    reporting; the CSP allows only `'self'`. The server talks to nothing but
    the declared push hosts.

### API (`/api/v1`, bearer token unless noted)

| Route | What |
|---|---|
| `POST /join` (no token) | `{invite, username, displayName?, passphrase?, device}` -> `{token, me}` |
| `POST /login` (no token) | `{username, passphrase, device}` -> `{token, me}` |
| `POST /link/claim` (no token) | `{code, device}` -> `{token, me}` |
| `POST /link` | a device-link QR payload `{url, expiresAt}` |
| `POST /logout` | revoke this device's token (and its push endpoint) |
| `GET/PATCH/DELETE /me`, `GET /me/export` | profile, display name, deletion, export |
| `GET /people` | everyone, to pick opponents |
| `POST /invites` | admins only; `{note?, admin?, inPerson?}` -> `{url, expiresAt}` (10 minutes in person, else 7 days) |
| `GET /games`, `POST /games` | list; create `{config, seats: [{userId, rackSize?, scorePercent?, team?}], turnTimerHours?}` |
| `GET /games/{id}` | `{id, rev, players, turnTimerHours, turnStartedAt, createdAt, view}` |
| `POST /games/{id}/actions` | a core `Action` -> `{game, turn?, hint?}` |
| `POST /games/{id}/nudge` | `204`, or `429` |
| `GET /scoreboard` | `{players, headToHead, recent}` |
| `GET /changes?since=N` | long-poll, `{generation, changed}` within 25 s |
| `PUT/DELETE /push` | `{url, pubKey?, auth?}`: this device's UnifiedPush endpoint and WebPush keys (both or neither) |
| `PUT /me/passphrase` | `{current?, passphrase}`: the first needs no current one; changing does |

## Planned: app (`/workspace/wordwarz`)

- Kotlin + Compose, kagami/slopstream conventions, minSdk to match BSfreeTV
  and slopstream; no Play Services (GrapheneOS).
- The core via UniFFI (`crates/ffi`), lexicons as assets: live score
  preview and word check while dragging tiles, offline pass-and-play.
- Feel: spring drag and snap, tiles flip in for the opponent's move, the score
  counts up and flies to the badge, a particle burst for a bingo; wooden tile
  sounds with pitch variation, a chime per premium; haptics from Android's
  composition primitives (tick on pick-up, click on snap, thud on submit, a
  rising sequence for a bingo) with a fallback where they are unsupported.

## Roadmap

1. The app's online play (milestone 2; pass-and-play is built, see the app's AGENTS.md).
2. Polish pass (sound, haptics, animation) and the About screen attribution.
3. Later modifiers (each a new rules version where it changes replays):
   letter market (pick 1 of 3 face-up tiles), daily puzzle (same rack and
   board for everyone, best score of the day), live blitz with chess clocks,
   a server-side practice bot (the core's `bot` already plays in the app).
4. Chat and taunts: a per-game message table with a fixed set of emoji
   taunts first (no free text from children to moderate).
5. Encrypted pushes need a WebPush-capable distributor; if a family phone's
   distributor sends no keys, consider refusing plaintext endpoints.

### FFI and android-core (decided 2026-11-02)

25. **JSON across the boundary.** The app gets views, actions, configs and
    errors as the core's JSON, the same shapes `/api/v1` sends, so one set of
    Kotlin models serves local and online games and no wire shape is
    duplicated in UniFFI records. `LocalGame` holds the full state (bag
    included) for pass-and-play; the app only renders views.
26. **`docker/build-android-core.sh`** writes `dist/` (gitignored): the
    stripped `.so` for arm64-v8a and x86_64 (API 30, the app's minSdk),
    bindings generated from the UNSTRIPPED debug build (library mode needs
    the metadata), the host `.so` for the app's JVM tests, and
    `assets/lexicons/` (both `.fst` plus licence and sources). `HOST_ONLY=1`
    skips cargo-ndk (the app's CI and the agent sandbox). Toolchain pins as
    quantifyme-v2: NDK r27d, cargo-ndk 4.1.2.
27. **The practice bot** (`core::bot`): easy picks from the lower half of up
    to 60 candidates, medium from the best quarter, hard the best;
    deterministic per seed.

## Invariants

- **Replays are exact**: `Game::replay(config, seed, log)` equals the live
  game (`bot_games_in_every_mode_replay_exactly`). Anything that changes
  the result of a stored log needs a new `RULES_VERSION` with the old
  behaviour kept for old games.
- **Tiles are conserved**: board + racks + bag = the tile set's total after
  every action (checked in the bot games).
- **The preview equals the server's score** minus bounties
  (`the_view_previews_what_the_server_scores`).
- **Every generated move is legal by the full check** (checked in the bot games).
- **Views never leak**: no bag order, no hidden racks, no unclaimed bounties
  (`views_hide_what_they_should`).
- **Every action is serialized per game** (row lock) and takes its log index
  inside it.
- **Views never leak over the API either**: no `seed`, no `bag`
  (`a_whole_game_over_the_api_...`), and push bodies carry only an event and
  a game id (`nudges_push_only_an_event_...`).
- **The wire contract is golden** (`tests/golden/*.json`, `golden_pg`): the
  app's `ApiContractTest` parses every file with its models. Without
  `UPDATE_GOLDEN=1` the live responses must keep the stored shapes (variants
  of a tagged union and the optional `blank` key aside); secrets are
  redacted to full-length `example000...` placeholders. Regenerate only on
  purpose, then update the app's models in the same change.
- **People only reach their own games** over the API (`404` otherwise), and
  tokens only ever act as their owner.
- Strict CSP: no inline script or style anywhere (QR codes are inline SVG
  with attributes, no style); the page test checks every page. Writes from
  browsers need a same-origin `Origin`, plus `SameSite=Strict` cookies;
  referrer policy stays `same-origin`.
- Logs never contain racks, words in progress, tokens or push URLs.
- The last active admin cannot demote, disable or delete themselves while
  others remain.
- A new crate needs a `COPY crates/<name>/Cargo.toml` line in
  `docker/Dockerfile` (CI checks it); the release tag must equal
  `[workspace.package] version`.
- Anchored edits by tools fail loudly: `wordwarz-lexicon tiles` refuses to
  write unless the `@generated` markers occur exactly once.
- The owner does all git; never commit, tag or push.
