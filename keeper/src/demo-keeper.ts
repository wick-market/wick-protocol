/**
 * Demo keeper — runs 1-minute rounds using the test oracle.
 *
 * Every 60 seconds:
 *   1. Update oracle price (small random walk from last price)
 *   2. If current round past settle_ts → settle it
 *   3. If round settled or none → create new round
 *   4. Auto-bet from test wallets on both sides (so rounds never void)
 *
 * Uses the test oracle contract (CABYY3...) so we control price timing.
 * Switch to Reflector oracle for production.
 */
import "dotenv/config";
import {
  Contract, Keypair, Networks, rpc,
  TransactionBuilder, BASE_FEE, nativeToScVal, scValToNative, xdr,
} from "@stellar/stellar-sdk";

// ── Config ────────────────────────────────────────────────────────────────────

const PREDICT_CONTRACT = process.env.PREDICT_CONTRACT
  ?? "CBJDHRRZ7G62S5ZGDEM53CIHRS3OMKCGOHM27I5XYBD2ANNVNIAJHTX2";
const ORACLE_CONTRACT = "CDSO4XUHS27LTG2ND3PCAU2NE6EPZWFTBJMZYZBVQUEDKFLPTGTP5UY3"; // no-auth version
const RPC_URL = "https://soroban-testnet.stellar.org";
const NETWORK = Networks.TESTNET;
const ROUND_SECS = 60;     // oracle_interval — must match initialize()
const LOCK_SECS  = 45;     // lock_offset — must match initialize()

// Admin for oracle updates + creating rounds
const ADMIN_SECRET = process.env.ADMIN_SECRET
  ?? "SDGZDDLJCCE6BQGROTACAZGLI3OIFNM3DTJJL7RZRM5KNQSDLXQUS73E";

// Test wallets that auto-bet both sides (funded testnet accounts)
const BETTORS = [
  { secret: "SBB4U4OKPILVJBWNIBBFOSHMBKHBMN6HHDX6DEZV5HWANM2FRYSIRRT2", name: "wallet-1", side: "above" as const, amountStroops: 2000000000n }, // 200 XLM Above
  { secret: "SCALTGO6MAGGWL43HACE5L6STUQZ7T3TYO6MBQ7HAAVBK6FRTRY7TPH3", name: "wallet-2", side: "below" as const, amountStroops: 1500000000n }, // 150 XLM Below
  { secret: "SB4RSZRZ6GQLA5IQNPFO7OXSOTOSWLJV7LRCQM3UMU2DXWAUL2X5VHOZ", name: "wallet-3", side: "below" as const, amountStroops: 1000000000n }, // 100 XLM Below
];

const server = new rpc.Server(RPC_URL, { allowHttp: false });
const adminKeypair = Keypair.fromSecret(ADMIN_SECRET);
const predictContract = new Contract(PREDICT_CONTRACT);
const oracleContract  = new Contract(ORACLE_CONTRACT);

function log(msg: string, data?: Record<string, unknown>) {
  console.log(JSON.stringify({ ts: new Date().toISOString(), msg, ...data }));
}

// ── RPC helpers ───────────────────────────────────────────────────────────────

