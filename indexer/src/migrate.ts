/**
 * Run once to create the Postgres schema.
 * Usage: npx ts-node src/migrate.ts
 */
import "dotenv/config";
import { Pool } from "pg";

const pool = new Pool({ connectionString: process.env.DATABASE_URL });

const SCHEMA = `
-- Tracks the last event cursor so the indexer can resume after restart.
-- Soroban RPC only retains ~7 days of events; the indexer must run continuously.
CREATE TABLE IF NOT EXISTS cursor (
  id      INTEGER PRIMARY KEY DEFAULT 1 CHECK (id = 1),
  value   TEXT NOT NULL DEFAULT '0'
);
INSERT INTO cursor (id, value) VALUES (1, '0') ON CONFLICT DO NOTHING;

-- One row per round, keyed by the on-chain round_id.
CREATE TABLE IF NOT EXISTS rounds (
  round_id    BIGINT PRIMARY KEY,
  asset       TEXT NOT NULL,
  strike      NUMERIC NOT NULL,
  strike_ts   BIGINT NOT NULL,
  lock_ts     BIGINT NOT NULL,
  settle_ts   BIGINT NOT NULL,
  pool_up     NUMERIC NOT NULL DEFAULT 0,
  pool_down   NUMERIC NOT NULL DEFAULT 0,
  status      TEXT NOT NULL DEFAULT 'Open',
  outcome     TEXT,
  settle_price NUMERIC,
  created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  settled_at  TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS rounds_asset_idx ON rounds (asset, settle_ts DESC);

-- One row per bet, keyed by (tx_hash, event_index) for idempotency.
CREATE TABLE IF NOT EXISTS bets (
  id          BIGSERIAL PRIMARY KEY,
  tx_hash     TEXT NOT NULL,
  event_index INTEGER NOT NULL,
  round_id    BIGINT NOT NULL REFERENCES rounds (round_id),
  user_addr   TEXT NOT NULL,
  side        TEXT NOT NULL,
  amount      NUMERIC NOT NULL,
  placed_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  UNIQUE (tx_hash, event_index)
);
CREATE INDEX IF NOT EXISTS bets_user_idx ON bets (user_addr);
CREATE INDEX IF NOT EXISTS bets_round_idx ON bets (round_id);

-- One row per claim event.
CREATE TABLE IF NOT EXISTS claims (
  id          BIGSERIAL PRIMARY KEY,
  tx_hash     TEXT NOT NULL,
  event_index INTEGER NOT NULL,
  round_id    BIGINT NOT NULL,
  user_addr   TEXT NOT NULL,
  payout      NUMERIC NOT NULL,
  claimed_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  UNIQUE (tx_hash, event_index)
);
CREATE INDEX IF NOT EXISTS claims_user_idx ON claims (user_addr);

-- Materialised leaderboard (refreshed periodically).
CREATE MATERIALIZED VIEW IF NOT EXISTS leaderboard AS
  SELECT
    b.user_addr,
    COUNT(DISTINCT b.round_id)                                    AS rounds_entered,
    COALESCE(SUM(c.payout), 0)                                    AS total_won,
    COALESCE(SUM(b.amount), 0)                                    AS total_staked,
    COALESCE(SUM(c.payout), 0) - COALESCE(SUM(b.amount), 0)      AS net_pnl
  FROM bets b
  LEFT JOIN claims c ON c.user_addr = b.user_addr AND c.round_id = b.round_id
  GROUP BY b.user_addr
WITH NO DATA;

CREATE UNIQUE INDEX IF NOT EXISTS leaderboard_user_uidx ON leaderboard (user_addr);
`;

async function migrate() {
  console.log("Running migrations...");
  await pool.query(SCHEMA);
  console.log("Schema ready.");
  await pool.end();
}

migrate().catch((e) => { console.error(e); process.exit(1); });
