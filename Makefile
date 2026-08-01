.PHONY: build test clean deploy-testnet init genesis check-oracle keeper indexer api types bindings docker-up

# ── Rust contract ─────────────────────────────────────────────────────────────

build:
	bash scripts/build.sh

test:
	cargo test --package wick-fair-market
	cargo test --package wick-prediction

test-fair:
	cargo test --package wick-fair-market

clean:
	cargo clean

# ── Testnet lifecycle ─────────────────────────────────────────────────────────
# Run in order: deploy-testnet → init → check-oracle → genesis → keeper

deploy-testnet: build
	bash scripts/deploy-testnet.sh

init:
	bash scripts/init.sh

check-oracle:
	bash scripts/check-oracle.sh

genesis:
	bash scripts/genesis.sh

# ── Off-chain services ────────────────────────────────────────────────────────

keeper:
	cd keeper && npm ci && node dist/index.js

indexer:
	cd indexer && npm ci && node dist/index.js

api:
	cd api && npm ci && node dist/index.js

types:
	cd packages/types && npm install && npm run generate

bindings:
	bash scripts/gen-bindings.sh

docker-up:
	docker compose up --build

keeper-dev:
	cd keeper && npm ci && npx ts-node src/index.ts

indexer-dev:
	cd indexer && npm ci && npx ts-node src/index.ts

api-dev:
	cd api && npm ci && npx ts-node src/index.ts
