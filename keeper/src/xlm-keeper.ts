/**
 * Wick Predict keeper — XLM/USD only.
 */
import "dotenv/config";
import {
  Contract,
  Keypair,
  Networks,
  rpc,
  TransactionBuilder,
  BASE_FEE,
  nativeToScVal,
  scValToNative,
  xdr,
} from "@stellar/stellar-sdk";

const CONTRACT_ID =
  process.env.XLM_PREDICT_CONTRACT_ID ??
  "CB3UZK2OQZ3CNJ2R64N7NI3EW6MEKFJDC5TTXYEBY5BL2EL2CLPNHDD2";
const RPC_URL = "https://soroban-testnet.stellar.org";
const NETWORK_PASSPHRASE = Networks.TESTNET;
const ADMIN_SECRET = process.env.ADMIN_SECRET!;

const server = new rpc.Server(RPC_URL, { allowHttp: false });
const keypair = Keypair.fromSecret(ADMIN_SECRET);
const contract = new Contract(CONTRACT_ID);

function log(msg: string, data?: Record<string, unknown>) {
  console.log(JSON.stringify({ ts: new Date().toISOString(), msg, ...data }));
}

/** Simulate (read-only) */
async function query(method: string, args: xdr.ScVal[] = []): Promise<unknown> {
  const account = await server.getAccount(keypair.publicKey());
  const tx = new TransactionBuilder(account, {
    fee: BASE_FEE,
    networkPassphrase: NETWORK_PASSPHRASE,
  })
    .addOperation(contract.call(method, ...args))
    .setTimeout(30)
    .build();
  const sim = await server.simulateTransaction(tx);
  if (!rpc.Api.isSimulationSuccess(sim)) {
    const e = sim as rpc.Api.SimulateTransactionErrorResponse;
    throw new Error(`${method}: ${e.error}`);
  }
  return scValToNative((sim as rpc.Api.SimulateTransactionSuccessResponse).result!.retval);
}

/** Simulate → assemble → sign → submit → poll. Returns returnValue or null. */
async function invoke(method: string, args: xdr.ScVal[] = []): Promise<unknown> {
  const account = await server.getAccount(keypair.publicKey());
  // Build ONCE — assemble applies auth/resources to this same transaction.
  const tx = new TransactionBuilder(account, {
    fee: BASE_FEE,
    networkPassphrase: NETWORK_PASSPHRASE,
  })
    .addOperation(contract.call(method, ...args))
    .setTimeout(30)
    .build();

  const sim = await server.simulateTransaction(tx);
  if (!rpc.Api.isSimulationSuccess(sim)) {
    const e = sim as rpc.Api.SimulateTransactionErrorResponse;
    throw new Error(`${method}: ${e.error}`);
  }

  const assembled = rpc.assembleTransaction(tx, sim).build();
  assembled.sign(keypair);
  const send = await server.sendTransaction(assembled);
  if (send.status === "ERROR") throw new Error(`send ${method} failed`);

  for (let i = 0; i < 30; i++) {
    await new Promise((r) => setTimeout(r, 2000));
    const result = await server.getTransaction(send.hash);
    if (result.status === rpc.Api.GetTransactionStatus.SUCCESS) {
      const rv = (result as rpc.Api.GetSuccessfulTransactionResponse).returnValue;
      try {
        return rv ? scValToNative(rv) : null;
      } catch {
        // XDR parse error — value exists but can't be decoded; treat as null
        return null;
      }
    }
    if (result.status === rpc.Api.GetTransactionStatus.FAILED) {
      // Extract contract error code from the failed transaction result
      const failedRes = result as rpc.Api.GetFailedTransactionResponse;
      throw new Error(`on-chain ${method} failed: ${JSON.stringify(failedRes.resultXdr ?? "")}`);
    }
  }
  throw new Error(`${method} timed out`);
}

let currentRoundId = 0n;

async function openNewRound(): Promise<void> {
  try {
    const id = await invoke("create_round");
    if (id !== null && id !== undefined) {
      currentRoundId = BigInt(id as string | number | bigint);
      log("round created", { id: currentRoundId.toString() });
    } else {
      // null return = XDR parse error after successful tx.
      // Re-read the counter to get the actual new round id.
      const latest = await query("current_round_id");
      currentRoundId = BigInt(latest as string | number | bigint);
      log("round created (id from counter)", { id: currentRoundId.toString() });
    }
  } catch (e: unknown) {
    const msg = String(e);
    if (msg.includes("DuplicateRound") || msg.includes("#14") || msg.includes("Bad union")) {
      log("duplicate — oracle tick unchanged, waiting for next tick");
    } else {
      log("create_round error", { err: msg });
    }
  }
}

async function tick() {
  const nowSec = Math.floor(Date.now() / 1000);

  // Discover current round id if we don't know it
  if (currentRoundId === 0n) {
    try {
      const id = await query("current_round_id");
      const n = BigInt(id as string | number);
      if (n > 0n) currentRoundId = n;
    } catch { /* no rounds yet */ }
  }

  if (currentRoundId === 0n) {
    log("no rounds yet — creating first");
    await openNewRound();
    return;
  }

  const round = (await query("get_round", [
    nativeToScVal(currentRoundId, { type: "u64" }),
  ])) as Record<string, unknown>;

  const status = (round["status"] as { tag: string }).tag;
  const settleTs = Number(round["settle_ts"]);
  const outcome = (round["outcome"] as { tag: string }).tag;

  log("round", { id: currentRoundId.toString(), status, outcome, secsLeft: settleTs - nowSec });

  if (status === "Settled") {
    log("settled — opening next round");
    await openNewRound();
    return;
  }

  if (nowSec >= settleTs) {
    log("settling", { id: currentRoundId.toString() });
    try {
      await invoke("settle", [nativeToScVal(currentRoundId, { type: "u64" })]);
      log("settled ok");
      await openNewRound();
    } catch (e: unknown) {
      const msg = String(e);
      if (msg.includes("AlreadySettled") || msg.includes("#6")) {
        log("already settled — opening next round");
        await openNewRound();
      } else throw e;
    }
    return;
  }

  log("live", {
    secsUntilLock: Math.max(0, Number(round["lock_ts"]) - nowSec),
    secsUntilSettle: Math.max(0, settleTs - nowSec),
  });
}

async function main() {
  if (!ADMIN_SECRET) { console.error("ADMIN_SECRET not set"); process.exit(1); }
  log("xlm-keeper starting", { contract: CONTRACT_ID });
  while (true) {
    try { await tick(); } catch (e: unknown) { log("error", { err: String(e) }); }
    await new Promise((r) => setTimeout(r, 60_000));
  }
}

main();
