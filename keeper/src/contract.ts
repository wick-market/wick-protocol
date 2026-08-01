/**
 * Contract interaction helpers.
 *
 * View calls  → simulateTransaction (free, no sequence number consumed)
 * State calls → simulateTransaction → assembleTransaction → sign → send → poll
 *
 * All calls run sequentially — one Stellar account can't submit concurrent
 * transactions without sequence number conflicts.
 */
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
import { info, warn, error } from "./logger";

const RPC_URL = process.env.RPC_URL!;
const NETWORK_PASSPHRASE = process.env.NETWORK_PASSPHRASE ?? Networks.TESTNET;
const CONTRACT_ID = process.env.CONTRACT_ID!;
const ADMIN_SECRET = process.env.ADMIN_SECRET!;

const server = new rpc.Server(RPC_URL, { allowHttp: false });
const keypair = Keypair.fromSecret(ADMIN_SECRET);
const contract = new Contract(CONTRACT_ID);

// ── On-chain Round type ───────────────────────────────────────────────────────

export interface Round {
  id: bigint;
  asset: string;
  strike: bigint;
  strike_ts: bigint;
  settle_ts: bigint;
  lock_ts: bigint;
  pool_up: bigint;
  pool_down: bigint;
  status: "Open" | "Locked" | "Settled";
  outcome: "Up" | "Down" | "Void";
  settle_price: bigint;
}

// ── Helpers ───────────────────────────────────────────────────────────────────

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

/** Poll until the transaction is confirmed or throws on failure. */
async function waitForTx(hash: string): Promise<rpc.Api.GetSuccessfulTransactionResponse> {
  for (let i = 0; i < 30; i++) {
    await sleep(2000);
    const result = await server.getTransaction(hash);
    if (result.status === rpc.Api.GetTransactionStatus.SUCCESS) {
      return result as rpc.Api.GetSuccessfulTransactionResponse;
    }
    if (result.status === rpc.Api.GetTransactionStatus.FAILED) {
      throw new Error(`Transaction ${hash} failed on-chain`);
    }
  }
  throw new Error(`Transaction ${hash} timed out after 60s`);
}

/** Build, simulate, assemble, sign, submit, and poll a contract invocation. */
async function invoke(method: string, ...args: xdr.ScVal[]): Promise<xdr.ScVal> {
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
    const errSim = sim as rpc.Api.SimulateTransactionErrorResponse;
    throw new Error(`Simulation failed for ${method}: ${errSim.error}`);
  }

  const assembled = rpc.assembleTransaction(tx, sim).build();
  assembled.sign(keypair);

  const send = await server.sendTransaction(assembled);
  if (send.status === "ERROR") {
    throw new Error(`Submit failed for ${method}: ${JSON.stringify(send.errorResult)}`);
  }

  const confirmed = await waitForTx(send.hash);
  return confirmed.returnValue ?? xdr.ScVal.scvVoid();
}

/** Simulate a read-only call. No sequence number consumed. */
async function query(method: string, ...args: xdr.ScVal[]): Promise<xdr.ScVal> {
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
    throw new Error(`Query ${method} failed`);
  }
  return (sim as rpc.Api.SimulateTransactionSuccessResponse).result!.retval;
}

// ── Public API ────────────────────────────────────────────────────────────────

export async function getRound(roundId: bigint): Promise<Round | null> {
  try {
    const val = await query("get_round", nativeToScVal(roundId, { type: "u64" }));
    const raw = scValToNative(val) as Record<string, unknown>;
    return {
      id: BigInt(raw.id as string | number),
      asset: raw.asset as string,
      strike: BigInt(raw.strike as string | number),
      strike_ts: BigInt(raw.strike_ts as string | number),
      settle_ts: BigInt(raw.settle_ts as string | number),
      lock_ts: BigInt(raw.lock_ts as string | number),
      pool_up: BigInt(raw.pool_up as string | number),
      pool_down: BigInt(raw.pool_down as string | number),
      status: raw.status as Round["status"],
      outcome: raw.outcome as Round["outcome"],
      settle_price: BigInt(raw.settle_price as string | number),
    };
  } catch {
    return null;
  }
}

/** settle(round_id). Returns true on success, false if already settled or too early. */
export async function settle(roundId: bigint): Promise<boolean> {
  try {
    await invoke("settle", nativeToScVal(roundId, { type: "u64" }));
    return true;
  } catch (err: unknown) {
    const msg = String(err);
    if (msg.includes("AlreadySettled") || msg.includes("#7")) {
      warn("settle: already settled", { roundId: roundId.toString() });
      return false;
    }
    if (msg.includes("TooEarly") || msg.includes("#8")) {
      warn("settle: too early", { roundId: roundId.toString() });
      return false;
    }
    throw err;
  }
}

/** create_round(asset). Returns the new round ID. */
export async function createRound(asset: string): Promise<bigint> {
  const val = await invoke("create_round", nativeToScVal(asset, { type: "symbol" }));
  const native = scValToNative(val);
  return BigInt(native as string | number);
}

/** Log the admin account's XLM balance and alert if low. */
export async function checkBalance(thresholdXlm = 100): Promise<void> {
  try {
    // Use Horizon to get balance — rpc.Server.getAccount returns an SDK Account
    // object without balance information; Horizon returns the full account record.
    const horizonUrl = RPC_URL.replace("soroban-testnet", "horizon-testnet").replace(
      "soroban.",
      "horizon."
    );
    const resp = await fetch(
      `https://horizon-testnet.stellar.org/accounts/${keypair.publicKey()}`
    );
    const data = (await resp.json()) as {
      balances?: { asset_type: string; balance: string }[];
    };
    const xlm = data.balances?.find((b) => b.asset_type === "native");
    const balance = parseFloat(xlm?.balance ?? "0");
    if (balance < thresholdXlm) {
      warn("keeper balance low", { balance, thresholdXlm });
    } else {
      info("keeper balance ok", { balance });
    }
  } catch {
    warn("could not check keeper balance");
  }
}
