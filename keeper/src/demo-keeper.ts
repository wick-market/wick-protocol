/**
 * Demo keeper — drives fast rounds on all four markets using the test oracle.
 *
 * Each market runs its own independent loop, so a slow tx on BTC never stalls XLM.
 * Per market, one cycle is:
 *   1. push a fresh oracle price for the asset (this becomes the next strike_ts)
 *   2. settle the current round once it is past settle_ts
 *   3. claim every settled round for the bot wallets (recycles stake back)
 *   4. create the next round and immediately auto-bet both sides in parallel
 *
 * Step 3 is what keeps this running: without claiming, the bot wallets drain
 * within an hour and every round voids for want of an opposing side.
 */
import "dotenv/config";
import {
  Contract, Keypair, Networks, rpc,
  TransactionBuilder, BASE_FEE, nativeToScVal, scValToNative, xdr, Address
} from "@stellar/stellar-sdk";

// ── Config ────────────────────────────────────────────────────────────────────

const RPC_URL = "https://soroban-testnet.stellar.org";
const NETWORK = Networks.TESTNET;
const ADMIN_SECRET = (process.env.ADMIN_SECRET
  ?? "SDGZDDLJCCE6BQGROTACAZGLI3OIFNM3DTJJL7RZRM5KNQSDLXQUS73E").trim();
const ORACLE_CONTRACT = "CBCZDSMRMOYXRLV3IJNC6LC7HKV2UFE5KQ63P5LAKM73LAH4H4CNT4TM";

/** How often each market loop wakes up. Rounds are 60s, so this is just slack. */
const POLL_MS = 5_000;

interface MarketConfig {
  symbol: string;
  contractId: string;
  price: bigint;
  decimals: number;
  currentRoundId: bigint;
}

/**
 * Seed prices are in oracle units: USD × 1e14 (14 decimals, matching Reflector).
 * So $65,000 is 65_000 * 1e14 = 6.5e18, not 6.5e17 — the earlier values were a
 * factor of ten low and the UI showed BTC at $6,500.
 */
const MARKETS: MarketConfig[] = [
  { symbol: "XLM", contractId: "CCHKYSNNU27QYKBAWTPHCIHOZQISYQ3GUEC3KVCZ6QPPNWN4QQXTTM3K", price: 17500000000000n, decimals: 4, currentRoundId: 0n },      // $0.175
  { symbol: "BTC", contractId: "CBIDB2UVODQFULE5GDOFCITHADLRWCPOCCN3FUPGKUDEHGZ7P3KRXIA4", price: 6500000000000000000n, decimals: 2, currentRoundId: 0n }, // $65,000
  { symbol: "ETH", contractId: "CAIETRXOO3YYJE7YISGPODJ6HTF2SZY2PC3WPD54ZW2EAWWAMZZWL7IS", price: 340000000000000000n, decimals: 2, currentRoundId: 0n },  // $3,400
  { symbol: "SOL", contractId: "CDAL2IADNQYUWDLZHN72EERCTT2SSPDC6RBXFHR3ZZHDTVGQHD4LG3T3", price: 18000000000000000n, decimals: 2, currentRoundId: 0n },   // $180
];

/**
 * Bot bettors. Amounts are deliberately small (20-30 XLM) — four markets at one
 * round a minute is ~2k XLM/hour per wallet even with claiming, and friendbot
 * only tops up to 10k.
 */
const BETTORS = [
  { secret: "SBB4U4OKPILVJBWNIBBFOSHMBKHBMN6HHDX6DEZV5HWANM2FRYSIRRT2".trim(), name: "wallet-1", side: "above" as const, amountStroops: 300000000n }, // 30 XLM Above
  { secret: "SCALTGO6MAGGWL43HACE5L6STUQZ7T3TYO6MBQ7HAAVBK6FRTRY7TPH3".trim(), name: "wallet-2", side: "below" as const, amountStroops: 200000000n }, // 20 XLM Below
  { secret: "SB4RSZRZ6GQLA5IQNPFO7OXSOTOSWLJV7LRCQM3UMU2DXWAUL2X5VHOZ".trim(), name: "wallet-3", side: "below" as const, amountStroops: 200000000n }, // 20 XLM Below
];

