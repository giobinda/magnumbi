# Magnum BI: the server (API + built UI) in one image.
#   docker build -t magnum-bi .
# Runs as an unprivileged user; everything it writes goes to /data (DuckDB files, extensions,
# the secret key when MAGNUM_SECRET_KEY_FILE points there). See compose.yaml and deploy/helm.

FROM docker.io/library/node:22-bookworm-slim AS web
WORKDIR /src/frontend
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY frontend/ ./
RUN npm run build

# the project's declared minimum Rust (rust-version)
FROM docker.io/library/rust:1.89-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends libssl-dev pkg-config && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock rustfmt.toml ./
COPY backend/ backend/
RUN cargo build --release --locked && strip target/release/magnum-bi

FROM docker.io/library/debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libssl3 && rm -rf /var/lib/apt/lists/* \
 && useradd --uid 10001 --home-dir /data --no-create-home --shell /usr/sbin/nologin magnum \
 && mkdir -p /data /secrets && chown magnum /data /secrets
COPY --from=build /src/target/release/magnum-bi /usr/local/bin/magnum-bi
COPY --from=web /src/frontend/dist /app/frontend/dist
COPY project/ /app/project/
COPY LICENSE NOTICE TRADEMARKS.md /app/
WORKDIR /app
ENV MAGNUM_HOST=0.0.0.0 \
    MAGNUM_PORT=8000 \
    MAGNUM_FRONTEND_DIST=/app/frontend/dist \
    MAGNUM_PROJECT_DIR=/app/project \
    MAGNUM_DUCKDB_DIR=/data/duckdb
USER 10001
VOLUME /data
EXPOSE 8000
ENTRYPOINT ["magnum-bi"]
CMD ["serve"]
