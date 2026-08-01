/**
 * Wick Keeper
 *
 * Runs every 60 seconds. For each asset (BTC, ETH, SOL):
 *   1. If the tracked round is past its settle_ts → call settle()
 *   2. If the tracked round is Settled (or we have none) → call create_round()
 *      and save the new round ID to keeper-state.json
 *
 * Uses oracle.price(asset, settle_ts) under the hood (via the contract's settle
 * function), so the outcome is deterministic — it doesn't matter if the keeper
 * fires late. A second keeper running in parallel is harmless because the
 * contract's AlreadySettled / DuplicateRound guards make every call idempotent.
 *
 * Usage:
 *   node dist/index.js          normal mode
 *   node dist/index.js --dry-run  log actions without submitting transactions
 */
import "dotenv/config";
import { getRound, settle, createRound, checkBalance } from "./contract";
import { getAsset, saveAsset } from "./state";
import { info, warn, error, alert, isDryRun, dryRun } from "./logger";

const ASSETS = ["BTC", "ETH", "SOL"];
const LOOP_INTERVAL_MS = 60_000; // check every 60 seconds
const MIN_KEEPER_BALANCE_XLM = 50; // alert if below this

// ── Per-asset logic ───────────────────────────────────────────────────────────

async function processAsset(asset: string): Promise<void> {
  const now = Math.floor(Date.now() / 1000);
  const saved = getAsset(asset);

  // ── Case 1: we have a tracked round ──────────────────────────────────────

  if (saved) {
    const round = await getRound(BigInt(saved.roundId));

    if (!round) {
      // Round was archived (TTL expired after 7 days). Start fresh.
      warn("tracked round archived — creating new", { asset, roundId: saved.roundId });
      await openNewRound(asset);
      return;
    }

    info("round status", {
      asset,
      roundId: round.id.toString(),
      status: round.status,
      settle_ts: Number(round.settle_ts),
      now,
      secsUntilSettle: Number(round.settle_ts) - now,
    });

    if (round.status === "Settled") {
      // Done — open the next one immediately.
      await openNewRound(asset);
      return;
    }

    if (now >= Number(round.settle_ts)) {
      // Time to settle.
      if (isDryRun()) {
        dryRun("settle", { asset, roundId: round.id.toString() });
        return;
      }
      info("settling round", { asset, roundId: round.id.toString() });
      const ok = await settle(round.id);
      if (ok) {
        info("settled", { asset, roundId: round.id.toString() });
        // Open next round in the same tick.
        await openNewRound(asset);
      }
      return;
    }

    // Round is still live (Open or Locked). Nothing to do this tick.
    info("round in progress", {
      asset,
      roundId: round.id.toString(),
      status: round.status,
      secsUntilLock: Math.max(0, Number(round.lock_ts) - now),
      secsUntilSettle: Math.max(0, Number(round.settle_ts) - now),
    });
    return;
  }

  // ── Case 2: no tracked round — open one ──────────────────────────────────

  await openNewRound(asset);
}

async function openNewRound(asset: string): Promise<void> {
  if (isDryRun()) {
    dryRun("create_round", { asset });
    return;
  }
  try {
    const roundId = await createRound(asset);
    const round = await getRound(roundId);
    if (!round) throw new Error("round not found immediately after creation");

    saveAsset(asset, {
      asset,
      roundId: roundId.toString(),
      settleTs: Number(round.settle_ts),
    });
    info("round created", {
      asset,
      roundId: roundId.toString(),
      strike: round.strike.toString(),
      lock_ts: Number(round.lock_ts),
      settle_ts: Number(round.settle_ts),
    });
  } catch (err: unknown) {
    const msg = String(err);
    if (msg.includes("DuplicateRound") || msg.includes("#16")) {
      // Another keeper already created this round — that's fine.
      warn("create_round: duplicate (another keeper beat us)", { asset });
    } else {
      error("create_round failed", { asset, err: msg });
      await alert("create_round failed", { asset, err: msg });
    }
  }
}

// ── Main loop ─────────────────────────────────────────────────────────────────

async function tick(): Promise<void> {
  info("keeper tick start", { assets: ASSETS, dryRun: isDryRun() });

  // Check keeper has enough XLM to keep submitting transactions.
  await checkBalance(MIN_KEEPER_BALANCE_XLM);

  // Process assets sequentially — one account, sequential sequence numbers.
  for (const asset of ASSETS) {
    try {
      await processAsset(asset);
    } catch (err: unknown) {
      const msg = String(err);
      error("processAsset failed", { asset, err: msg });
      await alert(`keeper error: ${asset}`, { err: msg });
    }
  }

  info("keeper tick end");
}

async function main(): Promise<void> {
  if (!process.env.CONTRACT_ID) {
    error("CONTRACT_ID not set in environment — exiting");
    process.exit(1);
  }
  if (!process.env.ADMIN_SECRET) {
    error("ADMIN_SECRET not set in environment — exiting");
    process.exit(1);
  }

  info("wick keeper starting", {
    contractId: process.env.CONTRACT_ID,
    network: process.env.NETWORK,
    dryRun: isDryRun(),
    loopIntervalMs: LOOP_INTERVAL_MS,
  });

  // Run immediately on start, then every LOOP_INTERVAL_MS.
  while (true) {
    await tick();
    await new Promise((r) => setTimeout(r, LOOP_INTERVAL_MS));
  }
}

main().catch((err) => {
  error("keeper crashed", { err: String(err) });
  process.exit(1);
});