const server = new rpc.Server(RPC_URL, { allowHttp: false });
const adminKeypair = Keypair.fromSecret(ADMIN_SECRET);
const oracleContract = new Contract(ORACLE_CONTRACT);
const bettorKeypairs = BETTORS.map((b) => Keypair.fromSecret(b.secret));

function log(msg: string, data?: Record<string, unknown>) {
  console.log(JSON.stringify({ ts: new Date().toISOString(), msg, ...data }));
}

// ── RPC helpers ───────────────────────────────────────────────────────────────

function i128(v: bigint): xdr.ScVal {
  return xdr.ScVal.scvI128(new xdr.Int128Parts({
    hi: xdr.Int64.fromString((v >> 64n).toString()),
    lo: xdr.Uint64.fromString((v & 0xFFFFFFFFFFFFFFFFn).toString()),
  }));
}

/**
 * One in-flight tx per signing account.
 *
 * Every tx is built from a freshly fetched sequence number, so two market loops
 * signing with the same key at the same moment build two txs claiming the same
 * seq — one lands and the other dies with a send failure or a timeout. Four
 * markets all push oracle prices as admin, so that collision was constant.
 * Serializing per key costs a little latency and removes the whole class.
 */
const txLocks = new Map<string, Promise<unknown>>();

function withAccountLock<T>(pubkey: string, fn: () => Promise<T>): Promise<T> {
  const prev = txLocks.get(pubkey) ?? Promise.resolve();
  const next = prev.then(fn, fn); // run regardless of how the previous tx ended
  txLocks.set(pubkey, next.catch(() => {}));
  return next;
}

async function invoke(keypair: Keypair, contract: Contract, method: string, args: xdr.ScVal[] = []): Promise<unknown> {
  return withAccountLock(keypair.publicKey(), () => invokeUnlocked(keypair, contract, method, args));
}

async function invokeUnlocked(keypair: Keypair, contract: Contract, method: string, args: xdr.ScVal[] = []): Promise<unknown> {
  const account = await server.getAccount(keypair.publicKey());
  const tx = new TransactionBuilder(account, { fee: BASE_FEE, networkPassphrase: NETWORK })
    .addOperation(contract.call(method, ...args))
    .setTimeout(30).build();
  const sim = await server.simulateTransaction(tx);
  if (!rpc.Api.isSimulationSuccess(sim)) {
    throw new Error(`sim ${method}: ${(sim as rpc.Api.SimulateTransactionErrorResponse).error}`);
  }
  const assembled = rpc.assembleTransaction(tx, sim).build();
  assembled.sign(keypair);
  const send = await server.sendTransaction(assembled);
  if (send.status === "ERROR") throw new Error(`send ${method} failed`);
  for (let i = 0; i < 35; i++) {
    await new Promise(r => setTimeout(r, 1000));
    const res = await server.getTransaction(send.hash);
    if (res.status === rpc.Api.GetTransactionStatus.SUCCESS) {
      const rv = (res as rpc.Api.GetSuccessfulTransactionResponse).returnValue;
      try { return rv ? scValToNative(rv) : null; } catch { return null; }
    }
    if (res.status === rpc.Api.GetTransactionStatus.FAILED) throw new Error(`tx ${method} failed`);
  }
  throw new Error(`${method} timed out`);
}

async function query(method: string, contract: Contract, args: xdr.ScVal[] = []): Promise<unknown> {
  const account = await server.getAccount(adminKeypair.publicKey());
  const tx = new TransactionBuilder(account, { fee: BASE_FEE, networkPassphrase: NETWORK })
    .addOperation(contract.call(method, ...args)).setTimeout(30).build();
  const sim = await server.simulateTransaction(tx);
  if (!rpc.Api.isSimulationSuccess(sim)) return null;
  return scValToNative((sim as rpc.Api.SimulateTransactionSuccessResponse).result!.retval);
}

// ── Oracle ────────────────────────────────────────────────────────────────────

/** Random walk, ±0.3% per tick. */
function walkPrice(m: MarketConfig): bigint {
  const changePct = (Math.random() - 0.5) * 0.006;
  m.price = BigInt(Math.round(Number(m.price) * (1 + changePct)));
  return m.price;
}

