# Wick — Architecture

## Overview

Wick is a 5-minute binary price prediction market on Stellar/Soroban.
Users stake XLM on whether BTC, ETH, or SOL will close above or below
a reference price after a 5-minute window. Winners split the total pot
proportionally minus a fee (parimutuel, no AMM, no order book).

---

## Round Lifecycle

```
                  Oracle update N                Oracle update N+1
                        │                               │
  create_round()        │    bet() window               │  settle()
  calls lastprice()     │    closes at lock_ts          │  calls price(asset, settle_ts)
         │              │         │                     │         │
         ▼              ▼         ▼                     ▼         ▼
    ┌────────────────────────────────────────────────────────────────┐
    │                       ROUND k                                  │
    │  strike = P_N                           settle = P_(N+1)        │
    │  strike_ts = oracle.ts                  settle_ts = strike_ts+300│
    ├──────────────────────┬─────────────────┤                        │
    │   OPEN (bet here)    │  LOCKED (buffer)│      → SETTLED         │
    │   0:00 to 3:00       │  3:00 to 5:00   │                        │
    └──────────────────────┴─────────────────┴────────────────────────┘
         ↑                      ↑
    lock_offset=180s      no bets accepted
    (configurable, min 90s)
```

### Why the lock buffer is non-negotiable

Reflector's price for update N+1 is aggregated from public market data.
Anyone watching a CEX at t=4:50 knows, with near certainty, what P_(N+1)
will be. If betting is open at that moment, they are not predicting —
they are reading the answer. The 2-minute buffer prevents this.

### Why settlement is deterministic

Settlement reads `oracle.price(asset, settle_ts)` — a specific past
timestamp, not `lastprice()`. This means:

- The outcome is identical no matter who calls `settle()` or when
- A late keeper call produces the exact same result as an on-time one
- Settlement is permissionless — no trusted party can influence it

---

## Components

```
contracts/fair-market/    Soroban smart contract (Rust)
keeper/                   Settlement automation (TypeScript)
indexer/                  Event ingestion into Postgres (TypeScript)
api/                      REST + WebSocket server (TypeScript)
```

### Smart Contract

**Storage:**
- Config, round counter, fee accumulator → **persistent** (30-day TTL)
- Round, Position → **temporary** (7-day TTL, extended on write)

Rounds in temporary storage means ~420k rounds/year don't accumulate
in persistent storage and pay rent forever. Unclaimed winnings expire
after 7 days — surface this prominently in the UI.

**Key invariants:**
```
total       = pool_up + pool_down
fee         = total * fee_bps / 10_000        (only on non-void rounds)
distributed = total - fee
payout_i    = position_i.amount * distributed / winning_pool
sum(payouts) ≤ distributed                   (integer truncation)
```

**Void conditions (gross refund, no fee):**
| Condition | Reason |
|---|---|
| `pool_up == 0 \|\| pool_down == 0` | No counterparty — paying yourself |
| `settle_price == strike` | Exact tie — don't pick a winner arbitrarily |
| `oracle returns None` | Feed gap — protect users from oracle outage |

### Keeper

Runs every 60 seconds. For each asset:
1. If tracked round is past `settle_ts` → call `settle(round_id)`
2. If tracked round is Settled (or none exists) → call `create_round(asset)`

Idempotency: `AlreadySettled` and `DuplicateRound` errors are treated
as success — two keepers running simultaneously cannot corrupt state.

### Indexer

Polls `getEvents` every 10 seconds, persists a cursor to Postgres.
Handles replays idempotently via `(tx_hash, event_index)` uniqueness.

**Critical:** Soroban RPC retains events for ~7 days. The indexer must
run continuously — a gap longer than 7 days permanently loses history.

### API

REST endpoints over Postgres. WebSocket pushes live round state and
an indicative price ticker from Binance.

**Price separation:**
```
Binance WebSocket  →  ws.ts "price" message  →  UI chart (indicative)
Reflector oracle   →  settle() on-chain      →  settlement price (binding)
```
These are two completely different data sources. The displayed chart
price and the settlement price will diverge slightly — this is expected
and documented. Never let them be confused in the code.

---

## Testnet Addresses

| Resource | Address |
|---|---|
| Contract | `CBJINJPV6DKXYGC3XUARWXLYQ3V6CAW2BH5PATAKEEIWSWOT5K4KEIAF` |
| Oracle (Reflector ReflectorPulse, USD-base) | `CCYOZJCOPG34LLQQ7N24YXBM7LL62R7ONMZ3G6WZAAYPB5OYKOMJRN63` |
| XLM SAC | `CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC` |
| Admin | `GBGCQGIFNIPDRZ6GN5CFSW5T5KCTGLXDY5HD7ISY6EBDVU7Q2YFBDXUJ` |

---

## Attack Surface

| Vector | Severity | Mitigation |
|---|---|---|
| Last-look betting near settle | **Critical** | 2-min lock buffer; min 90s enforced in contract |
| Settler picks favourable price | **Critical** | `price(asset, settle_ts)`, never `lastprice()` on settle |
| XLM as underlying (thin book) | **Medium** | Included — Stellar's native asset, expected by users. Add a per-round pool cap in v2 to bound manipulation cost |
| Oracle gap / node outage | Medium | Void + gross refund |
| One-sided pool self-dealing | Medium | Void when either pool is zero |
| Rounding drain via dust bets | Low | `min_bet = 10 XLM` enforced |
| Reentrancy on claim | Low | `claimed = true` set before transfer |
