import { Keypair, Contract, rpc, Networks, TransactionBuilder, xdr, Address } from "@stellar/stellar-sdk";

const RPC_URL = "https://soroban-testnet.stellar.org";
const NETWORK = Networks.TESTNET;
const ADMIN_SECRET = "SDGZDDLJCCE6BQGROTACAZGLI3OIFNM3DTJJL7RZRM5KNQSDLXQUS73E";
const TOKEN_ADDRESS = "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC";
const ORACLE_CONTRACT = "CBCZDSMRMOYXRLV3IJNC6LC7HKV2UFE5KQ63P5LAKM73LAH4H4CNT4TM";

// Oracle units: USD × 1e14 (14 decimals, matching Reflector).
const MARKETS = [
  { symbol: "XLM", contractId: "CCHKYSNNU27QYKBAWTPHCIHOZQISYQ3GUEC3KVCZ6QPPNWN4QQXTTM3K", initialPrice: 17500000000000n }, // $0.175
  { symbol: "BTC", contractId: "CBIDB2UVODQFULE5GDOFCITHADLRWCPOCCN3FUPGKUDEHGZ7P3KRXIA4", initialPrice: 6500000000000000000n }, // $65,000
  { symbol: "ETH", contractId: "CAIETRXOO3YYJE7YISGPODJ6HTF2SZY2PC3WPD54ZW2EAWWAMZZWL7IS", initialPrice: 340000000000000000n }, // $3,400
  { symbol: "SOL", contractId: "CDAL2IADNQYUWDLZHN72EERCTT2SSPDC6RBXFHR3ZZHDTVGQHD4LG3T3", initialPrice: 18000000000000000n }, // $180
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
