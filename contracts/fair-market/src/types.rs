use soroban_sdk::{contracttype, Address, Symbol};

#[contracttype]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Up,
    Down,
}

#[contracttype]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Up,
    Down,
    Void,
}

/// Stored status — only Open or Settled persisted.
/// get_round() derives Locked at read time from lock_ts.
#[contracttype]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Open,
    Locked,
    Settled,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Round {
    pub id: u64,
    pub asset: Symbol,
    /// P_N from oracle lastprice — the reference price bettors are predicting against.
    pub strike: i128,
    /// oracle's own timestamp for P_N — derived from PriceData.timestamp, NOT ledger time.
    pub strike_ts: u64,
    /// strike_ts + oracle_interval (300s). oracle.price(asset, settle_ts) is the settlement read.
    pub settle_ts: u64,
    /// strike_ts + lock_offset. No bets accepted on or after this time.
    pub lock_ts: u64,
    pub pool_up: i128,
    pub pool_down: i128,
    pub status: Status,
    pub outcome: Outcome,
    /// P_(N+1) from oracle.price(asset, settle_ts). Zero until settled.
    pub settle_price: i128,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Position {
    pub round_id: u64,
    pub side: Side,
    pub amount: i128,
    pub claimed: bool,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Config {
    pub admin: Address,
    /// Reflector ReflectorPulse oracle on testnet:
    ///   CCYOZJCOPG34LLQQ7N24YXBM7LL62R7ONMZ3G6WZAAYPB5OYKOMJRN63
    pub oracle: Address,
    /// XLM Stellar Asset Contract — TODO: run
    ///   `stellar contract id asset --asset native --network testnet`
    pub token: Address,
    pub fee_bps: u32,
    pub min_bet: i128,
    /// Seconds after strike_ts before betting closes. Default 180, min 90.
    pub lock_offset: u64,
    /// oracle.decimals() read at init — for UI normalisation only.
    pub oracle_decimals: u32,
}
