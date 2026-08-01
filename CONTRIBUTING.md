# Contributing to Wick

This is the **backend** repository. The frontend lives at
[wick-app](https://github.com/Tijesunimi004/wick-app) (separate repo).

---

## Local setup

### Prerequisites

| Tool | Minimum version | Install |
|---|---|---|
| Rust | 1.80+ | [rustup.rs](https://rustup.rs) |
| stellar CLI | 25+ | [docs](https://developers.stellar.org/docs/tools/cli/install-cli) |
| Node.js | 18+ | [nodejs.org](https://nodejs.org) |
| Docker | 24+ | [docker.com](https://www.docker.com) |
| PostgreSQL | 15+ | via Docker (see below) |

### 1 — Clone and configure

```bash
git clone https://github.com/Tijesunimi004/wick.git
cd wick
cp .env.example .env
# Fill in ADMIN_SECRET, ADMIN_ADDRESS, CONTRACT_ID (see README.md)
```

### 2 — Run everything locally

```bash
# Start Postgres + api + indexer in one command
docker compose up --build
```

The API will be available at `http://localhost:3000`.
The indexer will start polling testnet events immediately.

Alternatively, run each service manually:

```bash
# Terminal 1 — Postgres
docker run -e POSTGRES_DB=wick -e POSTGRES_USER=wick -e POSTGRES_PASSWORD=wick_dev -p 5432:5432 postgres:16-alpine

# Terminal 2 — run migrations, then indexer
cd indexer && npm install && npm run migrate && npm run dev

# Terminal 3 — API
cd api && npm install && npm run dev
```

### 3 — Run contract tests

```bash
cargo test --package wick-fair-market   # 27 tests
cargo test --package wick-prediction    # 35 tests (prototype, kept for reference)
```

### 4 — Regenerate types (after changing openapi.yaml)

```bash
make types
# Commits the regenerated packages/types/index.ts
```

---

## PR conventions

- **Branch names**: `feat/`, `fix/`, `docs/`, `chore/` prefix
- **Commit messages**: conventional commits (`feat:`, `fix:`, `docs:`)
- **Contract changes**: tests must stay green (`cargo test --package wick-fair-market`)
- **API changes**: update `openapi.yaml` and re-run `make types`
- **No secrets**: `.env` is in `.gitignore` — never commit it

## How to claim an issue

Comment "I'll take this" on any open issue. Assign yourself. Open a draft PR
when you start so others know it's in progress.

## Code layout

```
contracts/fair-market/   Soroban contract (Rust) — source of truth
contracts/prediction/    Prototype — do not modify, kept for reference
keeper/                  Round automation
indexer/                 Event ingestion
api/                     REST + WebSocket
packages/types/          Generated TS types (openapi-typescript)
packages/bindings/       Generated contract bindings (stellar CLI)
fixtures/                Mock API responses for wick-app dev mode
docs/                    Architecture, API prose docs
```
