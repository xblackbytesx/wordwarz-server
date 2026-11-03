# Deploying the WordWarZ server

The released image `ghcr.io/xblackbytesx/wordwarz-server` behind Traefik,
with Postgres 17. Tags: `X.Y.Z`, `X.Y`, `X`, `latest`.

The image is built by GitHub Actions on a `vX.Y.Z` tag and published to
GitHub's registry, so any host can pull it. Make the package public once
(GitHub, your profile, Packages, wordwarz-server, Package settings, Change
visibility); a private package needs `docker login ghcr.io` with a token
that can read packages on every host.

## Files

Copy `docker-compose.yml` and `.env.example` (as `.env`) into your stack.

```sh
mkdir -p $DOCKER_ROOT/wordwarz-server/database
```

Everything the server keeps lives in Postgres; the app container has no
volume. The word lists are built into the image.

## Configuration

| Variable | Required | Meaning |
|---|---|---|
| `DATABASE_URL` | yes | Postgres connection string |
| `WORDWARZ_PUBLIC_URL` | no | The address devices use (`https://wordwarz.example.com`); invite and device-link QR codes point here. Default: the request's host |
| `WORDWARZ_PUSH_HOSTS` | no | UnifiedPush servers (host names, comma-separated) the apps may register endpoints on. Empty: push off |
| `WORDWARZ_BIND` | no | Listen address, default `0.0.0.0:8080` |
| `WORDWARZ_COOKIE_SECURE` | no | `false` only for plain-http LAN use (default on) |
| `RUST_LOG` | no | Log level, default `info` |

There is no secret key: the server stores only hashes of passphrases and
tokens, and nothing it would need to decrypt.

## DNS and ports

- One host name, e.g. `wordwarz.example.com`, routed by Traefik to port 8080.
  The app and the website use the same name.
- The router uses the `https-redirect`, `non-www`, `secure-headers` and
  `gzip-compress` file middlewares. The server sets its own
  Content-Security-Policy (everything `'self'`); if `secure-headers` also sets
  one, the combination must still allow `script-src 'self'` and
  `connect-src 'self'`.
- Postgres stays on the internal network.
- Open apps hold `GET /api/v1/changes` for up to 25 seconds (long-poll). Any
  proxy timeout above 30 seconds is fine.
- Outbound: only POSTs to the UnifiedPush hosts you list, carrying
  `{"event": "...", "game": "<id>"}` and nothing else.

## Push

Push is UnifiedPush, never Google's FCM. Run or pick an ntfy server, install
a distributor (the ntfy app) on each phone, and list the ntfy host in
`WORDWARZ_PUSH_HOSTS`. The server refuses endpoints on any other host, so an
account cannot make it call arbitrary URLs. Every push is encrypted to the
phone's own WebPush keys (RFC 8291), so ntfy relays ciphertext: it cannot see
which game or event it carries. Without push, apps still update while open.

## First run

```sh
docker compose up -d
docker compose exec wordwarz-server-app wordwarz-server create-admin mum
docker compose exec -T wordwarz-server-app wordwarz-server set-password mum
```

`set-password` reads one line from stdin. Then log in on the website, open
People, and create an invite for each family member. Scanning the QR code with
the app creates the account and signs that phone in; children never need a
passphrase. `wordwarz-server invite mum` prints an invite link from the shell.

## Upgrades

Migrations run at start. Pin `WORDWARZ_VERSION` to a release, pull, restart.
Running games keep their rules version.

## Backups

Back up the Postgres volume. That is everything.

## Verification

```sh
curl -fsS https://wordwarz.example.com/healthz          # ok
curl -sI https://wordwarz.example.com/login | grep -i content-security-policy
docker compose logs -f wordwarz-server-app              # "listening"
```
