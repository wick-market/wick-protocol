import { Pool } from "pg";

export const pool = new Pool({ connectionString: process.env.DATABASE_URL });

// ── Rounds ────────────────────────────────────────────────────────────────────

export async function getCurrentRounds(asset: string) {
  const { rows } = await pool.query(
    `SELECT * FROM rounds
     WHERE asset = $1 AND status != 'Settled'
     ORDER BY settle_ts ASC
     LIMIT 5`,
    [asset]
  );
  return rows;
}

export async function getRoundHistory(asset: string, limit: number) {
  const { rows } = await pool.query(
    `SELECT * FROM rounds
     WHERE asset = $1
     ORDER BY settle_ts DESC
     LIMIT $2`,
    [asset, Math.min(limit, 200)]
  );
  return rows;
}

export async function getRoundById(round_id: string) {
  const { rows } = await pool.query(
    "SELECT * FROM rounds WHERE round_id = $1",
    [round_id]
  );
  return rows[0] ?? null;
}

// ── Users ─────────────────────────────────────────────────────────────────────

export async function getUserPositions(address: string) {
  const { rows } = await pool.query(
    `SELECT b.*, r.asset, r.status, r.outcome, r.settle_ts,
            r.pool_up, r.pool_down, r.strike, r.settle_price
     FROM bets b
     JOIN rounds r ON r.round_id = b.round_id
     WHERE b.user_addr = $1
     ORDER BY b.placed_at DESC
     LIMIT 100`,
    [address]
  );
  return rows;
}

export async function getUserClaimable(address: string) {
  const { rows } = await pool.query(
    `SELECT b.round_id, b.side, b.amount, r.outcome,
            r.pool_up, r.pool_down, r.settle_price, r.strike,
            r.asset, r.settle_ts,
            CASE
              WHEN r.outcome = 'Void' THEN b.amount
              WHEN (r.outcome = 'Up' AND b.side = 'Up')
                OR (r.outcome = 'Down' AND b.side = 'Down')
              THEN b.amount * (r.pool_up + r.pool_down)
                   * (10000 - $2) / 10000
                   / CASE r.outcome
                       WHEN 'Up' THEN r.pool_up
                       ELSE r.pool_down
                     END
              ELSE 0
            END AS claimable
     FROM bets b
     JOIN rounds r ON r.round_id = b.round_id
     LEFT JOIN claims c ON c.round_id = b.round_id AND c.user_addr = b.user_addr
     WHERE b.user_addr = $1
       AND r.status = 'Settled'
       AND c.id IS NULL
       AND r.outcome IS NOT NULL`,
    [address, process.env.FEE_BPS ?? 200]
  );
  return rows.filter((r) => Number(r.claimable) > 0);
}

// ── Leaderboard ───────────────────────────────────────────────────────────────

export async function getLeaderboard(window: string) {
  let interval: string;
  switch (window) {
    case "24h": interval = "1 day"; break;
    case "7d":  interval = "7 days"; break;
    case "30d": interval = "30 days"; break;
    default:    interval = "7 days";
  }

  const { rows } = await pool.query(
    `SELECT
       b.user_addr,
       COUNT(DISTINCT b.round_id)                             AS rounds_entered,
       COALESCE(SUM(c.payout), 0)                            AS total_won,
       COALESCE(SUM(b.amount), 0)                            AS total_staked,
       COALESCE(SUM(c.payout), 0) - COALESCE(SUM(b.amount), 0) AS net_pnl
     FROM bets b
     LEFT JOIN claims c ON c.user_addr = b.user_addr AND c.round_id = b.round_id
     WHERE b.placed_at > NOW() - INTERVAL '${interval}'
     GROUP BY b.user_addr
     ORDER BY net_pnl DESC
     LIMIT 100`
  );
  return rows;
}

// ── Stats ─────────────────────────────────────────────────────────────────────

export async function getStats() {
  const { rows } = await pool.query(`
    SELECT
      COUNT(*)                              AS total_rounds,
      COUNT(*) FILTER (WHERE status = 'Settled') AS settled_rounds,
      COALESCE(SUM(pool_up + pool_down), 0) AS total_volume,
      COUNT(DISTINCT asset)                 AS active_assets
    FROM rounds
  `);
  return rows[0];
}
