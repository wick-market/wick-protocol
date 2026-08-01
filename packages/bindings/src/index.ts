import { Buffer } from "buffer";
import { Address } from "@stellar/stellar-sdk";
import {
  AssembledTransaction,
  Client as ContractClient,
  ClientOptions as ContractClientOptions,
  MethodOptions,
  Result,
  Spec as ContractSpec,
} from "@stellar/stellar-sdk/contract";
import type {
  u32,
  i32,
  u64,
  i64,
  u128,
  i128,
  u256,
  i256,
  Option,
  Timepoint,
  Duration,
} from "@stellar/stellar-sdk/contract";
export * from "@stellar/stellar-sdk";
export * as contract from "@stellar/stellar-sdk/contract";
export * as rpc from "@stellar/stellar-sdk/rpc";

if (typeof window !== "undefined") {
  //@ts-ignore Buffer exists
  window.Buffer = window.Buffer || Buffer;
}


export const networks = {
  testnet: {
    networkPassphrase: "Test SDF Network ; September 2015",
    contractId: "CBJINJPV6DKXYGC3XUARWXLYQ3V6CAW2BH5PATAKEEIWSWOT5K4KEIAF",
  }
} as const

export const Errors = {
  1: {message:"AlreadyInitialized"},
  2: {message:"NotInitialized"},
  3: {message:"InvalidAsset"},
  4: {message:"RoundNotFound"},
  5: {message:"RoundLocked"},
  6: {message:"RoundNotSettled"},
  7: {message:"AlreadySettled"},
  8: {message:"TooEarly"},
  9: {message:"BetTooSmall"},
  10: {message:"AlreadyBet"},
  11: {message:"AlreadyClaimed"},
  12: {message:"NothingToClaim"},
  13: {message:"FeeTooHigh"},
  14: {message:"LockOffsetTooSmall"},
  15: {message:"Unauthorized"},
  16: {message:"DuplicateRound"},
  17: {message:"ClaimBatchTooLarge"}
}

export type Side = {tag: "Up", values: void} | {tag: "Down", values: void};


export interface Round {
  asset: string;
  id: u64;
  /**
 * strike_ts + lock_offset. No bets accepted on or after this time.
 */
lock_ts: u64;
  outcome: Outcome;
  pool_down: i128;
  pool_up: i128;
  /**
 * P_(N+1) from oracle.price(asset, settle_ts). Zero until settled.
 */
settle_price: i128;
  /**
 * strike_ts + oracle_interval (300s). oracle.price(asset, settle_ts) is the settlement read.
 */
settle_ts: u64;
  status: Status;
  /**
 * P_N from oracle lastprice — the reference price bettors are predicting against.
 */
strike: i128;
  /**
 * oracle's own timestamp for P_N — derived from PriceData.timestamp, NOT ledger time.
 */
strike_ts: u64;
}


export interface Config {
  admin: string;
  fee_bps: u32;
  /**
 * Seconds after strike_ts before betting closes. Default 180, min 90.
 */
lock_offset: u64;
  min_bet: i128;
  /**
 * Reflector ReflectorPulse oracle on testnet:
 * CCYOZJCOPG34LLQQ7N24YXBM7LL62R7ONMZ3G6WZAAYPB5OYKOMJRN63
 */
oracle: string;
  /**
 * oracle.decimals() read at init — for UI normalisation only.
 */
oracle_decimals: u32;
  /**
 * XLM Stellar Asset Contract — TODO: run
 * `stellar contract id asset --asset native --network testnet`
 */
token: string;
}

/**
 * Stored status — only Open or Settled persisted.
 * get_round() derives Locked at read time from lock_ts.
 */
export type Status = {tag: "Open", values: void} | {tag: "Locked", values: void} | {tag: "Settled", values: void};

export type Outcome = {tag: "Up", values: void} | {tag: "Down", values: void} | {tag: "Void", values: void};


export interface Position {
  amount: i128;
  claimed: boolean;
  round_id: u64;
  side: Side;
}


export interface PriceData {
  price: i128;
  timestamp: u64;
}

/**
 * Reflector asset discriminant — mirrors the on-chain type exactly so XDR
 * encoding is compatible when the prediction contract calls the oracle.
 */
export type OracleAsset = {tag: "Stellar", values: readonly [string]} | {tag: "Other", values: readonly [string]};

