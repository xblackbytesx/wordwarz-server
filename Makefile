# WordWarZ server. Every target runs through docker compose; no host toolchain.
#
# Three compose files, three jobs:
#   docker/docker-compose-dev.yml    development + tests (bacon, Postgres)
#   docker/docker-compose-build.yml  build the production image LOCALLY and smoke-test it
#   deploy/docker-compose.yml        production example: released registry image behind Traefik
# CI runs on every push (.github/workflows/ci.yml); a vX.Y.Z tag publishes the
# image to ghcr.io (.github/workflows/release.yml).
#
# Development
#   make dev           dev stack (bacon hot-reload + Postgres) in the foreground
#   make reset         wipe the dev database, restart dev
#   make test          full test suite (core rules + Postgres integration)
#   make lint          rustfmt --check + clippy -D warnings (same gate as CI)
#   make fmt           rustfmt
#   make android-core  wordwarz-ffi .so + Kotlin bindings + lexicon assets into dist/
#                      (for the Android app, ../wordwarz)
#   make lexicons      re-download the pinned word lists, rebuild lexicons/*.fst
#                      and the tile sets (review the tilesets.rs diff: decision 3)
#
# Local production-image smoke test (reads ./.env)
#   make image         build wordwarz-server:local from docker/Dockerfile
#   make up / down     start / stop it with Postgres on ${PORT:-8080}
#   make logs
#   make admin NAME=mum         (first account; then set its passphrase)
#   make password NAME=mum      (prompts, input not echoed)
#   make invite NAME=mum        (prints a one-time invite link)

DEV_COMPOSE   = docker compose -f docker/docker-compose-dev.yml
LOCAL_COMPOSE = docker compose --env-file .env -f docker/docker-compose-build.yml
DEV_RUN       = $(DEV_COMPOSE) run --rm --build wordwarz-server
# NAME, not USER: USER is already set in every shell (your login name).
need_name     = $(if $(NAME),$(NAME),$(error set NAME=<username>))

.PHONY: dev reset test lint fmt android-core lexicons image up down logs admin password invite

dev:
	$(DEV_COMPOSE) up --build

reset:
	@printf "This wipes the DEV database. Continue? [y/N] " && read a && [ "$$a" = "y" ]
	$(DEV_COMPOSE) down -v
	$(DEV_COMPOSE) up --build

test:
	$(DEV_RUN) cargo test --workspace --release

lint:
	$(DEV_RUN) sh -c 'cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings'

fmt:
	$(DEV_RUN) cargo fmt --all

android-core:
	$(DEV_COMPOSE) --profile android run --rm --build android-core

lexicons:
	$(DEV_RUN) scripts/lexicons.sh

image:
	$(LOCAL_COMPOSE) build

up:
	$(LOCAL_COMPOSE) up --build -d

down:
	$(DEV_COMPOSE) down
	$(LOCAL_COMPOSE) down

logs:
	$(LOCAL_COMPOSE) logs -f wordwarz-server

admin:
	$(LOCAL_COMPOSE) exec wordwarz-server wordwarz-server create-admin $(call need_name)

password:
	@stty -echo; $(LOCAL_COMPOSE) exec -T wordwarz-server wordwarz-server set-password $(call need_name); status=$$?; stty echo; echo; exit $$status

invite:
	$(LOCAL_COMPOSE) exec wordwarz-server wordwarz-server invite $(call need_name)
