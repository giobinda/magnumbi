.PHONY: install db-up db-down db-reset db-psql org-db key migrate seed demo user dev-backend dev-frontend build serve lite desktop check test test-db lint e2e-attack screenshots

# local settings (git-ignored), e.g. MAGNUM_SYSTEM_OWNERS / Google client: see .env.local.example
LOCAL_ENV = set -a && { [ ! -f .env.local ] || . ./.env.local; } && set +a
# the development ScramDB (devtools/scramdb.sh), unless .env.local sets another
DEV_DB = MAGNUM_DATABASE_URL=$${MAGNUM_DATABASE_URL:-$$(devtools/scramdb.sh url)}
RUN = $(LOCAL_ENV) && $(DEV_DB) cargo run -q --

install:            ## frontend dependencies (cargo fetches Rust crates on first build)
	cd frontend && npm install

db-up:              ## start the primary dev ScramDB on :55432 (Podman or Docker)
	devtools/scramdb.sh up

db-down:            ## stop it (data kept)
	devtools/scramdb.sh down

db-reset:           ## delete the dev ScramDB and its data
	devtools/scramdb.sh reset

db-psql:            ## psql shell on the primary dev ScramDB
	devtools/scramdb.sh psql

org-db: db-up       ## an organisation's own ScramDB: make org-db ORG=acme PORT=55433 (save the organisation first)
	devtools/scramdb.sh up $(ORG) $(PORT)
	devtools/scramdb.sh url $(ORG) | ($(LOCAL_ENV) && $(DEV_DB) cargo run -q -- org set-database $(ORG))

key:                ## add a fresh MAGNUM_SECRET_KEY to .env.local (once: losing it means re-entering org databases)
	@grep -q '^MAGNUM_SECRET_KEY=.' .env.local 2>/dev/null && echo "MAGNUM_SECRET_KEY is already set in .env.local" || \
	  (echo "MAGNUM_SECRET_KEY=$$(cargo run -q -- gen-key)" >> .env.local && echo "MAGNUM_SECRET_KEY added to .env.local")

migrate: db-up      ## apply migrations (the server also does this at startup)
	$(RUN) migrate

seed: db-up         ## (re)create the sample_sales demo table
	$(RUN) seed-sample

demo: db-up         ## the NYC demo, taxi trips x weather: make demo [ORG=globex] [MONTHS=13] (about 60 MB a month)
	$(RUN) demo --org "$(ORG)" --months $(or $(MONTHS),13)

user: db-up         ## create an account / set its password: make user EMAIL=you@example.com
	$(RUN) user set-password $(EMAIL)

dev-backend: db-up  ## API on :8000 against the dev ScramDB
	$(RUN) serve

dev-frontend:       ## Vite dev server on :5173 (proxies /api to :8000)
	cd frontend && npm run dev

build:              ## build the frontend into frontend/dist (served by the backend) and the release binary
	cd frontend && npm run build
	cargo build --release

serve: build db-up  ## single server on :8000 (API + built UI)
	$(LOCAL_ENV) && $(DEV_DB) target/release/magnum-bi serve

lite: build         ## Magnum BI for one person on this computer, no container (its own PostgreSQL in ~/.magnum-bi)
	target/release/magnum-bi lite

desktop:            ## lite in a window of its own (needs WebKitGTK on Linux: webkit2gtk4.1-devel / libwebkit2gtk-4.1-dev)
	cd frontend && npm run build
	cargo build --release --features desktop
	target/release/magnum-bi desktop

check:              ## validate the project files (models, blends, themes, dashboards)
	cargo run -q -- check

test:               ## backend unit tests + frontend typecheck and tests
	cargo test
	cd frontend && npx tsc -b && npm test

# tests at once on the one ScramDB: one per core on a big machine queues its queries past their
# lock timeouts (CI's runners have 4)
TEST_THREADS ?= 8

test-db:             ## also the tests that need ScramDB (a fresh one in memory: devtools/scramdb.sh up test), DuckDB extensions (and Postgres, with MAGNUM_TEST_POSTGRES_URL)
	devtools/scramdb.sh reset test >/dev/null && devtools/scramdb.sh up test
	$(LOCAL_ENV) && MAGNUM_DATABASE_URL=$$(devtools/scramdb.sh url test) cargo run -q -- seed-sample
	MAGNUM_TEST_DATABASE_URL=$$(devtools/scramdb.sh url test) MAGNUM_TEST_DUCKDB_EXTENSIONS=$(CURDIR)/data/duckdb/_extensions RUST_TEST_THREADS=$(TEST_THREADS) cargo test; \
	  status=$$?; devtools/scramdb.sh reset test >/dev/null; exit $$status

e2e-attack:         ## try script injection in a real browser (local only: Podman/Docker, Node, Chromium)
	cd frontend && npm run build
	cargo build
	e2e/run.sh

screenshots:        ## the website's screenshots (local only: Podman/Docker, Node, Chromium)
	cd frontend && npm run build
	cargo build
	e2e/screens.sh

lint:
	cargo clippy --all-targets
	cd frontend && npx oxlint