export interface Client {
  /**
   * Construct and simulate a bet transaction. Returns an `AssembledTransaction` object which will have a `result` field containing the result of the simulation. If this transaction changes contract state, you will need to call `signAndSend()` on the returned object.
   */
  bet: ({user, round_id, side, amount}: {user: string, round_id: u64, side: Side, amount: i128}, options?: MethodOptions) => Promise<AssembledTransaction<null>>

  /**
   * Construct and simulate a claim transaction. Returns an `AssembledTransaction` object which will have a `result` field containing the result of the simulation. If this transaction changes contract state, you will need to call `signAndSend()` on the returned object.
   */
  claim: ({user, round_id}: {user: string, round_id: u64}, options?: MethodOptions) => Promise<AssembledTransaction<i128>>

  /**
   * Construct and simulate a settle transaction. Returns an `AssembledTransaction` object which will have a `result` field containing the result of the simulation. If this transaction changes contract state, you will need to call `signAndSend()` on the returned object.
   */
  settle: ({round_id}: {round_id: u64}, options?: MethodOptions) => Promise<AssembledTransaction<null>>

  /**
   * Construct and simulate a get_round transaction. Returns an `AssembledTransaction` object which will have a `result` field containing the result of the simulation. If this transaction changes contract state, you will need to call `signAndSend()` on the returned object.
   */
  get_round: ({round_id}: {round_id: u64}, options?: MethodOptions) => Promise<AssembledTransaction<Round>>

  /**
   * Construct and simulate a claim_many transaction. Returns an `AssembledTransaction` object which will have a `result` field containing the result of the simulation. If this transaction changes contract state, you will need to call `signAndSend()` on the returned object.
   * Batch claim, capped at MAX_CLAIM_BATCH rounds per call.
   */
  claim_many: ({user, round_ids}: {user: string, round_ids: Array<u64>}, options?: MethodOptions) => Promise<AssembledTransaction<i128>>

  /**
   * Construct and simulate a get_config transaction. Returns an `AssembledTransaction` object which will have a `result` field containing the result of the simulation. If this transaction changes contract state, you will need to call `signAndSend()` on the returned object.
   */
  get_config: (options?: MethodOptions) => Promise<AssembledTransaction<Config>>

  /**
   * Construct and simulate a initialize transaction. Returns an `AssembledTransaction` object which will have a `result` field containing the result of the simulation. If this transaction changes contract state, you will need to call `signAndSend()` on the returned object.
   */
  initialize: ({admin, oracle, token, fee_bps, min_bet, lock_offset}: {admin: string, oracle: string, token: string, fee_bps: u32, min_bet: i128, lock_offset: u64}, options?: MethodOptions) => Promise<AssembledTransaction<null>>

  /**
   * Construct and simulate a sweep_fees transaction. Returns an `AssembledTransaction` object which will have a `result` field containing the result of the simulation. If this transaction changes contract state, you will need to call `signAndSend()` on the returned object.
   * Sweep accumulated treasury fees to the admin address.
   */
  sweep_fees: (options?: MethodOptions) => Promise<AssembledTransaction<null>>

  /**
   * Construct and simulate a set_fee_bps transaction. Returns an `AssembledTransaction` object which will have a `result` field containing the result of the simulation. If this transaction changes contract state, you will need to call `signAndSend()` on the returned object.
   */
  set_fee_bps: ({bps}: {bps: u32}, options?: MethodOptions) => Promise<AssembledTransaction<null>>

  /**
   * Construct and simulate a create_round transaction. Returns an `AssembledTransaction` object which will have a `result` field containing the result of the simulation. If this transaction changes contract state, you will need to call `signAndSend()` on the returned object.
   */
  create_round: ({asset}: {asset: string}, options?: MethodOptions) => Promise<AssembledTransaction<u64>>

  /**
   * Construct and simulate a get_position transaction. Returns an `AssembledTransaction` object which will have a `result` field containing the result of the simulation. If this transaction changes contract state, you will need to call `signAndSend()` on the returned object.
   */
  get_position: ({round_id, user}: {round_id: u64, user: string}, options?: MethodOptions) => Promise<AssembledTransaction<Option<Position>>>

