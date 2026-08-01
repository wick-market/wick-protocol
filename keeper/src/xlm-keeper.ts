/**
 * Wick Predict keeper — XLM/USD only.
 * Every 60s: settle if past settle_ts, then create a new round.
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
  "CB6PWNWBO5BTWVAYFWUZ4PG3X6LHSZNBUQG7A6AK2TNCMKY72LRQIPCC";
const RPC_URL = "https://soroban-testnet.stellar.org";
const NETWORK_PASSPHRASE = Networks.TESTNET;
const ADMIN_SECRET = process.env.ADMIN_SECRET!;

const server = new rpc.Server(RPC_URL, { allowHttp: false });
const keypair = Keypair.fromSecret(ADMIN_SECRET);
const contract = new Contract(CONTRACT_ID);

function log(msg: string, data?: Record<string, unknown>) {
  console.log(JSON.stringify({ ts: new Date().toISOString(), msg, ...data }));
}

async function call(method: string, args: xdr.ScVal[] = []) {
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
    throw new Error(`sim ${method} failed: ${e.error}`);
  }
  return sim as rpc.Api.SimulateTransactionSuccessResponse;
}

async function invoke(method: string, args: xdr.ScVal[] = []): Promise<xdr.ScVal | undefined> {
  const sim = await call(method, args);
  const assembled = rpc.assembleTransaction(
    // rebuild the tx fresh for signing
    await (async () => {
      const account = await server.getAccount(keypair.publicKey());
      return new TransactionBuilder(account, { fee: BASE_FEE, networkPassphrase: NETWORK_PASSPHRASE })
        .addOperation(contract.call(method, ...args))
        .setTimeout(30)
        .build();
    })(),
    sim
  ).build();
  assembled.sign(keypair);
  const send = await server.sendTransaction(assembled);
  if (send.status === "ERROR") throw new Error(`send ${method} failed`);
  for (let i = 0; i < 30; i++) {
    await new Promise((r) => setTimeout(r, 2000));
    const result = await server.getTransaction(send.hash);
    if (result.status === rpc.Api.GetTransactionStatus.SUCCESS) {
      return (result as rpc.Api.GetSuccessfulTransactionResponse).returnValue;
    }
    if (result.status === rpc.Api.GetTransactionStatus.FAILED)
      throw new Error(`tx ${method} failed on-chain`);
  }
  throw new Error(`tx ${method} timed out`);
}

async function query(method: string, args: xdr.ScVal[] = []): Promise<unknown> {
  const sim = await call(method, args);
  return scValToNative(sim.result!.retval);
}

let currentRoundId = 0n;

async function tick() {
  const nowSec = Math.floor(Date.now() / 1000);

  if (currentRoundId === 0n) {
    try {
      const id = await query("current_round_id");
      const n = BigInt(id as string | number);
      if (n > 0n) currentRoundId = n;
    } catch {
      /* no rounds yet */
    }
  }

  if (currentRoundId > 0n) {
    const round = (await query("get_round", [
      nativeToScVal(currentRoundId, { type: "u64" }),
    ])) as Record<string, unknown>;

    const status = (round["status"] as { tag: string }).tag;
    const settleTs = Number(round["settle_ts"]);

    log("round", { id: currentRoundId.toString(), status, outcome: (round["outcome"] as { tag: string }).tag });

    if (status === "Settled") {
      log("settled — opening new round");
      const ret = await invoke("create_round");
      if (ret) currentRoundId = BigInt(scValToNative(ret) as string | number);
      log("new round", { id: currentRoundId.toString() });
      return;
    }

    if (nowSec >= settleTs) {
      log("settling", { id: currentRoundId.toString() });
      try {
        await invoke("settle", [nativeToScVal(currentRoundId, { type: "u64" })]);
        log("settled ok", { id: currentRoundId.toString() });
      } catch (e: unknown) {
        const msg = String(e);
        if (msg.includes("AlreadySettled") || msg.includes("#6")) {
          log("already settled — opening next round immediately");
          const ret = await invoke("create_round");
          if (ret) currentRoundId = BigInt(scValToNative(ret) as string | number);
          log("new round", { id: currentRoundId.toString() });
        } else throw e;
      }
      return;
    }

    log("live", {
      secsUntilLock: Math.max(0, Number(round["lock_ts"]) - nowSec),
      secsUntilSettle: Math.max(0, settleTs - nowSec),
    });
    return;
  }

  log("no round — creating first");
  const ret = await invoke("create_round");
  if (ret) currentRoundId = BigInt(scValToNative(ret) as string | number);
  log("created", { id: currentRoundId.toString() });
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