/**
 * Push a new price for this asset. create_round uses the oracle's timestamp as
 * strike_ts and rejects a repeat, so this must land before every create_round.
 */
async function pushPrice(m: MarketConfig): Promise<void> {
  const newPrice = walkPrice(m);
  // Two cycles landing on the same number would give the next round the same
  // strike as the last — read-back tie, instant Void. Nudge so strikes differ.
  if (newPrice === m.price) {
    m.price = (m.price * 1000000001n) / 1000000000n;
  }
  await invoke(adminKeypair, oracleContract, "update_asset_price", [
    xdr.ScVal.scvSymbol(m.symbol),
    i128(m.price),
  ]);
  log("oracle updated", { market: m.symbol, price: (Number(m.price) / 1e14).toFixed(m.decimals) });
}

// ── Betting ───────────────────────────────────────────────────────────────────

const betTracker = new Set<string>();   // "symbol-roundId-pubkey"
const claimTracker = new Set<string>(); // "symbol-roundId-pubkey"

/** All bots bet at once — sequential bets can miss the lock window. */
async function autoBet(m: MarketConfig, roundId: bigint): Promise<void> {
  const predictContract = new Contract(m.contractId);
  await Promise.all(BETTORS.map(async (bettor, idx) => {
    const keypair = bettorKeypairs[idx];
    const key = `${m.symbol}-${roundId}-${keypair.publicKey()}`;
    if (betTracker.has(key)) return;
    betTracker.add(key); // claim the slot up front so a retry can't double-bet

    const fn = bettor.side === "above" ? "bet_above" : "bet_below";
    try {
      await invoke(keypair, predictContract, fn, [
        new Address(keypair.publicKey()).toScVal(),
        nativeToScVal(roundId, { type: "u64" }),
        i128(bettor.amountStroops),
      ]);
      log("bet placed", {
        market: m.symbol, round: roundId.toString(), wallet: bettor.name,
        side: bettor.side, amountXlm: Number(bettor.amountStroops) / 1e7,
      });
    } catch (e) {
      betTracker.delete(key); // let the next cycle retry while the round is open
      log("bet failed", { market: m.symbol, round: roundId.toString(), wallet: bettor.name, err: String(e) });
    }
  }));
}

/**
 * Claim a settled round for every bot. This is what recycles stake — winners get
 * their payout, losers get the Ninetails partial refund, and a void round gives
 * everything back. Without it the bots go broke and rounds void forever.
 */
async function autoClaim(m: MarketConfig, roundId: bigint): Promise<void> {
  const predictContract = new Contract(m.contractId);
  await Promise.all(BETTORS.map(async (bettor, idx) => {
    const keypair = bettorKeypairs[idx];
    const key = `${m.symbol}-${roundId}-${keypair.publicKey()}`;
    if (claimTracker.has(key)) return;
    claimTracker.add(key);

    try {
      const payout = await invoke(keypair, predictContract, "claim", [
        new Address(keypair.publicKey()).toScVal(),
        nativeToScVal(roundId, { type: "u64" }),
      ]);
      log("claimed", {
        market: m.symbol, round: roundId.toString(), wallet: bettor.name,
        payoutXlm: payout != null ? Number(payout) / 1e7 : "unknown",
      });
    } catch (e) {
      // NothingToClaim (#10) is normal — the bot never bet this round, or the
      // position was already claimed. Don't retry those; #9 is AlreadyBet.
      const msg = String(e);
      if (!msg.includes("#10") && !msg.includes("NothingToClaim")) {
        claimTracker.delete(key);
        log("claim failed", { market: m.symbol, round: roundId.toString(), wallet: bettor.name, err: msg });
      }
    }
  }));
}

// ── Per-market cycle ──────────────────────────────────────────────────────────