async function invoke(keypair: Keypair, contract: Contract, method: string, args: xdr.ScVal[] = []): Promise<unknown> {
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
  for (let i = 0; i < 20; i++) {
    await new Promise(r => setTimeout(r, 2000));
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

// ── Oracle price walk ─────────────────────────────────────────────────────────

let currentPrice = 17500000000000n; // ~$0.175 with 14 decimals

function walkPrice(): bigint {
  // ±0.3% random walk per tick
  const changePct = (Math.random() - 0.5) * 0.006;
  currentPrice = BigInt(Math.round(Number(currentPrice) * (1 + changePct)));
  return currentPrice;
}

// ── State ─────────────────────────────────────────────────────────────────────

let currentRoundId = 0n;
const betTracker = new Set<string>(); // "roundId-walletPubkey"

// ── Main loop ─────────────────────────────────────────────────────────────────

async function tick() {
  const now = Math.floor(Date.now() / 1000);

  // 1. Advance oracle price
  const newPrice = walkPrice();
  try {
    await invoke(adminKeypair, oracleContract, "update_price", [
      xdr.ScVal.scvI128(new xdr.Int128Parts({
        hi: xdr.Int64.fromString((newPrice >> 64n).toString()),
        lo: xdr.Uint64.fromString((newPrice & 0xFFFFFFFFFFFFFFFFn).toString()),
      })),
    ]);
    log("oracle updated", { price: (Number(newPrice) / 1e14).toFixed(4) });
  } catch (e) { log("oracle update failed", { err: String(e) }); }

  // 2. Handle existing round
  if (currentRoundId > 0n) {
    const round = (await query("get_round", predictContract, [
      nativeToScVal(currentRoundId, { type: "u64" }),
    ])) as Record<string, unknown> | null;

    if (round) {
      const status = (round.status as { tag: string }).tag;
      const settleTs = Number(round.settle_ts);
      const lockTs = Number(round.lock_ts);

      log("round", {
        id: currentRoundId.toString(), status,
        secsUntilLock: Math.max(0, lockTs - now),
        secsUntilSettle: Math.max(0, settleTs - now),
      });

      // Auto-bet if still open
      if (now < lockTs && status === "Open") {
        await autoBet(currentRoundId);
      }

      // Settle if ready
      if (now >= settleTs && status !== "Settled") {
        try {
          await invoke(adminKeypair, predictContract, "settle", [
            nativeToScVal(currentRoundId, { type: "u64" }),
          ]);
          const settled = (await query("get_round", predictContract, [
            nativeToScVal(currentRoundId, { type: "u64" }),
          ])) as Record<string, unknown>;
          log("settled", {
            id: currentRoundId.toString(),
            outcome: (settled.outcome as { tag: string }).tag,
            settle_price: (Number(settled.settle_price as bigint) / 1e14).toFixed(4),
          });
          // Open next round
          await openNewRound();
        } catch (e) {
          const msg = String(e);
          if (msg.includes("#6") || msg.includes("AlreadySettled")) {
            await openNewRound();
          } else { log("settle error", { err: msg }); }
        }
        return;
      }

      if (status === "Settled") await openNewRound();
      return;
    }
  }

  await openNewRound();
}

async function openNewRound() {
  try {
    const id = await invoke(adminKeypair, predictContract, "create_round");
    if (id !== null) {
      currentRoundId = BigInt(id as string | number | bigint);
      betTracker.clear();
      log("round created", { id: currentRoundId.toString() });
      await autoBet(currentRoundId);
    }
  } catch (e) {
    const msg = String(e);
    if (msg.includes("#14") || msg.includes("DuplicateRound") || msg.includes("Bad union")) {
      // Oracle tick unchanged — re-read counter
      const latest = await query("current_round_id", predictContract);
      if (latest) currentRoundId = BigInt(latest as string | number | bigint);
    } else { log("create_round error", { err: msg }); }
  }
}

async function autoBet(roundId: bigint) {
  for (const bettor of BETTORS) {
    if (!bettor.secret) continue;
    const keypair = Keypair.fromSecret(bettor.secret);
    const key = `${roundId}-${keypair.publicKey()}`;
    if (betTracker.has(key)) continue;
    try {
      const method = bettor.side === "above" ? "bet_above" : "bet_below";
      await invoke(keypair, predictContract, method, [
        nativeToScVal(keypair.publicKey(), { type: "address" }),
        nativeToScVal(roundId, { type: "u64" }),
        xdr.ScVal.scvI128(new xdr.Int128Parts({
          hi: xdr.Int64.fromString((bettor.amountStroops >> 64n).toString()),
          lo: xdr.Uint64.fromString((bettor.amountStroops & 0xFFFFFFFFFFFFFFFFn).toString()),
        })),
      ]);
      betTracker.add(key);
      log("auto-bet", {
        wallet: bettor.name, side: bettor.side,
        amount: `${Number(bettor.amountStroops) / 1e7} XLM`,
      });
    } catch (e) {
      const msg = String(e);
      if (!msg.includes("#9") && !msg.includes("AlreadyBet") && !msg.includes("#4")) {
        log("bet error", { wallet: bettor.name, err: msg });
      }
    }
  }
}

async function main() {
  // Read current state
  const id = await query("current_round_id", predictContract);
  if (id) currentRoundId = BigInt(id as string | number | bigint);

  log("demo-keeper starting", {
    predict: PREDICT_CONTRACT,
    oracle: ORACLE_CONTRACT,
    roundSecs: ROUND_SECS,
    lockSecs: LOCK_SECS,
  });

  while (true) {
    try { await tick(); } catch (e) { log("tick error", { err: String(e) }); }
    await new Promise(r => setTimeout(r, ROUND_SECS * 1000));
  }
}

main();
