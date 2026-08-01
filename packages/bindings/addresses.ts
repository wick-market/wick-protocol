/**
 * Deployed Wick contract addresses.
 * Import these instead of hard-coding addresses in the frontend.
 *
 * @example
 * import { TESTNET } from "@wick/bindings/addresses";
 * const client = new Client({ contractId: TESTNET.FAIR_MARKET, ... });
 */

export const TESTNET = {
  /** Wick Fair Market prediction contract */
  FAIR_MARKET: "CBJINJPV6DKXYGC3XUARWXLYQ3V6CAW2BH5PATAKEEIWSWOT5K4KEIAF",

  /** Reflector ReflectorPulse oracle — USD-base, BTC/ETH/SOL/XLM, free, 5-min */
  ORACLE: "CCYOZJCOPG34LLQQ7N24YXBM7LL62R7ONMZ3G6WZAAYPB5OYKOMJRN63",

  /** XLM Stellar Asset Contract — native XLM wrapped as a token interface */
  XLM_SAC: "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",

  /** Stellar testnet network passphrase */
  NETWORK_PASSPHRASE: "Test SDF Network ; September 2015",

  /** Soroban RPC endpoint for testnet */
  RPC_URL: "https://soroban-testnet.stellar.org",
} as const;

// MAINNET is intentionally absent — this project has not deployed to mainnet.
// Get legal advice before accepting real funds.
