# Wick API — Human-readable reference

For machine-readable types see `openapi.yaml` at the repo root.
For the generated TypeScript types see `packages/types/index.ts`.

---

## Base URL

| Environment | URL |
|---|---|
| Local | `http://localhost:3000` |
| Testnet (Render) | `https://wick-api.onrender.com` |

---

## Important: amounts are strings

All XLM amounts, oracle prices, and round IDs are returned as **strings**.
PostgreSQL's `BIGINT` and `NUMERIC` types can exceed JavaScript's `Number.MAX_SAFE_INTEGER`,
so the `pg` driver returns them as strings. Parse with `BigInt()` or a decimal
library. Never pass them through `Number()`.

1 XLM = 10,000,000 stroops (7 decimal places).
Oracle prices have 14 decimal places. Actual USD = `price / 10^14`.

---

## Round lifecycle

```
Open  →  Locked  →  Settled
               ↘  Void (on Settled with no winner)
```

A round is **Open** from creation until `lock_ts` (3 minutes after the
oracle tick). Bets are rejected at or after `lock_ts`.

A round is **Locked** between `lock_ts` and `settle_ts`. This is derived
from timestamps in `get_round()` — no on-chain transaction is needed.

A round is **Settled** after `settle()` is called on-chain. Outcome is
one of: `Up` (settle_price > strike), `Down`, or `Void`.

**Void** rounds (empty pool, exact tie, or oracle gap) refund every bettor
the exact amount they staked with no fee deducted.

---

## Endpoints

### GET /health

Returns 200 when the process is alive. Does not check database connectivity.

```json
{ "ok": true, "ts": 1785577900000 }
```

---

### GET /api/rounds/current?asset=BTC

Returns up to 5 non-Settled rounds for the given asset, ordered by
`settle_ts` ascending. Typically 1–2 rows: one Open and one Locked.

**Query params:**
- `asset` (required): `BTC`, `ETH`, `SOL`, or `XLM`

See `fixtures/rounds-current-btc.json` for an example response.

---

### GET /api/rounds/history?asset=BTC&limit=50

Returns settled and active rounds for an asset, newest first.

**Query params:**
- `asset` (required)
- `limit` (optional, default 50, max 200)

---

### GET /api/rounds/:id

Returns a single round by its on-chain ID. Returns 404 if not indexed yet.

---

### GET /api/users/:address/positions

Returns the 100 most recent bets for a Stellar G-address, joined with
the round's current state. Includes won, lost, pending, and void positions.

---

### GET /api/users/:address/claimable

Returns positions with a non-zero unclaimed payout. The `claimable` field
is the computed payout in stroops. Returns an empty array if nothing is owed.

The payout formula is:
```
distributed = (pool_up + pool_down) × (10000 − fee_bps) / 10000
payout      = amount × distributed / winning_pool
```

For void rounds: `claimable = amount` (gross refund, no fee).

---

### GET /api/leaderboard?window=7d

Returns up to 100 users ranked by net P&L (`total_won − total_staked`).

**Query params:**
- `window`: `24h`, `7d` (default), or `30d`

---

### GET /api/stats

Global aggregates across all rounds and assets since contract deployment.

```json
{
  "total_rounds":   "288",
  "settled_rounds": "276",
  "total_volume":   "2880000000000",
  "active_assets":  "4"
}
```

---

## WebSocket — GET /ws (upgrade)

Connect and receive messages without sending anything.

### Message types

#### `connected`
Sent once on connection.
```json
{ "type": "connected" }
```

#### `round`
Sent every 5 seconds for each non-Settled round. Drive your countdown
timers and pool size displays from `lock_ts` and `settle_ts`, not from
the server clock.
```json
{ "type": "round", "data": { ...Round } }
```

#### `price`
Indicative USD price from Binance miniTicker. **This is not the settlement
price.** Settlement reads `oracle.price(asset, settle_ts)` from Reflector —
a separate source that may differ by a few cents. Never use the `price`
message in payout calculations. Label it "indicative" in the UI.
```json
{
  "type": "price",
  "asset": "BTC",
  "price": "63038.12",
  "ts": 1785577912345
}
```

---

## Error responses

All endpoints return `{ "error": "message" }` on error.

| Status | Meaning |
|---|---|
| 400 | Missing required query parameter (asset) |
| 404 | Round not found in index |
| 500 | Database or internal error |