async function openRound(m: MarketConfig): Promise<void> {
  const predictContract = new Contract(m.contractId);

  // Fresh tick first, or create_round trips the DuplicateRound guard.
  await pushPrice(m);

  let newId: bigint | null = null;
  try {
    const res = await invoke(adminKeypair, predictContract, "create_round");
    if (typeof res === "bigint" || typeof res === "number") newId = BigInt(res);
  } catch (e) {
    log("create_round failed", { market: m.symbol, err: String(e) });
  }

  // create_round can return null on an XDR decode hiccup even though the tx
  // landed. Re-read the counter rather than lose the round.
  if (newId === null) {
    const latest = await query("current_round_id", predictContract);
    if (typeof latest === "bigint" || typeof latest === "number") {
      const n = BigInt(latest);
      if (n > m.currentRoundId) newId = n;
    }
  }

  if (newId === null) return;

  m.currentRoundId = newId;
  log("round created", { market: m.symbol, round: newId.toString() });
  await autoBet(m, newId);
}

async function processMarket(m: MarketConfig): Promise<void> {
  const predictContract = new Contract(m.contractId);
  const now = Math.floor(Date.now() / 1000);

  if (m.currentRoundId === 0n) {
    const latestId = await query("current_round_id", predictContract);
    if (typeof latestId === "bigint" || typeof latestId === "number") {
      m.currentRoundId = BigInt(latestId);
    }
  }

  if (m.currentRoundId === 0n) {
    await openRound(m);
    return;
  }

  const round = (await query("get_round", predictContract, [
    nativeToScVal(m.currentRoundId, { type: "u64" }),
  ])) as Record<string, unknown> | null;

  if (!round) {
    // Round fell out of temporary storage — start a fresh one.
    m.currentRoundId = 0n;
    await openRound(m);
    return;
  }

  const status = (round.status as { tag: string }).tag;
  const outcome = (round.outcome as { tag: string }).tag;
  const settleTs = Number(round.settle_ts);
  const lockTs = Number(round.lock_ts);

  log("round status", {
    market: m.symbol, round: m.currentRoundId.toString(), status,
    poolAbove: Number(round.pool_above) / 1e7,
    poolBelow: Number(round.pool_below) / 1e7,
    secsUntilLock: Math.max(0, lockTs - now),
    secsUntilSettle: Math.max(0, settleTs - now),
  });

  // Still open — make sure both sides are covered, then wait.
  if (status === "Open" && now < lockTs) {
    await autoBet(m, m.currentRoundId);
    return;
  }

  // Settle once the window has passed.
  if (status !== "Settled" && now >= settleTs) {
    // Move the price first. The test oracle's price(asset, ts) ignores ts and
    // returns whatever is stored right now, so without a push between create
    // and settle the contract reads back the exact strike — a tie, which is
    // Void. Every round voided this way regardless of how the bots bet.
    await pushPrice(m);

    try {
      await invoke(adminKeypair, predictContract, "settle", [
        nativeToScVal(m.currentRoundId, { type: "u64" }),
      ]);
      log("round settled", { market: m.symbol, round: m.currentRoundId.toString() });
    } catch (e) {
      const msg = String(e);
      if (!msg.includes("#6") && !msg.includes("AlreadySettled")) {
        log("settle failed", { market: m.symbol, round: m.currentRoundId.toString(), err: msg });
        return; // try again next cycle rather than abandoning the round
      }
    }
  }

  // Settled: pay the bots out, then roll straight into the next round.
  if (status === "Settled" || now >= settleTs) {
    const settledId = m.currentRoundId;
    await autoClaim(m, settledId);
    log("rolling to next round", { market: m.symbol, settled: settledId.toString(), outcome });
    await openRound(m);
  }
}

/**
 * One independent loop per market. Self-scheduling: the next cycle is queued
 * only after this one finishes, so slow RPC never stacks overlapping ticks the
 * way a fixed setInterval does.
 */
async function marketLoop(m: MarketConfig): Promise<void> {
  for (;;) {
    try {
      await processMarket(m);
    } catch (e) {
      log("market error", { market: m.symbol, err: String(e) });
    }
    await new Promise(r => setTimeout(r, POLL_MS));
  }
}

async function main() {
  log("demo-keeper starting", {
    oracle: ORACLE_CONTRACT,
    pollMs: POLL_MS,
    markets: MARKETS.map(m => `${m.symbol}:${m.contractId}`),
    bettors: bettorKeypairs.map(k => k.publicKey()),
  });

  // All four markets run concurrently and never block each other.
  await Promise.all(MARKETS.map(marketLoop));
}

main().catch((e) => { console.error(e); process.exit(1); });
