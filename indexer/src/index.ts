/**
 * Wick Indexer
 *
 * Polls Soroban RPC getEvents, decodes Wick contract events, and writes them
 * to Postgres. Persists a cursor so it resumes after restart without gaps.
 *
 * IMPORTANT: Soroban RPC retains events for ~7 days. This indexer MUST run
 * continuously — if it stops for more than 7 days, historical events are lost
 * permanently. Monitor the process and alert on gaps.
 *
 * Event topics emitted by the contract:
 *   ["wick","created",  asset]         → round opened
 *   ["wick","bet",      round_id]      → bet placed
 *   ["wick","settled",  round_id]      → round settled
 *   ["wick","claim",    round_id]      → payout claimed
 *   ["wick","claims",   batch_size]    → batch claim
 */
import "dotenv/config";
import { rpc, scValToNative, xdr } from "@stellar/stellar-sdk";
import {
  pool,
  getCursor,
  setCursor,
  upsertRound,
  insertBet,
  settleRound,
  insertClaim,
  refreshLeaderboard,
} from "./db";

const SERVER = new rpc.Server(process.env.RPC_URL!, { allowHttp: false });
const CONTRACT_ID = process.env.CONTRACT_ID!;
const POLL_INTERVAL_MS = 10_000; // poll every 10 seconds
const LEADERBOARD_REFRESH_INTERVAL = 60; // refresh every 60 polls (~10 min)

function log(msg: string, data?: object) {
  console.log(JSON.stringify({ ts: new Date().toISOString(), msg, ...data }));
}

function err(msg: string, data?: object) {
  console.error(JSON.stringify({ ts: new Date().toISOString(), level: "error", msg, ...data }));
}

// ── Event parsing ─────────────────────────────────────────────────────────────

interface RawEvent {
  id: string;
  txHash: string;
  type: string;
  contractId?: string;
  topic: xdr.ScVal[];
  value: xdr.ScVal;
  ledger: number;
  ledgerClosedAt: string;
  pagingToken: string;
}

function topic(ev: RawEvent, idx: number): string {
  try {
    return String(scValToNative(ev.topic[idx]));
  } catch {
    return "";
  }
}

function body(ev: RawEvent): unknown {
  try {
    return scValToNative(ev.value);
  } catch {
    return null;
  }
}

// ── Processing ────────────────────────────────────────────────────────────────

async function processEvent(ev: RawEvent, eventIndex: number) {
  if (ev.contractId !== CONTRACT_ID) return;

  // topic layout: ["wick", event_type, ...]
  const kind = topic(ev, 1);
  const data = body(ev) as Record<string, unknown> | unknown[];

  const client = await pool.connect();
  try {
    await client.query("BEGIN");

    if (kind === "created") {
      // body: [round_id, strike, strike_ts, settle_ts, lock_ts]
      const [round_id, strike, strike_ts, settle_ts, lock_ts] = data as bigint[];
      await upsertRound(client, {
        round_id: round_id.toString(),
        asset: topic(ev, 2),
        strike: strike.toString(),
        strike_ts: strike_ts.toString(),
        lock_ts: lock_ts.toString(),
        settle_ts: settle_ts.toString(),
      });
    } else if (kind === "bet") {
      // body: [user, side, amount, pool_up, pool_down]
      const [user, side, amount, pool_up, pool_down] = data as [string, string, bigint, bigint, bigint];
      await insertBet(client, {
        tx_hash: ev.txHash,
        event_index: eventIndex,
        round_id: topic(ev, 2),
        user_addr: user,
        side: String(side),
        amount: amount.toString(),
        pool_up: pool_up.toString(),
        pool_down: pool_down.toString(),
      });
    } else if (kind === "settled") {
      // body: [outcome, settle_price]
      const [outcome, settle_price] = data as [string, bigint];
      await settleRound(client, topic(ev, 2), String(outcome), settle_price.toString());
    } else if (kind === "claim" || kind === "claims") {
      // body: [user, payout]
      const [user, payout] = data as [string, bigint];
      await insertClaim(client, {
        tx_hash: ev.txHash,
        event_index: eventIndex,
        round_id: topic(ev, 2),
        user_addr: user,
        payout: payout.toString(),
      });
    }

    await client.query("COMMIT");
  } catch (e) {
    await client.query("ROLLBACK");
    err("event processing failed", { kind, txHash: ev.txHash, error: String(e) });
  } finally {
    client.release();
  }
}

// ── Main poll loop ────────────────────────────────────────────────────────────

async function poll(cursor: string): Promise<string> {
  const result = await SERVER.getEvents({
    startLedger: cursor === "0" ? undefined : undefined,
    cursor: cursor === "0" ? undefined : cursor,
    filters: [
      {
        type: "contract",
        contractIds: [CONTRACT_ID],
      },
    ],
    limit: 200,
  });

  const events = (result.events ?? []) as RawEvent[];
  if (events.length === 0) return cursor;

  for (let i = 0; i < events.length; i++) {
    await processEvent(events[i], i);
  }

  // Return the paging token of the last event as the new cursor.
  const lastToken = events[events.length - 1].pagingToken;
  log("indexed events", { count: events.length, newCursor: lastToken });
  return lastToken;
}

async function main() {
  if (!CONTRACT_ID) { err("CONTRACT_ID not set"); process.exit(1); }
  if (!process.env.DATABASE_URL) { err("DATABASE_URL not set"); process.exit(1); }

  log("indexer starting", { contractId: CONTRACT_ID });

  let cursor = await getCursor();
  let pollCount = 0;

  while (true) {
    try {
      const newCursor = await poll(cursor);

      if (newCursor !== cursor) {
        // Persist cursor atomically inside a transaction.
        const client = await pool.connect();
        try {
          await client.query("BEGIN");
          await setCursor(client, newCursor);
          await client.query("COMMIT");
          cursor = newCursor;
        } finally {
          client.release();
        }
      }

      pollCount++;
      if (pollCount % LEADERBOARD_REFRESH_INTERVAL === 0) {
        await refreshLeaderboard().catch(() => {}); // non-fatal
      }
    } catch (e) {
      err("poll failed", { error: String(e) });
    }

    await new Promise((r) => setTimeout(r, POLL_INTERVAL_MS));
  }
}

main().catch((e) => { err("indexer crashed", { error: String(e) }); process.exit(1); });
