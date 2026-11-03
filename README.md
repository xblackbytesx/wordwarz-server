# WordWarZ server

The self-hosted server behind online play in
[WordWarZ](https://github.com/xblackbytesx/wordwarz), the family word game for
Android. Accounts, games, turns, nudges and a scoreboard, plus the rules engine
the app runs on the phone.

[![CI](https://github.com/xblackbytesx/wordwarz-server/actions/workflows/ci.yml/badge.svg)](https://github.com/xblackbytesx/wordwarz-server/actions/workflows/ci.yml)
[![Release](https://github.com/xblackbytesx/wordwarz-server/actions/workflows/release.yml/badge.svg)](https://github.com/xblackbytesx/wordwarz-server/actions/workflows/release.yml)
[![Latest release](https://img.shields.io/github/v/release/xblackbytesx/wordwarz-server?sort=semver)](https://github.com/xblackbytesx/wordwarz-server/releases/latest)
[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](LICENSE)
[![Docker image](https://img.shields.io/badge/ghcr.io-wordwarz--server-2496ED?logo=docker&logoColor=white)](https://github.com/xblackbytesx/wordwarz-server/pkgs/container/wordwarz-server)

- **Invite-only, username-only accounts**: an admin shows a one-time QR code
  (10 minutes, or a 7-day link to send); scanning it in the app creates the
  account and signs that phone in. No email, no real names, no passphrase
  needed for children.
- **Turn-based games for 2 to 4 players**, teams of two, per-player handicaps,
  turn timers that pass and then resign, and nudges (once per 4 hours).
- **Push through UnifiedPush (ntfy), never FCM**, encrypted end to end with
  WebPush (RFC 8291) so the push server relays ciphertext; only hosts you list
  may receive pushes. Open apps update within seconds by long-poll.
- **Scoreboard**: wins, averages, best words, bingos, head to head records and
  move-by-move replays of every game on the website.
- **A small website** for invites, roles, devices, game replays, data export
  and account deletion. No third-party scripts, a strict CSP.
- **One rules engine** (`crates/core`) shared with the app through UniFFI, so
  the phone's live score and the server's always agree. Dutch (OpenTaal) and
  English (ENABLE) word lists built in.

## Deploy

The image is `ghcr.io/xblackbytesx/wordwarz-server` (amd64 and arm64), with
Postgres 17 next to it. [deploy/](deploy/) has a Traefik example and the
[deploy guide](deploy/README.md): configuration, DNS, push and the first admin.

```sh
docker compose up -d
docker compose exec wordwarz-server-app wordwarz-server create-admin mum
docker compose exec -T wordwarz-server-app wordwarz-server set-password mum
```

Then log in to the app with that username and passphrase and invite the
family from **Account, Invite someone**.

## Develop

Everything runs through `make` (Docker), no host toolchain:

```
make dev            # hot reload + Postgres on :8080
make test           # rules, bot games, Postgres integration tests
make lint           # rustfmt + clippy -D warnings
make android-core   # the app's Rust core, bindings and word lists into dist/
```

The decisions, the rules, the API and the invariants live in
[AGENTS.md](AGENTS.md).

## Word lists

- Dutch: [OpenTaal](https://www.opentaal.org) word list, BSD licence
  ([lexicons/LICENSE-opentaal.txt](lexicons/LICENSE-opentaal.txt)).
- English: ENABLE, public domain.

Sources and pins: [lexicons/SOURCES.md](lexicons/SOURCES.md).

## License

[GPL-3.0](LICENSE).
