set shell := ["bash", "-euo", "pipefail", "-c"]

default: check

setup:
    cd web && npm ci --no-audit --no-fund
    cd sdk/python && uv sync --frozen

fmt:
    cargo fmt --all
    cd sdk/python && uv run --frozen ruff check --fix && uv run --frozen ruff format

gen:
    #!/usr/bin/env bash
    set -euo pipefail
    out=$(mktemp -d)
    trap 'rm -rf "$out"' EXIT
    just _generate "$out"
    rm -rf fixtures/protocol
    cp -R "$out/protocol" fixtures/protocol
    cp "$out/openapi.json" openapi.json
    cp "$out/schema.d.ts" web/src/api/schema.d.ts

drift:
    #!/usr/bin/env bash
    set -euo pipefail
    out=$(mktemp -d)
    trap 'rm -rf "$out"' EXIT
    just _generate "$out"
    diff -r "$out/protocol" fixtures/protocol
    diff "$out/openapi.json" openapi.json
    diff "$out/schema.d.ts" web/src/api/schema.d.ts

_generate out:
    cargo run --quiet --locked -p buizz-core --example golden_frames -- "{{out}}/protocol"
    cargo run --quiet --locked -p buizz-api --bin openapi > "{{out}}/openapi.json"
    cd web && npx --no-install openapi-typescript "{{out}}/openapi.json" --output "{{out}}/schema.d.ts" --silent

test:
    cargo test --workspace --locked
    if [ -d sdk/python/tests ]; then cd sdk/python && uv run --frozen pytest; fi

lint:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cd sdk/python && uv run --frozen ruff format --check && uv run --frozen ruff check && uv run --frozen pyright
    cd web && npm run --silent typecheck
    uv run --no-project --python 3.13 python scripts/no_comments.py

build:
    cargo build --workspace --locked
    cd web && npm run --silent build

check: lint test build drift

dev:
    #!/usr/bin/env bash
    set -euo pipefail
    export BUIZZ_ADMIN_PASSWORD=dev BUIZZ_SECURE_COOKIES=false BUIZZ_INGEST=false
    export BUIZZ_BIND=127.0.0.1:8080 BUIZZ_FIXTURE_DIR=fixtures/market
    cargo run -p buizz-spine &
    trap 'kill $!' EXIT
    cd web && npm run dev
