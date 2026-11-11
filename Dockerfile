FROM rust:1.99.1-bookworm AS chef
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo install --locked cargo-chef
ARG CARGO_BUILD_JOBS=4
ENV CARGO_BUILD_JOBS=$CARGO_BUILD_JOBS
WORKDIR /src

FROM chef AS planner
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates crates
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS rust-build
COPY --from=planner /src/recipe.json recipe.json
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target,id=buizz-target \
    cargo chef cook --release --locked --recipe-path recipe.json -p buizz-spine
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates crates
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target,id=buizz-target \
    cargo build --release --locked -p buizz-spine \
    && install -D target/release/buizz-spine /out/buizz-spine \
    && strip /out/buizz-spine

FROM node:24-bookworm-slim AS web-build
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN --mount=type=cache,target=/root/.npm npm ci --no-audit --no-fund
COPY web ./
RUN npm run --silent build

FROM python:3.13-slim-bookworm AS sdk-build
COPY --from=ghcr.io/astral-sh/uv:0.9 /uv /usr/local/bin/uv
ENV UV_PYTHON_DOWNLOADS=never \
    UV_PYTHON_PREFERENCE=only-system \
    UV_LINK_MODE=copy
WORKDIR /app/sdk/python
COPY sdk/python/pyproject.toml sdk/python/uv.lock ./
COPY sdk/python/src src
RUN --mount=type=cache,target=/root/.cache/uv \
    uv sync --frozen --no-dev --no-editable --compile-bytecode

FROM python:3.13-slim-bookworm AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends tini \
    && rm -rf /var/lib/apt/lists/*
COPY --from=ghcr.io/astral-sh/uv:0.9 /uv /usr/local/bin/uv
COPY --from=sdk-build /app/sdk/python /app/sdk/python
COPY bots /app/bots
COPY --from=web-build /web/dist /app/web
COPY --from=rust-build /out/buizz-spine /usr/local/bin/buizz-spine
RUN mkdir /data && chown 10001:10001 /data
ENV BUIZZ_DATA_DIR=/data \
    BUIZZ_BOTS_DIR=/app/bots \
    BUIZZ_WEB_DIR=/app/web \
    BUIZZ_BIND=0.0.0.0:8080 \
    UV_NO_SYNC=1 \
    UV_PYTHON_DOWNLOADS=never \
    UV_PYTHON_PREFERENCE=only-system \
    UV_CACHE_DIR=/tmp/uv-cache \
    PYTHONDONTWRITEBYTECODE=1 \
    HOME=/tmp
WORKDIR /
USER 10001:10001
EXPOSE 8080
VOLUME /data
ENTRYPOINT ["tini", "--", "buizz-spine"]
