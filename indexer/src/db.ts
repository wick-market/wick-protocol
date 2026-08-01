import { Pool, PoolClient } from "pg";

export const pool = new Pool({ connectionString: process.env.DATABASE_URL });

// ── Cursor ────────────────────────────────────────────────────────────────────

export async function getCursor(): Promise<string> {
  const { rows } = await pool.query("SELECT value FROM cursor WHERE id = 1");
  return rows[0]?.value ?? "0";
}

export async function setCursor(client: PoolClient, value: string) {
  await client.query("UPDATE cursor SET value = $1 WHERE id = 1", [value]);
}

// ── Rounds ────────────────────────────────────────────────────────────────────

export async function upsertRound(
  client: PoolClient,
  r: {
    round_id: string;
    asset: string;
    strike: string;
    strike_ts: string;
    lock_ts: string;
    settle_ts: string;
  }
) {
  await client.query(
    `INSERT INTO rounds (round_id, asset, strike, strike_ts, lock_ts, settle_ts)
     VALUES ($1, $2, $3, $4, $5, $6)
     ON CONFLICT (round_id) DO NOTHING`,
    [r.round_id, r.asset, r.strike, r.strike_ts, r.lock_ts, r.settle_ts]
  );
}

export async function updateRoundPools(
  client: PoolClient,
  round_id: string,
  pool_up: string,
  pool_down: string
) {
  await client.query(
    "UPDATE rounds SET pool_up = $2, pool_down = $3 WHERE round_id = $1",
    [round_id, pool_up, pool_down]
  );
}

export async function settleRound(
  client: PoolClient,
  round_id: string,
  outcome: string,
  settle_price: string
) {
  await client.query(
    `UPDATE rounds
     SET status = 'Settled', outcome = $2, settle_price = $3, settled_at = NOW()
     WHERE round_id = $1`,
    [round_id, outcome, settle_price]
  );
}

// ── Bets ──────────────────────────────────────────────────────────────────────

export async function insertBet(
  client: PoolClient,
  b: {
    tx_hash: string;
    event_index: number;
    round_id: string;
    user_addr: string;
    side: string;
    amount: string;
    pool_up: string;
    pool_down: string;
  }
) {
  await client.query(
    `INSERT INTO bets (tx_hash, event_index, round_id, user_addr, side, amount)
     VALUES ($1, $2, $3, $4, $5, $6)
     ON CONFLICT (tx_hash, event_index) DO NOTHING`,
    [b.tx_hash, b.event_index, b.round_id, b.user_addr, b.side, b.amount]
  );
  await updateRoundPools(client, b.round_id, b.pool_up, b.pool_down);
}

// ── Claims ────────────────────────────────────────────────────────────────────

export async function insertClaim(
  client: PoolClient,
  c: {
    tx_hash: string;
    event_index: number;
    round_id: string;
    user_addr: string;
    payout: string;
  }
) {
  await client.query(
    `INSERT INTO claims (tx_hash, event_index, round_id, user_addr, payout)
     VALUES ($1, $2, $3, $4, $5)
     ON CONFLICT (tx_hash, event_index) DO NOTHING`,
    [c.tx_hash, c.event_index, c.round_id, c.user_addr, c.payout]
  );
}

// ── Leaderboard ───────────────────────────────────────────────────────────────

export async function refreshLeaderboard() {
  await pool.query(
    "REFRESH MATERIALIZED VIEW CONCURRENTLY leaderboard"
  );
}
