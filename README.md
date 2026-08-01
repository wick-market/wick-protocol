# Wick

5-minute binary price prediction market on Stellar/Soroban.

Users stake XLM on whether BTC, ETH, or SOL will close **above or below**
a reference price after a 5-minute window. Winners split the pot
proportionally minus a 2% fee. No AMM, no order book, no house edge.

**Testnet only.** This contract holds user funds — do not use on mainnet
without a security audit.

---

## How it works

1. Every 5 minutes the Reflector oracle publishes a new price tick
2. `create_round(asset)` opens a round using that price as the **strike**
3. Users bet UP or DOWN on the next oracle tick (betting closes at 3 min)
4. `settle(round_id)` reads the oracle at the exact 5-min mark and pays out
5. Winners split the total pot proportionally; voided rounds refund in full

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the full design.

---

## Quick start (testnet)

### Prerequisites

- Rust + `wasm32v1-none` target
- [Stellar CLI](https://developers.stellar.org/docs/tools/cli/install-cli) 25+
- Node.js 18+
- PostgreSQL (for indexer + API)

### 1 — Set up environment

```bash
cp .env.example .env
# Edit .env — fill in ADMIN_SECRET and ADMIN_ADDRESS
```

### 2 — Build and deploy

```bash
# Build the contract
stellar contract build --package wick-fair-market --optimize

# Deploy (writes CONTRACT_ID to .env)
bash scripts/deploy-testnet.sh

# Initialise
bash scripts/init.sh

# Verify oracle is reachable
bash scripts/check-oracle.sh

# Open first rounds for BTC, ETH, SOL
bash scripts/genesis.sh
```

### 3 — Run the keeper

```bash
cd keeper && npm install && npx ts-node src/index.ts
```

### 4 — Run indexer + API

```bash
# Indexer
cd indexer && npm install
npx ts-node src/migrate.ts   # create Postgres tables (once)
npx ts-node src/index.ts

# API (separate terminal)
cd api && npm install && npx ts-node src/index.ts
```

---

## API

```
GET  /api/rounds/current?asset=BTC
GET  /api/rounds/history?asset=BTC&limit=50
GET  /api/rounds/:id
GET  /api/users/:address/positions
GET  /api/users/:address/claimable
GET  /api/leaderboard?window=7d
GET  /api/stats
WS   /ws
```

---

## Contract addresses (testnet)

| | Address |
|---|---|
| Fair Market | `CBJINJPV6DKXYGC3XUARWXLYQ3V6CAW2BH5PATAKEEIWSWOT5K4KEIAF` |
| Oracle | `CCYOZJCOPG34LLQQ7N24YXBM7LL62R7ONMZ3G6WZAAYPB5OYKOMJRN63` |
| XLM SAC | `CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC` |

---

## Project structure

```
contracts/
  fair-market/     Soroban contract (Rust) — the source of truth
  prediction/      Earlier prototype — kept for reference
keeper/            Automates create_round + settle every 5 min
indexer/           Reads on-chain events → Postgres
api/               REST + WebSocket server
scripts/           Build, deploy, init, genesis helpers
docs/              Architecture, invariants, failure modes
```

---

## Legal

This is testnet software. A 5-minute up/down contract on an asset price
is functionally a binary option — a regulated instrument in many jurisdictions.
Get legal advice before deploying to mainnet or accepting real funds.