  /**
   * Construct and simulate a set_lock_offset transaction. Returns an `AssembledTransaction` object which will have a `result` field containing the result of the simulation. If this transaction changes contract state, you will need to call `signAndSend()` on the returned object.
   */
  set_lock_offset: ({seconds}: {seconds: u64}, options?: MethodOptions) => Promise<AssembledTransaction<null>>

}
export class Client extends ContractClient {
  static async deploy<T = Client>(
    /** Options for initializing a Client as well as for calling a method, with extras specific to deploying. */
    options: MethodOptions &
      Omit<ContractClientOptions, "contractId"> & {
        /** The hash of the Wasm blob, which must already be installed on-chain. */
        wasmHash: Buffer | string;
        /** Salt used to generate the contract's ID. Passed through to {@link Operation.createCustomContract}. Default: random. */
        salt?: Buffer | Uint8Array;
        /** The format used to decode `wasmHash`, if it's provided as a string. */
        format?: "hex" | "base64";
      }
  ): Promise<AssembledTransaction<T>> {
    return ContractClient.deploy(null, options)
  }
  constructor(public readonly options: ContractClientOptions) {
    super(
      new ContractSpec([ "AAAABAAAAAAAAAAAAAAABUVycm9yAAAAAAAAEQAAAAAAAAASQWxyZWFkeUluaXRpYWxpemVkAAAAAAABAAAAAAAAAA5Ob3RJbml0aWFsaXplZAAAAAAAAgAAAAAAAAAMSW52YWxpZEFzc2V0AAAAAwAAAAAAAAANUm91bmROb3RGb3VuZAAAAAAAAAQAAAAAAAAAC1JvdW5kTG9ja2VkAAAAAAUAAAAAAAAAD1JvdW5kTm90U2V0dGxlZAAAAAAGAAAAAAAAAA5BbHJlYWR5U2V0dGxlZAAAAAAABwAAAAAAAAAIVG9vRWFybHkAAAAIAAAAAAAAAAtCZXRUb29TbWFsbAAAAAAJAAAAAAAAAApBbHJlYWR5QmV0AAAAAAAKAAAAAAAAAA5BbHJlYWR5Q2xhaW1lZAAAAAAACwAAAAAAAAAOTm90aGluZ1RvQ2xhaW0AAAAAAAwAAAAAAAAACkZlZVRvb0hpZ2gAAAAAAA0AAAAAAAAAEkxvY2tPZmZzZXRUb29TbWFsbAAAAAAADgAAAAAAAAAMVW5hdXRob3JpemVkAAAADwAAAAAAAAAORHVwbGljYXRlUm91bmQAAAAAABAAAAAAAAAAEkNsYWltQmF0Y2hUb29MYXJnZQAAAAAAEQ==",
        "AAAAAAAAAAAAAAADYmV0AAAAAAQAAAAAAAAABHVzZXIAAAATAAAAAAAAAAhyb3VuZF9pZAAAAAYAAAAAAAAABHNpZGUAAAfQAAAABFNpZGUAAAAAAAAABmFtb3VudAAAAAAACwAAAAA=",
        "AAAAAAAAAAAAAAAFY2xhaW0AAAAAAAACAAAAAAAAAAR1c2VyAAAAEwAAAAAAAAAIcm91bmRfaWQAAAAGAAAAAQAAAAs=",
        "AAAAAAAAAAAAAAAGc2V0dGxlAAAAAAABAAAAAAAAAAhyb3VuZF9pZAAAAAYAAAAA",
        "AAAAAAAAAAAAAAAJZ2V0X3JvdW5kAAAAAAAAAQAAAAAAAAAIcm91bmRfaWQAAAAGAAAAAQAAB9AAAAAFUm91bmQAAAA=",
        "AAAAAAAAADdCYXRjaCBjbGFpbSwgY2FwcGVkIGF0IE1BWF9DTEFJTV9CQVRDSCByb3VuZHMgcGVyIGNhbGwuAAAAAApjbGFpbV9tYW55AAAAAAACAAAAAAAAAAR1c2VyAAAAEwAAAAAAAAAJcm91bmRfaWRzAAAAAAAD6gAAAAYAAAABAAAACw==",
        "AAAAAAAAAAAAAAAKZ2V0X2NvbmZpZwAAAAAAAAAAAAEAAAfQAAAABkNvbmZpZwAA",
        "AAAAAAAAAAAAAAAKaW5pdGlhbGl6ZQAAAAAABgAAAAAAAAAFYWRtaW4AAAAAAAATAAAAAAAAAAZvcmFjbGUAAAAAABMAAAAAAAAABXRva2VuAAAAAAAAEwAAAAAAAAAHZmVlX2JwcwAAAAAEAAAAAAAAAAdtaW5fYmV0AAAAAAsAAAAAAAAAC2xvY2tfb2Zmc2V0AAAAAAYAAAAA",
        "AAAAAAAAADVTd2VlcCBhY2N1bXVsYXRlZCB0cmVhc3VyeSBmZWVzIHRvIHRoZSBhZG1pbiBhZGRyZXNzLgAAAAAAAApzd2VlcF9mZWVzAAAAAAAAAAAAAA==",
        "AAAAAAAAAAAAAAALc2V0X2ZlZV9icHMAAAAAAQAAAAAAAAADYnBzAAAAAAQAAAAA",
        "AAAAAAAAAAAAAAAMY3JlYXRlX3JvdW5kAAAAAQAAAAAAAAAFYXNzZXQAAAAAAAARAAAAAQAAAAY=",
        "AAAAAAAAAAAAAAAMZ2V0X3Bvc2l0aW9uAAAAAgAAAAAAAAAIcm91bmRfaWQAAAAGAAAAAAAAAAR1c2VyAAAAEwAAAAEAAAPoAAAH0AAAAAhQb3NpdGlvbg==",
        "AAAAAAAAAAAAAAAPc2V0X2xvY2tfb2Zmc2V0AAAAAAEAAAAAAAAAB3NlY29uZHMAAAAABgAAAAA=",
        "AAAAAgAAAAAAAAAAAAAABFNpZGUAAAACAAAAAAAAAAAAAAACVXAAAAAAAAAAAAAAAAAABERvd24=",
        "AAAAAQAAAAAAAAAAAAAABVJvdW5kAAAAAAAACwAAAAAAAAAFYXNzZXQAAAAAAAARAAAAAAAAAAJpZAAAAAAABgAAAEBzdHJpa2VfdHMgKyBsb2NrX29mZnNldC4gTm8gYmV0cyBhY2NlcHRlZCBvbiBvciBhZnRlciB0aGlzIHRpbWUuAAAAB2xvY2tfdHMAAAAABgAAAAAAAAAHb3V0Y29tZQAAAAfQAAAAB091dGNvbWUAAAAAAAAAAAlwb29sX2Rvd24AAAAAAAALAAAAAAAAAAdwb29sX3VwAAAAAAsAAABAUF8oTisxKSBmcm9tIG9yYWNsZS5wcmljZShhc3NldCwgc2V0dGxlX3RzKS4gWmVybyB1bnRpbCBzZXR0bGVkLgAAAAxzZXR0bGVfcHJpY2UAAAALAAAAWnN0cmlrZV90cyArIG9yYWNsZV9pbnRlcnZhbCAoMzAwcykuIG9yYWNsZS5wcmljZShhc3NldCwgc2V0dGxlX3RzKSBpcyB0aGUgc2V0dGxlbWVudCByZWFkLgAAAAAACXNldHRsZV90cwAAAAAAAAYAAAAAAAAABnN0YXR1cwAAAAAH0AAAAAZTdGF0dXMAAAAAAFFQX04gZnJvbSBvcmFjbGUgbGFzdHByaWNlIOKAlCB0aGUgcmVmZXJlbmNlIHByaWNlIGJldHRvcnMgYXJlIHByZWRpY3RpbmcgYWdhaW5zdC4AAAAAAAAGc3RyaWtlAAAAAAALAAAAVW9yYWNsZSdzIG93biB0aW1lc3RhbXAgZm9yIFBfTiDigJQgZGVyaXZlZCBmcm9tIFByaWNlRGF0YS50aW1lc3RhbXAsIE5PVCBsZWRnZXIgdGltZS4AAAAAAAAJc3RyaWtlX3RzAAAAAAAABg==",
        "AAAAAQAAAAAAAAAAAAAABkNvbmZpZwAAAAAABwAAAAAAAAAFYWRtaW4AAAAAAAATAAAAAAAAAAdmZWVfYnBzAAAAAAQAAABDU2Vjb25kcyBhZnRlciBzdHJpa2VfdHMgYmVmb3JlIGJldHRpbmcgY2xvc2VzLiBEZWZhdWx0IDE4MCwgbWluIDkwLgAAAAALbG9ja19vZmZzZXQAAAAABgAAAAAAAAAHbWluX2JldAAAAAALAAAAZFJlZmxlY3RvciBSZWZsZWN0b3JQdWxzZSBvcmFjbGUgb24gdGVzdG5ldDoKQ0NZT1pKQ09QRzM0TExRUTdOMjRZWEJNN0xMNjJSN09OTVozRzZXWkFBWVBCNU9ZS09NSlJONjMAAAAGb3JhY2xlAAAAAAATAAAAPW9yYWNsZS5kZWNpbWFscygpIHJlYWQgYXQgaW5pdCDigJQgZm9yIFVJIG5vcm1hbGlzYXRpb24gb25seS4AAAAAAAAPb3JhY2xlX2RlY2ltYWxzAAAAAAQAAABlWExNIFN0ZWxsYXIgQXNzZXQgQ29udHJhY3Qg4oCUIFRPRE86IHJ1bgpgc3RlbGxhciBjb250cmFjdCBpZCBhc3NldCAtLWFzc2V0IG5hdGl2ZSAtLW5ldHdvcmsgdGVzdG5ldGAAAAAAAAAFdG9rZW4AAAAAAAAT",
        "AAAAAgAAAGdTdG9yZWQgc3RhdHVzIOKAlCBvbmx5IE9wZW4gb3IgU2V0dGxlZCBwZXJzaXN0ZWQuCmdldF9yb3VuZCgpIGRlcml2ZXMgTG9ja2VkIGF0IHJlYWQgdGltZSBmcm9tIGxvY2tfdHMuAAAAAAAAAAAGU3RhdHVzAAAAAAADAAAAAAAAAAAAAAAET3BlbgAAAAAAAAAAAAAABkxvY2tlZAAAAAAAAAAAAAAAAAAHU2V0dGxlZAA=",
        "AAAAAgAAAAAAAAAAAAAAB091dGNvbWUAAAAAAwAAAAAAAAAAAAAAAlVwAAAAAAAAAAAAAAAAAAREb3duAAAAAAAAAAAAAAAEVm9pZA==",
        "AAAAAQAAAAAAAAAAAAAACFBvc2l0aW9uAAAABAAAAAAAAAAGYW1vdW50AAAAAAALAAAAAAAAAAdjbGFpbWVkAAAAAAEAAAAAAAAACHJvdW5kX2lkAAAABgAAAAAAAAAEc2lkZQAAB9AAAAAEU2lkZQ==",
        "AAAAAQAAAAAAAAAAAAAACVByaWNlRGF0YQAAAAAAAAIAAAAAAAAABXByaWNlAAAAAAAACwAAAAAAAAAJdGltZXN0YW1wAAAAAAAABg==",
        "AAAAAgAAAI9SZWZsZWN0b3IgYXNzZXQgZGlzY3JpbWluYW50IOKAlCBtaXJyb3JzIHRoZSBvbi1jaGFpbiB0eXBlIGV4YWN0bHkgc28gWERSCmVuY29kaW5nIGlzIGNvbXBhdGlibGUgd2hlbiB0aGUgcHJlZGljdGlvbiBjb250cmFjdCBjYWxscyB0aGUgb3JhY2xlLgAAAAAAAAAAC09yYWNsZUFzc2V0AAAAAAIAAAABAAAAAAAAAAdTdGVsbGFyAAAAAAEAAAATAAAAAQAAAAAAAAAFT3RoZXIAAAAAAAABAAAAEQ==" ]),
      options
    )
  }
  public readonly fromJSON = {
    bet: this.txFromJSON<null>,
        claim: this.txFromJSON<i128>,
        settle: this.txFromJSON<null>,
        get_round: this.txFromJSON<Round>,
        claim_many: this.txFromJSON<i128>,
        get_config: this.txFromJSON<Config>,
        initialize: this.txFromJSON<null>,
        sweep_fees: this.txFromJSON<null>,
        set_fee_bps: this.txFromJSON<null>,
        create_round: this.txFromJSON<u64>,
        get_position: this.txFromJSON<Option<Position>>,
        set_lock_offset: this.txFromJSON<null>
  }
}