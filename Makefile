.PHONY: install db-up db-down db-reset db-psql migrate seed user dev-backend dev-frontend build serve check test lint

# local settings (git-ignored), e.g. MAGNUM_SYSTEM_OWNERS / Google client: see .env.local.example
LOCAL_ENV = set -a && { [ ! -f .env.local ] || . ./.env.local; } && set +a
# the development ScramDB (devtools/scramdb.sh), unless .env.local sets another
DEV_DB = MAGNUM_DATABASE_URL=$${MAGNUM_DATABASE_URL:-$$(devtools/scramdb.sh url)}
RUN = $(LOCAL_ENV) && $(DEV_DB) cargo run -q --

install:            ## frontend dependencies (cargo fetches Rust crates on first build)
	cd frontend && npm install

db-up:              ## start the dev ScramDB on :55432 (Podman or Docker)
	devtools/scramdb.sh up

db-down:            ## stop it (data kept)
	devtools/scramdb.sh down

db-reset:           ## delete the dev ScramDB and its data
	devtools/scramdb.sh reset

db-psql:            ## psql shell on the dev ScramDB
	devtools/scramdb.sh psql

migrate: db-up      ## apply migrations (the server also does this at startup)
	$(RUN) migrate

seed: db-up         ## (re)create the sample_sales demo table
	$(RUN) seed-sample

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

check:              ## validate the project files (connections, models, dashboards)
	cargo run -q -- check

test:               ## backend unit tests + frontend typecheck
	cargo test
	cd frontend && npx tsc -b

lint:
	cargo clippy --all-targets
	cd frontend && npx oxlint
