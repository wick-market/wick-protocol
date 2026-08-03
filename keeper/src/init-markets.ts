import { Keypair, Contract, rpc, Networks, TransactionBuilder, xdr, Address } from "@stellar/stellar-sdk";
import { ADMIN_SECRET } from "./env";

const RPC_URL = "https://soroban-testnet.stellar.org";
const NETWORK = Networks.TESTNET;
const TOKEN_ADDRESS = "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC";
const ORACLE_CONTRACT = "CBGWLGKZMEMW2IHDSXRL5QUTYRXSBGNCMQHFHRVZH6UETCNXV4NFDHAG";

// Oracle units: USD × 1e14 (14 decimals, matching Reflector).
// Cold-start seeds only — the keeper overwrites these with real spot on its first
// tick, so they just need to be the right order of magnitude.
const MARKETS = [
  { symbol: "XLM", contractId: "CBQMEF4YZVTEVV3KHSUVOBWWHTTJ2D6YAOGE5TGZT7WTMHJ7ORYMT5HU", initialPrice: 17200000000000n }, // $0.172
  { symbol: "BTC", contractId: "CAHYZ6K54567DHRSHEOB6BEFDD6GOXGIMX736RJWSSSA6DHLOHKFDOX2", initialPrice: 6370000000000000000n }, // $63,700
  { symbol: "ETH", contractId: "CC4MONXL6CIZ5F4VSNE2CFQZ6HXTSDJHEEUNGVUSFPUEZUJ7KAOWXSOM", initialPrice: 186000000000000000n }, // $1,860
  { symbol: "SOL", contractId: "CDDEM7TRNHQMIYTOYZUIFLOAF32K5MBP4WSDMKUGGGPGDCGU73CEYCJN", initialPrice: 7350000000000000n }, // $73.50
];

const server = new rpc.Server(RPC_URL, { allowHttp: false });
const adminKeypair = Keypair.fromSecret(ADMIN_SECRET);

async function invoke(contractId: string, method: string, args: xdr.ScVal[]) {
  const account = await server.getAccount(adminKeypair.publicKey());
  const contract = new Contract(contractId);
  const tx = new TransactionBuilder(account, { fee: "100000", networkPassphrase: NETWORK })
    .addOperation(contract.call(method, ...args))
    .setTimeout(30)
    .build();

  console.log(`[${method}] Simulating...`);
  const sim = await server.simulateTransaction(tx);
  if (rpc.Api.isSimulationError(sim)) {
    console.error(`Sim error ${contractId}.${method}:`, sim.error);
    return false;
  }

  console.log(`[${method}] Assembling & sending...`);
  const assembled = rpc.assembleTransaction(tx, sim).build();
  assembled.sign(adminKeypair);
  const send = await server.sendTransaction(assembled);
  if (send.status === "ERROR") {
    console.error(`Send error ${contractId}.${method}:`, send.errorResult);
    return false;
  }
  console.log(`[${method}] Tx sent (hash: ${send.hash}), polling...`);

  for (let i = 0; i < 40; i++) {
    await new Promise((r) => setTimeout(r, 1000));
    const res = await server.getTransaction(send.hash);
    if (res.status === rpc.Api.GetTransactionStatus.SUCCESS) {
      console.log(`✅ ${contractId}.${method} success!`);
      return true;
    }
    if (res.status === rpc.Api.GetTransactionStatus.FAILED) {
      console.error(`❌ ${contractId}.${method} tx failed on chain`);
      return false;
    }
  }
  console.warn(`⏳ ${contractId}.${method} polling timeout (hash: ${send.hash})`);
  return true;
}

async function main() {
  console.log("Initializing Test Oracle prices...");
  const oracleContract = new Contract(ORACLE_CONTRACT);

  for (const m of MARKETS) {
    console.log(`Setting oracle price for ${m.symbol}...`);
    await invoke(
      ORACLE_CONTRACT,
      "update_asset_price",
      [
        xdr.ScVal.scvSymbol(m.symbol),
        xdr.ScVal.scvI128(new xdr.Int128Parts({ lo: xdr.Uint64.fromString((m.initialPrice & 0xffffffffffffffffn).toString()), hi: xdr.Int64.fromString((m.initialPrice >> 64n).toString()) }))
      ]
    );
  }

  for (const m of MARKETS) {
    console.log(`Initializing market contract for ${m.symbol} (${m.contractId})...`);
    await invoke(
      m.contractId,
      "initialize",
      [
        new Address(adminKeypair.publicKey()).toScVal(),
        new Address(ORACLE_CONTRACT).toScVal(),
        new Address(TOKEN_ADDRESS).toScVal(),
        xdr.ScVal.scvSymbol(m.symbol),
        xdr.ScVal.scvU32(200),             // fee_bps = 2%
        xdr.ScVal.scvI128(new xdr.Int128Parts({ lo: xdr.Uint64.fromString("10000000"), hi: xdr.Int64.fromString("0") })), // min_bet = 1 XLM
        xdr.ScVal.scvU64(xdr.Uint64.fromString("45")), // lock_offset = 45s
        xdr.ScVal.scvU64(xdr.Uint64.fromString("60")), // oracle_interval = 60s
      ]
    );
  }

  console.log("\nMarket contracts initialized!");
}

main().catch(console.error);
